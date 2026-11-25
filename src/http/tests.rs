use super::*;
use crate::cookies::parse_netscape;
use crate::test_http;

/// Replies with `status` and echoes the request's Cookie header.
async fn serve_once(status: &'static str) -> String {
    let addr = test_http::serve(move |request| {
        let cookie = request
            .lines()
            .find_map(|line| line.strip_prefix("cookie: ").or_else(|| line.strip_prefix("Cookie: ")))
            .unwrap_or("");
        let response = test_http::response(status, "", cookie.as_bytes());
        async move { Some(response) }
    })
    .await;
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
        "--password",
        "topsecret",
        "--extractor-args",
        "youtube:po_token=web.gvs+SECRET_A,web.player+SECRET_B;player_client=web",
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
            "--password",
            "<redacted>",
            "--extractor-args",
            "youtube:po_token=<redacted>;player_client=web",
        ]
    );
}
