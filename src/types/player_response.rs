//! `ytInitialPlayerResponse` / `ytInitialData` from YouTube pages, trimmed to
//! the fields used. Every field defaults so partial responses still parse.

use serde::Deserialize;

pub const PLAYABLE_OK: &str = "OK";
pub const PLAYABLE_OFFLINE: &str = "LIVE_STREAM_OFFLINE";
pub const PLAYABLE_UNPLAYABLE: &str = "UNPLAYABLE";
pub const PLAYABLE_ERROR: &str = "ERROR";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlayerResponse {
    pub response_context: ResponseContext,
    pub playability_status: PlayabilityStatus,
    pub streaming_data: StreamingData,
    pub video_details: VideoDetails,
    pub microformat: Microformat,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResponseContext {
    pub main_app_web_response_context: MainAppWebResponseContext,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MainAppWebResponseContext {
    pub logged_out: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlayabilityStatus {
    pub status: String,
    pub reason: String,
    pub live_streamability: LiveStreamability,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LiveStreamability {
    pub live_streamability_renderer: LiveStreamabilityRenderer,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LiveStreamabilityRenderer {
    pub video_id: String,
    pub offline_slate: OfflineSlate,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OfflineSlate {
    pub live_stream_offline_slate_renderer: OfflineSlateRenderer,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OfflineSlateRenderer {
    /// Unix seconds as a string.
    pub scheduled_start_time: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StreamingData {
    pub adaptive_formats: Vec<AdaptiveFormat>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AdaptiveFormat {
    pub itag: u32,
    pub url: String,
    pub target_duration_sec: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VideoDetails {
    pub video_id: String,
    pub title: String,
    pub channel_id: String,
    pub short_description: String,
    pub author: String,
    pub is_live_content: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Microformat {
    pub player_microformat_renderer: PlayerMicroformatRenderer,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlayerMicroformatRenderer {
    pub thumbnail: Thumbnails,
    pub live_broadcast_details: LiveBroadcastDetails,
    /// `YYYY-MM-DD...`
    pub publish_date: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Thumbnails {
    pub thumbnails: Vec<Thumbnail>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Thumbnail {
    pub url: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LiveBroadcastDetails {
    pub is_live_now: bool,
    /// RFC 3339.
    pub start_timestamp: String,
    pub end_timestamp: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct YtInitialData {
    pub contents: InitialDataContents,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InitialDataContents {
    pub two_column_browse_results_renderer: TwoColumnBrowseResults,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TwoColumnBrowseResults {
    pub tabs: Vec<Tab>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Tab {
    pub tab_renderer: TabRenderer,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TabRenderer {
    pub endpoint: Endpoint,
    pub content: TabContent,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Endpoint {
    pub command_metadata: CommandMetadata,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CommandMetadata {
    pub web_command_metadata: WebCommandMetadata,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct WebCommandMetadata {
    pub url: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TabContent {
    pub rich_grid_renderer: RichGridRenderer,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RichGridRenderer {
    pub contents: Vec<RichGridContent>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RichGridContent {
    pub rich_item_renderer: RichItemRenderer,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RichItemRenderer {
    pub content: RichItemContent,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RichItemContent {
    pub video_renderer: VideoRenderer,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VideoRenderer {
    pub video_id: String,
    pub thumbnail_overlays: Vec<ThumbnailOverlay>,
    pub badges: Vec<Badge>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThumbnailOverlay {
    pub thumbnail_overlay_time_status_renderer: StyleHolder,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Badge {
    pub metadata_badge_renderer: StyleHolder,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct StyleHolder {
    pub style: String,
}
