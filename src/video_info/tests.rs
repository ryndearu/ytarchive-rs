//! Tests for watch-page channel polling and state transitions.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use super::player::Outcome;
use super::*;
use crate::http::Http;
use crate::input_url::InputUrl;
use crate::types::{Action, Options};

fn watch_html(video_id: &str, status: &str, is_live_now: bool, end_timestamp: &str) -> String {
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
                    "endTimestamp": end_timestamp
                }
            }
        }
    });
    format!(
        r#"<html><head><link rel="canonical" href="https://www.youtube.com/watch?v={video_id}"><script>var ytInitialPlayerResponse = {payload};</script></head></html>"#
    )
}

fn empty_details_html() -> String {
    r#"<html><head><script>var ytInitialPlayerResponse = {"playabilityStatus":{"status":"OK"},"videoDetails":{}};</script></head></html>"#
        .to_string()
}

type Handler = Arc<dyn Fn(&str) -> (&'static str, String) + Send + Sync>;

async fn serve_pages(handler: Handler) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let handler = handler.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let mut count = 0;
                while !buf[..count].windows(4).any(|window| window == b"\r\n\r\n") {
                    match sock.read(&mut buf[count..]).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => count += read,
                    }
                }
                let req = String::from_utf8_lossy(&buf[..count]);
                let path = req.split_whitespace().nth(1).unwrap_or("/");
                let (status, body) = handler(path);
                let hdr = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                sock.write_all(hdr.as_bytes()).await.ok();
                sock.write_all(body.as_bytes()).await.ok();
            });
        }
    });
    format!("http://127.0.0.1:{}", addr.port())
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

#[tokio::test]
async fn channel_wait_resets_to_live_after_watch_error_empty_or_offline() {
    for (case, watch_response) in [
        ("http-error", ("500 Internal Server Error", String::new())),
        ("empty-details", ("200 OK", empty_details_html())),
        ("offline", ("200 OK", watch_html("vid1", "LIVE_STREAM_OFFLINE", false, ""))),
    ] {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let live_hits = Arc::new(Mutex::new(0usize));
        let base = serve_pages(Arc::new({
            let seen = seen.clone();
            let live_hits = live_hits.clone();
            move |path| {
                seen.lock().unwrap().push(path.to_string());
                match path {
                    "/@chan/streams" => ("404 Not Found", String::new()),
                    "/@chan/live" => {
                        let mut hits = live_hits.lock().unwrap();
                        *hits += 1;
                        if *hits == 1 {
                            ("200 OK", watch_html("vid1", "OK", true, ""))
                        } else {
                            ("200 OK", watch_html("vid2", "OK", true, ""))
                        }
                    }
                    "/watch?v=vid1" => watch_response.clone(),
                    "/watch?v=vid2" => ("200 OK", watch_html("vid2", "OK", true, "")),
                    other => panic!("unexpected path in case {case}: {other}"),
                }
            }
        }))
        .await;

        let mut extractor = channel_extractor(&base);
        let outcome = extractor.get_playable_player_response().await;
        match outcome {
            Outcome::Found(response) => assert_eq!(response.video_details.video_id, "vid2", "case {case}"),
            _ => panic!("expected Found(vid2) in case {case}"),
        }
        let paths = seen.lock().unwrap().clone();
        assert!(
            paths.iter().filter(|path| path.as_str() == "/@chan/live").count() >= 2,
            "case {case} should have returned to /@chan/live: {paths:?}"
        );
    }
}
