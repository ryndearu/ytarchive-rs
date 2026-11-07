//! Command-line definition. Flag names match Go ytarchive so existing scripts
//! work, including Go-style single-dash long flags (`-threads 4`, `-td DIR`).
//! Options are grouped under headings like yt-dlp's help output.

mod normalize;
mod resolve;
#[cfg(test)]
mod tests;

use std::path::PathBuf;

use clap::{ArgAction, Parser};

pub use normalize::normalize_args;

use crate::types::{parse_codec_list, VideoCodec, DEFAULT_FILENAME_FORMAT, DEFAULT_FRAG_MAX_TRIES};

const AFTER_HELP: &str = "\
QUALITY is a slash-delimited list from most to least wanted, e.g. 1080p60/720p/best.
Valid values: audio_only, 144p, 240p, 360p, 480p, 720p, 720p60, 1080p, 1080p60, 1440p, 1440p60, 2160p, 2160p60, best

CODECS for --codec is a slash-delimited list from most to least wanted, e.g. vp9/h264.
Valid values: av1, vp9, h264, best (= av1/vp9/h264). Codecs not listed are used as a fallback.

Durations accept units (1d8h10m, 90s) or time strings (01:30:00).
--live-from also accepts negative durations (-30m) and 'now'.";

const GENERAL: &str = "General Options";
const WAITING: &str = "Waiting Options";
const DOWNLOAD: &str = "Download Options";
const FORMAT: &str = "Format Selection Options";
const FILESYSTEM: &str = "Filesystem Options";
const POST: &str = "Post-Processing Options";
const INTERRUPT: &str = "Interrupt (Ctrl-C) Options";
const EXTRACTION: &str = "Extraction Options";
const NETWORK: &str = "Network Options";
const VERBOSITY: &str = "Verbosity Options";

/// A parsed `--codec` list (newtype so clap treats it as one value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecList(pub Vec<VideoCodec>);

#[derive(Debug, Parser)]
#[command(
    name = "ytarchive",
    version = crate::version::VERSION,
    about = "Archive a YouTube livestream from the start",
    after_help = AFTER_HELP,
    disable_help_flag = true,
    disable_version_flag = true
)]
pub struct Cli {
    /// YouTube livestream, channel, or googlevideo URL. Prompted for if omitted.
    pub url: Option<String>,
    /// Slash-delimited quality list. Prompted for if omitted (not needed with --info-only).
    pub quality: Option<String>,

    // ---- general ----
    /// Print this help text and exit.
    #[arg(short = 'h', long, action = ArgAction::Help, help_heading = GENERAL)]
    pub help: Option<bool>,
    /// Print the version and exit.
    #[arg(short = 'V', long, action = ArgAction::Version, help_heading = GENERAL)]
    pub version: Option<bool>,
    /// Print stream information (title, available and selected quality, duration) and exit. Only the URL is needed.
    #[arg(long, help_heading = GENERAL)]
    pub info_only: bool,
    /// Continually monitor a channel URL for new streams (requires URL and QUALITY).
    #[arg(long, help_heading = GENERAL)]
    pub monitor_channel: bool,
    /// Only download members-only streams (channel URLs; requires cookies).
    #[arg(long, help_heading = GENERAL)]
    pub members_only: bool,

    // ---- waiting ----
    /// Wait for a scheduled stream to start.
    #[arg(short = 'w', long, help_heading = WAITING)]
    pub wait: bool,
    /// Do not wait for a scheduled stream to start.
    #[arg(short = 'n', long, help_heading = WAITING)]
    pub no_wait: bool,
    /// Seconds between stream status checks while waiting (min 15; 30 when monitoring).
    #[arg(short = 'r', long, value_name = "SECS", help_heading = WAITING)]
    pub retry_stream: Option<u64>,
    /// Wait this long after the stream starts before capturing.
    #[arg(long, value_name = "DURATION", help_heading = WAITING)]
    pub start_delay: Option<String>,

    // ---- download ----
    /// Start from a point in the stream: duration (-30m), time string, or 'now'.
    #[arg(long, value_name = "WHEN", allow_hyphen_values = true, help_heading = DOWNLOAD)]
    pub live_from: Option<String>,
    /// Capture this much of the stream, then finish.
    #[arg(long, value_name = "DURATION", help_heading = DOWNLOAD)]
    pub capture_duration: Option<String>,
    /// Download threads per stream type.
    #[arg(long, value_name = "N", default_value_t = 1, help_heading = DOWNLOAD)]
    pub threads: usize,
    /// Attempts per fragment before refreshing URLs / giving up (0 = unlimited).
    #[arg(long, value_name = "ATTEMPTS", default_value_t = DEFAULT_FRAG_MAX_TRIES, help_heading = DOWNLOAD)]
    pub retry_frags: u32,
    /// Keep out-of-order fragments in memory instead of temp files.
    #[arg(long, help_heading = DOWNLOAD)]
    pub no_frag_files: bool,
    /// Store fragments in a SQLite database (more robust to gaps).
    #[arg(long, help_heading = DOWNLOAD)]
    pub sqlite: bool,

    // ---- format selection ----
    /// Video codec preference, e.g. vp9/h264. Asked for when a video quality is chosen if not given.
    #[arg(long, value_name = "CODECS", value_parser = parse_codecs, conflicts_with_all = ["av1", "vp9", "h264"], help_heading = FORMAT)]
    pub codec: Option<CodecList>,
    /// Prefer AV1 video when available (same as --codec av1).
    #[arg(long, help_heading = FORMAT)]
    pub av1: bool,
    /// Prefer VP9 video when available (same as --codec vp9).
    #[arg(long, help_heading = FORMAT)]
    pub vp9: bool,
    /// Prefer H.264 video (same as --codec h264).
    #[arg(long, help_heading = FORMAT)]
    pub h264: bool,
    /// Only download the audio stream.
    #[arg(long, help_heading = FORMAT)]
    pub no_video: bool,
    /// Only download the video stream.
    #[arg(long, help_heading = FORMAT)]
    pub no_audio: bool,

    // ---- filesystem ----
    /// Output filename template, e.g. '%(channel)s/%(upload_date)s %(title)s'.
    #[arg(short = 'o', long, value_name = "FORMAT", default_value = DEFAULT_FILENAME_FORMAT, help_heading = FILESYSTEM)]
    pub output: String,
    /// Directory for temporary files (created if missing; also -td).
    #[arg(long = "temporary-dir", value_name = "DIR", help_heading = FILESYSTEM)]
    pub temporary_dir: Option<PathBuf>,
    /// Force filenames to be Windows-compatible (default on Windows).
    #[arg(long, conflicts_with = "no_windows_filenames", help_heading = FILESYSTEM)]
    pub windows_filenames: bool,
    /// Sanitize filenames only minimally (default on other than Windows).
    #[arg(long, help_heading = FILESYSTEM)]
    pub no_windows_filenames: bool,
    /// Use lookalike characters in place of forbidden filename characters.
    #[arg(short = 'l', long, help_heading = FILESYSTEM)]
    pub lookalike_chars: bool,
    /// Write the description to a separate file.
    #[arg(long, help_heading = FILESYSTEM)]
    pub write_description: bool,
    /// Write the thumbnail to a separate file.
    #[arg(long, help_heading = FILESYSTEM)]
    pub write_thumbnail: bool,
    /// Keep the raw .ts files after muxing.
    #[arg(short = 'k', long, help_heading = FILESYSTEM)]
    pub keep_ts_files: bool,
    /// Disable resumable download state entirely.
    #[arg(long, help_heading = FILESYSTEM)]
    pub disable_save_state: bool,
    /// Permissions for created files (octal; also -fp).
    #[arg(long = "file-permissions", value_name = "MODE", default_value = "0644", value_parser = parse_octal, help_heading = FILESYSTEM)]
    pub file_permissions: u32,
    /// Permissions for created directories (octal; also -dp).
    #[arg(long = "directory-permissions", value_name = "MODE", default_value = "0755", value_parser = parse_octal, help_heading = FILESYSTEM)]
    pub directory_permissions: u32,

    // ---- post-processing ----
    /// Embed the thumbnail into the final file.
    #[arg(short = 't', long, help_heading = POST)]
    pub thumbnail: bool,
    /// Write basic metadata to the final file.
    #[arg(long, help_heading = POST)]
    pub add_metadata: bool,
    /// Override metadata, KEY=VALUE (VALUE is a format template; empty omits the key).
    #[arg(long, value_name = "KEY=VALUE", help_heading = POST)]
    pub metadata: Vec<String>,
    /// Mux into mkv instead of mp4 (ignored for audio only).
    #[arg(long, help_heading = POST)]
    pub mkv: bool,
    /// Also save the audio as a separate file.
    #[arg(long, help_heading = POST)]
    pub separate_audio: bool,
    /// Write the ffmpeg mux command to a file instead of muxing.
    #[arg(long, help_heading = POST)]
    pub write_mux_file: bool,
    /// ffmpeg executable to use.
    #[arg(long, value_name = "PATH", default_value = "ffmpeg", help_heading = POST)]
    pub ffmpeg_path: String,

    // ---- interrupt ----
    /// Automatically merge files when the download is cancelled.
    #[arg(long, help_heading = INTERRUPT)]
    pub merge: bool,
    /// Skip merging files when the download is cancelled.
    #[arg(long, help_heading = INTERRUPT)]
    pub no_merge: bool,
    /// Keep downloaded files on cancel if not merging.
    #[arg(long, help_heading = INTERRUPT)]
    pub save: bool,
    /// Delete downloaded files on cancel if not merging.
    #[arg(long, help_heading = INTERRUPT)]
    pub no_save: bool,
    /// Leave files for resuming later on cancel, if not saving files.
    #[arg(long, help_heading = INTERRUPT)]
    pub save_state: bool,
    /// Do not leave files for resuming later on cancel.
    #[arg(long, help_heading = INTERRUPT)]
    pub no_save_state: bool,

    // ---- extraction ----
    /// Netscape-format cookies file (needed for members-only streams).
    #[arg(short = 'c', long, value_name = "FILE", help_heading = EXTRACTION)]
    pub cookies: Option<PathBuf>,
    /// yt-dlp executable used for URL extraction.
    #[arg(long, value_name = "PATH", default_value = "yt-dlp", help_heading = EXTRACTION)]
    pub ytdlp_path: String,
    /// Extra arguments for yt-dlp, e.g. '--extractor-args youtube:player_client=web'.
    #[arg(long, value_name = "OPTIONS", allow_hyphen_values = true, help_heading = EXTRACTION)]
    pub ytdlp_opts: Option<String>,
    /// Let yt-dlp extract stream info; ytarchive only downloads fragments.
    #[arg(long, help_heading = EXTRACTION)]
    pub ytdlp_info: bool,
    /// Googlevideo URL to use for the video stream.
    #[arg(long, value_name = "URL", help_heading = EXTRACTION)]
    pub video_url: Option<String>,
    /// Googlevideo URL to use for the audio stream (itag 140).
    #[arg(long, value_name = "URL", help_heading = EXTRACTION)]
    pub audio_url: Option<String>,
    /// Ignored (accepted for compatibility); pass PO tokens to yt-dlp via --ytdlp-opts.
    #[arg(long, value_name = "TOKEN", help_heading = EXTRACTION)]
    pub potoken: Option<String>,
    /// Ignored (accepted for compatibility); pass visitor data to yt-dlp via --ytdlp-opts.
    #[arg(long, value_name = "VISITOR_DATA", help_heading = EXTRACTION)]
    pub visitor_data: Option<String>,

    // ---- network ----
    /// Force IPv4.
    #[arg(short = '4', long, conflicts_with = "ipv6", help_heading = NETWORK)]
    pub ipv4: bool,
    /// Force IPv6.
    #[arg(short = '6', long, help_heading = NETWORK)]
    pub ipv6: bool,
    /// Proxy URL (http, https, or socks5).
    #[arg(long, value_name = "URL", value_parser = parse_proxy, help_heading = NETWORK)]
    pub proxy: Option<url::Url>,

    // ---- verbosity ----
    /// Print nothing except prompts.
    #[arg(short = 'q', long, help_heading = VERBOSITY)]
    pub quiet: bool,
    /// Print only errors.
    #[arg(long, help_heading = VERBOSITY)]
    pub error: bool,
    /// Print warnings and errors (default).
    #[arg(long, help_heading = VERBOSITY)]
    pub warn: bool,
    /// Verbose output.
    #[arg(short = 'v', long, help_heading = VERBOSITY)]
    pub verbose: bool,
    /// Debug output.
    #[arg(long, help_heading = VERBOSITY)]
    pub debug: bool,
    /// Trace output.
    #[arg(long, help_heading = VERBOSITY)]
    pub trace: bool,
    /// Print progress on new lines instead of updating one line.
    #[arg(long, help_heading = VERBOSITY)]
    pub newline: bool,
}

fn parse_octal(s: &str) -> std::result::Result<u32, String> {
    let digits = s.strip_prefix("0o").unwrap_or(s);
    u32::from_str_radix(digits, 8)
        .ok()
        .filter(|m| *m <= 0o7777)
        .ok_or_else(|| format!("'{s}' is not an octal permission mode like 0644"))
}

fn parse_proxy(s: &str) -> std::result::Result<url::Url, String> {
    let u = url::Url::parse(s).map_err(|_| "invalid proxy URL".to_string())?;
    match u.scheme() {
        "http" | "https" | "socks5" => Ok(u),
        _ => Err("the proxy URL scheme must be http, https, or socks5".into()),
    }
}

fn parse_codecs(s: &str) -> std::result::Result<CodecList, String> {
    parse_codec_list(s)
        .map(CodecList)
        .ok_or_else(|| format!("'{s}' is not a codec list like vp9/h264 (valid: av1, vp9, h264, best)"))
}

impl Cli {
    pub fn parse_normalized() -> Self {
        Cli::parse_from(normalize_args(std::env::args_os()))
    }
}
