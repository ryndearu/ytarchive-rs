//! Fragment-level data passed between download workers and the writer.

use bytes::Bytes;
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

/// A downloaded fragment ready to be written.
#[derive(Debug, Clone)]
pub struct Fragment {
    pub seq: u64,
    /// Value of the `X-Head-Seqnum` response header (latest available seq).
    pub x_head_seqnum: Option<u64>,
    pub data: Bytes,
    pub mime_type: Option<String>,
    /// Set when the fragment took unusually long; used for thread scaling.
    pub slow: bool,
}

/// Sent from the writer to the progress reporter.
#[derive(Debug, Clone, Copy)]
pub struct ProgressInfo {
    pub itag: u32,
    pub byte_count: u64,
    pub max_seq: u64,
    pub start_frag: u64,
}

/// Next sequence a worker should fetch, plus latest known max.
#[derive(Debug, Clone, Copy)]
pub struct SeqInfo {
    pub cur_sequence: u64,
    pub max_sequence: u64,
}

/// Per-worker retry state for a single fragment.
#[derive(Debug, Clone)]
pub struct FragThreadState {
    pub name: String,
    pub data_type: DataType,
    pub seq: u64,
    pub max_seq: u64,
    pub tries: u32,
    pub full_retries: u32,
    pub is_403: bool,
    pub sleep: Duration,
}

impl FragThreadState {
    pub fn new(name: impl Into<String>, data_type: DataType) -> Self {
        Self {
            name: name.into(),
            data_type,
            seq: 0,
            max_seq: 0,
            tries: 0,
            full_retries: 0,
            is_403: false,
            sleep: Duration::from_secs(1),
        }
    }

    pub fn reset_for(&mut self, seq: u64) {
        self.seq = seq;
        self.tries = 0;
        self.full_retries = 0;
        self.is_403 = false;
    }
}
