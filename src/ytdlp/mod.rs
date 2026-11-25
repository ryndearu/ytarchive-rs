//! Stream info extraction by running yt-dlp and parsing its `--print` JSON.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use reqwest::header::{ORIGIN, REFERER};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::http::{Http, redact_proxy_args};
use crate::log;
use crate::types::{
    FragmentUrl, IpVersion, LiveStatus, LogLevel, Options, YTDLP_INFO_PRINT_TEMPLATE, YtdlpExtraction, YtdlpFormat,
    YtdlpRawInfo, is_fragmented,
};
use crate::util::{resolve_program, sleep_or_cancel};
use crate::{log_debug, log_trace, log_warn};

// Go allowed 30 seconds. A run that also solves the JavaScript challenge on a
// slow machine or connection can need more, and every retry would time out
// the same way.
const RUN_TIMEOUT: Duration = Duration::from_secs(60);
const RETRY_DELAY: Duration = Duration::from_secs(2);
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_ATTEMPTS: u32 = 3;
const PROBE_RETRY_DELAY: Duration = Duration::from_secs(1);
pub const DEFAULT_ATTEMPTS: u32 = 3;

/// Arguments for one yt-dlp invocation. `ignore_no_formats` keeps yt-dlp from
/// failing on upcoming streams so we can still read their status.
pub fn build_args(opts: &Options, url: &str, ignore_no_formats: bool) -> Vec<OsString> {
    // UTF-8 output regardless of the console code page (Windows), so titles
    // aren't mangled; user --ytdlp-opts come later and can override it.
    let mut args: Vec<OsString> = vec!["--encoding".into(), "utf-8".into(), "--live-from-start".into()];
    if let Some(path) = opts.ytdlp_cookies_file.as_ref().or(opts.cookies_file.as_ref()) {
        args.push("--cookies".into());
        args.push(path.as_os_str().to_os_string());
    }
    if let Some(url) = &opts.proxy {
        args.push("--proxy".into());
        args.push(url.as_str().into());
    }
    match opts.ip_version {
        IpVersion::Any => {}
        IpVersion::V4 => args.push("--force-ipv4".into()),
        IpVersion::V6 => args.push("--force-ipv6".into()),
    }
    if ignore_no_formats {
        args.push("--ignore-no-formats-error".into());
    }
    args.extend(opts.ytdlp_opts.iter().map(OsString::from));
    args.push("--print".into());
    args.push(YTDLP_INFO_PRINT_TEMPLATE.into());
    // `--` so a URL can never be read as an option.
    args.push("--".into());
    args.push(url.into());
    args
}

fn stderr_error_line(stderr: &str) -> &str {
    stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with("ERROR:"))
        .or_else(|| stderr.lines().rev().map(str::trim).find(|line| !line.is_empty()))
        .unwrap_or("")
}

/// Runs yt-dlp once and returns stdout, stopping early on cancel.
pub async fn run(opts: &Options, url: &str, ignore_no_formats: bool, cancel: &CancellationToken) -> Result<String> {
    #[cfg(test)]
    if let Some(stdout) = &opts.test_ytdlp_stdout {
        return Ok(stdout.clone());
    }
    let args = build_args(opts, url, ignore_no_formats);
    let executable = resolve_program(&opts.ytdlp_path).unwrap_or_else(|| PathBuf::from(&opts.ytdlp_path));
    if log::enabled(LogLevel::Trace) {
        let display_args: Vec<String> = args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
        log_trace!("Running {} {:?}", executable.display(), redact_proxy_args(&display_args));
    }

    let child = Command::new(&executable)
        .args(&args)
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| Error::Ytdlp(format!("failed to start '{}': {error}", opts.ytdlp_path)))?;

    let out = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(Error::Cancelled),
        result = tokio::time::timeout(RUN_TIMEOUT, child.wait_with_output()) => {
            result.map_err(|_| Error::Ytdlp(format!("timed out after {}s", RUN_TIMEOUT.as_secs())))??
        }
    };

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let line = stderr_error_line(&stderr);
        return Err(Error::Ytdlp(format!("{}: {line}", out.status)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Whether `error` came from failing to spawn the yt-dlp binary itself
/// (e.g. not found or not executable), so retrying won't help.
pub fn is_start_error(error: &Error) -> bool {
    matches!(error, Error::Ytdlp(message) if message.starts_with("failed to start '"))
}

/// Runs yt-dlp up to `attempts` times, 2s apart. Stops early on cancel or
/// spawn errors.
pub async fn run_with_retry(
    opts: &Options,
    url: &str,
    ignore_no_formats: bool,
    attempts: u32,
    cancel: &CancellationToken,
) -> Result<String> {
    let mut last_err = Error::Ytdlp("not run".into());
    for attempt in 1..=attempts {
        match run(opts, url, ignore_no_formats, cancel).await {
            Ok(out) => return Ok(out),
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(error) if is_start_error(&error) => return Err(error),
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
    is_fragmented(url)
        && format.manifest_url.as_deref().is_none_or(str::is_empty)
        && matches!(format.protocol.as_deref(), Some("http_dash_segments" | "http_dash_segments_generator"))
}

/// Converts raw yt-dlp output into an extraction, without network access.
/// Returns the adaptive format URLs in encounter order for sequence probing.
pub fn to_extraction(raw: YtdlpRawInfo) -> (YtdlpExtraction, Vec<String>) {
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

    let mut probe_urls = Vec::new();
    let mut exact_itags = HashSet::new();
    let mut fallback_target = 0u32;
    for format in raw.formats.unwrap_or_default() {
        let Some(itag) = format.itag() else { continue };
        let duration = format
            .target_duration
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
            .map(|seconds| (seconds.round() as u32).max(1));
        if !is_adaptive_format(&format) {
            if fallback_target == 0
                && let Some(target) = duration
            {
                fallback_target = target;
            }
            continue;
        }
        if info.target_duration == 0
            && let Some(target) = duration
        {
            info.target_duration = target;
        }
        let exact = !format.format_id.contains('-');
        let url = format.url.unwrap_or_default();
        if !probe_urls.contains(&url) {
            probe_urls.push(url.clone());
        }
        if exact {
            exact_itags.insert(itag);
            info.urls.insert(itag, FragmentUrl::with_sq_param(&url));
        } else if !exact_itags.contains(&itag) {
            info.urls.insert(itag, FragmentUrl::with_sq_param(&url));
        }
    }
    if info.target_duration == 0 {
        info.target_duration = fallback_target;
    }
    (info, probe_urls)
}

/// Reads the `X-Head-Seqnum` header (latest available fragment) via HEAD,
/// rotating across candidate adaptive format URLs on retry.
pub async fn probe_x_head_seqnum_candidates(http: &Http, urls: &[String], cancel: &CancellationToken) -> Option<u64> {
    if urls.is_empty() {
        return None;
    }
    for attempt in 1..=PROBE_ATTEMPTS {
        let url = urls.get((attempt as usize - 1) % urls.len())?;
        let referer = FragmentUrl::with_sq_param(url).referer().cloned();
        let mut req = http.client.head(url).header(ORIGIN, "https://www.youtube.com").timeout(PROBE_TIMEOUT);
        if let Some(value) = &referer {
            req = req.header(REFERER, value);
        }
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => return None,
            result = req.send() => result,
        };
        match resp {
            Ok(response) => {
                if let Some(head) = response
                    .headers()
                    .get("x-head-seqnum")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|text| text.trim().parse().ok())
                {
                    return Some(head);
                }
                log_debug!(
                    "X-Head-Seqnum probe returned HTTP {} without header (attempt {attempt}/{PROBE_ATTEMPTS})",
                    response.status()
                );
            }
            Err(error) => {
                log_debug!("X-Head-Seqnum probe failed (attempt {attempt}/{PROBE_ATTEMPTS}): {}", error.without_url());
            }
        }
        if attempt < PROBE_ATTEMPTS && !sleep_or_cancel(cancel, PROBE_RETRY_DELAY).await {
            return None;
        }
    }
    None
}

/// Sequence boundary for downloading. Post-live, the last couple of fragments
/// tend to return 503, so stop one short of the head.
pub fn seq_boundary(head: u64, post_live: bool) -> Option<u64> {
    if post_live { (head > 1).then(|| head - 1) } else { Some(head) }
}

async fn finish_extraction(http: &Http, out: &str, cancel: &CancellationToken) -> Result<YtdlpExtraction> {
    let (mut info, probe_urls) = to_extraction(parse_output(out)?);
    if !probe_urls.is_empty() {
        let post_live = info.live_status == LiveStatus::PostLive;
        info.last_sq = probe_x_head_seqnum_candidates(http, &probe_urls, cancel)
            .await
            .and_then(|head| seq_boundary(head, post_live));
    }
    if !info.urls.is_empty() {
        log_debug!("Loaded {} adaptive format URLs from yt-dlp", info.urls.len());
    }
    Ok(info)
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
    finish_extraction(http, &out, cancel).await
}

/// Extraction without the `X-Head-Seqnum` HEAD probe, for background URL
/// refreshes that only need fresh adaptive format URLs.
pub async fn extract_urls(
    opts: &Options,
    url: &str,
    ignore_no_formats: bool,
    cancel: &CancellationToken,
) -> Result<YtdlpExtraction> {
    let out = run_with_retry(opts, url, ignore_no_formats, DEFAULT_ATTEMPTS, cancel).await?;
    let (info, _) = to_extraction(parse_output(&out)?);
    if !info.urls.is_empty() {
        log_debug!("Loaded {} adaptive format URLs from yt-dlp", info.urls.len());
    }
    Ok(info)
}

/// Single-attempt extraction without retry warnings, used while polling a
/// channel's `/live` URL with `-r`.
pub async fn extract_once(
    http: &Http,
    opts: &Options,
    url: &str,
    ignore_no_formats: bool,
    cancel: &CancellationToken,
) -> Result<YtdlpExtraction> {
    let out = run(opts, url, ignore_no_formats, cancel).await?;
    finish_extraction(http, &out, cancel).await
}

#[cfg(test)]
mod tests;
