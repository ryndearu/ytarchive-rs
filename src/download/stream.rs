//! Downloading one stream (audio or video): parallel fragment workers feeding
//! a storage sink, with resumable state.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;

use super::dispatch::Dispatch;
use super::kept_db::{Saved, save_end, save_info, save_progress};
use super::recovery::{Span, recover, usable_until};
use super::sink::{self, Event, Sink};
use super::ts_sink::remove_frag_file;
use super::worker::{Msg, worker};
use crate::error::{Error, Result};
use crate::live_from::seekable_start_sq;
use crate::session::Session;
use crate::types::{DataType, DownloadState, ProgressInfo, SavedDownload, StorageMode};
use crate::util::{lock, set_mode};
use crate::{log_debug, log_error, log_trace, log_warn};

pub struct StreamParams {
    pub data_type: DataType,
    /// The `.ts` file fragments end up in.
    pub data_file: PathBuf,
    /// Prefix for temp fragment files (`{prefix}.frag{seq}.ts`).
    pub frag_prefix: PathBuf,
    /// Resume state; `fragments > 0` means a previous run can be continued.
    pub state: DownloadState,
    /// Where to persist `state` (`None` with `--disable-save-state`).
    pub state_file: Option<PathBuf>,
    pub last_sq: Option<u64>,
    /// From `--live-from` / `--start-delay`.
    pub start_sq: Option<u64>,
    pub capture_duration: Option<Duration>,
    /// Delay between attempts of a failing fragment.
    pub retry_sleep: Duration,
    pub progress: mpsc::UnboundedSender<ProgressInfo>,
    /// Stored in the SQLite database for retrying in a later run.
    pub saved: Option<Arc<SavedDownload>>,
}

#[derive(Debug)]
pub struct StreamOutcome {
    pub state: DownloadState,
    /// Fragments that could not be downloaded and were skipped.
    pub skipped: Vec<u64>,
    /// Set when fragments are missing but a later run can retry them (the
    /// database has a download URL that hasn't expired): until when, in
    /// Unix seconds (`u64::MAX` if the URLs don't say).
    pub retry_until: Option<u64>,
}

/// Where a fresh download starts: `--live-from`/`--start-delay`, else the
/// beginning (or as far back as YouTube still serves).
fn fresh_start(params: &StreamParams, target_duration: u32) -> u64 {
    let name = params.data_type.as_str();
    if let Some(start) = params.start_sq {
        log_debug!("{name}: Starting from sequence {start} (latest is {:?})", params.last_sq);
        return start;
    }
    let cur = params.last_sq.map_or(0, |last| seekable_start_sq(last, target_duration));
    if cur > 0 {
        log_warn!(
            "{name}: YT only retains the livestream 7 days past for seeking, starting from sequence {cur} (latest is {:?})",
            params.last_sq
        );
    }
    cur
}

/// Accounts for a stored fragment of `n` bytes.
fn record(params: &mut StreamParams, max_seq: Option<u64>, bytes: u64, mode: u32) {
    params.state.fragments += 1;
    params.state.size += bytes;
    save_state(params, mode);
    let info =
        ProgressInfo { data_type: params.data_type, byte_count: bytes, max_seq, start_frag: params.state.start_frag };
    // The progress line is optional; the download goes on without it.
    if params.progress.send(info).is_err() {
        log_trace!("{}: the progress reporter has stopped", params.data_type.as_str());
    }
}

fn save_state(params: &StreamParams, mode: u32) {
    let Some(path) = &params.state_file else { return };
    let res = serde_json::to_vec(&params.state)
        .map_err(std::io::Error::other)
        .and_then(|data| std::fs::write(path, data))
        .and_then(|()| set_mode(path, mode));
    if let Err(error) = res {
        log_warn!("Error when saving state: {error}");
    }
}

/// Downloads one stream until it ends, is stopped, or fails to store data.
pub async fn download_stream(session: Arc<Session>, mut params: StreamParams) -> Result<StreamOutcome> {
    let data_type = params.data_type;
    let name = format!("{}-download", data_type.as_str());
    let mode = session.opts.file_mode;

    if params.state.fragments > 0 && params.start_sq.is_some() {
        log_warn!("{name}: Options --live-from/--start-delay are ignored as a download is being resumed.");
    }
    let target = session.target_duration();
    let state = std::mem::take(&mut params.state);
    let want_sqlite = session.opts.storage == StorageMode::Sqlite;
    let opened = sink::open(want_sqlite, &params.data_file, state, || fresh_start(&params, target), mode, &name).await;
    let opened = match opened {
        Ok(opened) => opened,
        Err(error) => {
            log_error!("{name}: Error opening storage for {}: {error}", params.data_file.display());
            session.stop();
            return Err(Error::OpenStorage(Box::new(error)));
        }
    };
    let mut sink = opened.sink;
    let cur = opened.cur;
    params.state = opened.state;
    save_state(&params, mode);
    // Only --recover-private keeps stream info and URLs in the database.
    let recovering = session.opts.recover_private && sink.is_db();
    let mut saved = Saved::default();
    if recovering && let Sink::Db(database, _) = &sink {
        match save_info(&session, data_type, database, params.saved.as_deref()) {
            Ok(count) => saved.urls = count,
            Err(error) => log_warn!("{name}: Error saving the download info for later retries: {error}"),
        }
    }

    let end_seq = params.capture_duration.map(|duration| cur + duration.as_secs().div_ceil(u64::from(target)));
    let dispatch =
        Arc::new(Mutex::new(Dispatch { next: cur, max_seq: params.last_sq, end_seq, in_flight: 0, skip: opened.skip }));

    // A kept download whose stream can't be looked up anymore goes straight
    // to recovery (with SQLite); the workers would only use up their tries.
    let straight_to_recovery = recovering && session.is_unavailable();
    let threads = if straight_to_recovery { 0 } else { session.opts.threads.max(1) };
    let (sender, mut receiver) = mpsc::channel::<Msg>(threads.max(1) * 2);
    // SQLite stores fragments as they arrive, so nothing waits in temp files.
    let frag_prefix = (session.opts.frag_files && !sink.is_db()).then(|| params.frag_prefix.clone());
    let mut workers = Vec::with_capacity(threads);
    for index in 1..=threads {
        workers.push(tokio::spawn(worker(
            session.clone(),
            data_type,
            format!("{}{index}", data_type.as_str()),
            dispatch.clone(),
            sender.clone(),
            frag_prefix.clone(),
            params.retry_sleep,
        )));
    }
    drop(sender);

    let mut skipped = Vec::new();
    let mut store_err = None;
    while let Some(msg) = receiver.recv().await {
        let (seq, frag) = match msg {
            Msg::Done(fragment) => (fragment.seq, Some(fragment)),
            Msg::Missing(seq) => (seq, None),
        };
        let events = match sink.push(seq, frag).await {
            Ok(events) => events,
            Err(error) => {
                log_error!("{name}: Error storing fragment {seq} for {}: {error}", params.data_file.display());
                store_err = Some(error);
                session.stop();
                break;
            }
        };
        for event in events {
            match event {
                Event::Wrote(bytes) => {
                    let max_seq = lock(&dispatch).max_seq;
                    record(&mut params, max_seq, bytes, mode);
                    if recovering {
                        save_progress(&session, data_type, &sink, max_seq, &mut saved);
                    }
                }
                Event::Gap(gap) => {
                    // Fragments near the head of an ended stream often never
                    // existed; only report real gaps.
                    let max = lock(&dispatch).max_seq;
                    if !session.is_stopping() && max.is_none_or(|newest| gap + 2 < newest) {
                        log_warn!("{name}: Fragment {gap} could not be downloaded and was skipped");
                        skipped.push(gap);
                    }
                }
            }
        }
    }

    // Drain so workers blocked on send can exit, cleaning up their files.
    receiver.close();
    while let Some(msg) = receiver.recv().await {
        if let Msg::Done(fragment) = msg {
            remove_frag_file(&fragment.data).await;
        }
    }
    for worker in workers {
        if let Err(error) = worker.await {
            log_warn!("{name}: A download worker stopped unexpectedly: {error}");
        }
    }

    // --recover-private: recover what's missing if the stream went private,
    // then take the missing list from the database (it also covers
    // fragments after the point where the workers stopped).
    let mut head = lock(&dispatch).max_seq;
    let mut retry_until = None;
    if recovering && let Sink::Db(database, _) = &mut sink {
        if store_err.is_none()
            && !session.is_stopping()
            && let Some(head_seq) = head
        {
            let mut span = Span { start: cur, head: head_seq, end: end_seq };
            if session.is_unavailable() {
                match recover(&session, data_type, database, span, |bytes| {
                    record(&mut params, Some(head_seq), bytes, mode)
                })
                .await
                {
                    Ok(new_span) => span = new_span,
                    Err(error) => {
                        log_error!("{name}: Error storing recovered fragments: {error}");
                        store_err = Some(error);
                    }
                }
            }
            head = Some(span.head);
            match database.stored() {
                Ok(stored) => skipped = span.missing(&stored),
                Err(error) => store_err = store_err.or(Some(error)),
            }
        }
        if let Err(error) = save_end(&session, data_type, database, head) {
            log_warn!("{name}: Error saving the download info for later retries: {error}");
        }
        if !skipped.is_empty() {
            retry_until = usable_until(&session.known_urls(data_type));
        }
    }
    sink.finish(&name, mode).await?;
    if let Some(error) = store_err {
        return Err(error);
    }
    skipped.sort_unstable();
    log_debug!("{name} thread closing");
    Ok(StreamOutcome { state: params.state, skipped, retry_until })
}

/// Loads a saved state file, if present and valid.
pub fn load_state(path: &Path) -> Option<DownloadState> {
    let data = std::fs::read(path).ok()?;
    let mut state: DownloadState = serde_json::from_slice(&data).ok()?;
    state.file = path.to_path_buf();
    Some(state)
}
