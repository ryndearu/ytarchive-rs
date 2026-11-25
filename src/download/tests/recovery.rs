//! Private-stream recovery tests (`--recover-private`).

use std::fs;
use std::sync::Mutex;
use std::time::Instant;

use super::sqlite::sqlite_opts;
use super::*;
use crate::types::SavedDownload;

/// `--recover-private` (which turns on SQLite).
fn recovery_opts(threads: usize) -> Options {
    Options { recover_private: true, ..sqlite_opts(threads) }
}

/// A session whose URL stops working at `private_from` while the stream goes
/// private (as a refresh finding it private would mark it). With `older`, a
/// URL from an earlier refresh that still serves everything is remembered.
async fn going_private(options: Options, head: u64, private_from: u64, older: bool) -> (Arc<Session>, String) {
    let (url, cell) = forbidden_from(head, private_from, Session::set_unavailable).await;
    let session = session_for(options, &url, false);
    if older {
        session.remember(DataType::Audio, &FragmentUrl::with_sq_param(&serve(head, vec![]).await));
    }
    cell.set(session.clone()).ok().unwrap();
    (session, url)
}

#[tokio::test]
async fn recovers_fragments_after_the_stream_goes_private() {
    let (session, _) = going_private(recovery_opts(2), 12, 5, true).await;
    let dir = temp_dir("recover");
    let (stream_params, _rx) = params(&dir, 12, DownloadState::default());
    let out = download_stream(session, stream_params).await.unwrap();
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    assert_eq!(out.retry_until, None);
    assert_eq!(out.state.fragments, 12);
    out.pending_export.unwrap().export().await.unwrap();
    assert_output(&dir.join("out.f140.ts"), 0..12);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn recovery_gives_up_and_a_later_run_retries() {
    let dir = temp_dir("recover-later");
    let saved = Arc::new(SavedDownload {
        video_id: "abc".into(),
        label: "audio_only".into(),
        target_duration: 1,
        ..Default::default()
    });

    // First run: nothing works after fragment 5, so recovery gives up.
    let (session, url) = going_private(recovery_opts(2), 12, 5, false).await;
    let (mut stream_params, _rx) = params(&dir, 12, DownloadState::default());
    stream_params.saved = Some(saved.clone());
    let out = download_stream(session, stream_params).await.unwrap();
    // 10 and 11 are within two of the head (12), so they don't count.
    assert_eq!(out.skipped, [5, 6, 7, 8, 9]);
    // The fake server's URLs have no expire parameter.
    assert_eq!(out.retry_until, Some(u64::MAX));
    out.pending_export.unwrap().export().await.unwrap();
    assert_output(&dir.join("out.f140.ts"), 0..5);

    // The database has what a later run needs.
    let kept = read_kept(&dir, 140).unwrap();
    assert_eq!(kept.info, *saved);
    assert_eq!(kept.head, Some(12));
    assert_eq!(kept.urls, [FragmentUrl::with_sq_param(&url)]);

    // Later run, the stream still can't be looked up: straight to recovery,
    // where a working URL now fetches only the missing fragments.
    let working = serve(12, vec![]).await;
    let session = session_for(recovery_opts(2), &working, false);
    session.set_unavailable();
    let state = load_state(&dir.join("out.f140.state")).unwrap();
    let (stream_params, mut receiver) = params(&dir, 12, state);
    let out = download_stream(session, stream_params).await.unwrap();
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    // 0..=4 were stored; 5..=11 are fetched again, nothing else.
    assert_eq!(progress_counts(&mut receiver), [5, 1, 1, 1, 1, 1, 1, 1]);
    out.pending_export.unwrap().export().await.unwrap();
    assert_output(&dir.join("out.f140.ts"), 0..12);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn recovery_skips_fragments_stored_in_earlier_round() {
    let dir = temp_dir("recover-stored-set");
    stored_database(&dir.join("out.f140.sqlite"), [0, 1, 2, 3, 4, 7, 8, 9]).await;

    let seen = Arc::new(Mutex::new(Vec::new()));
    let hook: Arc<OnceLock<Arc<Session>>> = Arc::default();
    let working = serve_observed(
        10,
        vec![],
        u64::MAX,
        None,
        Some(Arc::new({
            let seen = Arc::clone(&seen);
            move |seq| seen.lock().unwrap().push(seq)
        })),
    )
    .await;
    // Round 1 recovers 5 and fails 6, registering `working` for round 2.
    let partial = serve_observed(
        10,
        vec![6],
        u64::MAX,
        None,
        Some(Arc::new({
            let seen = Arc::clone(&seen);
            let hook = Arc::clone(&hook);
            let working_url = FragmentUrl::with_sq_param(&working);
            move |seq| {
                seen.lock().unwrap().push(seq);
                if seq == 6
                    && let Some(session) = hook.get()
                {
                    session.remember(DataType::Audio, &working_url);
                }
            }
        })),
    )
    .await;

    let session = session_for(recovery_opts(1), &partial, false);
    session.set_unavailable();
    hook.set(session.clone()).ok().unwrap();
    let state = DownloadState { start_frag: 0, fragments: 8, ..Default::default() };
    let (stream_params, _rx) = params(&dir, 10, state);
    let out = download_stream(session, stream_params).await.unwrap();
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    assert_eq!(out.state.fragments, 10);

    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.iter().filter(|&&seq| seq == 5).count(), 1, "{requests:?}");
    assert!(requests.iter().filter(|&&seq| seq == 6).count() >= 2, "{requests:?}");

    out.pending_export.unwrap().export().await.unwrap();
    assert_output(&dir.join("out.f140.ts"), 0..10);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn a_killed_download_keeps_the_head() {
    // A kill (taskkill /F, a closed console) skips the end-of-download save,
    // so the head must already be in the database.
    let url = serve(200, vec![]).await;
    let dir = temp_dir("recover-killed");
    let (mut stream_params, mut receiver) = params(&dir, 200, DownloadState::default());
    stream_params.saved = Some(Arc::new(SavedDownload { video_id: "abc".into(), ..Default::default() }));
    let task = tokio::spawn(download_stream(session_for(recovery_opts(1), &url, false), stream_params));
    for _ in 0..3 {
        receiver.recv().await.unwrap();
    }
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled(), "the download ended before the kill");
    let kept = read_kept(&dir, 140).unwrap();
    assert_eq!(kept.head, Some(200));
    // A blocking write that outlives the abort can still be adding files here.
    fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn no_recovery_while_the_stream_is_available() {
    // A 403 on a public stream ends the download as before (no rounds).
    let url = serve_until(12, vec![], 5, None).await;
    let dir = temp_dir("recover-none");
    let (stream_params, _rx) = params(&dir, 12, DownloadState::default());
    let started = Instant::now();
    let out = download_stream(session_for(recovery_opts(1), &url, false), stream_params).await.unwrap();
    assert_eq!(out.skipped, [5, 6, 7, 8, 9]);
    assert!(started.elapsed() < Duration::from_secs(5));
    drop(out);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn no_recovery_without_the_flag() {
    // Plain --sqlite: a private stream ends the download as in v0.1.0, even
    // though a working URL is known, and nothing is kept for a later run.
    let (session, _) = going_private(sqlite_opts(1), 12, 5, true).await;
    let dir = temp_dir("recover-off");
    let (stream_params, _rx) = params(&dir, 12, DownloadState::default());
    let out = download_stream(session, stream_params).await.unwrap();
    assert_eq!(out.skipped, [5]);
    assert_eq!(out.retry_until, None);
    out.pending_export.unwrap().export().await.unwrap();
    assert_output(&dir.join("out.f140.ts"), 0..5);
    assert!(read_kept(&dir, 140).is_none());
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn unlimited_retries_still_enter_private_recovery_on_403() {
    let options = Options { frag_max_tries: 0, ..recovery_opts(2) };
    let (session, _) = going_private(options, 8, 3, true).await;
    let dir = temp_dir("recover-retry-zero");
    let out = download_quickly(session, &dir, 8).await;
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    assert_eq!(out.state.fragments, 8);
    drop(out);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn recovery_storage_error_stops_session() {
    let dir = temp_dir("recover-storage-err");
    let database_path = dir.join("out.f140.sqlite");
    stored_database(&database_path, 0..3).await;
    fail_inserts(&database_path, "");

    let url = serve(8, vec![]).await;
    let session = session_for(recovery_opts(2), &url, false);
    session.set_unavailable();
    let state = DownloadState { start_frag: 0, fragments: 3, ..Default::default() };
    let (stream_params, _rx) = params(&dir, 8, state);
    let err = download_stream(session.clone(), stream_params).await.unwrap_err();
    assert!(matches!(err, Error::Sqlite(_)), "{err}");
    assert!(session.is_stopping(), "recovery storage error must stop the sibling stream");
    fs::remove_dir_all(&dir).unwrap();
}
