use super::*;

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
    let out = download_quickly(session, &dir, 5).await;
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
    let head = 10u64;
    let seq2_attempts = Arc::new(AtomicUsize::new(0));
    let seen_404 = Arc::new(tokio::sync::Notify::new());
    let addr = test_http::serve(move |request| {
        let seq = request_sequence(&request);
        let seq2_attempts = seq2_attempts.clone();
        let seen_404 = seen_404.clone();
        async move {
            let (status, body) = if seq == 9 {
                seen_404.notify_waiters();
                ("404 Not Found", Vec::new())
            } else if seq == 2 && seq2_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                // Hold until the other worker hits the near-head 404 on seq 9.
                timeout(Duration::from_secs(5), seen_404.notified()).await.unwrap();
                ("500 Internal Server Error", Vec::new())
            } else {
                ("200 OK", fragment_body(seq))
            };
            Some(fragment_response(status, head, &body))
        }
    })
    .await;

    let dir = temp_dir("near-head-404");
    let out = download_quickly(session_for(opts(2, false), &fragment_url(addr), false), &dir, head).await;
    assert_complete(&out, &dir, 9);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn live_unavailable_stream_ends_on_near_head_non_403() {
    // A live stream that went private/unavailable while still marked live must
    // still end when fragments near the head fail with a non-403 (e.g. 404).
    let url = serve(5, vec![]).await;
    let dir = temp_dir("live-unavailable-404");
    let session = session_for(opts(2, false), &url, true);
    session.set_unavailable();
    let out = download_quickly(session, &dir, 5).await;
    assert_complete(&out, &dir, 6);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn unlimited_retries_stop_on_403_and_near_head_end_of_stream() {
    let options = Options { frag_max_tries: 0, ..opts(1, false) };
    let session = Session::for_test(options, None, false);
    let mut state = FragThreadState::new("audio1", DataType::Audio, Duration::from_millis(1));
    state.reset_for(9, Some(10), 0);
    state.tries = 10;
    assert_eq!(continue_fragment_download(&session, &mut state, moved(0)), RetryDecision::EndOfStream);

    state.reset_for(2, Some(10), 0);
    state.tries = 10;
    assert_eq!(continue_fragment_download(&session, &mut state, moved(0)), RetryDecision::Retry);

    state.is_403 = true;
    assert_eq!(continue_fragment_download(&session, &mut state, moved(0)), RetryDecision::GiveUp);
    assert!(session.is_finished(DataType::Audio));
}

#[tokio::test]
async fn near_head_transport_error_retries_on_ended_stream() {
    let head = 5u64;
    let seq4_attempts = AtomicUsize::new(0);
    let addr = test_http::serve(move |request| {
        let sequence = request_sequence(&request);
        // The first try of 4 gets no response: a transport error.
        let response = (sequence != 4 || seq4_attempts.fetch_add(1, Ordering::SeqCst) > 0)
            .then(|| fragment_response("200 OK", head, &fragment_body(sequence)));
        async move { response }
    })
    .await;

    let dir = temp_dir("near-head-transport");
    let out = download_quickly(session_for(opts(2, false), &fragment_url(addr), false), &dir, head).await;
    assert_complete(&out, &dir, 5);
    fs::remove_dir_all(&dir).unwrap();
}
