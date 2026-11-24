//! Retrying fragments: streams that end or expire mid-download, backing
//! off, and giving up on a fragment that is stuck on a live stream.

use std::fs;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use tokio::time::{sleep, timeout};

use super::super::fragment::{RetryDecision, continue_fragment_download, retry_delay};
use super::*;
use crate::http::without_query;
use crate::types::{FragThreadState, StreamProgress};

#[tokio::test]
async fn a_stream_that_ends_mid_download_is_finished() {
    let url = serve(10, vec![]).await;
    let dir = temp_dir("ends");
    let session = Session::for_test(opts(2, false), Some(FragmentUrl::with_sq_param(&url)), true);
    let (stream_params, _rx) = params(&dir, 10, DownloadState::default());
    // While live the workers keep asking for fragment 11, which never comes.
    tokio::spawn({
        let session = session.clone();
        async move {
            sleep(Duration::from_millis(150)).await;
            session.set_live(false);
        }
    });
    let out = download_stream(session, stream_params).await.unwrap();

    let want: Vec<u8> = (0..=10).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(fs::read(dir.join("out.f140.ts")).unwrap(), want);
    assert_eq!(out.state.fragments, 11);
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn expired_urls_on_an_ended_stream_keep_the_partial_file() {
    // Fragments from 5 on get 403, and by then the stream has ended: the
    // tries run out and what was downloaded is kept.
    let hook: Arc<OnceLock<Arc<Session>>> = Arc::default();
    let shared_hook = hook.clone();
    let on_forbidden: OnForbidden = Arc::new(move || {
        if let Some(session) = shared_hook.get() {
            session.set_live(false);
        }
    });
    let url = serve_until(12, vec![], 5, Some(on_forbidden)).await;
    let dir = temp_dir("expired");
    let session = Session::for_test(opts(2, false), Some(FragmentUrl::with_sq_param(&url)), true);
    hook.set(session.clone()).ok().unwrap();
    let (stream_params, _rx) = params(&dir, 12, DownloadState::default());
    let out = download_stream(session, stream_params).await.unwrap();

    let want: Vec<u8> = (0..5).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(fs::read(dir.join("out.f140.ts")).unwrap(), want);
    assert_eq!(out.state.fragments, 5);
    assert_eq!(load_state(&dir.join("out.f140.state")).unwrap().fragments, 5);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn a_stream_that_ends_while_its_fragments_fail_keeps_the_partial_file() {
    // Fragments from 5 on get 500. While live that is an outage and they are
    // retried without limit; once the stream has ended the tries run out,
    // and the download finishes normally (exit code 0 after muxing, as in
    // Go) with what it has.
    let url = serve(12, (5..=12).collect()).await;
    let dir = temp_dir("ends-blocked");
    let session = Session::for_test(opts(2, false), Some(FragmentUrl::with_sq_param(&url)), true);
    let (mut stream_params, _rx) = params(&dir, 12, DownloadState::default());
    stream_params.retry_sleep = Duration::from_millis(1);
    tokio::spawn({
        let session = session.clone();
        async move {
            sleep(Duration::from_millis(50)).await;
            session.set_live(false);
        }
    });
    let download = download_stream(session, stream_params);
    let out = timeout(Duration::from_secs(30), download).await.expect("download hangs").unwrap();

    let want: Vec<u8> = (0..5).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(fs::read(dir.join("out.f140.ts")).unwrap(), want);
    assert_eq!(out.state.fragments, 5);
    // 10 and 11 are within two of the head: taken as never created.
    assert_eq!(out.skipped, [5, 6, 7, 8, 9]);
    assert_eq!(load_state(&dir.join("out.f140.state")).unwrap().fragments, 5);
    fs::remove_dir_all(&dir).unwrap();
}

/// A live fragment whose tries (2 in these tests) have just run out.
fn exhausted(state: &mut FragThreadState) {
    state.tries = 2;
}

#[tokio::test]
async fn a_live_fragment_is_given_up_only_while_the_rest_downloads() {
    let session = Session::for_test(opts(2, false), None, true);
    let mut state = FragThreadState::new("audio1", DataType::Audio, Duration::from_millis(10));
    state.reset_for(3, Some(100), 4);

    // Nothing else downloads either (an outage): retried forever, as in Go.
    for _ in 0..20 {
        exhausted(&mut state);
        assert_eq!(continue_fragment_download(&session, &mut state, moved(4)), RetryDecision::Retry);
        assert_eq!(state.tries, 0);
    }
    // The rest of the stream moves on, round after round.
    for downloaded in [20, 40, 60] {
        exhausted(&mut state);
        assert_eq!(continue_fragment_download(&session, &mut state, moved(downloaded)), RetryDecision::Retry);
    }
    exhausted(&mut state);
    assert_eq!(continue_fragment_download(&session, &mut state, moved(80)), RetryDecision::GiveUp);
    // Only this fragment is dropped; the stream goes on.
    assert!(!session.is_finished(DataType::Audio));
}

fn moved(downloaded: u64) -> StreamProgress {
    StreamProgress { downloaded, held: false, max_seq: None }
}

#[tokio::test]
async fn stuck_fragments_do_not_keep_a_capture_from_finishing() {
    // Fragments 50 and 55 never download, and after the capture's last
    // fragment nothing else downloads to count against them.
    let url = serve(200, vec![50, 55]).await;
    let dir = temp_dir("capture-stuck");
    let session = Session::for_test(opts(4, true), Some(FragmentUrl::with_sq_param(&url)), true);
    let (mut stream_params, _rx) = params(&dir, 200, DownloadState::default());
    stream_params.start_sq = Some(0);
    stream_params.capture_duration = Some(Duration::from_secs(60));
    stream_params.retry_sleep = Duration::from_millis(1);
    let download = download_stream(session, stream_params);
    let out = timeout(Duration::from_secs(30), download).await.expect("download hangs").unwrap();

    assert_eq!(out.skipped, [50, 55]);
    assert_eq!(out.state.fragments, 58);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn fragments_waiting_in_memory_are_limited() {
    // Fragment 3 never downloads. The requests for it stop when it is given
    // up on; `ahead` is the newest fragment asked for by then.
    let newest = Arc::new(AtomicU64::new(0));
    let ahead = Arc::new(AtomicU64::new(0));
    let on_request: OnRequest = Arc::new({
        let (newest, ahead) = (newest.clone(), ahead.clone());
        move |sequence| {
            let seen = newest.fetch_max(sequence, Ordering::SeqCst).max(sequence);
            if sequence == 3 {
                ahead.store(seen, Ordering::SeqCst);
            }
        }
    });
    let url = serve_observed(200, vec![3], u64::MAX, None, Some(on_request)).await;
    let dir = temp_dir("memory-limit");
    let session = Session::for_test(opts(4, false), Some(FragmentUrl::with_sq_param(&url)), true);
    let (mut stream_params, _rx) = params(&dir, 200, DownloadState::default());
    stream_params.start_sq = Some(0);
    stream_params.capture_duration = Some(Duration::from_secs(60));
    stream_params.retry_sleep = Duration::from_millis(1);
    stream_params.memory_limit = 6 * fragment_body(4).len() as u64;
    let out = download_stream(session, stream_params).await.unwrap();

    // Six waiting fragments, plus what the workers and the channel to the
    // writer hold. Without the limit all 60 are downloaded long before.
    let ahead = ahead.load(Ordering::SeqCst);
    assert!(ahead <= 3 + 6 + 12, "{ahead}");
    assert_eq!(out.skipped, [3]);
    let want: Vec<u8> =
        (0..60).filter(|sequence| *sequence != 3).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(fs::read(dir.join("out.f140.ts")).unwrap(), want);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn waits_longer_after_repeated_server_failures() {
    let mut state = FragThreadState::new("audio1", DataType::Audio, Duration::from_secs(2));
    assert_eq!(retry_delay(&state), Duration::from_secs(2));
    state.backoff_step = 3;
    assert_eq!(retry_delay(&state), Duration::from_secs(8));
    state.backoff_step = 30;
    assert_eq!(retry_delay(&state), Duration::from_secs(15));
    // Retry-After wins, within limits, and never shortens the usual wait.
    state.retry_after = Some(Duration::from_secs(40));
    assert_eq!(retry_delay(&state), Duration::from_secs(40));
    state.retry_after = Some(Duration::from_secs(3600));
    assert_eq!(retry_delay(&state), Duration::from_secs(60));
    state.retry_after = Some(Duration::ZERO);
    assert_eq!(retry_delay(&state), Duration::from_secs(2));
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

#[tokio::test]
async fn refreshed_url_is_retried_after_403_on_ended_stream() {
    let expired_url = serve_until(5, vec![], 0, None).await;
    let fresh_url = serve(5, vec![]).await;
    let dir = temp_dir("refresh-403-ended");
    let session = Session::for_test_with_refresh(
        opts(1, false),
        Some(FragmentUrl::with_sq_param(&expired_url)),
        false,
        Arc::new({
            let fresh = FragmentUrl::with_sq_param(&fresh_url);
            move || Some(fresh.clone())
        }),
    );
    let (mut stream_params, _rx) = params(&dir, 5, DownloadState::default());
    stream_params.retry_sleep = Duration::from_millis(1);
    let out = download_stream(session, stream_params).await.unwrap();

    let want: Vec<u8> = (0..5).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(fs::read(dir.join("out.f140.ts")).unwrap(), want);
    assert_eq!(out.state.fragments, 5);
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn non_403_near_head_does_not_get_extra_refresh_rounds() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let on_request: OnRequest = Arc::new({
        let attempts = attempts.clone();
        move |sequence| {
            if sequence == 4 {
                attempts.fetch_add(1, Ordering::SeqCst);
            }
        }
    });
    let server_url = serve_status(5, vec![4], "503 Service Unavailable", Some(on_request)).await;
    let dir = temp_dir("refresh-503-ended");
    let refresh_id = AtomicUsize::new(0);
    let session = Session::for_test_with_refresh(
        opts(1, false),
        Some(FragmentUrl::with_sq_param(&server_url)),
        false,
        Arc::new(move || {
            let generation = refresh_id.fetch_add(1, Ordering::SeqCst) + 1;
            Some(FragmentUrl::with_sq_param(&format!("{server_url}&refresh={generation}")))
        }),
    );
    let (mut stream_params, _rx) = params(&dir, 5, DownloadState::default());
    stream_params.retry_sleep = Duration::from_millis(1);
    let out = download_stream(session, stream_params).await.unwrap();

    assert_eq!(out.state.fragments, 4);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn retry_judges_distance_against_current_max_seq() {
    let session = Session::for_test(opts(2, false), None, false);
    let mut state = FragThreadState::new("audio1", DataType::Audio, Duration::from_millis(10));
    // Started when head was 10, but other workers later saw head 50.
    state.reset_for(9, Some(10), 0);
    exhausted(&mut state);
    assert_eq!(
        continue_fragment_download(
            &session,
            &mut state,
            StreamProgress { downloaded: 40, held: false, max_seq: Some(50) }
        ),
        RetryDecision::Retry
    );
    assert_eq!(state.max_seq, Some(50));
    assert!(!session.is_finished(DataType::Audio));
}

#[tokio::test]
async fn near_head_404_does_not_abort_worker_retrying_earlier_fragment() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let head = 10u64;
    let seq2_attempts = Arc::new(AtomicUsize::new(0));
    let seen_404 = Arc::new(tokio::sync::Notify::new());
    tokio::spawn({
        let seq2_attempts = seq2_attempts.clone();
        let seen_404 = seen_404.clone();
        async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let seq2_attempts = seq2_attempts.clone();
                let seen_404 = seen_404.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let mut count = 0;
                    while !buf[..count].windows(4).any(|window| window == b"\r\n\r\n") {
                        match sock.read(&mut buf[count..]).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => count += read,
                        }
                    }
                    let req = String::from_utf8_lossy(&buf[..count]);
                    let seq: u64 = req
                        .split_whitespace()
                        .nth(1)
                        .and_then(|path| path.rsplit("sq=").next())
                        .and_then(|text| text.parse().ok())
                        .unwrap_or(u64::MAX);
                    let (status, body) = if seq == 9 {
                        seen_404.notify_waiters();
                        ("404 Not Found", Vec::new())
                    } else if seq == 2 {
                        let attempt = seq2_attempts.fetch_add(1, Ordering::SeqCst);
                        if attempt == 0 {
                            // Hold until the other worker hits the near-head 404 on seq 9.
                            timeout(Duration::from_secs(5), seen_404.notified()).await.unwrap();
                            ("500 Internal Server Error", Vec::new())
                        } else {
                            ("200 OK", fragment_body(seq))
                        }
                    } else {
                        ("200 OK", fragment_body(seq))
                    };
                    let hdr = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: video/mp4\r\nx-head-seqnum: {head}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    sock.write_all(hdr.as_bytes()).await.ok();
                    sock.write_all(&body).await.ok();
                });
            }
        }
    });

    let url = format!("http://127.0.0.1:{}/videoplayback?itag=140&noclen=1", addr.port());
    let dir = temp_dir("near-head-404");
    let session = Session::for_test(opts(2, false), Some(FragmentUrl::with_sq_param(&url)), false);
    let (mut stream_params, _rx) = params(&dir, head, DownloadState::default());
    stream_params.retry_sleep = Duration::from_millis(1);
    let out = timeout(Duration::from_secs(10), download_stream(session, stream_params))
        .await
        .expect("download hangs")
        .unwrap();

    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    assert_eq!(out.state.fragments, 9);
    let want: Vec<u8> = (0..9).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(fs::read(dir.join("out.f140.ts")).unwrap(), want);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn live_unavailable_stream_ends_on_near_head_non_403() {
    // A live stream that went private/unavailable while still marked live must
    // still end when fragments near the head fail with a non-403 (e.g. 404).
    let url = serve(5, vec![]).await;
    let dir = temp_dir("live-unavailable-404");
    let session = Session::for_test(opts(2, false), Some(FragmentUrl::with_sq_param(&url)), true);
    session.set_unavailable();
    let (mut stream_params, _rx) = params(&dir, 5, DownloadState::default());
    stream_params.retry_sleep = Duration::from_millis(1);
    let out = timeout(Duration::from_secs(10), download_stream(session, stream_params))
        .await
        .expect("download hangs")
        .unwrap();

    assert_eq!(out.state.fragments, 6);
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    let want: Vec<u8> = (0..=5).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(fs::read(dir.join("out.f140.ts")).unwrap(), want);
    fs::remove_dir_all(&dir).unwrap();
}
