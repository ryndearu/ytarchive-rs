//! Stream info extraction by running yt-dlp and parsing its `--print` JSON.

use std::process::Stdio;
use std::time::Duration;

use reqwest::header::ORIGIN;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::http::Http;
use crate::types::{
    FragmentUrl, LiveStatus, Options, YTDLP_INFO_PRINT_TEMPLATE, YtdlpExtraction, YtdlpFormat, YtdlpRawInfo,
};
use crate::{log_debug, log_trace, log_warn};

const RUN_TIMEOUT: Duration = Duration::from_secs(30);
const RETRY_DELAY: Duration = Duration::from_secs(2);
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_ATTEMPTS: u32 = 3;

/// Arguments for one yt-dlp invocation. `ignore_no_formats` keeps yt-dlp from
/// failing on upcoming streams so we can still read their status.
pub fn build_args(opts: &Options, url: &str, ignore_no_formats: bool) -> Vec<String> {
    // UTF-8 output regardless of the console code page (Windows), so titles
    // aren't mangled; user --ytdlp-opts come later and can override it.
    let mut args = vec!["--encoding".to_string(), "utf-8".into(), "--live-from-start".into()];
    if let Some(path) = opts.ytdlp_cookies_file.as_ref().or(opts.cookies_file.as_ref()) {
        args.push("--cookies".into());
        args.push(path.to_string_lossy().into_owned());
    }
    if let Some(url) = &opts.proxy {
        args.push("--proxy".into());
        args.push(url.to_string());
    }
    if ignore_no_formats {
        args.push("--ignore-no-formats-error".into());
    }
    args.extend(opts.ytdlp_opts.iter().cloned());
    args.push("--print".into());
    args.push(YTDLP_INFO_PRINT_TEMPLATE.into());
    // `--` so a URL can never be read as an option.
    args.push("--".into());
    args.push(url.into());
    args
}

/// Runs yt-dlp once and returns stdout.
pub async fn run(opts: &Options, url: &str, ignore_no_formats: bool) -> Result<String> {
    let args = build_args(opts, url, ignore_no_formats);
    log_trace!("Running {} {:?}", opts.ytdlp_path, args);

    let child = Command::new(&opts.ytdlp_path)
        .args(&args)
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| Error::Ytdlp(format!("failed to start '{}': {error}", opts.ytdlp_path)))?;

    let out = tokio::time::timeout(RUN_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| Error::Ytdlp(format!("timed out after {}s", RUN_TIMEOUT.as_secs())))??;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let last = stderr.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("");
        return Err(Error::Ytdlp(format!("{}: {last}", out.status)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Runs yt-dlp up to `attempts` times, 2s apart. Stops early on cancel.
pub async fn run_with_retry(
    opts: &Options,
    url: &str,
    ignore_no_formats: bool,
    attempts: u32,
    cancel: &CancellationToken,
) -> Result<String> {
    let mut last_err = Error::Ytdlp("not run".into());
    for attempt in 1..=attempts {
        match run(opts, url, ignore_no_formats).await {
            Ok(out) => return Ok(out),
            Err(error) => {
                log_warn!("yt-dlp attempt {attempt}/{attempts} failed: {error}");
                last_err = error;
            }
        }
        if attempt < attempts {
            tokio::select! {
                _ = cancel.cancelled() => return Err(Error::Cancelled),
                _ = tokio::time::sleep(RETRY_DELAY) => {}
            }
        }
    }
    log_warn!("Failed to get stream info from yt-dlp after {attempts} attempts");
    Err(last_err)
}

/// Parses the first JSON object in yt-dlp's output, skipping any leading
/// non-JSON lines. Anything after the object (e.g. further playlist entries)
/// is ignored.
pub fn parse_output(stdout: &str) -> Result<YtdlpRawInfo> {
    // split_inclusive keeps line endings, so offsets stay exact with CRLF too.
    let start = stdout
        .split_inclusive('\n')
        .scan(0usize, |offset, line| {
            let at = *offset;
            *offset += line.len();
            Some((at, line))
        })
        .find(|(_, line)| line.trim_start().starts_with('{'))
        .map(|(at, _)| at)
        .ok_or_else(|| Error::Ytdlp("no JSON in yt-dlp output".into()))?;
    let mut stream = serde_json::Deserializer::from_str(&stdout[start..]).into_iter::<YtdlpRawInfo>();
    match stream.next() {
        Some(result) => Ok(result?),
        None => Err(Error::Ytdlp("no JSON in yt-dlp output".into())),
    }
}

/// Adaptive fragment URLs: plain googlevideo `noclen` URLs delivered as DASH
/// segments, not manifest-based (HLS/DASH manifest) formats.
pub fn is_adaptive_format(format: &YtdlpFormat) -> bool {
    let Some(url) = format.url.as_deref() else {
        return false;
    };
    crate::types::is_fragmented(url)
        && format.manifest_url.as_deref().is_none_or(str::is_empty)
        && matches!(format.protocol.as_deref(), Some("http_dash_segments" | "http_dash_segments_generator"))
}

/// Converts raw yt-dlp output into an extraction, without network access.
/// Returns the raw URL of the first adaptive format for the sequence probe.
pub fn to_extraction(raw: YtdlpRawInfo) -> (YtdlpExtraction, Option<String>) {
    let live_status = LiveStatus::parse(raw.live_status.as_deref().unwrap_or(""));
    let start_unix = raw.release_timestamp.filter(|time| *time > 0).or(raw.timestamp.filter(|time| *time > 0));

    let mut info = YtdlpExtraction {
        id: raw.id.unwrap_or_default(),
        title: raw.title.unwrap_or_default().trim().to_string(),
        channel_id: raw.channel_id.unwrap_or_default(),
        channel: raw.channel.unwrap_or_default(),
        description: raw.description.unwrap_or_default(),
        thumbnail: raw.thumbnail.unwrap_or_default(),
        upload_date: raw.upload_date.unwrap_or_default(),
        start_unix,
        live_status,
        is_live: raw.is_live.unwrap_or(false) || live_status == LiveStatus::IsLive,
        ..Default::default()
    };

    let mut probe_url = None;
    for format in raw.formats.unwrap_or_default() {
        let Some(itag) = format.itag() else { continue };
        if info.target_duration == 0
            && let Some(target) = format.target_duration.filter(|seconds| *seconds > 0.0)
        {
            info.target_duration = target.round() as u32;
        }
        if !is_adaptive_format(&format) {
            continue;
        }
        let url = format.url.unwrap_or_default();
        probe_url.get_or_insert_with(|| url.clone());
        info.urls.insert(itag, FragmentUrl::with_sq_param(&url));
    }
    (info, probe_url)
}

/// Reads the `X-Head-Seqnum` header (latest available fragment) via HEAD.
pub async fn probe_x_head_seqnum(http: &Http, url: &str) -> Option<u64> {
    let resp =
        http.client.head(url).header(ORIGIN, "https://www.youtube.com").timeout(PROBE_TIMEOUT).send().await.ok()?;
    resp.headers().get("x-head-seqnum")?.to_str().ok()?.trim().parse().ok()
}

/// Sequence boundary for downloading. Post-live, the last couple of fragments
/// tend to return 503, so stop one short of the head.
pub fn seq_boundary(head: u64, post_live: bool) -> Option<u64> {
    if post_live { (head > 1).then(|| head - 1) } else { Some(head) }
}

/// Full extraction: run yt-dlp (with retries), parse, and probe the last sequence.
pub async fn extract(
    http: &Http,
    opts: &Options,
    url: &str,
    ignore_no_formats: bool,
    cancel: &CancellationToken,
) -> Result<YtdlpExtraction> {
    let out = run_with_retry(opts, url, ignore_no_formats, DEFAULT_ATTEMPTS, cancel).await?;
    let (mut info, probe_url) = to_extraction(parse_output(&out)?);
    if let Some(part) = probe_url {
        let post_live = info.live_status == LiveStatus::PostLive;
        info.last_sq = probe_x_head_seqnum(http, &part).await.and_then(|head| seq_boundary(head, post_live));
    }
    if !info.urls.is_empty() {
        log_debug!("Loaded {} adaptive format URLs from yt-dlp", info.urls.len());
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
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
            proxy: Some(url::Url::parse("socks5://127.0.0.1:1080").unwrap()),
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
    }

    #[test]
    fn parses_sample() {
        let (info, probe) = to_extraction(parse_output(SAMPLE).unwrap());
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
        assert_eq!(probe.as_deref(), Some("https://r1.googlevideo.com/videoplayback?itag=140&noclen=1&sq=9"));
    }

    #[test]
    fn upcoming_without_formats() {
        let raw = parse_output(r#"{"id":"x","live_status":"is_upcoming","timestamp":5,"formats":null}"#).unwrap();
        let (info, probe) = to_extraction(raw);
        assert_eq!(info.live_status, LiveStatus::IsUpcoming);
        assert!(!info.is_live);
        assert_eq!(info.start_unix, Some(5));
        assert!(info.urls.is_empty());
        assert!(probe.is_none());
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

    /// Hits YouTube. Run with `cargo test -- --ignored live_extraction`.
    /// Needs a JS runtime for yt-dlp; set YTDLP_TEST_OPTS (e.g. `--js-runtimes node`)
    /// if deno isn't installed.
    #[tokio::test]
    #[ignore]
    async fn live_extraction() {
        let opts = Options {
            ytdlp_path: "yt-dlp".into(),
            ytdlp_opts: std::env::var("YTDLP_TEST_OPTS")
                .unwrap_or_default()
                .split_whitespace()
                .map(String::from)
                .collect(),
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
}
