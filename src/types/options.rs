//! Immutable run options, resolved once from the CLI.
//! Mutable runtime data lives in `download_info.rs` so options can be shared freely.

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

impl Action {
    /// `yes` wins over `no`, matching Go's if/else-if order.
    pub fn from_flags(yes: bool, no: bool) -> Self {
        match (yes, no) {
            (true, _) => Action::Do,
            (false, true) => Action::DoNot,
            _ => Action::Ask,
        }
    }
}

/// Preferred codecs; any combination may be set. Priority when several are
/// set is av1 > vp9 > h264, then the remaining codecs in that order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CodecPrefs {
    pub av1: bool,
    pub vp9: bool,
    pub h264: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    Av1,
    Vp9,
    H264,
}

impl VideoCodec {
    pub fn name(self) -> &'static str {
        match self {
            VideoCodec::Av1 => "AV1",
            VideoCodec::Vp9 => "VP9",
            VideoCodec::H264 => "H264",
        }
    }
}

impl CodecPrefs {
    /// Flagged codecs first (in av1 > vp9 > h264 order), then the rest.
    pub fn priority_order(self) -> Vec<VideoCodec> {
        let base = [
            (VideoCodec::Av1, self.av1),
            (VideoCodec::Vp9, self.vp9),
            (VideoCodec::H264, self.h264),
        ];
        let preferred = base.iter().filter(|(_, on)| *on).map(|(c, _)| *c);
        let rest = base.iter().filter(|(_, on)| !*on).map(|(c, _)| *c);
        preferred.chain(rest).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IpVersion {
    #[default]
    Any,
    V4,
    V6,
}

/// Ordered from least to most output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum LogLevel {
    Quiet,
    Error,
    #[default]
    Warning,
    Info,
    Debug,
    Trace,
}

/// How fragments are persisted before muxing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StorageMode {
    /// Append in order to a .ts file; out-of-order fragments buffered as temp
    /// files (or in RAM with `--no-frag-files`).
    #[default]
    DirectTs,
    /// Store fragments in SQLite keyed by sequence; robust to gaps/recovery.
    Sqlite,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    // Input
    /// Stream, channel, or googlevideo URL. `None` = prompt the user.
    pub url: Option<String>,
    /// Slash-delimited quality list, e.g. `1080p60/best`. `None` = prompt.
    pub quality: Option<String>,
    pub video_url: Option<String>,
    pub audio_url: Option<String>,

    // Auth / extraction
    pub cookies_file: Option<PathBuf>,
    pub po_token: Option<String>,
    pub visitor_data: Option<String>,
    pub ytdlp_path: String,
    /// Extra yt-dlp args, already whitespace-split.
    pub ytdlp_opts: Vec<String>,
    pub ytdlp_info: bool,

    // Format
    pub codecs: CodecPrefs,
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
    pub separate_audio: bool,
    pub ffmpeg_path: String,
    pub file_mode: u32,
    pub dir_mode: u32,

    // Download behaviour
    pub threads: usize,
    /// 0 = retry indefinitely.
    pub frag_max_tries: u32,
    pub storage: StorageMode,
    /// Write out-of-order fragments to temp files instead of RAM.
    pub frag_files: bool,
    pub disable_save_state: bool,
    pub wait: Action,
    pub merge_on_cancel: Action,
    pub save_files_on_cancel: Action,
    pub save_state_on_cancel: Action,
    /// Poll interval while waiting for a stream; `None` = ask / scheduled time.
    pub retry_stream: Option<Duration>,
    /// Raw `--live-from` value (duration, time string, or `now`);
    /// resolved after the stream's target duration is known.
    pub live_from: Option<String>,
    pub start_delay: Option<Duration>,
    pub capture_duration: Option<Duration>,

    // Monitoring
    pub monitor_channel: bool,
    pub members_only: bool,

    // Network
    pub ip_version: IpVersion,
    pub proxy: Option<url::Url>,

    // Output/logging
    pub log_level: LogLevel,
    pub status_newlines: bool,
}
