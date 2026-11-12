//! Where downloaded fragments are stored: the in-order `.ts` writer
//! (default) or the SQLite store (`--sqlite`).

use std::collections::BTreeMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use bytes::Bytes;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use super::db_sink::{DbSink, db_path};
use crate::error::Result;
use crate::mp4_atoms::strip_for_concat;
use crate::types::{DownloadState, FragData, Fragment};
use crate::util::apply_mode;
use crate::{log_debug, log_info, log_warn};

/// Default storage: fragments appended in sequence order to the `.ts` file;
/// out-of-order arrivals wait in memory (or as temp files) until their turn.
pub struct TsSink {
    file: tokio::fs::File,
    path: PathBuf,
    next_write: u64,
    start_frag: u64,
    pending: BTreeMap<u64, Option<Fragment>>,
}

pub async fn fragment_bytes(data: &FragData) -> std::io::Result<Bytes> {
    match data {
        // Shares the downloaded buffer instead of copying it.
        FragData::Memory(bytes) => Ok(bytes.clone()),
        FragData::File(path) => Ok(tokio::fs::read(path).await?.into()),
    }
}

pub async fn remove_frag_file(data: &FragData) {
    if let FragData::File(path) = data
        && let Err(error) = tokio::fs::remove_file(path).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        log_debug!("Could not delete the fragment file {}: {error}", path.display());
    }
}

impl TsSink {
    /// Continues the file from `state` if it is consistent, else returns
    /// `None` so the caller starts fresh.
    pub async fn resume(path: &Path, state: &DownloadState, name: &str) -> std::io::Result<Option<(Self, u64)>> {
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
                let cur = state.start_frag + state.fragments;
                log_info!("{name}: Resuming download from sequence {cur}");
                Ok(Some((Self::new(file, path, cur, state.start_frag), cur)))
            }
            Err(error) => {
                log_warn!("{name}: Failed to open {} to resume download: {error}", path.display());
                log_warn!("{name}: Will start from the beginning");
                Ok(None)
            }
        }
    }

    pub async fn create(path: &Path, start: u64, mode: u32) -> std::io::Result<Self> {
        let file = tokio::fs::File::create(path).await?;
        apply_mode(path, mode);
        Ok(Self::new(file, path, start, start))
    }

    fn new(file: tokio::fs::File, path: &Path, next_write: u64, start_frag: u64) -> Self {
        Self { file, path: path.to_path_buf(), next_write, start_frag, pending: BTreeMap::new() }
    }

    /// Queues `frag` (`None` = given up) and writes everything now in order.
    pub async fn push(&mut self, seq: u64, frag: Option<Fragment>) -> std::io::Result<Vec<Event>> {
        self.pending.insert(seq, frag);
        let mut events = Vec::new();
        while let Some(entry) = self.pending.remove(&self.next_write) {
            let Some(frag) = entry else {
                events.push(Event::Gap(self.next_write));
                self.next_write += 1;
                continue;
            };
            let written = async {
                let raw = fragment_bytes(&frag.data).await?;
                let first = self.next_write == self.start_frag;
                let data = strip_for_concat(&raw, &frag.mime_type, first).map_or(raw, Bytes::from);
                self.file.write_all(&data).await?;
                // tokio writes in the background and reports a failure only
                // on the next call; without this a fragment that never
                // reached the file (full disk) is counted in the saved
                // state, and the next run can't resume from it.
                self.file.flush().await?;
                Ok::<_, std::io::Error>(data.len() as u64)
            }
            .await;
            remove_frag_file(&frag.data).await;
            match written {
                Ok(bytes) => events.push(Event::Wrote(bytes)),
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
    pub async fn finish(mut self) -> std::io::Result<usize> {
        let mut leftover = 0;
        for fragment in std::mem::take(&mut self.pending).into_values().flatten() {
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
    pub skip: std::collections::HashSet<u64>,
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
        let sink = DbSink::open(&database_path, mode)?;
        let skip = sink.stored()?;
        let (fragments, size) = sink.totals()?;
        let cur = if fragments > 0 && state.fragments > 0 {
            log_info!("{name}: Resuming from {}: {fragments} fragment(s) stored", sink.path().display());
            state.start_frag
        } else {
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
    let temp_dir = std::mem::take(&mut state.temp_dir);
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
async fn keep_existing_data(data_file: &Path, name: &str) {
    let has_data = tokio::fs::metadata(data_file).await.is_ok_and(|metadata| metadata.len() > 0);
    if !has_data {
        return;
    }
    let mut old_name = data_file.as_os_str().to_os_string();
    old_name.push(".old");
    let old_file = PathBuf::from(old_name);
    match tokio::fs::rename(data_file, &old_file).await {
        Ok(()) => log_warn!(
            "{name}: {} has data that can't be resumed; kept it as {}",
            data_file.display(),
            old_file.display()
        ),
        Err(error) => log_warn!("{name}: Could not keep the existing {}: {error}", data_file.display()),
    }
}

impl Sink {
    pub fn is_db(&self) -> bool {
        matches!(self, Sink::Db(..))
    }

    pub async fn push(&mut self, seq: u64, frag: Option<Fragment>) -> Result<Vec<Event>> {
        match self {
            Sink::Ts(ts_sink) => Ok(ts_sink.push(seq, frag).await?),
            Sink::Db(database, _) => database.push(seq, frag).await,
        }
    }

    /// Finalizes storage; for SQLite, exports the `.ts` file.
    pub async fn finish(self, name: &str, mode: u32) -> Result<()> {
        match self {
            Sink::Ts(ts_sink) => {
                let leftover = ts_sink.finish().await?;
                if leftover > 0 {
                    log_debug!("{name}: {leftover} downloaded fragment(s) after a gap were not written");
                }
            }
            Sink::Db(database, out) => {
                let database_path = database.path().to_path_buf();
                let target = out.clone();
                // The export copies the whole stream; on a runtime thread it
                // would hold up the other stream's download.
                let exported = tokio::task::spawn_blocking(move || database.export(&target, mode))
                    .await
                    .unwrap_or_else(|error| Err(std::io::Error::other(error).into()));
                let (count, bytes) = exported.inspect_err(|error| {
                    log_warn!("{name}: Failed to export {} to {}: {error}", database_path.display(), out.display());
                })?;
                log_debug!("{name}: Exported {count} fragment(s), {bytes} bytes to {}", out.display());
            }
        }
        Ok(())
    }
}
