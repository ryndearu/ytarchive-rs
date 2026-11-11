//! Top-level flow for one stream: resolve the URL, fetch info, download
//! (output paths, the audio/video download tasks), and finish (merge or
//! handle an interrupt).

use std::collections::HashSet;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::download::{StreamParams, download_stream};
use crate::finish::{Finish, Moved};
use crate::http::Http;
use crate::input_url::InputUrl;
use crate::interrupt::Interrupt;
use crate::live_from::{resolve_live_from, start_delay};
use crate::log::{Totals, report_progress};
use crate::mux::{MuxInput, command_line, ffmpeg_args};
use crate::output::{OutputPlan, ensure_dir, final_location, load_states, temp_dir};
use crate::prepare::{Prepared, prepare, prepared_from_kept};
use crate::session::Session;
use crate::types::{AUDIO_ITAG, DEFAULT_POLL_SECS, DataType, Options, PerType, SavedDownload};
use crate::util::{seconds_to_duration_and_time_str, write_file_with_mode};
use crate::{log_debug, log_error, log_general, log_info, log_warn};

pub fn exit_code(code: i32) -> ExitCode {
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

/// Result of one run: exit code, the video it was for (if resolved), and
/// whether a download was attempted.
pub struct RunResult {
    pub code: i32,
    pub video_id: Option<String>,
    pub downloaded: bool,
}

impl RunResult {
    fn code(code: i32) -> Self {
        Self { code, video_id: None, downloaded: false }
    }
}

/// Downloads one stream. `skip_ids`: videos already recorded by the channel
/// monitor, which are not downloaded again.
pub async fn run(
    opts: Arc<Options>,
    http: Http,
    input: &InputUrl,
    interrupt: Arc<Interrupt>,
    skip_ids: &HashSet<String>,
) -> RunResult {
    let cancel = interrupt.child_token();
    let prepared = match prepare(&opts, &http, &cancel, input).await {
        Some(prepared) => prepared,
        None => {
            // A download kept for retrying missing fragments can still go on
            // with its saved URLs after the stream went private.
            if !interrupt.is_set()
                && let InputUrl::Video { id } = input
                && let Some(prepared) = prepared_from_kept(&opts, id)
            {
                let code = download(opts.clone(), http, cancel, prepared, None, interrupt).await;
                return RunResult { code, video_id: Some(id.clone()), downloaded: true };
            }
            return RunResult::code(if interrupt.is_set() { 2 } else { 1 });
        }
    };
    if skip_ids.contains(&prepared.video_id) {
        log_info!("{} was already downloaded; not downloading it again", prepared.video_id);
        return RunResult { code: 0, video_id: Some(prepared.video_id), downloaded: false };
    }
    let video_id = Some(prepared.video_id.clone());

    let live_from_sq = match (&opts.live_from, prepared.last_sq) {
        (None, _) => None,
        (Some(value), Some(last)) => match resolve_live_from(value, last, prepared.target_duration) {
            Ok(live_from) => {
                log_general!("{}", live_from.message);
                Some(live_from.start_sq)
            }
            Err(error) => {
                log_error!("{error}");
                return RunResult::code(1);
            }
        },
        (Some(_), None) => {
            log_error!("--live-from needs the current stream position, which could not be determined");
            return RunResult::code(1);
        }
    };

    let code = download(opts, http, cancel, prepared, live_from_sq, interrupt).await;
    RunResult { code, video_id, downloaded: true }
}

async fn download(
    opts: Arc<Options>,
    http: Http,
    cancel: CancellationToken,
    mut prepared: Prepared,
    live_from_sq: Option<u64>,
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
        ensure_dir(temp_dir.clone(), opts.dir_mode, "temporary");
    }

    let streams: Vec<(DataType, u32)> = [
        prepared.sel.audio_url.as_ref().map(|_| (DataType::Audio, AUDIO_ITAG)),
        prepared.sel.video_url.as_ref().zip(prepared.sel.video_itag).map(|(_, itag)| (DataType::Video, itag)),
    ]
    .into_iter()
    .flatten()
    .collect();
    if streams.is_empty() {
        log_error!("Neither audio nor video downloads were started.");
        return 1;
    }
    let itags: Vec<u32> = streams.iter().map(|stream| stream.1).collect();
    let (mut states, prev_temp) = load_states(&opts, &prepared.video_id, &itags);
    let resuming = states.iter().any(|state| state.fragments > 0);

    // --start-delay (ignored when resuming).
    let mut start_sq = live_from_sq;
    if let Some(delay) = opts.start_delay {
        if resuming {
            log_warn!("Option --start-delay is being ignored as a download is being resumed.");
        } else if prepared.live
            && let Some(last) = prepared.last_sq
        {
            let (sequence, wait) = start_delay(delay, last, prepared.target_duration);
            log_general!("Waiting {} before starting to download...", seconds_to_duration_and_time_str(wait.as_secs()));
            log_debug!("Will start from sequence {sequence} [current is {last}]");
            tokio::select! {
                _ = cancel.cancelled() => return 2,
                _ = tokio::time::sleep(wait) => {}
            }
            start_sq = Some(sequence);
            if wait > Duration::from_secs(DEFAULT_POLL_SECS)
                && let Some(extractor) = prepared.extractor.as_mut()
            {
                match extractor.get_video_info().await {
                    Some(sel) => {
                        prepared.sel = sel;
                        prepared.live = extractor.live;
                    }
                    None => {
                        log_error!("Got an error when re-grabbing video info after the delay period elapsed. Exiting.");
                        return 1;
                    }
                }
            }
        }
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
        match http.download_to_file(&prepared.thumbnail_url, &temp, opts.file_mode).await {
            Ok(()) => thumbnail = Some(Moved { temp, dest: plan.final_path(".jpg") }),
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
    if let Err(error) =
        write_file_with_mode(&mux_file.temp, command_line(&opts.ffmpeg_path, &mux.args).as_bytes(), opts.file_mode)
            .await
    {
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

    let resumed = states.iter().any(|state| state.fragments > 0);
    let mut initial = Totals::default();
    for ((data_type, _), state) in streams.iter().zip(&states) {
        match data_type {
            DataType::Audio => initial.audio_frags = state.fragments,
            DataType::Video => initial.video_frags = state.fragments,
        }
        initial.bytes += state.size;
    }
    let (sender, receiver) = mpsc::unbounded_channel();
    let reporter = tokio::spawn(report_progress(receiver, initial, opts.status_newlines));

    let mut tasks = Vec::new();
    for ((data_type, itag), state) in streams.iter().copied().zip(states) {
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
            },
        )));
    }
    drop(sender);

    let mut frags: PerType<Option<u64>> = PerType::default();
    let mut storage_failed = false;
    let mut retry_until: Option<u64> = None;
    let mut state_files = Vec::new();
    for (task, (data_type, itag)) in tasks.into_iter().zip(&streams) {
        state_files.push(crate::output::state_file(&opts, &prepared.video_id, *itag));
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
            }
            Ok(Err(error)) => {
                storage_failed |= matches!(error, crate::error::Error::OpenStorage(_));
                log_error!("{} download failed: {error}", data_type.as_str());
            }
            Err(error) => log_error!("{} download task panicked: {error}", data_type.as_str()),
        }
    }
    if let Err(error) = reporter.await {
        log_debug!("The progress reporter stopped unexpectedly: {error}");
    }
    if let Some(handle) = refresher {
        handle.abort();
    }
    eprintln!();

    let fin = Finish {
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
    };
    if storage_failed {
        let had_data = resumed || [frags.audio, frags.video].iter().flatten().any(|fragments| *fragments > 0);
        return fin.on_storage_error(had_data);
    }
    if interrupt.is_set()
        && let Some(code) = fin.on_cancel().await
    {
        return code;
    }
    fin.finish().await
}
