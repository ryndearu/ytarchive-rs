//! Scraping YouTube watch/channel pages for embedded JSON
//! (`ytInitialPlayerResponse`, `ytInitialData`).

use std::collections::HashSet;

use serde::de::IgnoredAny;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::http::Http;
use crate::log_debug;
use crate::types::{PlayerResponse, YtInitialData};

pub const PLAYER_RESPONSE_DECL: &str = "var ytInitialPlayerResponse =";
pub const INITIAL_DATA_DECL: &str = "var ytInitialData =";
const CANONICAL_WATCH_LINK: &str = r#"<link rel="canonical" href="https://www.youtube.com/watch?v="#;
/// Membership streams are near the top; don't scan the whole /streams tab.
const MAX_STREAM_ITEM_CHECK: usize = 5;

/// Returns the JSON object assigned by `decl` inside a `<script>`.
pub fn extract_json<'a>(html: &'a str, decl: &str) -> Option<&'a str> {
    let decl_at = html.find(decl)?;
    let after_decl = html.get(decl_at + decl.len()..)?;
    let script = after_decl.get(..after_decl.find("</script>").unwrap_or(after_decl.len()))?;
    let trimmed = script.trim_start();
    if !trimmed.starts_with('{') {
        return None;
    }
    let mut stream = serde_json::Deserializer::from_str(trimmed).into_iter::<IgnoredAny>();
    match stream.next() {
        Some(Ok(IgnoredAny)) => trimmed.get(..stream.byte_offset()),
        _ => None,
    }
}

pub fn parse_player_response(html: &str) -> Result<PlayerResponse> {
    let json = extract_json(html, PLAYER_RESPONSE_DECL)
        .ok_or_else(|| Error::InvalidFormat("unable to find the player response in the watch page".into()))?;
    Ok(serde_json::from_str(json)?)
}

/// Video id from the page's canonical link (used when the page came from a
/// channel's /live URL).
pub fn video_id_from_watch_page(html: &str) -> Option<String> {
    let (_, after_link) = html.split_once(CANONICAL_WATCH_LINK)?;
    let (id, _) = after_link.split_once('"')?;
    (!id.is_empty()).then(|| id.to_string())
}

/// Watch URL of the first currently-live item on a channel's /streams tab not
/// in `skip_ids`. With `members_only`, only items with a members-only badge count.
pub fn newest_live_stream(data: &YtInitialData, members_only: bool, skip_ids: &HashSet<String>) -> Option<String> {
    let contents = data
        .contents
        .two_column_browse_results_renderer
        .tabs
        .iter()
        .rev()
        .find(|tab| tab.tab_renderer.endpoint.command_metadata.web_command_metadata.url.ends_with("/streams"))
        .map(|tab| &tab.tab_renderer.content.rich_grid_renderer.contents)?;

    contents
        .iter()
        .take(MAX_STREAM_ITEM_CHECK)
        .map(|content| &content.rich_item_renderer.content.video_renderer)
        .filter(|video| !skip_ids.contains(&video.video_id))
        .filter(|video| {
            !members_only
                || video
                    .badges
                    .iter()
                    .any(|badge| badge.metadata_badge_renderer.style == "BADGE_STYLE_TYPE_MEMBERS_ONLY")
        })
        .find(|video| {
            video
                .thumbnail_overlays
                .iter()
                .any(|overlay| overlay.thumbnail_overlay_time_status_renderer.style == "LIVE")
        })
        .map(|video| format!("https://www.youtube.com/watch?v={}", video.video_id))
}

/// Looks for a live stream on the /streams tab of a channel's /live URL.
pub async fn fetch_newest_live_stream(
    http: &Http,
    live_url: &str,
    members_only: bool,
    skip_ids: &HashSet<String>,
    cancel: &CancellationToken,
) -> Result<Option<String>> {
    let streams_url =
        live_url.strip_suffix("/live").map(|base| format!("{base}/streams")).unwrap_or_else(|| live_url.to_string());
    let html = http.get_bytes_cancellable(&streams_url, cancel).await?;
    let html = String::from_utf8_lossy(&html);
    let Some(json) = extract_json(&html, INITIAL_DATA_DECL) else {
        return Ok(None);
    };
    let data: YtInitialData = match serde_json::from_str(json) {
        Ok(data) => data,
        Err(error) => {
            log_debug!("Failed to parse ytInitialData: {error}");
            return Ok(None);
        }
    };
    Ok(newest_live_stream(&data, members_only, skip_ids))
}

/// HTML for the stream's watch page. For channel URLs, a live item on the
/// /streams tab is preferred; members-only mode never falls back to /live.
pub async fn get_video_html(
    http: &Http,
    page_url: &str,
    is_channel: bool,
    members_only: bool,
    skip_ids: &HashSet<String>,
    cancel: &CancellationToken,
) -> Result<String> {
    if is_channel {
        match fetch_newest_live_stream(http, page_url, members_only, skip_ids, cancel).await {
            Ok(Some(stream_url)) => match http.get_bytes_cancellable(&stream_url, cancel).await {
                Ok(page) => return Ok(String::from_utf8_lossy(&page).into_owned()),
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(error) if members_only => {
                    log_debug!("Failed to retrieve {stream_url}: {error}");
                    return Err(error);
                }
                Err(error) => log_debug!("Failed to retrieve {stream_url}: {error}"),
            },
            Ok(None) => {}
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(error) if members_only => {
                log_debug!("Failed to check /streams for {page_url}: {error}");
                return Err(error);
            }
            Err(error) => log_debug!("Failed to check /streams for {page_url}: {error}"),
        }
    }
    if members_only {
        return Err(Error::NoMembersStream);
    }
    match http.get_bytes_cancellable(page_url, cancel).await {
        Ok(page) => Ok(String::from_utf8_lossy(&page).into_owned()),
        Err(error) => {
            if !matches!(error, Error::Cancelled) {
                log_debug!("Failed to retrieve {page_url}: {error}");
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WATCH_HTML: &str = r#"<html><head>
<link rel="canonical" href="https://www.youtube.com/watch?v=vid123">
<script>var other = {"a":1};</script>
<script nonce="x">var ytInitialPlayerResponse = {"playabilityStatus":{"status":"OK","liveStreamability":{"liveStreamabilityRenderer":{"videoId":"vid123"}}},"videoDetails":{"videoId":"vid123","title":"T","author":"A","isLiveContent":true,"shortDescription":"};x"},"microformat":{"playerMicroformatRenderer":{"liveBroadcastDetails":{"isLiveNow":true,"startTimestamp":"2026-11-07T01:02:03+00:00"},"publishDate":"2026-11-07"}},"streamingData":{"adaptiveFormats":[{"itag":140,"url":"https://r/x?noclen=1","targetDurationSec":5}]}};var meta = 1;</script>
<script>var ytInitialData = {"x":1};</script>
</head></html>"#;

    #[test]
    fn extracts_player_response() {
        let player_response = parse_player_response(WATCH_HTML).unwrap();
        assert_eq!(player_response.playability_status.status, "OK");
        assert_eq!(player_response.video_details.video_id, "vid123");
        // A `};` inside a string doesn't end the object early.
        assert_eq!(player_response.video_details.short_description, "};x");
        assert!(player_response.microformat.player_microformat_renderer.live_broadcast_details.is_live_now);
        assert_eq!(player_response.streaming_data.adaptive_formats[0].target_duration_sec, 5.0);
        assert_eq!(extract_json(WATCH_HTML, INITIAL_DATA_DECL), Some(r#"{"x":1}"#));
        assert_eq!(
            extract_json(r#"<script>var ytInitialData = {"x":1};var other = {"y":2};</script>"#, INITIAL_DATA_DECL),
            Some(r#"{"x":1}"#)
        );
    }

    #[test]
    fn missing_player_response() {
        assert!(parse_player_response("<html></html>").is_err());
        assert!(extract_json("<script>var ytInitialPlayerResponse = null;</script>", PLAYER_RESPONSE_DECL).is_none());
    }

    #[test]
    fn canonical_id() {
        assert_eq!(video_id_from_watch_page(WATCH_HTML).as_deref(), Some("vid123"));
        assert_eq!(video_id_from_watch_page("<html>"), None);
    }

    fn item(id: &str, live: bool, members: bool) -> serde_json::Value {
        serde_json::json!({"richItemRenderer":{"content":{"videoRenderer":{
            "videoId": id,
            "thumbnailOverlays": [{"thumbnailOverlayTimeStatusRenderer":{"style": if live {"LIVE"} else {"DEFAULT"}}}],
            "badges": if members { serde_json::json!([{"metadataBadgeRenderer":{"style":"BADGE_STYLE_TYPE_MEMBERS_ONLY"}}]) } else { serde_json::json!([]) }
        }}}})
    }

    fn initial_data(items: Vec<serde_json::Value>) -> YtInitialData {
        serde_json::from_value(serde_json::json!({"contents":{"twoColumnBrowseResultsRenderer":{"tabs":[
            {"tabRenderer":{"endpoint":{"commandMetadata":{"webCommandMetadata":{"url":"/@x/videos"}}}}},
            {"tabRenderer":{"endpoint":{"commandMetadata":{"webCommandMetadata":{"url":"/@x/streams"}}},
             "content":{"richGridRenderer":{"contents": items}}}}
        ]}}}))
        .unwrap()
    }

    #[test]
    fn finds_live_stream() {
        let empty = HashSet::new();
        let data = initial_data(vec![item("a", false, false), item("b", true, false), item("c", true, true)]);
        assert_eq!(newest_live_stream(&data, false, &empty).as_deref(), Some("https://www.youtube.com/watch?v=b"));
        assert_eq!(newest_live_stream(&data, true, &empty).as_deref(), Some("https://www.youtube.com/watch?v=c"));
        let skip = HashSet::from(["b".to_string()]);
        assert_eq!(newest_live_stream(&data, false, &skip).as_deref(), Some("https://www.youtube.com/watch?v=c"));
    }

    #[test]
    fn only_first_five_items() {
        let mut items: Vec<_> = (0..5).map(|number| item(&number.to_string(), false, false)).collect();
        items.push(item("late", true, false));
        assert_eq!(newest_live_stream(&initial_data(items), false, &HashSet::new()), None);
    }

    /// Hits YouTube. Run with `cargo test -- --ignored live_watch_page`.
    #[tokio::test]
    #[ignore]
    async fn live_watch_page() {
        let http = Http::new(&Default::default(), Default::default()).unwrap();
        let html = get_video_html(
            &http,
            "https://www.youtube.com/@LofiGirl/live",
            true,
            false,
            &HashSet::new(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        let player_response = parse_player_response(&html).unwrap();
        assert_eq!(player_response.playability_status.status, "OK", "{:?}", player_response.playability_status);
        assert!(!player_response.video_details.video_id.is_empty());
        assert!(video_id_from_watch_page(&html).is_some());
    }
}
