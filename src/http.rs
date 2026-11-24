//! Shared HTTP client: proxy, forced IP version, timeouts, and cookies.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use reqwest::cookie::Jar;
use reqwest::{Client, Proxy, Url};
use tokio_util::sync::CancellationToken;

use crate::cookies;
use crate::error::{Error, Result};
use crate::types::{CookieSet, IpVersion, Options};
use crate::util::write_file_with_mode;
use crate::{log_debug, log_trace};

pub const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:152.0) Gecko/20100101 Firefox/152.0";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Max idle time between body reads; catches stalled fragment downloads.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(45);
const TCP_KEEPALIVE: Duration = Duration::from_secs(30);
/// YouTube sends ~20 KB of response headers to Firefox user agents, above
/// hyper's 16 KB HTTP/2 default, which fails with PROTOCOL_ERROR.
const HTTP2_MAX_HEADER_LIST_SIZE: u32 = 1 << 20;

#[derive(Debug, Clone)]
pub struct Http {
    pub client: Client,
}

impl Http {
    pub fn new(opts: &Options, cookies: CookieSet) -> Result<Self> {
        // Errors only if a provider is already installed, which is fine.
        if rustls::crypto::ring::default_provider().install_default().is_err() {
            log_trace!("A TLS crypto provider is already installed");
        }

        let jar = Arc::new(Jar::default());
        for cookie in &cookies.cookies {
            if let Some(url) = cookies::cookie_url(cookie) {
                jar.add_cookie_str(&cookies::to_set_cookie(cookie), &url);
            }
        }

        let mut builder = Client::builder()
            .user_agent(USER_AGENT)
            .cookie_provider(jar)
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .pool_idle_timeout(POOL_IDLE_TIMEOUT)
            .tcp_keepalive(TCP_KEEPALIVE)
            .http2_max_header_list_size(HTTP2_MAX_HEADER_LIST_SIZE);

        // Binding to the unspecified address of a family forces that family.
        builder = match opts.ip_version {
            IpVersion::Any => builder,
            IpVersion::V4 => builder.local_address(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            IpVersion::V6 => builder.local_address(IpAddr::V6(Ipv6Addr::UNSPECIFIED)),
        };

        // Without --proxy, reqwest falls back to the environment's proxy settings, as Go did.
        if let Some(proxy) = &opts.proxy {
            builder = builder.proxy(Proxy::all(proxy.as_str())?);
        }

        Ok(Self { client: builder.build()? })
    }

    /// GETs `url` and returns the body; non-2xx statuses are errors.
    #[cfg(test)]
    pub async fn get_bytes(&self, url: &str) -> Result<Bytes> {
        self.get_bytes_cancellable(url, &CancellationToken::new()).await
    }

    /// GETs `url` and returns the body, stopping immediately if `cancel` triggers.
    pub async fn get_bytes_cancellable(&self, url: &str, cancel: &CancellationToken) -> Result<Bytes> {
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            result = self.client.get(url).send() => result.map_err(without_query)?,
        };
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::HttpStatus { status: status.as_u16(), what: redact_query(url) });
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(Error::Cancelled),
            body = resp.bytes() => Ok(body.map_err(without_query)?),
        }
    }

    /// Downloads `url` to `path` with the given mode (used for thumbnails),
    /// stopping immediately if `cancel` triggers.
    pub async fn download_to_file_cancellable(
        &self,
        url: &str,
        path: &Path,
        mode: u32,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let data = self.get_bytes_cancellable(url, cancel).await?;
        log_debug!("Writing {} bytes to {}", data.len(), path.display());
        write_file_with_mode(path, &data, mode).await
    }
}

/// Drops the query string from a request error, which for googlevideo URLs
/// holds the signature and the user's IP address.
pub fn without_query(mut error: reqwest::Error) -> reqwest::Error {
    if let Some(url) = error.url_mut() {
        url.set_query(None);
    }
    error
}

/// Clears embedded credentials from a proxy URL before logging it.
pub fn redact_proxy(url: &Url) -> Url {
    let mut redacted = url.clone();
    if !redacted.username().is_empty() && redacted.set_username("").is_err() {
        log_trace!("Could not redact username from proxy URL");
    }
    if redacted.password().is_some() && redacted.set_password(None).is_err() {
        log_trace!("Could not redact password from proxy URL");
    }
    redacted
}

/// Returns `args` with credentials removed from any `--proxy <URL>` or
/// `--proxy=<URL>` arguments, for `--debug`/`--trace` logging.
pub fn redact_proxy_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut expect_proxy = false;
    for arg in args {
        if expect_proxy {
            expect_proxy = false;
            out.push(Url::parse(arg).map(|url| redact_proxy(&url).to_string()).unwrap_or_else(|_| arg.clone()));
            continue;
        }
        if arg == "--proxy" {
            expect_proxy = true;
            out.push(arg.clone());
        } else if let Some(raw) = arg.strip_prefix("--proxy=") {
            let safe =
                Url::parse(raw).map(|url| format!("--proxy={}", redact_proxy(&url))).unwrap_or_else(|_| arg.clone());
            out.push(safe);
        } else {
            out.push(arg.clone());
        }
    }
    out
}

/// Drops the query string, which for googlevideo URLs carries signatures and IPs.
fn redact_query(url: &str) -> String {
    url.split_once('?').map_or(url, |(base, _)| base).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cookies::parse_netscape;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// One-shot server: replies with `status` and echoes the request's Cookie header.
    async fn serve_once(status: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let count = sock.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..count]).to_string();
            let cookie = req
                .lines()
                .find_map(|line| line.strip_prefix("cookie: ").or_else(|| line.strip_prefix("Cookie: ")))
                .unwrap_or("")
                .to_string();
            let resp =
                format!("HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{cookie}", cookie.len());
            sock.write_all(resp.as_bytes()).await.unwrap();
        });
        format!("http://{addr}/x?sig=secret")
    }

    #[tokio::test]
    async fn sends_cookies_and_reads_body() {
        let url = serve_once("200 OK").await;
        let cookies = parse_netscape(b"127.0.0.1\tFALSE\t/\tFALSE\t0\tA\tb\n");
        let http = Http::new(&Options::default(), cookies).unwrap();
        assert_eq!(&http.get_bytes(&url).await.unwrap()[..], b"A=b");
    }

    #[tokio::test]
    async fn non_success_status_is_error_without_query() {
        let url = serve_once("403 Forbidden").await;
        let http = Http::new(&Options::default(), CookieSet::default()).unwrap();
        match http.get_bytes(&url).await {
            Err(Error::HttpStatus { status: 403, what }) => assert!(!what.contains("secret")),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn builds_with_proxy_and_ip_version() {
        let opts = Options {
            proxy: Some(reqwest::Url::parse("socks5://127.0.0.1:1080").unwrap()),
            ip_version: IpVersion::V4,
            ..Default::default()
        };
        assert!(Http::new(&opts, CookieSet::default()).is_ok());
    }
}
