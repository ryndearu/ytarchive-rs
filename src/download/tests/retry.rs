//! Retrying fragments: streams that end or expire mid-download, backing
//! off, and giving up on a fragment that is stuck on a live stream.

use std::sync::OnceLock;

use super::super::fragment::{continue_fragment_download, retry_delay, without_query};
use super::*;
use crate::types::FragThreadState;

#[tokio::test]
async fn a_stream_that_ends_mid_download_is_finished() {
    let url = serve(10, vec![]).await;
    let dir = temp_dir("ends");
    let session = Session::for_test(opts(2, false), Some(FragmentUrl::with_sq_param(&url)), true);
    let (stream_params, _rx) = params(&dir, 10, DownloadState::default());
    // While live the workers keep asking for fragment 11, which never comes.
    let ending = session.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        ending.set_live(false);
    });
    let out = download_stream(session, stream_params).await.unwrap();

    let want: Vec<u8> = (0..=10).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(std::fs::read(dir.join("out.f140.ts")).unwrap(), want);
    assert_eq!(out.state.fragments, 11);
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    std::fs::remove_dir_all(&dir).unwrap();
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
    assert_eq!(std::fs::read(dir.join("out.f140.ts")).unwrap(), want);
    assert_eq!(out.state.fragments, 5);
    assert_eq!(load_state(&dir.join("out.f140.state")).unwrap().fragments, 5);
    std::fs::remove_dir_all(&dir).unwrap();
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
        assert!(continue_fragment_download(&session, &mut state, 4));
        assert_eq!(state.tries, 0);
    }
    // The rest of the stream moves on, round after round.
    for downloaded in [20, 40, 60] {
        exhausted(&mut state);
        assert!(continue_fragment_download(&session, &mut state, downloaded));
    }
    exhausted(&mut state);
    assert!(!continue_fragment_download(&session, &mut state, 80));
    // Only this fragment is dropped; the stream goes on.
    assert!(!session.is_finished(DataType::Audio));
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
