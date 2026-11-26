//! Tests for watch-page channel polling and state transitions.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use super::player::Outcome;
use super::*;
use crate::http::Http;
use crate::input_url::InputUrl;
use crate::test_http;
use crate::types::{Action, Options};

fn watch_html(video_id: &str, status: &str, is_live_now: bool) -> String {
    let payload = serde_json::json!({
        "playabilityStatus": {
            "status": status,
            "liveStreamability": {"liveStreamabilityRenderer": {"videoId": video_id}}
        },
        "videoDetails": {
            "videoId": video_id,
            "title": format!("Title {video_id}"),
            "author": "Channel",
            "isLiveContent": true
        },
        "microformat": {
            "playerMicroformatRenderer": {
                "liveBroadcastDetails": {
                    "isLiveNow": is_live_now,
                    "startTimestamp": "2026-11-07T01:02:03+00:00",
                    "endTimestamp": ""
                }
            }
        }
    });
    page(video_id, &payload)
}

fn non_livestream_html(video_id: &str) -> String {
    let payload = serde_json::json!({
        "playabilityStatus": {"status": "OK"},
        "videoDetails": {
            "videoId": video_id,
            "title": format!("Title {video_id}"),
            "author": "Channel",
            "isLiveContent": false
        }
    });
    page(video_id, &payload)
}

fn page(video_id: &str, payload: &serde_json::Value) -> String {
    format!(
        r#"<html><head><link rel="canonical" href="https://www.youtube.com/watch?v={video_id}"><script>var ytInitialPlayerResponse = {payload};</script></head></html>"#
    )
}

fn empty_details_html() -> String {
    r#"<html><head><script>var ytInitialPlayerResponse = {"playabilityStatus":{"status":"OK"},"videoDetails":{}};</script></head></html>"#
        .to_string()
}

fn channel_extractor(base: &str) -> Extractor {
    let opts = Arc::new(Options {
        quality: Some("best".into()),
        wait: Action::Do,
        retry_stream: Some(Duration::ZERO),
        ..Default::default()
    });
    let http = Http::new(&opts, Default::default()).unwrap();
    let input = InputUrl::Channel { live_url: format!("{base}/@chan/live") };
    let mut extractor = Extractor::new(opts, http, CancellationToken::new(), &input, &HashSet::new());
    extractor.retry_secs = Some(0);
    extractor.test_base_url = Some(base.to_string());
    extractor
}

/// `/@chan/live` shows vid1 for `vid1_hits` polls, then vid2, which is live.
/// Every case must go back to `/@chan/live` and pick up vid2.
#[tokio::test]
async fn channel_wait_returns_to_live_when_the_found_stream_is_unusable() {
    let ok = |body: String| ("200 OK", body);
    for (case, vid1_live_now, vid1_watch, vid1_hits, reported) in [
        ("http-error", true, ("500 Internal Server Error", String::new()), 1, None),
        ("empty-details", true, ok(empty_details_html()), 1, None),
        ("offline", true, ok(watch_html("vid1", "LIVE_STREAM_OFFLINE", false)), 1, None),
        ("not-started", false, ok(watch_html("vid1", "OK", false)), 1, None),
        ("error", true, ok(watch_html("vid1", "ERROR", false)), 2, Some("ERROR")),
        ("unplayable", true, ok(watch_html("vid1", "UNPLAYABLE", false)), 2, Some("UNPLAYABLE")),
        ("non-livestream", true, ok(non_livestream_html("vid1")), 2, Some("NOT_LIVE")),
    ] {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let live_hits = AtomicUsize::new(0);
        let addr = test_http::serve({
            let seen = seen.clone();
            move |request| {
                let path = test_http::path(&request).to_string();
                seen.lock().unwrap().push(path.clone());
                let (status, body) = match path.as_str() {
                    "/@chan/streams" => ("404 Not Found", String::new()),
                    "/@chan/live" if live_hits.fetch_add(1, Ordering::SeqCst) < vid1_hits => {
                        ok(watch_html("vid1", "OK", vid1_live_now))
                    }
                    "/@chan/live" | "/watch?v=vid2" => ok(watch_html("vid2", "OK", true)),
                    "/watch?v=vid1" => vid1_watch.clone(),
                    other => panic!("unexpected path in case {case}: {other}"),
                };
                let response =
                    test_http::response(status, "content-type: text/html; charset=utf-8\r\n", body.as_bytes());
                async move { Some(response) }
            }
        })
        .await;

        let mut extractor = channel_extractor(&format!("http://{addr}"));
        match extractor.get_playable_player_response().await {
            Outcome::Found(response) => assert_eq!(response.video_details.video_id, "vid2", "case {case}"),
            _ => panic!("expected Found(vid2) in case {case}"),
        }
        let paths = seen.lock().unwrap().clone();
        let count = |wanted: &str| paths.iter().filter(|path| *path == wanted).count();
        assert!(count("/@chan/live") > vid1_hits, "case {case}: {paths:?}");
        assert_eq!(count("/watch?v=vid1"), vid1_hits, "case {case}: {paths:?}");
        if let Some(reason) = reported {
            assert_eq!(extractor.reported_unusable, HashSet::from([("vid1".into(), reason.into())]), "case {case}");
        }
    }
}

fn streams_html(items: &[(&str, bool)]) -> String {
    let items: Vec<_> = items
        .iter()
        .map(|(id, members)| {
            let badges = if *members {
                serde_json::json!([{"metadataBadgeRenderer": {"style": "BADGE_STYLE_TYPE_MEMBERS_ONLY"}}])
            } else {
                serde_json::json!([])
            };
            serde_json::json!({"richItemRenderer": {"content": {"videoRenderer": {
                "videoId": id,
                "thumbnailOverlays": [{"thumbnailOverlayTimeStatusRenderer": {"style": "LIVE"}}],
                "badges": badges
            }}}})
        })
        .collect();
    let data = serde_json::json!({"contents": {"twoColumnBrowseResultsRenderer": {"tabs": [
        {"tabRenderer": {"endpoint": {"commandMetadata": {"webCommandMetadata": {"url": "/@chan/streams"}}},
         "content": {"richGridRenderer": {"contents": items}}}}
    ]}}});
    format!("<html><script>var ytInitialData = {data};</script></html>")
}

/// An idle channel under --members-only is polled like an offline one (here
/// until cancelled), and without -r it ends at once instead of retrying.
#[tokio::test]
async fn members_only_idle_channel_is_not_an_error() {
    for (case, waits) in [("waiting", true), ("not-waiting", false)] {
        let streams_hits = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();
        let addr = test_http::serve({
            let (streams_hits, cancel) = (streams_hits.clone(), cancel.clone());
            move |request| {
                let path = test_http::path(&request).to_string();
                assert_eq!(path, "/@chan/streams", "case {case}");
                if streams_hits.fetch_add(1, Ordering::SeqCst) == 2 {
                    cancel.cancel();
                }
                let body = streams_html(&[("public", false)]);
                let response =
                    test_http::response("200 OK", "content-type: text/html; charset=utf-8\r\n", body.as_bytes());
                async move { Some(response) }
            }
        })
        .await;

        let mut extractor = channel_extractor(&format!("http://{addr}"));
        extractor.members_only = true;
        extractor.cancel = cancel;
        if !waits {
            extractor.retry_secs = None;
        }
        let outcome = extractor.get_playable_player_response().await;
        let hits = streams_hits.load(Ordering::SeqCst);
        match (waits, outcome) {
            (true, Outcome::NotUsable) => assert_eq!(hits, 3, "case {case}"),
            (false, Outcome::NotFound) => assert_eq!(hits, 1, "case {case}"),
            _ => panic!("unexpected outcome in case {case} after {hits} lookups"),
        }
    }
}

#[tokio::test]
async fn ytdlp_mode_mid_download_refresh_skips_head_probe_while_start_delay_probes() {
    let head_hits = Arc::new(AtomicUsize::new(0));
    let addr = test_http::serve({
        let head_hits = head_hits.clone();
        move |_| {
            head_hits.fetch_add(1, Ordering::SeqCst);
            async { Some(test_http::response("200 OK", "x-head-seqnum: 42\r\n", b"")) }
        }
    })
    .await;

    let stdout = format!(
        r#"{{"id":"vid1","live_status":"is_live","is_live":true,"formats":[
            {{"format_id":"140","url":"http://{addr}/a?itag=140&noclen=1","protocol":"http_dash_segments","manifest_url":null}},
            {{"format_id":"299","url":"http://{addr}/v?itag=299&noclen=1","protocol":"http_dash_segments","manifest_url":null}}
        ]}}"#
    );
    let opts = Arc::new(Options { ytdlp_info: true, test_ytdlp_stdout: Some(stdout), ..Default::default() });
    let http = Http::new(&opts, Default::default()).unwrap();
    let input = InputUrl::Video { id: "vid1".into() };
    let mut extractor = Extractor::new(opts, http, CancellationToken::new(), &input, &HashSet::new());
    extractor.in_progress = true;
    extractor.fixed_itag = Some(Some(299));
    extractor.last_sq = Some(99);

    let selection = extractor.refresh_urls().await.expect("mid-download refresh should succeed without probe");
    assert!(!extractor.unavailable);
    assert_eq!(head_hits.load(Ordering::SeqCst), 0);
    assert_eq!(extractor.last_sq, Some(99));
    assert_eq!(selection.audio_url.unwrap().for_seq(5), format!("http://{addr}/a?itag=140&noclen=1&sq=5"));
    assert_eq!(selection.video_url.unwrap().for_seq(5), format!("http://{addr}/v?itag=299&noclen=1&sq=5"));

    // A --start-delay refresh calls get_video_info with in_progress=true and must still probe head.
    extractor.last_updated = None;
    let delayed = extractor.get_video_info().await.expect("start-delay refresh should probe head");
    assert!(delayed.video_url.is_some());
    assert!(head_hits.load(Ordering::SeqCst) >= 1);
    assert_eq!(extractor.last_sq, Some(42));
}
