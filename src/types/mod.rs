//! Shared types. The small ones live here; the larger groups have their own
//! files.

pub mod fragment;
pub mod options;
pub mod player_response;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use fragment::{
    DataType, FragData, FragThreadState, Fragment, FragmentUrl, ProgressInfo, StreamProgress, is_fragmented,
};
pub use options::{
    Action, BEST_CODEC_ORDER, IpVersion, LogLevel, Options, StorageMode, VideoCodec, complete_codec_order,
    parse_codec_list,
};
pub use player_response::{PlayerResponse, YtInitialData};

pub const AUDIO_ITAG: u32 = 140;
pub const DEFAULT_FILENAME_FORMAT: &str = "%(title)s-%(id)s";
/// Furthest back YouTube lets you seek in a live stream (7 days).
pub const LIVE_MAXIMUM_SEEKABLE_SECS: u64 = 86_400 * 7;
pub const DEFAULT_TARGET_DURATION_SECS: u32 = 5;
pub const DEFAULT_POLL_SECS: u64 = 15;
pub const DEFAULT_MONITOR_SECS: u64 = 60;
pub const MINIMUM_MONITOR_SECS: u64 = 30;
pub const DEFAULT_FRAG_MAX_TRIES: u32 = 10;
/// Background URL refresh interval (dreammu: 5h).
pub const URL_REFRESH_INTERVAL_SECS: u64 = 5 * 3600;

/// Every key available to the `%(key)s` output template.
pub const FORMAT_INFO_KEYS: &[&str] = &[
    "id",
    "title",
    "channel_id",
    "channel",
    "upload_date",
    "start_date",
    "year",
    "month",
    "day",
    "start_time",
    "hours",
    "minutes",
    "seconds",
    "publish_date",
    "description",
    "url",
];

/// Format-info keys that expand to nothing in filename templates.
pub const FILENAME_FORMAT_BLACKLIST: &[&str] = &["description"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoItag {
    pub h264: u32,
    pub vp9: u32,
    pub av1: u32,
}

const fn itags(h264: u32, vp9: u32, av1: u32) -> VideoItag {
    VideoItag { h264, vp9, av1 }
}

/// Ordered lowest to highest; order matters for "best" selection.
/// Itag source: https://gist.github.com/AgentOak/34d47c65b1d28829bb17c24c04a0096f
pub const VIDEO_QUALITIES: &[(&str, VideoItag)] = &[
    ("audio_only", itags(0, 0, 0)),
    ("144p", itags(160, 278, 394)),
    ("240p", itags(133, 242, 395)),
    ("360p", itags(134, 243, 396)),
    ("480p", itags(135, 244, 397)),
    ("720p", itags(136, 247, 398)),
    ("720p60", itags(298, 302, 398)),
    ("1080p", itags(137, 248, 399)),
    ("1080p60", itags(299, 303, 399)),
    ("1440p", itags(264, 271, 400)),
    ("1440p60", itags(304, 308, 400)),
    ("2160p", itags(266, 313, 401)),
    ("2160p60", itags(305, 315, 401)),
];

pub fn itags_for_label(label: &str) -> Option<VideoItag> {
    VIDEO_QUALITIES.iter().find(|(line, _)| *line == label).map(|(_, found)| *found)
}

/// `--print` template used with yt-dlp to get only the fields we need.
pub const YTDLP_INFO_PRINT_TEMPLATE: &str = r#"{"id":%(id|null)j,"title":%(fulltitle,title|null)j,"channel_id":%(channel_id|null)j,"channel":%(channel|null)j,"description":%(description|null)j,"thumbnail":%(thumbnail|null)j,"upload_date":%(upload_date|null)j,"release_timestamp":%(release_timestamp|null)j,"timestamp":%(timestamp|null)j,"live_status":%(live_status|null)j,"is_live":%(is_live|null)j,"formats":%(formats.:.{format_id,url,protocol,manifest_url,target_duration})j}"#;

/// One format in yt-dlp's JSON output (from `YTDLP_INFO_PRINT_TEMPLATE`).
#[derive(Debug, Clone, Deserialize)]
pub struct YtdlpFormat {
    #[serde(default)]
    pub format_id: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub manifest_url: Option<String>,
    #[serde(default)]
    pub target_duration: Option<f64>,
}

impl YtdlpFormat {
    /// yt-dlp format ids look like `299` or `299-drc`; we want the itag.
    pub fn itag(&self) -> Option<u32> {
        self.format_id.split('-').next()?.parse().ok().filter(|itag| *itag > 0)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct YtdlpRawInfo {
    pub id: Option<String>,
    pub title: Option<String>,
    pub channel_id: Option<String>,
    pub channel: Option<String>,
    pub description: Option<String>,
    pub thumbnail: Option<String>,
    pub upload_date: Option<String>,
    pub release_timestamp: Option<i64>,
    pub timestamp: Option<i64>,
    pub live_status: Option<String>,
    pub is_live: Option<bool>,
    #[serde(default)]
    pub formats: Option<Vec<YtdlpFormat>>,
}

/// yt-dlp's `live_status` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LiveStatus {
    IsLive,
    IsUpcoming,
    /// Stream ended, VOD not processed yet; fragments still downloadable.
    PostLive,
    /// Processed VOD.
    WasLive,
    NotLive,
    #[default]
    Unknown,
}

impl LiveStatus {
    pub fn parse(text: &str) -> Self {
        match text {
            "is_live" => Self::IsLive,
            "is_upcoming" => Self::IsUpcoming,
            "post_live" => Self::PostLive,
            "was_live" => Self::WasLive,
            "not_live" => Self::NotLive,
            _ => Self::Unknown,
        }
    }
}

/// Parsed and itag-indexed yt-dlp extraction result.
#[derive(Debug, Clone, Default)]
pub struct YtdlpExtraction {
    /// itag -> adaptive fragment URL.
    pub urls: HashMap<u32, FragmentUrl>,
    /// Latest (or, post-live, last safe) sequence number; `None` if unknown.
    pub last_sq: Option<u64>,
    /// Fragment length in seconds; 0 if yt-dlp didn't report it.
    pub target_duration: u32,
    pub id: String,
    pub title: String,
    pub channel_id: String,
    pub channel: String,
    pub description: String,
    pub thumbnail: String,
    /// `YYYYMMDD` as reported by yt-dlp.
    pub upload_date: String,
    /// Unix start time (release_timestamp, else timestamp).
    pub start_unix: Option<i64>,
    pub live_status: LiveStatus,
    pub is_live: bool,
}

/// One cookies.txt (Netscape format) entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetscapeCookie {
    /// Lowercased, without the `#HttpOnly_` prefix; may start with `.`.
    pub domain: String,
    pub path: String,
    pub secure: bool,
    /// Unix seconds; `None` for session cookies (expiry `0`).
    pub expires: Option<i64>,
    pub name: String,
    pub value: String,
    pub http_only: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CookieSet {
    pub cookies: Vec<NetscapeCookie>,
}

/// Resumable on-disk download state (one file per itag).
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

/// Values exposed to the `%(key)s` filename template.
pub type FormatInfo = BTreeMap<String, String>;

/// Metadata written to the final file (key -> value, values may be templates
/// before expansion).
pub type MetaInfo = BTreeMap<String, String>;

/// All template keys present with empty values, so templates can be
/// validated before extraction fills them in.
pub fn new_format_info() -> FormatInfo {
    FORMAT_INFO_KEYS.iter().map(|key| (key.to_string(), String::new())).collect()
}

/// What an SQLite download stores about the stream, so a later run can retry
/// missing fragments even when the stream can no longer be looked up (it went
/// private, or its live download is gone).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedDownload {
    pub video_id: String,
    /// Output template values (title, channel, ...), for the file name.
    pub format_info: FormatInfo,
    /// Metadata for the final file and the description.
    pub metadata: MetaInfo,
    pub thumbnail_url: String,
    pub target_duration: u32,
    /// Quality label that was selected, e.g. `1080p60` or `audio_only`.
    pub label: String,
    /// `None` for audio only.
    pub video_itag: Option<u32>,
}

/// Result of choosing which audio/video streams to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatSelection {
    /// Quality label that matched, e.g. `1080p60` or `audio_only`.
    pub label: String,
    /// `None` for audio only.
    pub codec: Option<VideoCodec>,
    /// Video itag; `None` for audio only.
    pub video_itag: Option<u32>,
    pub audio_url: Option<FragmentUrl>,
    pub video_url: Option<FragmentUrl>,
}

impl FormatSelection {
    pub fn is_audio_only(&self) -> bool {
        self.video_itag.is_none()
    }
}

/// One value for the audio stream and one for the video stream.
#[derive(Debug, Clone, Copy, Default)]
pub struct PerType<T> {
    pub audio: T,
    pub video: T,
}

impl<T> PerType<T> {
    pub fn get(&self, data_type: DataType) -> &T {
        match data_type {
            DataType::Audio => &self.audio,
            DataType::Video => &self.video,
        }
    }

    pub fn get_mut(&mut self, data_type: DataType) -> &mut T {
        match data_type {
            DataType::Audio => &mut self.audio,
            DataType::Video => &mut self.video,
        }
    }
}
