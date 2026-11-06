//! Downloading one stream (audio or video): parallel fragment workers and an
//! in-order writer with resumable state.

use std::collections::BTreeMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc;

use super::fragment::download_fragment;
use crate::error::Result;
use crate::live_from::seekable_start_sq;
use crate::mp4_atoms::strip_for_concat;
use crate::session::Session;
use crate::types::{DataType, DownloadState, FragData, FragThreadState, Fragment, ProgressInfo};
use crate::util::set_mode;
use crate::{log_debug, log_info, log_warn};

/// How long a worker waits when it's ahead of the newest fragment.
const AHEAD_WAIT: Duration = Duration::from_millis(500);

pub struct StreamParams {
    pub data_type: DataType,
    pub itag: u32,
    /// The `.ts` file fragments are appended to.
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
    /// Given up on; the writer skips it instead of stalling forever.
    Missing(u64),
}

/// Hands out sequence numbers to workers, never running more than one
/// fragment past the newest known one.
#[derive(Debug)]
struct Dispatch {
    next: u64,
    max_seq: Option<u64>,
    end_seq: Option<u64>,
    in_flight: usize,
}

enum Take {
    Seq(u64, Option<u64>),
    Wait,
    Done,
}

impl Dispatch {
    fn take(&mut self, live: bool) -> Take {
        if self.end_seq.is_some_and(|e| self.next >= e) {
            log_debug!("Reached the maximum duration specified by --capture-duration.");
            return Take::Done;
        }
        if let Some(max) = self.max_seq {
            if !live && self.next >= max {
                return Take::Done;
            }
            if self.next > max + 1 && self.in_flight > 0 {
                return Take::Wait;
            }
        }
        let seq = self.next;
        self.next += 1;
        self.in_flight += 1;
        Take::Seq(seq, self.max_seq)
    }

    fn complete(&mut self, head: Option<u64>) {
        self.in_flight = self.in_flight.saturating_sub(1);
        if let Some(h) = head {
            self.max_seq = Some(self.max_seq.map_or(h, |m| m.max(h)));
        }
    }
}

/// Opens the data file. Returns the sequence to start at and whether a
/// previous download is being continued.
async fn open_data_file(p: &mut StreamParams, s: &Session) -> Result<(tokio::fs::File, u64)> {
    let name = p.data_type.as_str();
    if p.state.fragments > 0 {
        if p.start_sq.is_some() {
            log_warn!("{name}: Options --live-from/--start-delay are ignored as a download is being resumed.");
        }
        match tokio::fs::OpenOptions::new().read(true).write(true).open(&p.data_file).await {
            Ok(mut f) => {
                let len = f.metadata().await?.len();
                if len >= p.state.size {
                    // Drop anything written after the last saved state.
                    f.set_len(p.state.size).await?;
                    f.seek(SeekFrom::End(0)).await?;
                    let cur = p.state.start_frag + p.state.fragments;
                    log_info!("{name}: Resuming download from sequence {cur}");
                    return Ok((f, cur));
                }
                log_warn!("{name}: {} is smaller than the saved state; starting from the beginning", p.data_file.display());
            }
            Err(e) => {
                log_warn!("{name}: Failed to open {} to resume download: {e}", p.data_file.display());
                log_warn!("{name}: Will start from the beginning");
            }
        }
        let temp_dir = std::mem::take(&mut p.state.temp_dir);
        p.state = DownloadState { temp_dir, ..Default::default() };
    }

    let mut cur = p.last_sq.map_or(0, |l| seekable_start_sq(l, s.target_duration()));
    if let Some(start) = p.start_sq {
        cur = start;
        log_debug!("{name}: Starting from sequence {start} (latest is {:?})", p.last_sq);
    } else if cur > 0 {
        log_warn!(
            "{name}: YT only retains the livestream 7 days past for seeking, starting from sequence {cur} (latest is {:?})",
            p.last_sq
        );
    }
    p.state.start_frag = cur;
    let f = tokio::fs::File::create(&p.data_file).await?;
    let _ = set_mode(&p.data_file, s.opts.file_mode);
    Ok((f, cur))
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

async fn fragment_bytes(data: &FragData) -> std::io::Result<Vec<u8>> {
    match data {
        FragData::Memory(b) => Ok(b.to_vec()),
        FragData::File(p) => tokio::fs::read(p).await,
    }
}

async fn remove_frag_file(data: &FragData) {
    if let FragData::File(p) = data {
        let _ = tokio::fs::remove_file(p).await;
    }
}

/// Downloads one stream until it ends, is stopped, or fails to write.
pub async fn download_stream(s: Arc<Session>, mut p: StreamParams) -> Result<StreamOutcome> {
    let dt = p.data_type;
    let name = format!("{}-download", dt.as_str());
    let (mut file, cur) = match open_data_file(&mut p, &s).await {
        Ok(v) => v,
        Err(e) => {
            crate::log_error!("{name}: Error opening {} for writing: {e}", p.data_file.display());
            s.stop();
            return Err(e);
        }
    };
    save_state(&p, s.opts.file_mode);

    let end_seq = p.capture_duration.map(|d| {
        cur + d.as_secs().div_ceil(u64::from(s.target_duration()))
    });
    let dispatch = Arc::new(Mutex::new(Dispatch {
        next: cur,
        max_seq: p.last_sq,
        end_seq,
        in_flight: 0,
    }));

    let threads = s.opts.threads.max(1);
    let (tx, mut rx) = mpsc::channel::<Msg>(threads * 2);
    let frag_prefix = s.opts.frag_files.then(|| p.frag_prefix.clone());
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

    let mut next_write = cur;
    let mut pending: BTreeMap<u64, Option<Fragment>> = BTreeMap::new();
    let mut skipped = Vec::new();
    let mut write_err = None;

    'recv: while let Some(msg) = rx.recv().await {
        match msg {
            Msg::Done(f) => pending.insert(f.seq, Some(f)),
            Msg::Missing(seq) => pending.insert(seq, None),
        };
        while let Some(entry) = pending.remove(&next_write) {
            let Some(frag) = entry else {
                // Fragments near the head of an ended stream often never
                // existed; only report real gaps.
                let max = dispatch.lock().unwrap().max_seq;
                if !s.is_stopping() && max.is_none_or(|m| next_write + 2 < m) {
                    log_warn!("{name}: Fragment {next_write} could not be downloaded and was skipped");
                    skipped.push(next_write);
                }
                next_write += 1;
                continue;
            };
            let written = async {
                let raw = fragment_bytes(&frag.data).await?;
                let first = next_write == p.state.start_frag;
                let data = strip_for_concat(&raw, &frag.mime_type, first).unwrap_or(raw);
                file.write_all(&data).await?;
                Ok::<_, std::io::Error>(data.len() as u64)
            }
            .await;
            remove_frag_file(&frag.data).await;
            match written {
                Ok(n) => {
                    p.state.fragments += 1;
                    p.state.size += n;
                    save_state(&p, s.opts.file_mode);
                    let _ = p.progress.send(ProgressInfo {
                        data_type: dt,
                        itag: p.itag,
                        byte_count: n,
                        max_seq: dispatch.lock().unwrap().max_seq,
                        start_frag: p.state.start_frag,
                    });
                    next_write += 1;
                }
                Err(e) => {
                    crate::log_error!(
                        "{name}: Error writing fragment {next_write} to {}: {e}",
                        p.data_file.display()
                    );
                    write_err = Some(e);
                    s.stop();
                    break 'recv;
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
    let mut leftover = 0;
    for f in pending.into_values().flatten() {
        remove_frag_file(&f.data).await;
        leftover += 1;
    }
    if leftover > 0 {
        log_debug!("{name}: {leftover} downloaded fragment(s) after a gap were not written");
    }
    file.flush().await?;
    if let Some(e) = write_err {
        return Err(e.into());
    }
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
