//! In-order `.ts` writer (default storage): appends fragments in sequence
//! order, buffering out-of-order arrivals in memory or as temp files.

use std::collections::BTreeMap;
use std::io::{self, SeekFrom};
use std::mem;
use std::path::{Path, PathBuf};

use bytes::Bytes;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use super::Event;
use crate::mp4_atoms::strip_for_concat;
use crate::types::{DownloadState, FragData, Fragment};
use crate::util::apply_mode;
use crate::{log_debug, log_info, log_warn};

/// Default storage: fragments appended in sequence order to the `.ts` file;
/// out-of-order arrivals wait in memory (or as temp files) until their turn.
pub struct TsSink {
    pub(super) file: tokio::fs::File,
    pub(super) path: PathBuf,
    next_write: u64,
    /// True once at least one fragment has been written, so only the first
    /// written fragment keeps its `ftyp` box even if `start_frag` was skipped.
    wrote_any: bool,
    pub(super) pending: BTreeMap<u64, Option<Fragment>>,
    /// Bytes of `pending` held in memory (fragment files don't count).
    pub(super) waiting_bytes: u64,
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
                let data = strip_for_concat(&raw, &frag.mime_type, !self.wrote_any).map_or(raw, Bytes::from);
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
