//! yt-dlp JSON output (from `YTDLP_INFO_PRINT_TEMPLATE`) and the parsed result.

use serde::Deserialize;
use std::collections::HashMap;

use super::fragment_url::FragmentUrl;

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
        self.format_id.split('-').next()?.parse().ok().filter(|i| *i > 0)
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
    pub fn parse(s: &str) -> Self {
        match s {
            "is_live" => Self::IsLive,
            "is_upcoming" => Self::IsUpcoming,
            "post_live" => Self::PostLive,
            "was_live" => Self::WasLive,
            "not_live" => Self::NotLive,
            _ => Self::Unknown,
        }
    }
}

/// Parsed and itag-indexed extraction result.
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
