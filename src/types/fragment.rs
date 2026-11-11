//! Fragment-level data passed between download workers and the writer.

use bytes::Bytes;
use std::path::PathBuf;
use std::time::Duration;

/// Which half of the stream a worker is handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataType {
    Audio,
    Video,
}

impl DataType {
    pub fn as_str(self) -> &'static str {
        match self {
            DataType::Audio => "audio",
            DataType::Video => "video",
        }
    }
}

/// Where a downloaded fragment's bytes are held until written in order.
#[derive(Debug, Clone)]
pub enum FragData {
    Memory(Bytes),
    /// Temp file (`--no-frag-files` not set); deleted after writing.
    File(PathBuf),
}

/// A downloaded fragment ready to be written.
#[derive(Debug, Clone)]
pub struct Fragment {
    pub seq: u64,
    /// Value of the `X-Head-Seqnum` response header (latest available seq).
    pub x_head_seqnum: Option<u64>,
    pub data: FragData,
    pub mime_type: String,
}

/// Sent from a stream writer to the progress reporter after each fragment.
#[derive(Debug, Clone, Copy)]
pub struct ProgressInfo {
    pub data_type: DataType,
    pub byte_count: u64,
    pub max_seq: Option<u64>,
    pub start_frag: u64,
}

/// Per-worker retry state for a single fragment.
#[derive(Debug, Clone)]
pub struct FragThreadState {
    pub name: String,
    pub data_type: DataType,
    pub seq: u64,
    pub max_seq: Option<u64>,
    pub tries: u32,
    /// Extra rounds of `frag_max_tries` allowed for fragments well behind
    /// the head after the stream ended.
    pub full_retries: u32,
    pub is_403: bool,
    pub sleep: Duration,
}

impl FragThreadState {
    pub fn new(name: impl Into<String>, data_type: DataType, sleep: Duration) -> Self {
        Self {
            name: name.into(),
            data_type,
            seq: 0,
            max_seq: None,
            tries: 0,
            full_retries: 3,
            is_403: false,
            sleep,
        }
    }

    pub fn reset_for(&mut self, seq: u64, max_seq: Option<u64>) {
        self.seq = seq;
        self.max_seq = max_seq;
        self.tries = 0;
        self.full_retries = 3;
        self.is_403 = false;
    }
}
