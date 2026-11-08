//! Downloading one stream (audio or video): parallel fragment workers feeding
//! a storage sink, with resumable state.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;

use super::dispatch::Dispatch;
use super::kept_db::{save_end, save_info, save_progress, Saved};
use super::recovery::{recover, usable_until, Span};
use super::sink::{self, Event, Sink};
use super::ts_sink::remove_frag_file;
use super::worker::{worker, Msg};
use crate::error::Result;
use crate::live_from::seekable_start_sq;
use crate::session::Session;
use crate::types::{DataType, DownloadState, ProgressInfo, SavedDownload, StorageMode};
use crate::util::set_mode;
use crate::{log_debug, log_error, log_warn};

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
fn fresh_start(p: &StreamParams, target_duration: u32) -> u64 {
    let name = p.data_type.as_str();
    if let Some(start) = p.start_sq {
        log_debug!("{name}: Starting from sequence {start} (latest is {:?})", p.last_sq);
        return start;
    }
    let cur = p.last_sq.map_or(0, |l| seekable_start_sq(l, target_duration));
    if cur > 0 {
        log_warn!(
            "{name}: YT only retains the livestream 7 days past for seeking, starting from sequence {cur} (latest is {:?})",
            p.last_sq
        );
    }
    cur
}

/// Accounts for a stored fragment of `n` bytes.
fn record(p: &mut StreamParams, max_seq: Option<u64>, n: u64, mode: u32) {
    p.state.fragments += 1;
    p.state.size += n;
    save_state(p, mode);
    let _ = p.progress.send(ProgressInfo {
        data_type: p.data_type,
        byte_count: n,
        max_seq,
        start_frag: p.state.start_frag,
    });
}

fn save_state(p: &StreamParams, mode: u32) {
    let Some(path) = &p.state_file else { return };
    let res = serde_json::to_vec(&p.state)
        .map_err(std::io::Error::other)
        .and_then(|d| std::fs::write(path, d))
        .and_then(|()| set_mode(path, mode));
    if let Err(e) = res {
        log_warn!("Error when saving state: {e}");
    }
}

/// Downloads one stream until it ends, is stopped, or fails to store data.
pub async fn download_stream(s: Arc<Session>, mut p: StreamParams) -> Result<StreamOutcome> {
    let dt = p.data_type;
    let name = format!("{}-download", dt.as_str());
    let mode = s.opts.file_mode;

    if p.state.fragments > 0 && p.start_sq.is_some() {
        log_warn!("{name}: Options --live-from/--start-delay are ignored as a download is being resumed.");
    }
    let td = s.target_duration();
    let state = std::mem::take(&mut p.state);
    let want_sqlite = s.opts.storage == StorageMode::Sqlite;
    let opened = sink::open(want_sqlite, &p.data_file, state, || fresh_start(&p, td), mode, &name).await;
    let opened = match opened {
        Ok(o) => o,
        Err(e) => {
            log_error!("{name}: Error opening storage for {}: {e}", p.data_file.display());
            s.stop();
            return Err(e);
        }
    };
    let mut sink = opened.sink;
    let cur = opened.cur;
    p.state = opened.state;
    save_state(&p, mode);
    // Only --recover-private keeps stream info and URLs in the database.
    let recovering = s.opts.recover_private && sink.is_db();
    let mut saved = Saved::default();
    if recovering && let Sink::Db(db, _) = &sink {
        match save_info(&s, dt, db, p.saved.as_deref()) {
            Ok(n) => saved.urls = n,
            Err(e) => log_warn!("{name}: Error saving the download info for later retries: {e}"),
        }
    }

    let end_seq = p.capture_duration.map(|d| cur + d.as_secs().div_ceil(u64::from(td)));
    let dispatch = Arc::new(Mutex::new(Dispatch {
        next: cur,
        max_seq: p.last_sq,
        end_seq,
        in_flight: 0,
        skip: opened.skip,
    }));

    // A kept download whose stream can't be looked up anymore goes straight
    // to recovery (with SQLite); the workers would only use up their tries.
    let straight_to_recovery = recovering && s.is_unavailable();
    let threads = if straight_to_recovery { 0 } else { s.opts.threads.max(1) };
    let (tx, mut rx) = mpsc::channel::<Msg>(threads.max(1) * 2);
    // SQLite stores fragments as they arrive, so nothing waits in temp files.
    let frag_prefix = (s.opts.frag_files && !sink.is_db()).then(|| p.frag_prefix.clone());
    let mut workers = Vec::with_capacity(threads);
    for i in 1..=threads {
        workers.push(tokio::spawn(worker(
            s.clone(),
            dt,
            format!("{}{i}", dt.as_str()),
            dispatch.clone(),
            tx.clone(),
            frag_prefix.clone(),
            p.retry_sleep,
        )));
    }
    drop(tx);

    let mut skipped = Vec::new();
    let mut store_err = None;
    while let Some(msg) = rx.recv().await {
        let (seq, frag) = match msg {
            Msg::Done(f) => (f.seq, Some(f)),
            Msg::Missing(seq) => (seq, None),
        };
        let events = match sink.push(seq, frag).await {
            Ok(ev) => ev,
            Err(e) => {
                log_error!("{name}: Error storing fragment {seq} for {}: {e}", p.data_file.display());
                store_err = Some(e);
                s.stop();
                break;
            }
        };
        for ev in events {
            match ev {
                Event::Wrote(n) => {
                    let max_seq = dispatch.lock().unwrap().max_seq;
                    record(&mut p, max_seq, n, mode);
                    if recovering {
                        save_progress(&s, dt, &sink, max_seq, &mut saved);
                    }
                }
                Event::Gap(gap) => {
                    // Fragments near the head of an ended stream often never
                    // existed; only report real gaps.
                    let max = dispatch.lock().unwrap().max_seq;
                    if !s.is_stopping() && max.is_none_or(|m| gap + 2 < m) {
                        log_warn!("{name}: Fragment {gap} could not be downloaded and was skipped");
                        skipped.push(gap);
                    }
                }
            }
        }
    }

    // Drain so workers blocked on send can exit, cleaning up their files.
    rx.close();
    while let Some(msg) = rx.recv().await {
        if let Msg::Done(f) = msg {
            remove_frag_file(&f.data).await;
        }
    }
    for w in workers {
        let _ = w.await;
    }

    // --recover-private: recover what's missing if the stream went private,
    // then take the missing list from the database (it also covers
    // fragments after the point where the workers stopped).
    let mut head = dispatch.lock().unwrap().max_seq;
    let mut retry_until = None;
    if recovering && let Sink::Db(db, _) = &mut sink {
        if store_err.is_none()
            && !s.is_stopping()
            && let Some(h) = head
        {
            let mut span = Span { start: cur, head: h, end: end_seq };
            if s.is_unavailable() {
                match recover(&s, dt, db, span, |n| record(&mut p, Some(h), n, mode)).await {
                    Ok(sp) => span = sp,
                    Err(e) => {
                        log_error!("{name}: Error storing recovered fragments: {e}");
                        store_err = Some(e);
                    }
                }
            }
            head = Some(span.head);
            match db.stored() {
                Ok(stored) => skipped = span.missing(&stored),
                Err(e) => store_err = store_err.or(Some(e)),
            }
        }
        if let Err(e) = save_end(&s, dt, db, head) {
            log_warn!("{name}: Error saving the download info for later retries: {e}");
        }
        if !skipped.is_empty() {
            retry_until = usable_until(&s.known_urls(dt));
        }
    }
    sink.finish(&name, mode).await?;
    if let Some(e) = store_err {
        return Err(e);
    }
    skipped.sort_unstable();
    log_debug!("{name} thread closing");
    Ok(StreamOutcome { state: p.state, skipped, retry_until })
}

/// Loads a saved state file, if present and valid.
pub fn load_state(path: &Path) -> Option<DownloadState> {
    let data = std::fs::read(path).ok()?;
    let mut st: DownloadState = serde_json::from_slice(&data).ok()?;
    st.file = path.to_path_buf();
    Some(st)
}
