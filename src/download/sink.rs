//! Where downloaded fragments are stored: the in-order `.ts` writer
//! (default) or the SQLite store (`--sqlite`).

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::io::{self, SeekFrom};
use std::mem;
use std::path::{Path, PathBuf};

use bytes::Bytes;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::task::spawn_blocking;

use super::db_sink::{DbSink, MetaUpdate, db_path};
use crate::error::Result;
use crate::mp4_atoms::strip_bytes;
use crate::types::{DownloadState, FragData, Fragment};
use crate::util::apply_mode;
use crate::{log_debug, log_info, log_warn};

/// Default storage: fragments appended in sequence order to the `.ts` file;
/// out-of-order arrivals wait in memory (or as temp files) until their turn.
pub struct TsSink {
    file: tokio::fs::File,
    path: PathBuf,
    next_write: u64,
    /// True once at least one fragment has been written, so only the first
    /// written fragment keeps its `ftyp` box even if `start_frag` was skipped.
    wrote_any: bool,
    pending: BTreeMap<u64, Option<Fragment>>,
    /// Bytes of `pending` held in memory (fragment files don't count).
    waiting_bytes: u64,
}

fn memory_bytes(entry: Option<&Fragment>) -> u64 {
    match entry.map(|fragment| &fragment.data) {
        Some(FragData::Memory(bytes)) => bytes.len() as u64,
        _ => 0,
    }
}

pub async fn fragment_bytes(data: &FragData) -> io::Result<Bytes> {
    match data {
        // Shares the downloaded buffer instead of copying it.
        FragData::Memory(bytes) => Ok(bytes.clone()),
        FragData::File(path) => Ok(tokio::fs::read(path).await?.into()),
    }
}

pub async fn remove_frag_file(data: &FragData) {
    if let FragData::File(path) = data
        && let Err(error) = tokio::fs::remove_file(path).await
        && error.kind() != io::ErrorKind::NotFound
    {
        log_debug!("Could not delete the fragment file {}: {error}", path.display());
    }
}

impl TsSink {
    /// Continues the file from `state` if it is consistent, else returns
    /// `None` so the caller starts fresh.
    pub async fn resume(path: &Path, state: &DownloadState, name: &str) -> io::Result<Option<(Self, u64)>> {
        match tokio::fs::OpenOptions::new().read(true).write(true).open(path).await {
            Ok(mut file) => {
                if file.metadata().await?.len() < state.size {
                    log_warn!(
                        "{name}: {} is smaller than the saved state; starting from the beginning",
                        path.display()
                    );
                    return Ok(None);
                }
                // Drop anything written after the last saved state.
                file.set_len(state.size).await?;
                file.seek(SeekFrom::End(0)).await?;
                let cur = state.start_frag + state.fragments + state.skipped;
                log_info!("{name}: Resuming download from sequence {cur}");
                Ok(Some((Self::new(file, path, cur, state.fragments > 0), cur)))
            }
            Err(error) => {
                log_warn!("{name}: Failed to open {} to resume download: {error}", path.display());
                log_warn!("{name}: Will start from the beginning");
                Ok(None)
            }
        }
    }

    pub async fn create(path: &Path, start: u64, mode: u32) -> io::Result<Self> {
        let file = tokio::fs::File::create(path).await?;
        apply_mode(path, mode);
        Ok(Self::new(file, path, start, false))
    }

    fn new(file: tokio::fs::File, path: &Path, next_write: u64, wrote_any: bool) -> Self {
        Self { file, path: path.to_path_buf(), next_write, wrote_any, pending: BTreeMap::new(), waiting_bytes: 0 }
    }

    /// Queues `frag` (`None` = given up) and writes everything now in order.
    pub async fn push(&mut self, seq: u64, frag: Option<Fragment>) -> io::Result<Vec<Event>> {
        self.waiting_bytes += memory_bytes(frag.as_ref());
        if let Some(replaced) = self.pending.insert(seq, frag) {
            self.waiting_bytes = self.waiting_bytes.saturating_sub(memory_bytes(replaced.as_ref()));
        }
        let mut events = Vec::new();
        while let Some(entry) = self.pending.remove(&self.next_write) {
            self.waiting_bytes = self.waiting_bytes.saturating_sub(memory_bytes(entry.as_ref()));
            let Some(frag) = entry else {
                events.push(Event::Gap(self.next_write));
                self.next_write += 1;
                continue;
            };
            let written = async {
                let raw = fragment_bytes(&frag.data).await?;
                let data = strip_bytes(raw, &frag.mime_type, !self.wrote_any);
                self.file.write_all(&data).await?;
                // tokio writes in the background and reports a failure only
                // on the next call; without this a fragment that never
                // reached the file (full disk) is counted in the saved
                // state, and the next run can't resume from it.
                self.file.flush().await?;
                Ok::<_, io::Error>(data.len() as u64)
            }
            .await;
            remove_frag_file(&frag.data).await;
            match written {
                Ok(bytes) => {
                    self.wrote_any = true;
                    events.push(Event::Wrote(bytes));
                }
                Err(error) => {
                    log_warn!("Error writing fragment {} to {}: {error}", self.next_write, self.path.display());
                    return Err(error);
                }
            }
            self.next_write += 1;
        }
        Ok(events)
    }

    /// Flushes; returns how many downloaded fragments never got written
    /// (stuck behind a gap when the download stopped).
    pub async fn finish(mut self) -> io::Result<usize> {
        let mut leftover = 0;
        for fragment in mem::take(&mut self.pending).into_values().flatten() {
            remove_frag_file(&fragment.data).await;
            leftover += 1;
        }
        self.file.flush().await?;
        Ok(leftover)
    }
}

/// What happened to fragments handed to a sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A fragment was stored; bytes stored.
    Wrote(u64),
    /// This sequence was given up on.
    Gap(u64),
}

pub enum Sink {
    Ts(TsSink),
    Db(DbSink, PathBuf),
}

/// An opened sink plus where downloading starts and what to skip.
pub struct Opened {
    pub sink: Sink,
    pub cur: u64,
    pub skip: HashSet<u64>,
    /// State after reconciling with what's on disk.
    pub state: DownloadState,
}

/// Opens storage for `data_file`, resuming if `state` says there is
/// something to resume. `fresh_start` is the sequence for a new download.
pub async fn open(
    sqlite: bool,
    data_file: &Path,
    mut state: DownloadState,
    fresh_start: impl FnOnce() -> u64,
    mode: u32,
    name: &str,
) -> Result<Opened> {
    let database_path = db_path(data_file);
    // A download is resumed in the storage it was started with, whatever
    // the flags say now: a .ts download isn't replaced by an empty
    // database's export (e.g. when rerun with --recover-private), and a
    // database isn't ignored.
    let resume_db = state.fragments > 0 && database_path.exists();
    let resume_ts = state.fragments > 0 && !database_path.exists() && data_file.exists();
    let use_db = resume_db || (sqlite && !resume_ts);
    if use_db != sqlite {
        let how = if use_db { "with SQLite" } else { "without SQLite" };
        log_info!("{name}: Resuming {} {how}, as it was started", data_file.display());
    }
    if use_db {
        let mut sink = DbSink::open_async(database_path.clone(), mode).await?;
        let mut skip = sink.stored_async().await?;
        let (mut fragments, mut size) = sink.totals_async().await?;
        let cur = if fragments > 0 && state.fragments > 0 {
            log_info!("{name}: Resuming from {}: {fragments} fragment(s) stored", sink.path().display());
            state.start_frag
        } else {
            if fragments > 0 {
                // Unresumable database (e.g. reused temp dir whose state has 0
                // fragments): set it aside rather than splicing two runs.
                drop(sink);
                keep_existing_db(&database_path, name).await;
                sink = DbSink::open_async(database_path, mode).await?;
                skip.clear();
                fragments = 0;
                size = 0;
            }
            let start = fresh_start();
            state.start_frag = start;
            start
        };
        state.fragments = fragments;
        state.size = size;
        return Ok(Opened { sink: Sink::Db(sink, data_file.to_path_buf()), cur, skip, state });
    }

    if state.fragments > 0
        && let Some((sink, cur)) = TsSink::resume(data_file, &state, name).await?
    {
        return Ok(Opened { sink: Sink::Ts(sink), cur, skip: Default::default(), state });
    }
    let temp_dir = mem::take(&mut state.temp_dir);
    state = DownloadState { temp_dir, file: state.file, ..Default::default() };
    let start = fresh_start();
    state.start_frag = start;
    keep_existing_data(data_file, name).await;
    let sink = TsSink::create(data_file, start, mode).await?;
    Ok(Opened { sink: Sink::Ts(sink), cur: start, skip: Default::default(), state })
}

/// Data that can't be resumed (its state file is missing, unreadable, or
/// doesn't match) is set aside as `{file}.old` rather than overwritten by
/// the new download. That only happens in a folder that is reused: the
/// other stream's state was still good, or the file is shorter than its
/// state says. With no usable state at all the run gets a new folder, and
/// the old one is left as it is.
async fn keep_existing_data(data_file: &Path, name: &str) -> bool {
    let has_data = tokio::fs::metadata(data_file).await.is_ok_and(|metadata| metadata.len() > 0);
    if !has_data {
        return false;
    }
    let mut old_name = data_file.as_os_str().to_os_string();
    old_name.push(".old");
    let old_file = PathBuf::from(old_name);
    match tokio::fs::rename(data_file, &old_file).await {
        Ok(()) => {
            log_warn!(
                "{name}: {} has data that can't be resumed; kept it as {}",
                data_file.display(),
                old_file.display()
            );
            true
        }
        Err(error) => {
            log_warn!("{name}: Could not keep the existing {}: {error}", data_file.display());
            false
        }
    }
}

pub(super) async fn keep_existing_db(database_path: &Path, name: &str) {
    if !keep_existing_data(database_path, name).await {
        return;
    }
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = database_path.as_os_str().to_os_string();
        sidecar.push(suffix);
        let sidecar_path = PathBuf::from(sidecar);
        let mut old_sidecar = database_path.as_os_str().to_os_string();
        old_sidecar.push(".old");
        old_sidecar.push(suffix);
        let old_sidecar_path = PathBuf::from(old_sidecar);
        match tokio::fs::rename(&sidecar_path, &old_sidecar_path).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // Remove any stale sidecar from an earlier backup so SQLite
                // never replays an unrelated WAL onto the newly saved DB.
                if let Err(remove_error) = tokio::fs::remove_file(&old_sidecar_path).await
                    && remove_error.kind() != io::ErrorKind::NotFound
                {
                    log_debug!("{name}: Could not delete stale {}: {remove_error}", old_sidecar_path.display());
                }
            }
            Err(error) => {
                log_warn!("{name}: Could not keep {}: {error}", sidecar_path.display());
            }
        }
    }
}

/// A SQLite database ready to export to `.ts`, deferred until all streams finish.
pub struct PendingExport {
    database: DbSink,
    out: PathBuf,
    name: String,
    mode: u32,
}

impl fmt::Debug for PendingExport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("PendingExport").field("out", &self.out).finish_non_exhaustive()
    }
}

impl PendingExport {
    pub fn out(&self) -> &Path {
        &self.out
    }

    #[cfg(test)]
    pub async fn for_test(database_path: &Path, out: PathBuf) -> Self {
        let mut database = DbSink::open(database_path, 0o644).unwrap();
        let frag = Fragment {
            seq: 0,
            x_head_seqnum: None,
            data: FragData::Memory(Bytes::from_static(b"frag")),
            mime_type: "video/webm".into(),
        };
        database.push(0, Some(frag)).await.unwrap();
        Self { database, out, name: "audio".into(), mode: 0o644 }
    }

    pub async fn export(self) -> Result<()> {
        let database_path = self.database.path().to_path_buf();
        let target = self.out.clone();
        let mode = self.mode;
        let database = self.database;
        // The export copies the whole stream; on a runtime thread it
        // would hold up the other stream's download.
        let exported = spawn_blocking(move || database.export(&target, mode))
            .await
            .unwrap_or_else(|error| Err(io::Error::other(error).into()));
        let (count, bytes) = exported.inspect_err(|error| {
            log_warn!("{}: Failed to export {} to {}: {error}", self.name, database_path.display(), self.out.display());
        })?;
        log_debug!("{}: Exported {count} fragment(s), {bytes} bytes to {}", self.name, self.out.display());
        Ok(())
    }
}

impl Sink {
    pub fn is_db(&self) -> bool {
        matches!(self, Sink::Db(..))
    }

    /// Bytes of downloaded fragments held in memory until an earlier one
    /// arrives. The database stores each fragment at once.
    pub fn waiting_bytes(&self) -> u64 {
        match self {
            Sink::Ts(ts_sink) => ts_sink.waiting_bytes,
            Sink::Db(..) => 0,
        }
    }

    pub async fn push(&mut self, seq: u64, frag: Option<Fragment>, meta: MetaUpdate) -> Result<Vec<Event>> {
        match self {
            Sink::Ts(ts_sink) => Ok(ts_sink.push(seq, frag).await?),
            Sink::Db(database, _) => database.push_with_meta(seq, frag, meta).await,
        }
    }

    /// Finalizes storage; for SQLite, returns a `PendingExport` so exporting
    /// waits until every stream finishes without storage errors (and past
    /// Ctrl-C prompts).
    pub async fn finish(self, name: &str, mode: u32) -> Result<Option<PendingExport>> {
        match self {
            Sink::Ts(ts_sink) => {
                let leftover = ts_sink.finish().await?;
                if leftover > 0 {
                    log_debug!("{name}: {leftover} downloaded fragment(s) after a gap were not written");
                }
                Ok(None)
            }
            Sink::Db(database, out) => Ok(Some(PendingExport { database, out, name: name.to_string(), mode })),
        }
    }

    /// Cleans up buffered fragment files on abort without exporting SQLite.
    pub async fn abort(self) {
        if let Sink::Ts(mut ts_sink) = self {
            for fragment in mem::take(&mut ts_sink.pending).into_values().flatten() {
                remove_frag_file(&fragment.data).await;
            }
            if let Err(error) = ts_sink.file.flush().await {
                log_debug!("Could not flush {} after storage error: {error}", ts_sink.path.display());
            }
        }
    }
}
