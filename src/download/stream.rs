//! Downloading one stream (audio or video): parallel fragment workers feeding
//! a storage sink, with resumable state.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;

use super::dispatch::{Dispatch, Take};
use super::fragment::download_fragment;
use super::recovery::{recover, Span};
use super::sink::{self, Event, Sink};
use super::ts_sink::remove_frag_file;
use crate::error::Result;
use crate::live_from::seekable_start_sq;
use crate::session::Session;
use crate::types::{DataType, DownloadState, FragThreadState, Fragment, ProgressInfo, StorageMode};
use crate::util::set_mode;
use crate::{log_debug, log_error, log_warn};

/// How long a worker waits when it's ahead of the newest fragment.
const AHEAD_WAIT: Duration = Duration::from_millis(500);

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
}

#[derive(Debug)]
pub struct StreamOutcome {
    pub state: DownloadState,
    /// Fragments that could not be downloaded and were skipped.
    pub skipped: Vec<u64>,
}

enum Msg {
    Done(Fragment),
    /// Given up on; the sink skips it instead of stalling forever.
    Missing(u64),
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

async fn worker(
    s: Arc<Session>,
    dt: DataType,
    name: String,
    dispatch: Arc<Mutex<Dispatch>>,
    tx: mpsc::Sender<Msg>,
    frag_prefix: Option<PathBuf>,
    retry_sleep: Duration,
) {
    let mut st = FragThreadState::new(name.clone(), dt, retry_sleep);
    loop {
        if s.is_stopping() || s.is_finished(dt) {
            break;
        }
        let take = dispatch.lock().unwrap().take(s.is_live());
        let (seq, max) = match take {
            // Only this worker stops. Go marked the whole stream finished here,
            // which made other workers abandon fragments they were retrying.
            Take::Done => {
                log_debug!("{name}: Stream is finished and highest sequence reached");
                break;
            }
            Take::Wait => {
                tokio::select! {
                    _ = s.cancel.cancelled() => break,
                    _ = tokio::time::sleep(AHEAD_WAIT) => continue,
                }
            }
            Take::Seq(seq, max) => (seq, max),
        };
        st.reset_for(seq, max);
        let frag_file = frag_prefix
            .as_ref()
            .map(|p| PathBuf::from(format!("{}.frag{seq}.ts", p.display())));
        let frag = download_fragment(&s, &mut st, frag_file).await;
        dispatch.lock().unwrap().complete(frag.as_ref().and_then(|f| f.x_head_seqnum));
        let msg = match frag {
            Some(f) => Msg::Done(f),
            None => Msg::Missing(seq),
        };
        if tx.send(msg).await.is_err() {
            break;
        }
    }
    log_debug!("{name}: exiting");
}

/// Downloads one stream until it ends, is stopped, or fails to store data.
pub async fn download_stream(s: Arc<Session>, mut p: StreamParams) -> Result<StreamOutcome> {
    let dt = p.data_type;
    let name = format!("{}-download", dt.as_str());
    let sqlite = s.opts.storage == StorageMode::Sqlite;
    let mode = s.opts.file_mode;

    if p.state.fragments > 0 && p.start_sq.is_some() {
        log_warn!("{name}: Options --live-from/--start-delay are ignored as a download is being resumed.");
    }
    let td = s.target_duration();
    let state = std::mem::take(&mut p.state);
    let opened = sink::open(sqlite, &p.data_file, state, || fresh_start(&p, td), mode, &name).await;
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

    let end_seq = p.capture_duration.map(|d| cur + d.as_secs().div_ceil(u64::from(td)));
    let dispatch = Arc::new(Mutex::new(Dispatch {
        next: cur,
        max_seq: p.last_sq,
        end_seq,
        in_flight: 0,
        skip: opened.skip,
    }));

    let threads = s.opts.threads.max(1);
    let (tx, mut rx) = mpsc::channel::<Msg>(threads * 2);
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
    let head = dispatch.lock().unwrap().max_seq;
    if s.opts.recover_private
        && store_err.is_none()
        && !s.is_stopping()
        && let Sink::Db(db, _) = &mut sink
        && let Some(head) = head
    {
        let mut span = Span { start: cur, head, end: end_seq };
        if s.is_unavailable() {
            match recover(&s, dt, db, span, |n| record(&mut p, Some(head), n, mode)).await {
                Ok(sp) => span = sp,
                Err(e) => {
                    log_error!("{name}: Error storing recovered fragments: {e}");
                    store_err = Some(e);
                }
            }
        }
        match db.stored() {
            Ok(stored) => skipped = span.missing(&stored),
            Err(e) => store_err = store_err.or(Some(e)),
        }
    }
    sink.finish(&name, mode).await?;
    if let Some(e) = store_err {
        return Err(e);
    }
    skipped.sort_unstable();
    log_debug!("{name} thread closing");
    Ok(StreamOutcome { state: p.state, skipped })
}

/// Loads a saved state file, if present and valid.
pub fn load_state(path: &Path) -> Option<DownloadState> {
    let data = std::fs::read(path).ok()?;
    let mut st: DownloadState = serde_json::from_slice(&data).ok()?;
    st.file = path.to_path_buf();
    Some(st)
}
