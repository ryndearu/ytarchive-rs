use super::*;
use crate::test_http;

const SAMPLE: &str = r#"WARNING: something
{"id":"abc123","title":" My Stream ","channel_id":"UC1","channel":"Chan","description":"d","thumbnail":"https://i/t.jpg","upload_date":"20261107","release_timestamp":1762500000,"timestamp":null,"live_status":"is_live","is_live":null,"formats":[
 {"format_id":"140-drc","url":"https://r1.googlevideo.com/videoplayback?itag=140&noclen=1&sq=9","protocol":"http_dash_segments","manifest_url":null,"target_duration":5.0},
 {"format_id":"299","url":"https://r1.googlevideo.com/videoplayback?itag=299&noclen=1","protocol":"http_dash_segments_generator","manifest_url":null,"target_duration":null},
 {"format_id":"95","url":"https://manifest.googlevideo.com/x.m3u8","protocol":"m3u8_native","manifest_url":"https://m/x.m3u8","target_duration":null},
 {"format_id":"303","url":"https://r1.googlevideo.com/videoplayback?itag=303&noclen=1","protocol":"http_dash_segments","manifest_url":"https://m/mpd","target_duration":null},
 {"format_id":"sb0","url":null,"protocol":"mhtml","manifest_url":null,"target_duration":null}
]}"#;

#[test]
fn args() {
    let opts = Options {
        cookies_file: Some("c.txt".into()),
        proxy: Some(reqwest::Url::parse("socks5://127.0.0.1:1080").unwrap()),
        ytdlp_opts: vec!["--extractor-args".into(), "youtube:x".into()],
        ..Default::default()
    };
    let args = build_args(&opts, "https://youtu.be/x", true);
    assert_eq!(
        &args[..10],
        [
            "--encoding",
            "utf-8",
            "--live-from-start",
            "--cookies",
            "c.txt",
            "--proxy",
            "socks5://127.0.0.1:1080",
            "--ignore-no-formats-error",
            "--extractor-args",
            "youtube:x"
        ]
    );
    assert_eq!(&args[args.len() - 2..], ["--", "https://youtu.be/x"]);
    assert!(!build_args(&Options::default(), "u", false).contains(&"--ignore-no-formats-error".to_string()));
    let repaired = Options { ytdlp_cookies_file: Some("fixed.txt".into()), ..opts };
    assert_eq!(&build_args(&repaired, "u", false)[3..5], ["--cookies", "fixed.txt"]);
    let v4 = Options { ip_version: IpVersion::V4, ..Options::default() };
    assert!(build_args(&v4, "u", false).contains(&"--force-ipv4".to_string()));
    let v6 = Options { ip_version: IpVersion::V6, ..Options::default() };
    assert!(build_args(&v6, "u", false).contains(&"--force-ipv6".to_string()));
}

#[test]
fn parses_sample() {
    let (info, probe_urls) = to_extraction(parse_output(SAMPLE).unwrap());
    assert_eq!(info.id, "abc123");
    assert_eq!(info.title, "My Stream");
    assert!(info.is_live);
    assert_eq!(info.live_status, LiveStatus::IsLive);
    assert_eq!(info.start_unix, Some(1_762_500_000));
    assert_eq!(info.target_duration, 5);

    let mut itags: Vec<_> = info.urls.keys().copied().collect();
    itags.sort();
    assert_eq!(itags, [140, 299]);
    assert_eq!(info.urls[&140].for_seq(3), "https://r1.googlevideo.com/videoplayback?itag=140&noclen=1&sq=3");
    assert_eq!(
        probe_urls,
        [
            "https://r1.googlevideo.com/videoplayback?itag=140&noclen=1&sq=9",
            "https://r1.googlevideo.com/videoplayback?itag=299&noclen=1",
        ]
    );
}

#[test]
fn prefers_exact_format_id_and_adaptive_target_duration() {
    let raw = parse_output(
        r#"{"id":"x","formats":[
            {"format_id":"95","url":"https://manifest.googlevideo.com/x.m3u8","protocol":"m3u8_native","manifest_url":"https://m/x.m3u8","target_duration":2.0},
            {"format_id":"140-drc","url":"https://drc.googlevideo.com/videoplayback?itag=140&noclen=1","protocol":"http_dash_segments","manifest_url":null,"target_duration":5.0},
            {"format_id":"140","url":"https://orig.googlevideo.com/videoplayback?itag=140&noclen=1","protocol":"http_dash_segments","manifest_url":null,"target_duration":null},
            {"format_id":"140-1","url":"https://alt.googlevideo.com/videoplayback?itag=140&noclen=1","protocol":"http_dash_segments","manifest_url":null,"target_duration":null}
        ]}"#,
    )
    .unwrap();
    let (info, _) = to_extraction(raw);
    assert_eq!(info.target_duration, 5);
    assert_eq!(info.urls[&140].for_seq(1), "https://orig.googlevideo.com/videoplayback?itag=140&noclen=1&sq=1");

    let suffixed = parse_output(
        r#"{"id":"x","formats":[
            {"format_id":"140-0","url":"https://dub.googlevideo.com/videoplayback?itag=140&noclen=1","protocol":"http_dash_segments","manifest_url":null},
            {"format_id":"140-1","url":"https://main.googlevideo.com/videoplayback?itag=140&noclen=1","protocol":"http_dash_segments","manifest_url":null}
        ]}"#,
    )
    .unwrap();
    let (info, _) = to_extraction(suffixed);
    assert_eq!(info.urls[&140].for_seq(1), "https://main.googlevideo.com/videoplayback?itag=140&noclen=1&sq=1");
}

#[test]
fn upcoming_without_formats() {
    let raw = parse_output(r#"{"id":"x","live_status":"is_upcoming","timestamp":5,"formats":null}"#).unwrap();
    let (info, probe_urls) = to_extraction(raw);
    assert_eq!(info.live_status, LiveStatus::IsUpcoming);
    assert!(!info.is_live);
    assert_eq!(info.start_unix, Some(5));
    assert!(info.urls.is_empty());
    assert!(probe_urls.is_empty());
}

#[test]
fn parse_skips_crlf_warnings_and_rejects_bad_json() {
    let raw = parse_output("WARNING: a\r\nWARNING: b\r\n{\"id\":\"x\"}\r\n").unwrap();
    assert_eq!(raw.id.as_deref(), Some("x"));
    assert!(parse_output("").is_err());
    assert!(parse_output("{not json").is_err());
}

#[tokio::test]
async fn head_probe_falls_back_across_adaptive_formats() {
    let addr = test_http::serve(|request| {
        let response = if test_http::path(&request).starts_with("/b") {
            test_http::response("200 OK", "x-head-seqnum: 42\r\n", b"")
        } else {
            test_http::response("404 Not Found", "", b"")
        };
        async move { Some(response) }
    })
    .await;

    let base = format!("http://{addr}");
    let stdout = format!(
        r#"{{"id":"x","live_status":"is_live","formats":[
            {{"format_id":"140","url":"{base}/a?itag=140&noclen=1","protocol":"http_dash_segments","manifest_url":null}},
            {{"format_id":"299","url":"{base}/b?itag=299&noclen=1","protocol":"http_dash_segments","manifest_url":null}}
        ]}}"#
    );
    let http = Http::new(&Options::default(), Default::default()).unwrap();
    let info = finish_extraction(&http, &stdout, &CancellationToken::new()).await.unwrap();
    assert_eq!(info.last_sq, Some(42));
}

#[tokio::test]
async fn run_with_retry_fails_fast_on_missing_binary() {
    let opts = Options { ytdlp_path: "definitely-nonexistent-yt-dlp-binary-xyz".into(), ..Default::default() };
    let started = std::time::Instant::now();
    let err = run_with_retry(&opts, "https://youtu.be/x", false, 3, &CancellationToken::new()).await.unwrap_err();
    assert!(is_start_error(&err), "{err}");
    assert!(started.elapsed() < RETRY_DELAY, "should not sleep between spawn failures");
}

/// Hits YouTube. Run with `cargo test -- --ignored live_extraction`.
/// Needs a JS runtime for yt-dlp; set YTDLP_TEST_OPTS (e.g. `--js-runtimes node`)
/// if deno isn't installed.
#[tokio::test]
#[ignore]
async fn live_extraction() {
    let opts = Options {
        ytdlp_path: "yt-dlp".into(),
        ytdlp_opts: std::env::var("YTDLP_TEST_OPTS").unwrap_or_default().split_whitespace().map(String::from).collect(),
        ..Default::default()
    };
    let http = Http::new(&opts, Default::default()).unwrap();
    let info = extract(
        &http,
        &opts,
        "https://www.youtube.com/@LofiGirl/live", // 24/7 stream
        true,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(info.is_live, "{info:?}");
    assert!(info.urls.contains_key(&140), "{:?}", info.urls.keys());
    assert!(info.last_sq.is_some());
    assert!(info.target_duration > 0);
}
