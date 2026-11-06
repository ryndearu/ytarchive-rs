//! Scraping YouTube watch/channel pages for embedded JSON
//! (`ytInitialPlayerResponse`, `ytInitialData`).

use crate::error::{Error, Result};
use crate::http::Http;
use crate::log_debug;
use crate::types::{PlayerResponse, YtInitialData};

pub const PLAYER_RESPONSE_DECL: &str = "var ytInitialPlayerResponse =";
pub const INITIAL_DATA_DECL: &str = "var ytInitialData =";
const CANONICAL_WATCH_LINK: &str = r#"<link rel="canonical" href="https://www.youtube.com/watch?v="#;
/// Membership streams are near the top; don't scan the whole /streams tab.
const MAX_STREAM_ITEM_CHECK: usize = 5;

/// Returns the JSON object assigned by `decl` inside a `<script>`: from the
/// first `{` after the declaration to the last `};` before `</script>`.
pub fn extract_json<'a>(html: &'a str, decl: &str) -> Option<&'a str> {
    let decl_at = html.find(decl)?;
    let rest = &html[decl_at..];
    let script = &rest[..rest.find("</script>").unwrap_or(rest.len())];
    let start = script.find('{')?;
    let end = script.rfind("};")?;
    (end > start).then(|| &script[start..=end])
}

pub fn parse_player_response(html: &str) -> Result<PlayerResponse> {
    let json = extract_json(html, PLAYER_RESPONSE_DECL).ok_or_else(|| {
        Error::InvalidFormat("unable to find the player response in the watch page".into())
    })?;
    Ok(serde_json::from_str(json)?)
}

/// Video id from the page's canonical link (used when the page came from a
/// channel's /live URL).
pub fn video_id_from_watch_page(html: &str) -> Option<String> {
    let start = html.find(CANONICAL_WATCH_LINK)? + CANONICAL_WATCH_LINK.len();
    let len = html[start..].find('"')?;
    let id = &html[start..start + len];
    (!id.is_empty()).then(|| id.to_string())
}

/// Watch URL of the first currently-live item on a channel's /streams tab.
/// With `members_only`, only items with a members-only badge count.
pub fn newest_live_stream(data: &YtInitialData, members_only: bool) -> Option<String> {
    let contents = data
        .contents
        .two_column_browse_results_renderer
        .tabs
        .iter()
        .rev()
        .find(|t| {
            t.tab_renderer
                .endpoint
                .command_metadata
                .web_command_metadata
                .url
                .ends_with("/streams")
        })
        .map(|t| &t.tab_renderer.content.rich_grid_renderer.contents)?;

    contents
        .iter()
        .take(MAX_STREAM_ITEM_CHECK)
        .map(|c| &c.rich_item_renderer.content.video_renderer)
        .filter(|v| {
            !members_only
                || v.badges
                    .iter()
                    .any(|b| b.metadata_badge_renderer.style == "BADGE_STYLE_TYPE_MEMBERS_ONLY")
        })
        .find(|v| {
            v.thumbnail_overlays
                .iter()
                .any(|o| o.thumbnail_overlay_time_status_renderer.style == "LIVE")
        })
        .map(|v| format!("https://www.youtube.com/watch?v={}", v.video_id))
}

/// Looks for a live stream on the /streams tab of a channel's /live URL.
pub async fn fetch_newest_live_stream(http: &Http, live_url: &str, members_only: bool) -> Option<String> {
    let streams_url = live_url.replacen("/live", "/streams", 1);
    let html = http.get_bytes(&streams_url).await.ok()?;
    let html = String::from_utf8_lossy(&html);
    let json = extract_json(&html, INITIAL_DATA_DECL)?;
    let data: YtInitialData = serde_json::from_str(json)
        .inspect_err(|e| log_debug!("Failed to parse ytInitialData: {e}"))
        .ok()?;
    newest_live_stream(&data, members_only)
}

/// HTML for the stream's watch page. For channel URLs, a live item on the
/// /streams tab is preferred; members-only mode never falls back to /live.
pub async fn get_video_html(
    http: &Http,
    page_url: &str,
    is_channel: bool,
    members_only: bool,
) -> Option<String> {
    if is_channel
        && let Some(stream_url) = fetch_newest_live_stream(http, page_url, members_only).await
        && let Ok(b) = http.get_bytes(&stream_url).await
    {
        return Some(String::from_utf8_lossy(&b).into_owned());
    }
    if members_only {
        return None;
    }
    match http.get_bytes(page_url).await {
        Ok(b) => Some(String::from_utf8_lossy(&b).into_owned()),
        Err(e) => {
            log_debug!("Failed to retrieve {page_url}: {e}");
            None
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
        let pr = parse_player_response(WATCH_HTML).unwrap();
        assert_eq!(pr.playability_status.status, "OK");
        assert_eq!(pr.video_details.video_id, "vid123");
        // A `};` inside a string doesn't end the object early.
        assert_eq!(pr.video_details.short_description, "};x");
        assert!(pr.microformat.player_microformat_renderer.live_broadcast_details.is_live_now);
        assert_eq!(pr.streaming_data.adaptive_formats[0].target_duration_sec, 5.0);
        assert_eq!(extract_json(WATCH_HTML, INITIAL_DATA_DECL), Some(r#"{"x":1}"#));
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
        let d = initial_data(vec![item("a", false, false), item("b", true, false), item("c", true, true)]);
        assert_eq!(newest_live_stream(&d, false).as_deref(), Some("https://www.youtube.com/watch?v=b"));
        assert_eq!(newest_live_stream(&d, true).as_deref(), Some("https://www.youtube.com/watch?v=c"));
    }

    #[test]
    fn only_first_five_items() {
        let mut items: Vec<_> = (0..5).map(|i| item(&i.to_string(), false, false)).collect();
        items.push(item("late", true, false));
        assert_eq!(newest_live_stream(&initial_data(items), false), None);
    }

    /// Hits YouTube. Run with `cargo test -- --ignored live_watch_page`.
    #[tokio::test]
    #[ignore]
    async fn live_watch_page() {
        let http = Http::new(&Default::default(), Default::default()).unwrap();
        let html = get_video_html(&http, "https://www.youtube.com/@LofiGirl/live", true, false)
            .await
            .unwrap();
        let pr = parse_player_response(&html).unwrap();
        assert_eq!(pr.playability_status.status, "OK", "{:?}", pr.playability_status);
        assert!(!pr.video_details.video_id.is_empty());
        assert!(video_id_from_watch_page(&html).is_some());
    }
}
