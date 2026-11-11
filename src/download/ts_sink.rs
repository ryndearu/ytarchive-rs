//! Default storage: fragments appended in sequence order to the `.ts` file;
//! out-of-order arrivals wait in memory (or as temp files) until their turn.

use std::collections::BTreeMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use super::sink::Event;
use crate::mp4_atoms::strip_for_concat;
use crate::types::{DownloadState, FragData, Fragment};
use crate::util::apply_mode;
use crate::{log_debug, log_info, log_warn};

pub struct TsSink {
    file: tokio::fs::File,
    path: PathBuf,
    next_write: u64,
    start_frag: u64,
    pending: BTreeMap<u64, Option<Fragment>>,
}

pub async fn fragment_bytes(data: &FragData) -> std::io::Result<Vec<u8>> {
    match data {
        FragData::Memory(b) => Ok(b.to_vec()),
        FragData::File(p) => tokio::fs::read(p).await,
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
    pub async fn resume(
        path: &Path,
        state: &DownloadState,
        name: &str,
    ) -> std::io::Result<Option<(Self, u64)>> {
        match tokio::fs::OpenOptions::new().read(true).write(true).open(path).await {
            Ok(mut f) => {
                if f.metadata().await?.len() < state.size {
                    log_warn!("{name}: {} is smaller than the saved state; starting from the beginning", path.display());
                    return Ok(None);
                }
                // Drop anything written after the last saved state.
                f.set_len(state.size).await?;
                f.seek(SeekFrom::End(0)).await?;
                let cur = state.start_frag + state.fragments;
                log_info!("{name}: Resuming download from sequence {cur}");
                Ok(Some((Self::new(f, path, cur, state.start_frag), cur)))
            }
            Err(e) => {
                log_warn!("{name}: Failed to open {} to resume download: {e}", path.display());
                log_warn!("{name}: Will start from the beginning");
                Ok(None)
            }
        }
    }

    pub async fn create(path: &Path, start: u64, mode: u32) -> std::io::Result<Self> {
        let f = tokio::fs::File::create(path).await?;
        apply_mode(path, mode);
        Ok(Self::new(f, path, start, start))
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
                let data = strip_for_concat(&raw, &frag.mime_type, first).unwrap_or(raw);
                self.file.write_all(&data).await?;
                Ok::<_, std::io::Error>(data.len() as u64)
            }
            .await;
            remove_frag_file(&frag.data).await;
            match written {
                Ok(n) => events.push(Event::Wrote(n)),
                Err(e) => {
                    log_warn!("Error writing fragment {} to {}: {e}", self.next_write, self.path.display());
                    return Err(e);
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
        for f in std::mem::take(&mut self.pending).into_values().flatten() {
            remove_frag_file(&f.data).await;
            leftover += 1;
        }
        self.file.flush().await?;
        Ok(leftover)
    }
}
