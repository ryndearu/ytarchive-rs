//! yt-dlp JSON output (from `YTDLP_INFO_PRINT_TEMPLATE`) and the parsed result.

use serde::Deserialize;
use std::collections::HashMap;

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
        self.format_id.split('-').next()?.parse().ok()
    }
}

#[derive(Debug, Clone, Deserialize)]
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
    pub formats: Vec<YtdlpFormat>,
}

/// Parsed and itag-indexed extraction result.
#[derive(Debug, Clone, Default)]
pub struct YtdlpExtraction {
    /// itag -> fragment base URL (DASH/HLS-derived).
    pub urls: HashMap<u32, String>,
    /// itag -> adaptive URL (preferred; supports `&sq=` seeking).
    pub adaptive_urls: HashMap<u32, String>,
    pub last_sq: Option<u64>,
    pub target_duration: u32,
    pub id: String,
    pub title: String,
    pub channel_id: String,
    pub channel: String,
    pub description: String,
    pub thumbnail: String,
    pub upload_date: String,
    pub start_date: String,
    pub start_unix: Option<i64>,
    pub live_status: String,
    pub is_live: bool,
}
