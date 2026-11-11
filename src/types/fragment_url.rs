//! A googlevideo URL template with a slot for the fragment sequence number.
//! Go stored these as `Sprintf` templates (`...&sq=%d` with `%` escaped as
//! `%%`); splitting around the slot avoids the escaping entirely.

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
