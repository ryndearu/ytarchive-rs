//! Netscape cookies.txt loading.
//! (Go's SAPISIDHASH auth was only used by its unreachable InnerTube
//! request, so it isn't ported.)

use crate::log_warn;
use crate::types::{CookieSet, NetscapeCookie};

const HTTPONLY_PREFIX: &str = "#httponly_";

/// Parses cookies.txt content as Go did. Lines without exactly 7
/// tab-separated fields (comments, blanks) are skipped, as are values
/// containing `"` (Google's JSON cookies, which aren't valid cookie values).
/// The include-subdomains column is ignored, as in Go: every cookie also
/// applies to subdomains.
pub fn parse_netscape(data: &[u8]) -> CookieSet {
    let cookies = data
        .split(|&byte| byte == b'\n')
        .filter_map(|line| {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let parts: Vec<&[u8]> = line.split(|&byte| byte == b'\t').collect();
            let [domain, _subdomains, path, secure, expires, name, value] = parts[..] else {
                return None;
            };
            if value.contains(&b'"') {
                return None;
            }
            let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();

            let mut domain = text(domain).to_lowercase();
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
            let name = text(name);
            let path = text(path);

            Some(NetscapeCookie {
                domain,
                path: if path.is_empty() { "/".into() } else { path },
                secure: secure.eq_ignore_ascii_case(b"true"),
                expires: text(expires).parse::<i64>().ok().filter(|expiry| *expiry > 0),
                value: sanitize_value(&name, value),
                name,
                http_only,
            })
        })
        .collect();
    CookieSet { cookies }
}

/// Go's `net/http` cookie value rules: bytes outside printable ASCII and
/// `"`, `;`, `\` are dropped with a warning, and a value with a space or
/// comma is sent in double quotes.
fn sanitize_value(name: &str, value: &[u8]) -> String {
    let valid = |byte: u8| (0x20..0x7f).contains(&byte) && !matches!(byte, b'"' | b';' | b'\\');
    if let Some(&bad) = value.iter().find(|&&byte| !valid(byte)) {
        log_warn!("Cookie {name}: invalid byte {bad:#04x} in its value; dropping invalid bytes");
    }
    let clean: String = value.iter().filter(|&&byte| valid(byte)).map(|&byte| byte as char).collect();
    if clean.contains([' ', ',']) { format!("\"{clean}\"") } else { clean }
}

/// URL the cookie belongs to, for inserting into a cookie jar.
pub fn cookie_url(cookie: &NetscapeCookie) -> Option<url::Url> {
    url::Url::parse(&format!("https://{}{}", cookie.domain.trim_start_matches('.'), cookie.path)).ok()
}

/// Renders the cookie as a `Set-Cookie` header value.
/// Session cookies (expiry 0) get no `Expires`, unlike Go which set them to
/// 1970 and so silently dropped them.
pub fn to_set_cookie(cookie: &NetscapeCookie) -> String {
    let mut text = format!("{}={}; Path={}", cookie.name, cookie.value, cookie.path);
    // Go always set the domain, which makes it a domain cookie unless the
    // domain is an IP address.
    if cookie.domain.parse::<std::net::IpAddr>().is_err() {
        text.push_str("; Domain=");
        text.push_str(cookie.domain.trim_start_matches('.'));
    }
    if let Some(date_time) = cookie.expires.and_then(|expiry| chrono::DateTime::from_timestamp(expiry, 0)) {
        text.push_str(&date_time.format("; Expires=%a, %d %b %Y %H:%M:%S GMT").to_string());
    }
    if cookie.secure {
        text.push_str("; Secure");
    }
    if cookie.http_only {
        text.push_str("; HttpOnly");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = b"# Netscape HTTP Cookie File\n\
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
        assert!(!pref.secure);
        assert_eq!(pref.path, "/watch");
    }

    #[test]
    fn handles_crlf_and_sanitizes_values_as_go() {
        let set = parse_netscape(b".youtube.com\tTRUE\t/\tTRUE\t0\tA\tb\xff\xc3\xa9;c\r\n");
        assert_eq!(set.cookies[0].value, "bc");
        let set = parse_netscape(b".youtube.com\tTRUE\t/\tTRUE\t0\tA\tb c,d\n");
        assert_eq!(set.cookies[0].value, "\"b c,d\"");
    }

    #[test]
    fn jar_sends_to_subdomains_with_quotes_as_go() {
        use reqwest::cookie::{CookieStore, Jar};
        let jar = Jar::default();
        for cookie in parse_netscape(b"youtube.com\tFALSE\t/\tTRUE\t0\tA\tb c\n").cookies {
            jar.add_cookie_str(&to_set_cookie(&cookie), &cookie_url(&cookie).unwrap());
        }
        let sent = jar.cookies(&"https://www.youtube.com/watch".parse().unwrap());
        assert_eq!(sent.unwrap().to_str().unwrap(), "A=\"b c\"");
    }
}
