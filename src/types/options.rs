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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    Av1,
    Vp9,
    H264,
}

/// Order used when nothing else is chosen (dreammu's default).
pub const DEFAULT_CODEC_ORDER: [VideoCodec; 3] = [VideoCodec::Av1, VideoCodec::Vp9, VideoCodec::H264];

impl VideoCodec {
    pub fn name(self) -> &'static str {
        match self {
            VideoCodec::Av1 => "AV1",
            VideoCodec::Vp9 => "VP9",
            VideoCodec::H264 => "H264",
        }
    }

    /// Lowercase name as typed on the command line.
    pub fn arg(self) -> &'static str {
        match self {
            VideoCodec::Av1 => "av1",
            VideoCodec::Vp9 => "vp9",
            VideoCodec::H264 => "h264",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "av1" | "av01" => Some(VideoCodec::Av1),
            "vp9" | "vp09" => Some(VideoCodec::Vp9),
            "h264" | "h.264" | "avc" | "avc1" => Some(VideoCodec::H264),
            _ => None,
        }
    }
}

/// `preferred` first, then the remaining codecs in default order, so a
/// stream without the preferred codecs can still be downloaded.
pub fn complete_codec_order(preferred: &[VideoCodec]) -> Vec<VideoCodec> {
    let mut out: Vec<VideoCodec> = Vec::with_capacity(3);
    for c in preferred.iter().chain(DEFAULT_CODEC_ORDER.iter()) {
        if !out.contains(c) {
            out.push(*c);
        }
    }
    out
}

/// Parses a slash-delimited codec list (`vp9/h264`); `best`/`any`/empty
/// mean the default order.
pub fn parse_codec_list(s: &str) -> Option<Vec<VideoCodec>> {
    let parts: Vec<&str> = s.split('/').map(str::trim).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() || parts.iter().any(|p| p.eq_ignore_ascii_case("best") || p.eq_ignore_ascii_case("any")) {
        return Some(DEFAULT_CODEC_ORDER.to_vec());
    }
    let codecs: Option<Vec<VideoCodec>> = parts.iter().map(|p| VideoCodec::parse(p)).collect();
    codecs.map(|c| complete_codec_order(&c))
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
    pub ytdlp_path: String,
    /// Extra yt-dlp args, already whitespace-split.
    pub ytdlp_opts: Vec<String>,
    pub ytdlp_info: bool,

    // Format
    /// Video codec preference order; `None` = not chosen yet (the user is
    /// asked when a video quality is selected).
    pub codecs: Option<Vec<VideoCodec>>,
    pub mkv: bool,
    pub audio_only: bool,
    pub video_only: bool,

    // Output
    pub output_format: String,
    pub temp_dir: Option<PathBuf>,
    pub lookalike_chars: bool,
    /// Make file/folder names valid on Windows (default on Windows only).
    pub windows_filenames: bool,
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
    /// Print stream info (title, quality, start time, duration) and exit.
    pub info_only: bool,
    pub log_level: LogLevel,
    pub status_newlines: bool,
}
