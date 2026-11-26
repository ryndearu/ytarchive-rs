use tokio::net::TcpListener;

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
    assert_eq!(&http.get_bytes_cancellable(&url, &CancellationToken::new()).await.unwrap()[..], b"A=b");
}

#[tokio::test]
async fn non_success_status_is_error_without_query() {
    let url = serve_once("403 Forbidden").await;
    let http = Http::new(&Options::default(), CookieSet::default()).unwrap();
    match http.get_bytes_cancellable(&url, &CancellationToken::new()).await {
        Err(Error::HttpStatus { status: 403, what }) => assert!(!what.contains("secret")),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn default_consent_cookie_only_without_consent_or_sign_in() {
    let cases = [
        ("no cookies", "", true),
        ("unrelated", "PREF\tf6=8", true),
        ("rejected", "SOCS\tCAAaBgiA", true),
        ("consented", "SOCS\tCAESEwgD", false),
        ("signed in", "SAPISID\tx", false),
    ];
    for (name, entry, expected) in cases {
        let file = if entry.is_empty() { String::new() } else { format!(".youtube.com\tTRUE\t/\tTRUE\t0\t{entry}\n") };
        assert_eq!(needs_default_consent(&parse_netscape(file.as_bytes())), expected, "{name}");
    }
}

#[tokio::test]
async fn logged_request_errors_leave_out_the_signature_and_ip() {
    // Nothing listens once the listener is dropped, so the request fails.
    let port = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/videoplayback?ip=203.0.113.9&sig=SECRET&sq=5");
    let error = reqwest::Client::new().get(&url).send().await.unwrap_err();
    assert!(error.to_string().contains("SECRET"));

    let logged = without_query(error).to_string();
    assert!(logged.contains("/videoplayback"), "{logged}");
    assert!(!logged.contains("SECRET") && !logged.contains("203.0.113.9"), "{logged}");
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
