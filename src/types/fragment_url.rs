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
        Self {
            prefix: format!("{base}{sep}sq="),
            suffix: String::new(),
        }
    }

    pub fn from_parts(prefix: impl Into<String>, suffix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            suffix: suffix.into(),
        }
    }

    pub fn for_seq(&self, seq: u64) -> String {
        format!("{}{seq}{}", self.prefix, self.suffix)
    }

    /// The URL without any sequence number (useful for HEAD probes).
    pub fn base(&self) -> &str {
        let p = &self.prefix;
        p.strip_suffix("&sq=")
            .or_else(|| p.strip_suffix("?sq="))
            .unwrap_or(p)
    }

    pub fn host(&self) -> Option<String> {
        url::Url::parse(self.base())
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
    }

    /// googlevideo URLs meant for fragmented download carry `noclen`
    /// (whole-file URLs carry `clen` instead).
    pub fn is_fragmented(&self) -> bool {
        is_fragmented(&self.prefix)
    }
}

pub fn is_fragmented(url: &str) -> bool {
    url.to_ascii_lowercase().find("noclen").is_some_and(|i| i > 0)
}

/// Removes every `sq=` query parameter, keeping the rest of the URL as-is.
fn strip_sq_param(url: &str) -> String {
    let Some((base, query)) = url.split_once('?') else {
        return url.to_string();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|p| !p.is_empty() && !p.starts_with("sq="))
        .collect();
    if kept.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", kept.join("&"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sq_param() {
        let u = FragmentUrl::with_sq_param("https://x.googlevideo.com/videoplayback?itag=140&noclen=1&sq=55&a=%2F");
        assert_eq!(
            u.for_seq(7),
            "https://x.googlevideo.com/videoplayback?itag=140&noclen=1&a=%2F&sq=7"
        );
        assert_eq!(
            u.base(),
            "https://x.googlevideo.com/videoplayback?itag=140&noclen=1&a=%2F"
        );
        assert_eq!(u.host().as_deref(), Some("x.googlevideo.com"));
        assert!(u.is_fragmented());
    }

    #[test]
    fn no_query() {
        let u = FragmentUrl::with_sq_param("https://h/p");
        assert_eq!(u.for_seq(1), "https://h/p?sq=1");
        assert_eq!(u.base(), "https://h/p");
    }

    #[test]
    fn parts() {
        let u = FragmentUrl::from_parts("https://h/base/sq/", "/pot/TOKEN");
        assert_eq!(u.for_seq(3), "https://h/base/sq/3/pot/TOKEN");
    }

    #[test]
    fn fragmented() {
        assert!(!is_fragmented("https://h/p?clen=100"));
        assert!(is_fragmented("https://h/p?NOCLEN=1"));
        assert!(!FragmentUrl::with_sq_param("https://h/p?clen=1").is_fragmented());
    }
}
