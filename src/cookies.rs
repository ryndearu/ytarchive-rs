//! Netscape cookies.txt loading.
//! (Go's SAPISIDHASH auth was only used by its unreachable InnerTube
//! request, so it isn't ported.)

use std::path::Path;

use crate::error::Result;
use crate::types::{CookieSet, NetscapeCookie};

const HTTPONLY_PREFIX: &str = "#httponly_";

/// Parses cookies.txt content. Lines without exactly 7 tab-separated fields
/// (comments, blanks) are skipped, as are values containing `"` (Google's
/// JSON cookies, which aren't valid cookie values).
pub fn parse_netscape(text: &str) -> CookieSet {
    let cookies = text
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.trim_end_matches('\r').split('\t').collect();
            let [domain, subdomains, path, secure, expires, name, value] = parts[..] else {
                return None;
            };
            if value.contains('"') {
                return None;
            }

            let mut domain = domain.to_ascii_lowercase();
            let http_only = match domain.strip_prefix(HTTPONLY_PREFIX) {
                Some(rest) => {
                    domain = rest.to_string();
                    true
                }
                None => false,
            };
            if domain.is_empty() || domain.starts_with('#') {
                return None;
            }

            Some(NetscapeCookie {
                domain,
                include_subdomains: subdomains.eq_ignore_ascii_case("true"),
                path: if path.is_empty() { "/".into() } else { path.to_string() },
                secure: secure.eq_ignore_ascii_case("true"),
                expires: expires.parse::<i64>().ok().filter(|e| *e > 0),
                name: name.to_string(),
                value: value.to_string(),
                http_only,
            })
        })
        .collect();
    CookieSet { cookies }
}

pub fn load_file(path: &Path) -> Result<CookieSet> {
    // Lossy so a cookie file with stray non-UTF-8 bytes still loads.
    Ok(parse_netscape(&String::from_utf8_lossy(&std::fs::read(path)?)))
}

/// URL the cookie belongs to, for inserting into a cookie jar.
pub fn cookie_url(c: &NetscapeCookie) -> Option<url::Url> {
    url::Url::parse(&format!("https://{}{}", c.domain.trim_start_matches('.'), c.path)).ok()
}

/// Renders the cookie as a `Set-Cookie` header value.
/// Session cookies (expiry 0) get no `Expires`, unlike Go which set them to
/// 1970 and so silently dropped them.
pub fn to_set_cookie(c: &NetscapeCookie) -> String {
    let mut s = format!("{}={}; Path={}", c.name, c.value, c.path);
    if c.include_subdomains || c.domain.starts_with('.') {
        s.push_str("; Domain=");
        s.push_str(c.domain.trim_start_matches('.'));
    }
    if let Some(dt) = c.expires.and_then(|e| chrono::DateTime::from_timestamp(e, 0)) {
        s.push_str(&dt.format("; Expires=%a, %d %b %Y %H:%M:%S GMT").to_string());
    }
    if c.secure {
        s.push_str("; Secure");
    }
    if c.http_only {
        s.push_str("; HttpOnly");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Netscape HTTP Cookie File\n\
        # comment line\n\
        \n\
        .youtube.com\tTRUE\t/\tTRUE\t1893456000\tSAPISID\tabc/def\n\
        #HttpOnly_.YouTube.com\tTRUE\t/\tTRUE\t0\tLOGIN_INFO\txyz\n\
        www.youtube.com\tFALSE\t/watch\tFALSE\t1893456000\tPREF\tf6=40000000\n\
        .google.com\tTRUE\t/\tTRUE\t1893456000\tJSON\t\"{a}\"\n\
        too\tfew\tfields\n";

    #[test]
    fn parses_entries() {
        let set = parse_netscape(SAMPLE);
        assert_eq!(set.cookies.len(), 3);

        let login = &set.cookies[1];
        assert_eq!(login.domain, ".youtube.com");
        assert!(login.http_only);
        assert_eq!(login.expires, None);

        let pref = &set.cookies[2];
        assert!(!pref.include_subdomains);
        assert!(!pref.secure);
        assert_eq!(pref.path, "/watch");
    }

    #[test]
    fn handles_crlf() {
        let set = parse_netscape(".youtube.com\tTRUE\t/\tTRUE\t0\tA\tb\r\n");
        assert_eq!(set.cookies[0].value, "b");
    }

    #[test]
    fn set_cookie_rendering() {
        let set = parse_netscape(SAMPLE);
        assert_eq!(
            to_set_cookie(&set.cookies[0]),
            "SAPISID=abc/def; Path=/; Domain=youtube.com; Expires=Tue, 01 Jan 2030 00:00:00 GMT; Secure"
        );
        assert_eq!(
            to_set_cookie(&set.cookies[1]),
            "LOGIN_INFO=xyz; Path=/; Domain=youtube.com; Secure; HttpOnly"
        );
        assert_eq!(to_set_cookie(&set.cookies[2]), "PREF=f6=40000000; Path=/watch; Expires=Tue, 01 Jan 2030 00:00:00 GMT");
        assert_eq!(cookie_url(&set.cookies[2]).unwrap().as_str(), "https://www.youtube.com/watch");
    }
}
