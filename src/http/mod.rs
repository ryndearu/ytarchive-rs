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

/// Redacts credentials and sensitive query parameters from a URL before
/// logging it. Watch URLs keep their `v` video ID; all other query strings
/// (including googlevideo signatures and IPs) are dropped.
pub fn redact_url_for_log(raw: &str) -> String {
    let Ok(mut url) = Url::parse(raw) else {
        return redact_query(raw);
    };
    url = redact_proxy(&url);
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    let is_watch = matches!(host.as_str(), "youtube.com" | "www.youtube.com" | "m.youtube.com")
        && url.path().to_ascii_lowercase().starts_with("/watch");
    if is_watch {
        let video_id = url.query_pairs().find(|(key, _)| key == "v").map(|(_, value)| value.into_owned());
        url.set_query(None);
        if let Some(id) = video_id {
            url.query_pairs_mut().append_pair("v", &id);
        }
    } else {
        url.set_query(None);
    }
    url.to_string()
}

fn redact_proxy_str(raw: &str) -> String {
    if raw.contains("://")
        && let Ok(url) = Url::parse(raw)
    {
        return redact_proxy(&url).to_string();
    }
    let (authority, rest) = match raw.find('/') {
        Some(pos) => raw.split_at(pos),
        None => (raw, ""),
    };
    match authority.rsplit_once('@') {
        Some((_, host)) => format!("{host}{rest}"),
        None => raw.to_string(),
    }
}

fn redact_extractor_args(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(pos) = rest.find("po_token=") {
        let (before, after_pos) = rest.split_at(pos);
        out.push_str(before);
        out.push_str("po_token=<redacted>");
        let token_start = after_pos.strip_prefix("po_token=").unwrap_or("");
        match token_start.find(';') {
            Some(end) => rest = token_start.split_at(end).1,
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Returns `args` with proxy credentials, passwords, and `po_token` values
/// removed, for `--debug`/`--trace` logging.
pub fn redact_proxy_args(args: &[String]) -> Vec<String> {
    enum Expect {
        None,
        Proxy,
        Secret,
        ExtractorArgs,
    }
    let mut out = Vec::with_capacity(args.len());
    let mut expect = Expect::None;
    for arg in args {
        match expect {
            Expect::Proxy => {
                expect = Expect::None;
                out.push(redact_proxy_str(arg));
                continue;
            }
            Expect::Secret => {
                expect = Expect::None;
                out.push("<redacted>".into());
                continue;
            }
            Expect::ExtractorArgs => {
                expect = Expect::None;
                out.push(redact_extractor_args(arg));
                continue;
            }
            Expect::None => {}
        }
        if matches!(arg.as_str(), "--proxy" | "--geo-verification-proxy") {
            expect = Expect::Proxy;
            out.push(arg.clone());
        } else if let Some(raw) = arg.strip_prefix("--proxy=") {
            out.push(format!("--proxy={}", redact_proxy_str(raw)));
        } else if let Some(raw) = arg.strip_prefix("--geo-verification-proxy=") {
            out.push(format!("--geo-verification-proxy={}", redact_proxy_str(raw)));
        } else if matches!(arg.as_str(), "--password" | "-p" | "--ap-password") {
            expect = Expect::Secret;
            out.push(arg.clone());
        } else if arg.starts_with("--password=") {
            out.push("--password=<redacted>".into());
        } else if arg.starts_with("--ap-password=") {
            out.push("--ap-password=<redacted>".into());
        } else if arg.starts_with("-p") && arg.len() > 2 {
            out.push("-p<redacted>".into());
        } else if arg == "--extractor-args" {
            expect = Expect::ExtractorArgs;
            out.push(arg.clone());
        } else if let Some(raw) = arg.strip_prefix("--extractor-args=") {
            out.push(format!("--extractor-args={}", redact_extractor_args(raw)));
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
mod tests;
