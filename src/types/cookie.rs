//! Cookies loaded from a Netscape-format cookies.txt.

/// One cookies.txt entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetscapeCookie {
    /// Lowercased, without the `#HttpOnly_` prefix; may start with `.`.
    pub domain: String,
    /// `TRUE` in the second column: cookie also applies to subdomains.
    pub include_subdomains: bool,
    pub path: String,
    pub secure: bool,
    /// Unix seconds; `None` for session cookies (expiry `0`).
    pub expires: Option<i64>,
    pub name: String,
    pub value: String,
    pub http_only: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CookieSet {
    pub cookies: Vec<NetscapeCookie>,
}
