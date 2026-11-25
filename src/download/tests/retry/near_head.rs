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
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let head = 5u64;
    let seq4_attempts = Arc::new(AtomicUsize::new(0));
    tokio::spawn({
        let seq4_attempts = seq4_attempts.clone();
        async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let seq4_attempts = seq4_attempts.clone();
                tokio::spawn(async move {
                    let mut buffer = vec![0u8; 4096];
                    let mut count = 0;
                    while !buffer[..count].windows(4).any(|window| window == b"\r\n\r\n") {
                        match sock.read(&mut buffer[count..]).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => count += read,
                        }
                    }
                    let request = String::from_utf8_lossy(&buffer[..count]);
                    let sequence: u64 = request
                        .split_whitespace()
                        .nth(1)
                        .and_then(|path| path.rsplit("sq=").next())
                        .and_then(|text| text.parse().ok())
                        .unwrap_or(u64::MAX);
                    if sequence == 4 && seq4_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                        // Drop connection without responding so the client hits a transport error.
                        return;
                    }
                    let body = fragment_body(sequence);
                    let header = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: video/mp4\r\nx-head-seqnum: {head}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    sock.write_all(header.as_bytes()).await.ok();
                    sock.write_all(&body).await.ok();
                });
            }
        }
    });

    let url = format!("http://127.0.0.1:{}/videoplayback?itag=140&noclen=1", addr.port());
    let dir = temp_dir("near-head-transport");
    let session = Session::for_test(opts(2, false), Some(FragmentUrl::with_sq_param(&url)), false);
    let (mut stream_params, _rx) = params(&dir, head, DownloadState::default());
    stream_params.retry_sleep = Duration::from_millis(1);
    let out = timeout(Duration::from_secs(10), download_stream(session, stream_params))
        .await
        .expect("download hangs")
        .unwrap();

    assert_eq!(out.state.fragments, 5);
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    let want: Vec<u8> = (0..5).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(fs::read(dir.join("out.f140.ts")).unwrap(), want);
    fs::remove_dir_all(&dir).unwrap();
}
