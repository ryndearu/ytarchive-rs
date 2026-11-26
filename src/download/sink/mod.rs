//! Where downloaded fragments are stored: the in-order `.ts` writer
//! (default) or the SQLite store (`--sqlite`).

mod ts;

use std::collections::HashSet;
use std::fmt;
use std::io;
use std::mem;
use std::path::{Path, PathBuf};

#[cfg(test)]
use bytes::Bytes;
use tokio::io::AsyncWriteExt;
use tokio::task::spawn_blocking;

pub use ts::{TsSink, fragment_bytes, remove_frag_file};

use super::db_sink::{DbSink, MetaUpdate, db_path};
use crate::error::Result;
#[cfg(test)]
use crate::types::FragData;
use crate::types::{DownloadState, Fragment};
use crate::util::rename_with_retry_async;
use crate::{log_debug, log_info, log_warn};

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

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Extracts `.f{itag}` from a `{base}.f{itag}.ts` data file path.
fn itag_suffix(data_file: &Path) -> Option<String> {
    let stem = data_file.file_stem()?.to_str()?;
    let (_, itag) = stem.rsplit_once(".f")?;
    (!itag.is_empty() && itag.bytes().all(|byte| byte.is_ascii_digit())).then(|| format!(".f{itag}"))
}

/// Returns the sole file in `dir` whose name ends with `suffix`, or `None` if
/// zero or multiple match.
async fn find_unique_by_suffix(dir: &Path, suffix: &str) -> io::Result<Option<PathBuf>> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut found = None;
    while let Some(entry) = entries.next_entry().await? {
        if !entry.file_name().to_string_lossy().ends_with(suffix) {
            continue;
        }
        if !entry.file_type().await.is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        if found.is_some() {
            return Ok(None);
        }
        found = Some(entry.path());
    }
    Ok(found)
}

/// Renames SQLite `-wal` and `-shm` sidecars alongside a moved database,
/// removing any stale destination sidecar when the source has none.
async fn rename_db_sidecars(from: &Path, to: &Path, name: &str) -> io::Result<()> {
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    for suffix in ["-wal", "-shm"] {
        let sidecar_path = with_suffix(from, suffix);
        let dest_sidecar = with_suffix(to, suffix);
        match rename_with_retry_async(sidecar_path.clone(), dest_sidecar.clone()).await {
            Ok(()) => moved.push((sidecar_path, dest_sidecar)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // Remove any stale sidecar from an earlier file so SQLite
                // never replays an unrelated WAL onto the moved DB.
                if let Err(remove_error) = tokio::fs::remove_file(&dest_sidecar).await
                    && remove_error.kind() != io::ErrorKind::NotFound
                {
                    log_debug!("{name}: Could not delete stale {}: {remove_error}", dest_sidecar.display());
                }
            }
            Err(error) => {
                log_warn!("{name}: Could not move {}: {error}", sidecar_path.display());
                for (orig, dest) in moved.into_iter().rev() {
                    if let Err(rollback_error) = rename_with_retry_async(dest, orig.clone()).await {
                        log_warn!("{name}: Could not restore {}: {rollback_error}", orig.display());
                    }
                }
                return Err(error);
            }
        }
    }
    Ok(())
}

/// When resuming inside an owned temp dir after the stream title or `-o`
/// template changed, renames the existing `*.f{itag}.sqlite` (plus sidecars)
/// or `*.f{itag}.ts` to the new base name so resuming continues instead of
/// starting over.
async fn adopt_renamed_storage(data_file: &Path, database_path: &Path, name: &str) -> io::Result<()> {
    let (Some(dir), Some(itag)) = (data_file.parent(), itag_suffix(data_file)) else {
        return Ok(());
    };
    if let Some(found) = find_unique_by_suffix(dir, &format!("{itag}.sqlite")).await? {
        rename_with_retry_async(found.clone(), database_path.to_path_buf()).await?;
        if let Err(error) = rename_db_sidecars(&found, database_path, name).await {
            if let Err(rollback_error) = rename_with_retry_async(database_path.to_path_buf(), found.clone()).await {
                log_warn!("{name}: Could not restore {}: {rollback_error}", found.display());
            }
            return Err(error);
        }
        log_info!(
            "{name}: Found previous database {} under a different name; resuming as {}",
            found.display(),
            database_path.display()
        );
        return Ok(());
    }
    if let Some(found) = find_unique_by_suffix(dir, &format!("{itag}.ts")).await? {
        rename_with_retry_async(found.clone(), data_file.to_path_buf()).await?;
        log_info!(
            "{name}: Found previous data file {} under a different name; resuming as {}",
            found.display(),
            data_file.display()
        );
    }
    Ok(())
}

/// Opens storage for `data_file`, resuming if `state` says there is
/// something to resume. `fresh_start` is the sequence for a new download.
/// When `adopt_renamed` is true (an owned `{id}__*` temp dir), an existing
/// `.f{itag}.sqlite` or `.f{itag}.ts` under an older filename is renamed to
/// match `data_file` before resuming.
pub async fn open(
    sqlite: bool,
    data_file: &Path,
    mut state: DownloadState,
    fresh_start: impl FnOnce() -> u64,
    mode: u32,
    adopt_renamed: bool,
    name: &str,
) -> Result<Opened> {
    let database_path = db_path(data_file);
    if state.fragments > 0 && adopt_renamed && !database_path.exists() && !data_file.exists() {
        adopt_renamed_storage(data_file, &database_path, name).await?;
    }
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
        let (mut skip, mut size) = sink.stored_and_size_async().await?;
        let mut fragments = skip.len() as u64;
        let cur = if fragments > 0 && state.fragments > 0 {
            log_info!("{name}: Resuming from {}: {fragments} fragment(s) stored", sink.path().display());
            state.start_frag
        } else {
            if fragments > 0 {
                // Unresumable database (e.g. reused temp dir whose state has 0
                // fragments): set it aside rather than splicing two runs.
                drop(sink);
                keep_existing_db(&database_path, name).await?;
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
    keep_existing_data(data_file, name).await?;
    let sink = TsSink::create(data_file, start, mode).await?;
    Ok(Opened { sink: Sink::Ts(sink), cur: start, skip: Default::default(), state })
}

/// First of `{path}.old`, `{path}.1.old`, ... that isn't already a file,
/// so an earlier `.old` backup is never overwritten.
fn unused_old_path(data_file: &Path) -> PathBuf {
    let first = with_suffix(data_file, ".old");
    if !first.is_file() {
        return first;
    }
    for number in 1u32.. {
        let candidate = with_suffix(data_file, &format!(".{number}.old"));
        if !candidate.is_file() {
            return candidate;
        }
    }
    first
}

/// Data that can't be resumed (its state file is missing, unreadable, or
/// doesn't match) is set aside as `{file}.old` rather than overwritten by
/// the new download. That only happens in a folder that is reused: the
/// other stream's state was still good, or the file is shorter than its
/// state says. With no usable state at all the run gets a new folder, and
/// the old one is left as it is.
async fn keep_existing_data(data_file: &Path, name: &str) -> io::Result<Option<PathBuf>> {
    let has_data = tokio::fs::metadata(data_file).await.is_ok_and(|metadata| metadata.len() > 0);
    if !has_data {
        return Ok(None);
    }
    let old_file = unused_old_path(data_file);
    rename_with_retry_async(data_file.to_path_buf(), old_file.clone()).await?;
    log_warn!("{name}: {} has data that can't be resumed; kept it as {}", data_file.display(), old_file.display());
    Ok(Some(old_file))
}

pub(super) async fn keep_existing_db(database_path: &Path, name: &str) -> io::Result<()> {
    let Some(old_file) = keep_existing_data(database_path, name).await? else {
        return Ok(());
    };
    if let Err(error) = rename_db_sidecars(database_path, &old_file, name).await {
        if let Err(rollback_error) = rename_with_retry_async(old_file, database_path.to_path_buf()).await {
            log_warn!("{name}: Could not restore {}: {rollback_error}", database_path.display());
        }
        return Err(error);
    }
    Ok(())
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
