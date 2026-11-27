//! One stream's download once its info is known: output paths, the
//! audio/video download tasks, and handing over to `Finish`.

use std::fs;
use std::mem;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::download::{MEMORY_LIMIT, PendingExport, StreamParams, download_stream};
use crate::error::Error;
use crate::finish::{Finish, Moved};
use crate::http::Http;
use crate::interrupt::Interrupt;
use crate::live_from::{resolve_live_from, start_delay};
use crate::log::{finish_inline, report_progress};
use crate::mux::{MuxInput, ffmpeg_args, mux_file_contents};
use crate::output::{OutputPlan, ensure_dir, final_location, load_states, state_file, temp_dir};
use crate::prepare::Prepared;
use crate::session::Session;
use crate::types::{AUDIO_ITAG, DEFAULT_POLL_SECS, DataType, FormatSelection, Options, PerType, SavedDownload};
use crate::util::{seconds_to_duration_and_time_str, sleep_wall_clock, write_file_with_mode};
use crate::{log_debug, log_error, log_general, log_info, log_warn};

/// The sequence `--live-from` starts at. `Err` has been logged. Resolved
/// only for a fresh download, so resuming never fails on a position that
/// has since left the seek window.
fn live_from_start(opts: &Options, prepared: &Prepared) -> Result<Option<u64>, ()> {
    let Some(value) = &opts.live_from else { return Ok(None) };
    let Some(last) = prepared.last_sq else {
        log_error!("--live-from needs the current stream position, which could not be determined");
        return Err(());
    };
    match resolve_live_from(value, last, prepared.target_duration) {
        Ok(live_from) => {
            log_general!("{}", live_from.message);
            Ok(Some(live_from.start_sq))
        }
        Err(error) => {
            log_error!("{error}");
            Err(())
        }
    }
}

fn select_streams(sel: &FormatSelection) -> Vec<(DataType, u32)> {
    [
        sel.audio_url.as_ref().map(|_| (DataType::Audio, AUDIO_ITAG)),
        sel.video_url.as_ref().zip(sel.video_itag).map(|(_, itag)| (DataType::Video, itag)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

pub(super) async fn download(
    mut opts: Arc<Options>,
    http: Http,
    cancel: CancellationToken,
    mut prepared: Prepared,
    interrupt: Arc<Interrupt>,
) -> i32 {
    let (dir, file_name) = match final_location(&opts, &prepared.format_info) {
        Ok(location) => location,
        Err(error) => {
            log_error!("{error}");
            return 1;
        }
    };
    let final_dir = ensure_dir(dir, opts.dir_mode, "final file");
    if let Some(temp_dir) = &opts.temp_dir {
        let resolved = ensure_dir(temp_dir.clone(), opts.dir_mode, "temporary");
        if resolved != *temp_dir {
            let mut updated = (*opts).clone();
            updated.temp_dir = Some(resolved);
            opts = Arc::new(updated);
        }
    }

    let mut streams = select_streams(&prepared.sel);
    if streams.is_empty() {
        log_error!("Neither audio nor video downloads were started.");
        return 1;
    }
    let itags: Vec<u32> = streams.iter().map(|stream| stream.1).collect();
    let (mut states, mut prev_temp) = load_states(&opts, &prepared.video_id, &itags);
    let mut resuming = states.iter().any(|state| state.fragments > 0);

    // --live-from and --start-delay are ignored when resuming.
    let mut start_sq = None;
    if resuming {
        if opts.live_from.is_some() {
            log_warn!("Option --live-from is being ignored as a download is being resumed.");
        }
    } else {
        match live_from_start(&opts, &prepared) {
            Ok(sequence) => start_sq = sequence,
            Err(()) => return 1,
        }
    }
    if let Some(delay) = opts.start_delay {
        if resuming {
            log_warn!("Option --start-delay is being ignored as a download is being resumed.");
        } else if prepared.live
            && let Some(last) = prepared.last_sq
        {
            let (sequence, wait) = start_delay(delay, last, prepared.target_duration);
            log_general!("Waiting {} before starting to download...", seconds_to_duration_and_time_str(wait.as_secs()));
            log_debug!("Will start from sequence {sequence} [current is {last}]");
            if !sleep_wall_clock(&cancel, wait).await {
                return interrupt.cancel_code();
            }
            start_sq = Some(sequence);
            if wait > Duration::from_secs(DEFAULT_POLL_SECS)
                && let Some(extractor) = prepared.extractor.as_mut()
            {
                match extractor.get_video_info().await {
                    Some(sel) => {
                        // The video itag is fixed by now, and a fixed selection
                        // has no label; the saved download keeps the chosen one.
                        let label = mem::take(&mut prepared.sel.label);
                        prepared.sel = sel;
                        if prepared.sel.label.is_empty() {
                            prepared.sel.label = label;
                        }
                        prepared.live = extractor.live;
                        if let Some(latest) = extractor.last_sq {
                            prepared.last_sq = Some(latest);
                        }
                        if extractor.target_duration > 0 {
                            prepared.target_duration = extractor.target_duration;
                        }
                        let updated = select_streams(&prepared.sel);
                        if updated.is_empty() {
                            log_error!("Neither audio nor video downloads were started.");
                            return 1;
                        }
                        if updated != streams {
                            streams = updated;
                            let itags: Vec<u32> = streams.iter().map(|stream| stream.1).collect();
                            (states, prev_temp) = load_states(&opts, &prepared.video_id, &itags);
                            resuming = states.iter().any(|state| state.fragments > 0);
                        }
                    }
                    None => {
                        if interrupt.is_set() {
                            return interrupt.cancel_code();
                        }
                        log_error!("Got an error when re-grabbing video info after the delay period elapsed. Exiting.");
                        return 1;
                    }
                }
            }
        }
    }
    // A stream with nothing stored yet starts where the resumed one did;
    // from the stream's beginning the two would cover different parts.
    if resuming {
        start_sq = states.iter().filter(|state| state.fragments > 0).map(|state| state.start_frag).min();
    }

    let plan =
        OutputPlan { temp_dir: temp_dir(&opts, prev_temp, &final_dir, &prepared.video_id), final_dir, file_name };
    for state in &mut states {
        state.temp_dir = plan.temp_dir.clone();
    }

    // Thumbnail and description go to the temp dir first.
    let mut thumbnail = None;
    if (opts.embed_thumbnail || opts.write_thumbnail) && !prepared.thumbnail_url.is_empty() {
        let temp = plan.temp(".jpg");
        // Kept by a run that left fragments to retry.
        let kept = resuming && fs::metadata(&temp).is_ok_and(|metadata| metadata.len() > 0);
        let fetched = if kept {
            log_debug!("Reusing the thumbnail kept in {}", plan.temp_dir.display());
            Ok(())
        } else {
            http.download_to_file_cancellable(&prepared.thumbnail_url, &temp, opts.file_mode, &cancel).await
        };
        match fetched {
            Ok(()) => thumbnail = Some(Moved { temp, dest: plan.final_path(".jpg") }),
            Err(Error::Cancelled) => {
                if !resuming && !prepared.kept_other_stream {
                    plan.remove_temp_dir(&prepared.video_id);
                }
                return interrupt.cancel_code();
            }
            Err(error) => log_warn!("Failed to download thumbnail: {error}"),
        }
    }
    let mut description = None;
    if opts.write_description
        && let Some(comment) = prepared.metadata.get("comment").filter(|text| !text.is_empty())
    {
        let temp = plan.temp(".description");
        match write_file_with_mode(&temp, comment.as_bytes(), opts.file_mode).await {
            Ok(()) => description = Some(Moved { temp, dest: plan.final_path(".description") }),
            Err(error) => log_warn!("Error writing description file: {error}"),
        }
    }

    let moved_ts =
        |itag: u32| Moved { temp: plan.temp(&format!(".f{itag}.ts")), dest: plan.final_path(&format!(".f{itag}.ts")) };
    let audio_ts = streams.iter().find(|stream| stream.0 == DataType::Audio).map(|stream| moved_ts(stream.1));
    let video_ts = streams.iter().find(|stream| stream.0 == DataType::Video).map(|stream| moved_ts(stream.1));

    // Computed after the thumbnail download so a failed download isn't
    // referenced (Go built the command first).
    let embed = thumbnail.as_ref().filter(|_| opts.embed_thumbnail).map(|moved| moved.dest.as_path());
    let main_input = MuxInput {
        audio: audio_ts.as_ref().map(|moved| moved.dest.as_path()),
        video: video_ts.as_ref().map(|moved| moved.dest.as_path()),
        thumbnail: embed,
        mkv: opts.mkv,
        metadata: opts.add_metadata.then_some(&prepared.metadata),
        out_dir: &plan.final_dir,
        file_name: &plan.file_name,
    };
    let mux = ffmpeg_args(&main_input);
    let audio_mux = (opts.separate_audio && audio_ts.is_some() && video_ts.is_some())
        .then(|| ffmpeg_args(&MuxInput { video: None, ..main_input }));
    let mux_file = Moved { temp: plan.temp(".ffmpeg.txt"), dest: plan.final_path(".ffmpeg.txt") };
    let mux_text = mux_file_contents(&opts.ffmpeg_path, &mux, audio_mux.as_ref());
    if let Err(error) = write_file_with_mode(&mux_file.temp, mux_text.as_bytes(), opts.file_mode).await {
        log_warn!("Failed to write initial mux file: {error}");
    }

    let session = Session::new(
        opts.clone(),
        http,
        cancel,
        prepared.extractor.take(),
        &prepared.sel,
        prepared.live,
        prepared.target_duration,
    );
    if prepared.unavailable {
        session.set_unavailable();
    }
    let saved = Arc::new(SavedDownload {
        video_id: prepared.video_id.clone(),
        format_info: prepared.format_info.clone(),
        metadata: prepared.metadata.clone(),
        thumbnail_url: prepared.thumbnail_url.clone(),
        target_duration: prepared.target_duration,
        label: prepared.sel.label.clone(),
        video_itag: prepared.sel.video_itag,
    });
    let refresher = session.spawn_periodic_refresh();

    let (sender, receiver) = mpsc::unbounded_channel();
    let reporter = tokio::spawn(report_progress(receiver, opts.status_newlines));

    let mut tasks = Vec::new();
    for ((data_type, itag), state) in streams.iter().copied().zip(states) {
        let adopt_renamed = plan.owns_temp_dir(&prepared.video_id);
        let file = moved_ts(itag).temp;
        log_info!("Starting download to {}", file.display());
        let state_file = (!opts.disable_save_state).then(|| state.file.clone());
        tasks.push(tokio::spawn(download_stream(
            session.clone(),
            StreamParams {
                data_type,
                data_file: file,
                frag_prefix: plan.temp(&format!(".f{itag}")),
                state,
                state_file,
                last_sq: prepared.last_sq,
                start_sq,
                capture_duration: opts.capture_duration,
                retry_sleep: Duration::from_secs(u64::from(prepared.target_duration.max(1))),
                progress: sender.clone(),
                saved: Some(saved.clone()),
                memory_limit: MEMORY_LIMIT,
                adopt_renamed,
            },
        )));
    }
    drop(sender);

    let mut frags: PerType<Option<u64>> = PerType::default();
    let mut storage_failed = false;
    let mut stored_before_failing = false;
    let mut retry_until: Option<u64> = None;
    let mut pending_exports: Vec<PendingExport> = Vec::new();
    let mut state_files = Vec::new();
    for (task, (data_type, itag)) in tasks.into_iter().zip(&streams) {
        state_files.push(state_file(&opts, &prepared.video_id, *itag));
        match task.await {
            Ok(Ok(out)) => {
                if !out.skipped.is_empty() {
                    log_warn!(
                        "{}: {} fragment(s) could not be downloaded and are missing: {:?}",
                        data_type.as_str(),
                        out.skipped.len(),
                        out.skipped
                    );
                }
                *frags.get_mut(*data_type) = Some(out.state.fragments);
                if let Some(time) = out.retry_until {
                    retry_until = Some(retry_until.map_or(time, |until| until.max(time)));
                }
                if let Some(export) = out.pending_export {
                    pending_exports.push(export);
                }
            }
            Ok(Err(error)) => {
                // Every error a download returns is about its storage. One
                // that isn't from opening it came after data was stored.
                storage_failed = true;
                stored_before_failing |= !matches!(error, Error::OpenStorage(_));
                log_error!("{} download failed: {error}", data_type.as_str());
            }
            Err(error) => {
                storage_failed = true;
                stored_before_failing = true;
                log_error!("{} download task panicked: {error}", data_type.as_str());
            }
        }
    }
    if let Err(error) = reporter.await {
        log_debug!("The progress reporter stopped unexpectedly: {error}");
    }
    if let Some(handle) = refresher {
        handle.abort();
    }
    finish_inline();

    let mut fin = Finish {
        opts: &opts,
        plan: &plan,
        video_id: &prepared.video_id,
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
        keep_other_streams: prepared.kept_other_stream,
        pending_exports,
        interrupt: Some(&interrupt),
    };
    if storage_failed {
        let had_data = resuming
            || stored_before_failing
            || [frags.audio, frags.video].iter().flatten().any(|fragments| *fragments > 0);
        return fin.on_storage_error(had_data);
    }
    if interrupt.terminated() {
        return fin.on_terminate();
    }
    if interrupt.is_set()
        && let Some(code) = fin.on_cancel().await
    {
        return code;
    }
    fin.finish().await
}
