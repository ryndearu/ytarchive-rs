//! Constants and quality/itag tables.
//! Itag source: https://gist.github.com/AgentOak/34d47c65b1d28829bb17c24c04a0096f

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
