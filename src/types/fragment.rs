//! Fragment-level data passed between download workers and the writer.

use bytes::Bytes;
use std::path::PathBuf;
use std::time::Duration;

/// Which half of the stream a worker is handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataType {
    Audio,
    Video,
}

impl DataType {
    pub fn as_str(self) -> &'static str {
        match self {
            DataType::Audio => "audio",
            DataType::Video => "video",
        }
    }
}

/// Where a downloaded fragment's bytes are held until written in order.
#[derive(Debug, Clone)]
pub enum FragData {
    Memory(Bytes),
    /// Temp file (`--no-frag-files` not set); deleted after writing.
    File(PathBuf),
}

/// A downloaded fragment ready to be written.
#[derive(Debug, Clone)]
pub struct Fragment {
    pub seq: u64,
    /// Value of the `X-Head-Seqnum` response header (latest available seq).
    pub x_head_seqnum: Option<u64>,
    pub data: FragData,
    pub mime_type: String,
}

/// Sent from a stream writer to the progress reporter after each fragment.
#[derive(Debug, Clone, Copy)]
pub struct ProgressInfo {
    pub data_type: DataType,
    pub byte_count: u64,
    pub max_seq: Option<u64>,
    pub start_frag: u64,
}

/// How the rest of a stream is doing, for judging a fragment that keeps
/// failing.
#[derive(Debug, Clone, Copy, Default)]
pub struct StreamProgress {
    /// Fragments downloaded so far.
    pub downloaded: u64,
    /// No new fragments are started: too much data is waiting in memory
    /// behind one that is missing, or every fragment of the capture has
    /// been handed out.
    pub held: bool,
}

/// Per-worker retry state for a single fragment.
#[derive(Debug, Clone)]
pub struct FragThreadState {
    pub name: String,
    pub data_type: DataType,
    pub seq: u64,
    pub max_seq: Option<u64>,
    pub tries: u32,
    /// Extra rounds of `frag_max_tries` allowed for fragments well behind
    /// the head after the stream ended.
    pub full_retries: u32,
    pub is_403: bool,
    pub sleep: Duration,
    /// Failed attempts in a row where the server was unreachable, failing,
    /// or asked for fewer requests; each one doubles the wait.
    pub backoff_step: u32,
    /// The wait the server asked for (`Retry-After`) on the last attempt.
    pub retry_after: Option<Duration>,
    /// Fragments the stream had downloaded when this one was first tried,
    /// and when its tries last ran out.
    pub downloaded_at_start: u64,
    pub downloaded_at_round: u64,
    /// Rounds of tries used up on a live stream while other fragments kept
    /// downloading.
    pub stuck_rounds: u32,
}

impl FragThreadState {
    pub fn new(name: impl Into<String>, data_type: DataType, sleep: Duration) -> Self {
        Self {
            name: name.into(),
            data_type,
            seq: 0,
            max_seq: None,
            tries: 0,
            full_retries: 3,
            is_403: false,
            sleep,
            backoff_step: 0,
            retry_after: None,
            downloaded_at_start: 0,
            downloaded_at_round: 0,
            stuck_rounds: 0,
        }
    }

    /// Starts on fragment `seq`; `downloaded` is how many fragments the
    /// stream has downloaded so far.
    pub fn reset_for(&mut self, seq: u64, max_seq: Option<u64>, downloaded: u64) {
        self.seq = seq;
        self.max_seq = max_seq;
        self.tries = 0;
        self.full_retries = 3;
        self.is_403 = false;
        self.backoff_step = 0;
        self.retry_after = None;
        self.downloaded_at_start = downloaded;
        self.downloaded_at_round = downloaded;
        self.stuck_rounds = 0;
    }
}

/// A googlevideo URL template with a slot for the fragment sequence number.
/// Go stored these as `Sprintf` templates (`...&sq=%d` with `%` escaped as
/// `%%`); splitting around the slot avoids the escaping entirely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentUrl {
    prefix: String,
    suffix: String,
}

impl FragmentUrl {
    /// URL that takes the sequence as a trailing `&sq=N` query parameter.
    /// Any existing `sq` parameter is removed.
    pub fn with_sq_param(url: &str) -> Self {
        let base = strip_sq_param(url);
        let sep = if base.contains('?') { '&' } else { '?' };
        Self { prefix: format!("{base}{sep}sq="), suffix: String::new() }
    }

    /// The text around the sequence number, for storing the URL.
    pub fn parts(&self) -> (&str, &str) {
        (&self.prefix, &self.suffix)
    }

    /// Rebuilds a URL stored with `parts`.
    pub fn from_parts(prefix: String, suffix: String) -> Self {
        Self { prefix, suffix }
    }

    pub fn for_seq(&self, seq: u64) -> String {
        format!("{}{seq}{}", self.prefix, self.suffix)
    }

    /// The URL without any sequence number (useful for HEAD probes).
    pub fn base(&self) -> &str {
        let part = &self.prefix;
        part.strip_suffix("&sq=").or_else(|| part.strip_suffix("?sq=")).unwrap_or(part)
    }

    pub fn host(&self) -> Option<String> {
        url::Url::parse(self.base()).ok().and_then(|url| url.host_str().map(str::to_string))
    }

    /// googlevideo URLs meant for fragmented download carry `noclen`
    /// (whole-file URLs carry `clen` instead).
    pub fn is_fragmented(&self) -> bool {
        is_fragmented(&self.prefix)
    }

    /// When the URL stops working (its `expire` parameter, Unix seconds).
    pub fn expires_at(&self) -> Option<u64> {
        let url = url::Url::parse(self.base()).ok()?;
        let (_, value) = url.query_pairs().find(|(key, _)| key == "expire")?;
        value.parse().ok()
    }

    /// Whether the URL has expired by `now` (Unix seconds). URLs without
    /// an `expire` parameter never do.
    pub fn is_expired(&self, now: u64) -> bool {
        self.expires_at().is_some_and(|end| end <= now)
    }
}

pub fn is_fragmented(url: &str) -> bool {
    url.to_ascii_lowercase().find("noclen").is_some_and(|index| index > 0)
}

/// Removes every `sq=` query parameter, keeping the rest of the URL as-is.
fn strip_sq_param(url: &str) -> String {
    let Some((base, query)) = url.split_once('?') else {
        return url.to_string();
    };
    let kept: Vec<&str> = query.split('&').filter(|part| !part.is_empty() && !part.starts_with("sq=")).collect();
    if kept.is_empty() { base.to_string() } else { format!("{base}?{}", kept.join("&")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sq_param() {
        let url = FragmentUrl::with_sq_param("https://x.googlevideo.com/videoplayback?itag=140&noclen=1&sq=55&a=%2F");
        assert_eq!(url.for_seq(7), "https://x.googlevideo.com/videoplayback?itag=140&noclen=1&a=%2F&sq=7");
        assert_eq!(url.base(), "https://x.googlevideo.com/videoplayback?itag=140&noclen=1&a=%2F");
        assert_eq!(url.host().as_deref(), Some("x.googlevideo.com"));
        assert!(url.is_fragmented());
    }

    #[test]
    fn expiry() {
        let url = FragmentUrl::with_sq_param("https://h/videoplayback?expire=1700000000&itag=140");
        assert_eq!(url.expires_at(), Some(1_700_000_000));
        assert!(!url.is_expired(1_699_999_999));
        assert!(url.is_expired(1_700_000_000));
        let never = FragmentUrl::with_sq_param("https://h/videoplayback?itag=140");
        assert_eq!(never.expires_at(), None);
        assert!(!never.is_expired(u64::MAX));
    }
}
