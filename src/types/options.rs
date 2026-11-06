//! Immutable run options, resolved once from the CLI.
//! Mutable runtime data lives in `state.rs` so options can be shared freely.

use std::path::PathBuf;
use std::time::Duration;

/// Tri-state for prompts (`--wait` / `--no-wait`, `--merge`, save-state on cancel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Action {
    #[default]
    Ask,
    Do,
    DoNot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Codec {
    #[default]
    Auto,
    H264,
    Vp9,
    Av1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IpVersion {
    #[default]
    Any,
    V4,
    V6,
}

/// How fragments are persisted before muxing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StorageMode {
    /// Append in order to a .ts file; out-of-order fragments buffered in RAM.
    #[default]
    DirectTs,
    /// Store fragments in SQLite keyed by sequence; robust to gaps/recovery.
    Sqlite,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    // Input
    pub url: String,
    pub quality: Option<String>,

    // Auth / extraction
    pub cookies_file: Option<PathBuf>,
    pub po_token: Option<String>,
    pub visitor_data: Option<String>,
    pub ytdlp_path: String,
    pub ytdlp_opts: Vec<String>,
    pub ytdlp_info: bool,
    pub audio_url: Option<String>,
    pub video_url: Option<String>,

    // Format
    pub codec: Codec,
    pub mkv: bool,
    pub audio_only: bool,
    pub video_only: bool,

    // Output
    pub output_format: String,
    pub temp_dir: Option<PathBuf>,
    pub lookalike_chars: bool,
    pub add_metadata: bool,
    /// `--metadata KEY=VALUE` overrides; empty value omits the key.
    pub metadata_overrides: Vec<(String, String)>,
    pub write_description: bool,
    pub write_thumbnail: bool,
    pub embed_thumbnail: bool,
    pub write_mux_file: bool,
    pub keep_ts_files: bool,
    pub ffmpeg_path: String,
    pub file_mode: u32,
    pub dir_mode: u32,

    // Download behaviour
    pub threads: usize,
    pub frag_max_tries: u32,
    pub storage: StorageMode,
    pub disable_save_state: bool,
    pub save_state_on_cancel: Action,
    pub merge_on_cancel: Action,
    pub wait: Action,
    pub retry_stream: Option<Duration>,
    pub live_from: Option<String>,
    pub start_delay: Option<Duration>,
    pub capture_duration: Option<Duration>,

    // Monitoring
    pub monitor_channel: bool,
    pub members_only: bool,

    // Network
    pub ip_version: IpVersion,
    pub proxy: Option<String>,

    // Logging
    pub verbose: bool,
    pub debug: bool,
    pub trace: bool,
    pub quiet: bool,
}
