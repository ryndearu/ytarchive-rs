//! Where downloaded fragments are stored: the in-order `.ts` writer
//! (default) or the SQLite store (`--sqlite`).

use std::path::{Path, PathBuf};

use super::db_sink::{db_path, DbSink};
use super::ts_sink::TsSink;
use crate::error::Result;
use crate::types::{DownloadState, Fragment};
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
    let db = db_path(data_file);
    // A download is resumed in the storage it was started with, whatever
    // the flags say now: a .ts download isn't replaced by an empty
    // database's export (e.g. when rerun with --recover-private), and a
    // database isn't ignored.
    let resume_db = state.fragments > 0 && db.exists();
    let resume_ts = state.fragments > 0 && !db.exists() && data_file.exists();
    let use_db = resume_db || (sqlite && !resume_ts);
    if use_db != sqlite {
        let how = if use_db { "with SQLite" } else { "without SQLite" };
        log_info!("{name}: Resuming {} {how}, as it was started", data_file.display());
    }
    if use_db {
        let sink = DbSink::open(&db, mode)?;
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
    let sink = TsSink::create(data_file, start, mode).await?;
    Ok(Opened { sink: Sink::Ts(sink), cur: start, skip: Default::default(), state })
}

impl Sink {
    pub fn is_db(&self) -> bool {
        matches!(self, Sink::Db(..))
    }

    pub async fn push(&mut self, seq: u64, frag: Option<Fragment>) -> Result<Vec<Event>> {
        match self {
            Sink::Ts(t) => Ok(t.push(seq, frag).await?),
            Sink::Db(d, _) => d.push(seq, frag).await,
        }
    }

    /// Finalizes storage; for SQLite, exports the `.ts` file.
    pub async fn finish(self, name: &str, mode: u32) -> Result<()> {
        match self {
            Sink::Ts(t) => {
                let leftover = t.finish().await?;
                if leftover > 0 {
                    log_debug!("{name}: {leftover} downloaded fragment(s) after a gap were not written");
                }
            }
            Sink::Db(d, out) => {
                let (count, bytes) = d.export(&out, mode).inspect_err(|e| {
                    log_warn!("{name}: Failed to export {} to {}: {e}", d.path().display(), out.display());
                })?;
                log_debug!("{name}: Exported {count} fragment(s), {bytes} bytes to {}", out.display());
            }
        }
        Ok(())
    }
}
