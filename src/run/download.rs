//! Downloading a prepared stream: output paths, the audio/video download
//! tasks, and finishing (merge or interrupt handling).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::download::{StreamParams, download_stream};
use crate::finish::{Finish, Moved};
use crate::http::Http;
use crate::interrupt::Interrupt;
use crate::live_from::start_delay;
use crate::mux::{MuxInput, command_line, ffmpeg_args};
use crate::output::{OutputPlan, ensure_dir, final_location, load_states, temp_dir};
use crate::prepare::Prepared;
use crate::progress::{self, Totals};
use crate::session::Session;
use crate::types::{AUDIO_ITAG, DEFAULT_POLL_SECS, DataType, Options, PerType, SavedDownload};
use crate::util::{seconds_to_duration_and_time_str, write_file_with_mode};
use crate::{log_debug, log_error, log_general, log_info, log_warn};

pub(super) async fn download(
    opts: Arc<Options>,
    http: Http,
    cancel: CancellationToken,
    mut p: Prepared,
    live_from_sq: Option<u64>,
    interrupt: Arc<Interrupt>,
) -> i32 {
    let (dir, file_name) = match final_location(&opts, &p.format_info) {
        Ok(v) => v,
        Err(e) => {
            log_error!("{e}");
            return 1;
        }
    };
    let final_dir = ensure_dir(dir, opts.dir_mode, "final file");
    if let Some(td) = &opts.temp_dir {
        ensure_dir(td.clone(), opts.dir_mode, "temporary");
    }

    let streams: Vec<(DataType, u32)> = [
        p.sel.audio_url.as_ref().map(|_| (DataType::Audio, AUDIO_ITAG)),
        p.sel.video_url.as_ref().zip(p.sel.video_itag).map(|(_, i)| (DataType::Video, i)),
    ]
    .into_iter()
    .flatten()
    .collect();
    if streams.is_empty() {
        log_error!("Neither audio nor video downloads were started.");
        return 1;
    }
    let itags: Vec<u32> = streams.iter().map(|s| s.1).collect();
    let (mut states, prev_temp) = load_states(&opts, &p.video_id, &itags);
    let resuming = states.iter().any(|s| s.fragments > 0);

    // --start-delay (ignored when resuming).
    let mut start_sq = live_from_sq;
    if let Some(delay) = opts.start_delay {
        if resuming {
            log_warn!("Option --start-delay is being ignored as a download is being resumed.");
        } else if p.live
            && let Some(last) = p.last_sq
        {
            let (sq, wait) = start_delay(delay, last, p.target_duration);
            log_general!("Waiting {} before starting to download...", seconds_to_duration_and_time_str(wait.as_secs()));
            log_debug!("Will start from sequence {sq} [current is {last}]");
            tokio::select! {
                _ = cancel.cancelled() => return 2,
                _ = tokio::time::sleep(wait) => {}
            }
            start_sq = Some(sq);
            if wait > Duration::from_secs(DEFAULT_POLL_SECS)
                && let Some(ex) = p.extractor.as_mut()
            {
                match ex.get_video_info().await {
                    Some(sel) => {
                        p.sel = sel;
                        p.live = ex.live;
                    }
                    None => {
                        log_error!("Got an error when re-grabbing video info after the delay period elapsed. Exiting.");
                        return 1;
                    }
                }
            }
        }
    }

    let plan = OutputPlan { temp_dir: temp_dir(&opts, prev_temp, &final_dir, &p.video_id), final_dir, file_name };
    for st in &mut states {
        st.temp_dir = plan.temp_dir.clone();
    }

    // Thumbnail and description go to the temp dir first.
    let mut thumbnail = None;
    if (opts.embed_thumbnail || opts.write_thumbnail) && !p.thumbnail_url.is_empty() {
        let temp = plan.temp(".jpg");
        match http.download_to_file(&p.thumbnail_url, &temp, opts.file_mode).await {
            Ok(()) => thumbnail = Some(Moved { temp, dest: plan.final_path(".jpg") }),
            Err(e) => log_warn!("Failed to download thumbnail: {e}"),
        }
    }
    let mut description = None;
    if opts.write_description
        && let Some(comment) = p.metadata.get("comment").filter(|c| !c.is_empty())
    {
        let temp = plan.temp(".description");
        match write_file_with_mode(&temp, comment.as_bytes(), opts.file_mode).await {
            Ok(()) => description = Some(Moved { temp, dest: plan.final_path(".description") }),
            Err(e) => log_warn!("Error writing description file: {e}"),
        }
    }

    let ts =
        |itag: u32| Moved { temp: plan.temp(&format!(".f{itag}.ts")), dest: plan.final_path(&format!(".f{itag}.ts")) };
    let audio_ts = streams.iter().find(|s| s.0 == DataType::Audio).map(|s| ts(s.1));
    let video_ts = streams.iter().find(|s| s.0 == DataType::Video).map(|s| ts(s.1));

    // Computed after the thumbnail download so a failed download isn't
    // referenced (Go built the command first).
    let embed = thumbnail.as_ref().filter(|_| opts.embed_thumbnail).map(|t| t.dest.as_path());
    let main_input = MuxInput {
        audio: audio_ts.as_ref().map(|m| m.dest.as_path()),
        video: video_ts.as_ref().map(|m| m.dest.as_path()),
        thumbnail: embed,
        mkv: opts.mkv,
        metadata: opts.add_metadata.then_some(&p.metadata),
        out_dir: &plan.final_dir,
        file_name: &plan.file_name,
    };
    let mux = ffmpeg_args(&main_input);
    let audio_mux = (opts.separate_audio && audio_ts.is_some() && video_ts.is_some())
        .then(|| ffmpeg_args(&MuxInput { video: None, ..main_input }));
    let mux_file = Moved { temp: plan.temp(".ffmpeg.txt"), dest: plan.final_path(".ffmpeg.txt") };
    if let Err(e) =
        write_file_with_mode(&mux_file.temp, command_line(&opts.ffmpeg_path, &mux.args).as_bytes(), opts.file_mode)
            .await
    {
        log_warn!("Failed to write initial mux file: {e}");
    }

    let session = Session::new(opts.clone(), http, cancel, p.extractor.take(), &p.sel, p.live, p.target_duration);
    if p.unavailable {
        session.set_unavailable();
    }
    let saved = Arc::new(SavedDownload {
        video_id: p.video_id.clone(),
        format_info: p.format_info.clone(),
        metadata: p.metadata.clone(),
        thumbnail_url: p.thumbnail_url.clone(),
        target_duration: p.target_duration,
        label: p.sel.label.clone(),
        video_itag: p.sel.video_itag,
    });
    let refresher = session.spawn_periodic_refresh();

    let resumed = states.iter().any(|st| st.fragments > 0);
    let mut initial = Totals::default();
    for ((dt, _), st) in streams.iter().zip(&states) {
        match dt {
            DataType::Audio => initial.audio_frags = st.fragments,
            DataType::Video => initial.video_frags = st.fragments,
        }
        initial.bytes += st.size;
    }
    let (tx, rx) = mpsc::unbounded_channel();
    let reporter = tokio::spawn(progress::report(rx, initial, opts.status_newlines));

    let mut tasks = Vec::new();
    for ((dt, itag), st) in streams.iter().copied().zip(states) {
        let file = ts(itag).temp;
        log_info!("Starting download to {}", file.display());
        let state_file = (!opts.disable_save_state).then(|| st.file.clone());
        tasks.push(tokio::spawn(download_stream(
            session.clone(),
            StreamParams {
                data_type: dt,
                data_file: file,
                frag_prefix: plan.temp(&format!(".f{itag}")),
                state: st,
                state_file,
                last_sq: p.last_sq,
                start_sq,
                capture_duration: opts.capture_duration,
                retry_sleep: Duration::from_secs(u64::from(p.target_duration.max(1))),
                progress: tx.clone(),
                saved: Some(saved.clone()),
            },
        )));
    }
    drop(tx);

    let mut frags: PerType<Option<u64>> = PerType::default();
    let mut storage_failed = false;
    let mut retry_until: Option<u64> = None;
    let mut state_files = Vec::new();
    for (task, (dt, itag)) in tasks.into_iter().zip(&streams) {
        state_files.push(crate::output::state_file(&opts, &p.video_id, *itag));
        match task.await {
            Ok(Ok(out)) => {
                if !out.skipped.is_empty() {
                    log_warn!(
                        "{}: {} fragment(s) could not be downloaded and are missing: {:?}",
                        dt.as_str(),
                        out.skipped.len(),
                        out.skipped
                    );
                }
                *frags.get_mut(*dt) = Some(out.state.fragments);
                if let Some(t) = out.retry_until {
                    retry_until = Some(retry_until.map_or(t, |r| r.max(t)));
                }
            }
            Ok(Err(e)) => {
                storage_failed |= matches!(e, crate::error::Error::OpenStorage(_));
                log_error!("{} download failed: {e}", dt.as_str());
            }
            Err(e) => log_error!("{} download task panicked: {e}", dt.as_str()),
        }
    }
    if let Err(error) = reporter.await {
        log_debug!("The progress reporter stopped unexpectedly: {error}");
    }
    if let Some(r) = refresher {
        r.abort();
    }
    eprintln!();

    let fin = Finish {
        opts: &opts,
        plan: &plan,
        video_id: &p.video_id,
        audio_ts,
        video_ts,
        thumbnail,
        description,
        mux_file,
        mux,
        audio_mux,
        state_files,
        audio_frags: frags.audio,
        video_frags: frags.video,
        keep_for_retry: retry_until,
    };
    if storage_failed {
        let had_data = resumed || [frags.audio, frags.video].iter().flatten().any(|n| *n > 0);
        return fin.on_storage_error(had_data);
    }
    if interrupt.is_set()
        && let Some(code) = fin.on_cancel().await
    {
        return code;
    }
    fin.finish().await
}
