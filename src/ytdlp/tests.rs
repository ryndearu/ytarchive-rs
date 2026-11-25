use super::*;

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
    assert!(!build_args(&Options::default(), "u", false).contains(&OsString::from("--ignore-no-formats-error")));
    let repaired = Options { ytdlp_cookies_file: Some("fixed.txt".into()), ..opts };
    assert_eq!(&build_args(&repaired, "u", false)[3..5], ["--cookies", "fixed.txt"]);
    let v4 = Options { ip_version: IpVersion::V4, ..Options::default() };
    assert!(build_args(&v4, "u", false).contains(&OsString::from("--force-ipv4")));
    let v6 = Options { ip_version: IpVersion::V6, ..Options::default() };
    assert!(build_args(&v6, "u", false).contains(&OsString::from("--force-ipv6")));

    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let raw = PathBuf::from(OsStr::from_bytes(b"/tmp/cookies-\xff.txt"));
        let non_utf8 = Options { cookies_file: Some(raw.clone()), ..Options::default() };
        assert_eq!(build_args(&non_utf8, "u", false)[4], raw.as_os_str());
    }
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
fn clamps_subsecond_target_duration_and_ignores_non_positive() {
    let subsecond = parse_output(
        r#"{"id":"x","formats":[
            {"format_id":"95","url":"https://manifest.googlevideo.com/x.m3u8","protocol":"m3u8_native","manifest_url":"https://m/x.m3u8","target_duration":5.0},
            {"format_id":"140","url":"https://orig.googlevideo.com/videoplayback?itag=140&noclen=1","protocol":"http_dash_segments","manifest_url":null,"target_duration":0.4}
        ]}"#,
    )
    .unwrap();
    let (info, _) = to_extraction(subsecond);
    assert_eq!(info.target_duration, 1);

    let invalid = parse_output(
        r#"{"id":"x","formats":[
            {"format_id":"140","url":"https://a.googlevideo.com/videoplayback?itag=140&noclen=1","protocol":"http_dash_segments","manifest_url":null,"target_duration":0.0},
            {"format_id":"299","url":"https://b.googlevideo.com/videoplayback?itag=299&noclen=1","protocol":"http_dash_segments","manifest_url":null,"target_duration":-1.0}
        ]}"#,
    )
    .unwrap();
    let (info, _) = to_extraction(invalid);
    assert_eq!(info.target_duration, 0);
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
fn parses_after_crlf_lines() {
    let raw = parse_output("WARNING: a\r\nWARNING: b\r\n{\"id\":\"x\"}\r\n").unwrap();
    assert_eq!(raw.id.as_deref(), Some("x"));
}

#[test]
fn parse_errors() {
    assert!(parse_output("").is_err());
    assert!(parse_output("{not json").is_err());
}

#[test]
fn boundary() {
    assert_eq!(seq_boundary(100, false), Some(100));
    assert_eq!(seq_boundary(100, true), Some(99));
    assert_eq!(seq_boundary(1, true), None);
}

#[test]
fn stderr_prefers_error_line() {
    assert_eq!(
        stderr_error_line("WARNING: old\nERROR: [youtube] abc: Video unavailable\r\nWARNING: hint\n\n"),
        "ERROR: [youtube] abc: Video unavailable"
    );
    assert_eq!(stderr_error_line("ERROR: first\nERROR: second\nTrailing note\n"), "ERROR: second");
    assert_eq!(stderr_error_line("WARNING: one\n  plain failure  \n\n"), "plain failure");
    assert_eq!(stderr_error_line("  \n\r\n"), "");
}

#[tokio::test]
async fn head_probe_falls_back_across_adaptive_formats() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 4096];
                let mut count = 0;
                while !buffer[..count].windows(4).any(|window| window == b"\r\n\r\n") {
                    match sock.read(&mut buffer[count..]).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => count += read,
                    }
                }
                let request = String::from_utf8_lossy(&buffer[..count]);
                let path = request.split_whitespace().nth(1).unwrap_or("/");
                let response = if path.starts_with("/b") {
                    "HTTP/1.1 200 OK\r\nx-head-seqnum: 42\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                } else {
                    "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                };
                sock.write_all(response.as_bytes()).await.ok();
            });
        }
    });

    let base = format!("http://127.0.0.1:{}", addr.port());
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
