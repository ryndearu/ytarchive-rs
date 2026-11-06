//! Resumable on-disk download state (one file per itag).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")] // keeps state files compatible with Go ytarchive
pub struct DownloadState {
    pub start_frag: u64,
    pub fragments: u64,
    pub size: u64,
    pub temp_dir: PathBuf,
    /// Path of the state file itself; not serialized.
    #[serde(skip)]
    pub file: PathBuf,
}
