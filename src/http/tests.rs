use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;
use crate::cookies::parse_netscape;

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

#[test]
fn redacts_urls_and_sensitive_ytdlp_args() {
    assert_eq!(
        redact_url_for_log("https://rr1---sn-x.googlevideo.com/videoplayback?itag=140&ip=1.2.3.4&sig=SECRET"),
        "https://rr1---sn-x.googlevideo.com/videoplayback"
    );
    assert_eq!(
        redact_url_for_log("https://www.youtube.com/watch?v=abc123&si=tracking"),
        "https://www.youtube.com/watch?v=abc123"
    );
    let args: Vec<String> = [
        "--proxy",
        "user:pass@proxy.local:1080",
        "--proxy=socks5h://u:pw@127.0.0.1:9050",
        "--geo-verification-proxy",
        "user:pass@geo.local:8080",
        "--geo-verification-proxy=http://u:pw@geo.local:8080",
        "--password",
        "topsecret",
        "--video-password=vidpass",
        "-pSECRET",
        "-2",
        "123456",
        "-2CODE",
        "--client-certificate-password",
        "certpass",
        "--add-header",
        "Authorization:Bearer secret-token",
        "--add-headers=Cookie:SID=abc",
        "--add-header",
        "Accept-Language:en-US",
        "--extractor-args",
        "youtube:po_token=web.gvs+SECRET_A,web.player+SECRET_B;visitor_data=CgtSECRET;player_client=web",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let redacted = redact_proxy_args(&args);
    assert_eq!(
        redacted,
        [
            "--proxy",
            "proxy.local:1080",
            "--proxy=socks5h://127.0.0.1:9050",
            "--geo-verification-proxy",
            "geo.local:8080",
            "--geo-verification-proxy=http://geo.local:8080/",
            "--password",
            "<redacted>",
            "--video-password=<redacted>",
            "-p<redacted>",
            "-2",
            "<redacted>",
            "-2<redacted>",
            "--client-certificate-password",
            "<redacted>",
            "--add-header",
            "Authorization:<redacted>",
            "--add-headers=Cookie:<redacted>",
            "--add-header",
            "Accept-Language:en-US",
            "--extractor-args",
            "youtube:po_token=<redacted>;visitor_data=<redacted>;player_client=web",
        ]
    );
}
