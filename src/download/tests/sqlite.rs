//! SQLite storage and private-stream recovery tests.

use std::sync::OnceLock;

use super::*;

fn sqlite_opts(threads: usize) -> Options {
    Options { storage: crate::types::StorageMode::Sqlite, ..opts(threads, true) }
}

/// `--recover-private` (which turns on SQLite).
fn recovery_opts(threads: usize) -> Options {
    Options { recover_private: true, ..sqlite_opts(threads) }
}

#[tokio::test]
async fn sqlite_downloads_and_exports_in_order() {
    let url = serve(20, vec![]).await;
    let dir = temp_dir("sqlite");
    let s = Session::for_test(sqlite_opts(4), Some(FragmentUrl::with_sq_param(&url)), false);
    let (p, _rx) = params(&dir, 20, DownloadState::default());
    let out = download_stream(s, p).await.unwrap();
    let want: Vec<u8> = (0..20).flat_map(|i| expected(i, i == 0)).collect();
    assert_eq!(std::fs::read(dir.join("out.f140.ts")).unwrap(), want);
    assert_eq!(out.state.fragments, 20);
    assert!(dir.join("out.f140.sqlite").exists());
    // No temp fragment files are used in SQLite mode.
    assert!(!std::fs::read_dir(&dir).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().contains(".frag")));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn sqlite_resume_fills_gap() {
    let dir = temp_dir("sqlite-resume");

    // First run: fragment 3 always fails and is skipped.
    let url = serve(10, vec![3]).await;
    let s = Session::for_test(sqlite_opts(2), Some(FragmentUrl::with_sq_param(&url)), false);
    let (p, _rx) = params(&dir, 10, DownloadState::default());
    let first = download_stream(s, p).await.unwrap();
    assert_eq!(first.skipped, [3]);
    assert_eq!(first.state.fragments, 9);

    // Second run against a healthy server resumes from the saved state and
    // only needs the missing fragment.
    let url = serve(10, vec![]).await;
    let saved = load_state(&dir.join("out.f140.state")).unwrap();
    let s = Session::for_test(sqlite_opts(2), Some(FragmentUrl::with_sq_param(&url)), false);
    let (p, mut rx) = params(&dir, 10, saved);
    let second = download_stream(s, p).await.unwrap();
    assert!(second.skipped.is_empty());
    assert_eq!(second.state.fragments, 10);
    let mut fetched = 0;
    while rx.try_recv().is_ok() {
        fetched += 1;
    }
    assert_eq!(fetched, 1, "only the gap should be downloaded again");

    let want: Vec<u8> = (0..10).flat_map(|i| expected(i, i == 0)).collect();
    assert_eq!(std::fs::read(dir.join("out.f140.ts")).unwrap(), want);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn sqlite_resumes_a_ts_download_as_ts() {
    let url = serve(8, vec![]).await;
    let dir = temp_dir("sqlite-resume-ts");
    // Started with --no-sqlite (or by Go): 3 fragments in the .ts, no database.
    let first: Vec<u8> = (0..3).flat_map(|i| expected(i, i == 0)).collect();
    std::fs::write(dir.join("out.f140.ts"), &first).unwrap();
    let state = DownloadState { start_frag: 0, fragments: 3, size: first.len() as u64, ..Default::default() };

    let s = Session::for_test(sqlite_opts(2), Some(FragmentUrl::with_sq_param(&url)), false);
    let (p, _rx) = params(&dir, 8, state);
    let out = download_stream(s, p).await.unwrap();
    let want: Vec<u8> = (0..8).flat_map(|i| expected(i, i == 0)).collect();
    assert_eq!(std::fs::read(dir.join("out.f140.ts")).unwrap(), want);
    assert_eq!(out.state.fragments, 8);
    assert!(!dir.join("out.f140.sqlite").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A server whose fragments from `forbidden` on get 403, marking `session`
/// unavailable when that starts (as a refresh finding the stream private
/// would).
async fn private_from(head: u64, forbidden: u64) -> (String, Arc<OnceLock<Arc<Session>>>) {
    let hook: Arc<OnceLock<Arc<Session>>> = Arc::default();
    let h = hook.clone();
    let on_forbidden: OnForbidden = Arc::new(move || {
        if let Some(s) = h.get() {
            s.set_unavailable();
        }
    });
    (serve_until(head, vec![], forbidden, Some(on_forbidden)).await, hook)
}

#[tokio::test]
async fn recovers_fragments_after_the_stream_goes_private() {
    // The URL in use stops working at fragment 5; one from an earlier
    // refresh still serves everything.
    let (current, hook) = private_from(12, 5).await;
    let older = serve(12, vec![]).await;
    let dir = temp_dir("recover");
    let s = Session::for_test(recovery_opts(2), Some(FragmentUrl::with_sq_param(&current)), false);
    s.remember(DataType::Audio, &FragmentUrl::with_sq_param(&older));
    hook.set(s.clone()).ok().unwrap();
    let (p, _rx) = params(&dir, 12, DownloadState::default());
    let out = download_stream(s, p).await.unwrap();
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    assert_eq!(out.retry_until, None);
    assert_eq!(out.state.fragments, 12);
    let want: Vec<u8> = (0..12).flat_map(|i| expected(i, i == 0)).collect();
    assert_eq!(std::fs::read(dir.join("out.f140.ts")).unwrap(), want);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn recovery_gives_up_and_a_later_run_retries() {
    let dir = temp_dir("recover-later");
    let saved = Arc::new(crate::types::SavedDownload {
        video_id: "abc".into(),
        label: "audio_only".into(),
        target_duration: 1,
        ..Default::default()
    });

    // First run: nothing works after fragment 5, so recovery gives up.
    let (url, hook) = private_from(12, 5).await;
    let s = Session::for_test(recovery_opts(2), Some(FragmentUrl::with_sq_param(&url)), false);
    hook.set(s.clone()).ok().unwrap();
    let (mut p, _rx) = params(&dir, 12, DownloadState::default());
    p.saved = Some(saved.clone());
    let out = download_stream(s, p).await.unwrap();
    // 10 and 11 are within two of the head (12), so they don't count.
    assert_eq!(out.skipped, [5, 6, 7, 8, 9]);
    // The fake server's URLs have no expire parameter.
    assert_eq!(out.retry_until, Some(u64::MAX));
    let want: Vec<u8> = (0..5).flat_map(|i| expected(i, i == 0)).collect();
    assert_eq!(std::fs::read(dir.join("out.f140.ts")).unwrap(), want);

    // The database has what a later run needs.
    let kept = crate::download::read_kept(&dir, 140, 0o644).unwrap();
    assert_eq!(kept.info, *saved);
    assert_eq!(kept.head, Some(12));
    assert_eq!(kept.urls, [FragmentUrl::with_sq_param(&url)]);

    // Later run, the stream still can't be looked up: straight to recovery,
    // where a working URL now fetches only the missing fragments.
    let working = serve(12, vec![]).await;
    let s = Session::for_test(recovery_opts(2), Some(FragmentUrl::with_sq_param(&working)), false);
    s.set_unavailable();
    let state = load_state(&dir.join("out.f140.state")).unwrap();
    let (p, mut rx) = params(&dir, 12, state);
    let out = download_stream(s, p).await.unwrap();
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    let mut fetched = 0;
    while rx.try_recv().is_ok() {
        fetched += 1;
    }
    assert_eq!(fetched, 7, "5..=11 are fetched again, nothing else");
    let want: Vec<u8> = (0..12).flat_map(|i| expected(i, i == 0)).collect();
    assert_eq!(std::fs::read(dir.join("out.f140.ts")).unwrap(), want);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn a_killed_download_keeps_the_head() {
    // A kill (taskkill /F, a closed console) skips the end-of-download save,
    // so the head must already be in the database.
    let url = serve(200, vec![]).await;
    let dir = temp_dir("recover-killed");
    let s = Session::for_test(recovery_opts(1), Some(FragmentUrl::with_sq_param(&url)), false);
    let (mut p, mut rx) = params(&dir, 200, DownloadState::default());
    p.saved = Some(Arc::new(crate::types::SavedDownload { video_id: "abc".into(), ..Default::default() }));
    let task = tokio::spawn(download_stream(s, p));
    for _ in 0..3 {
        rx.recv().await.unwrap();
    }
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled(), "the download ended before the kill");
    let kept = crate::download::read_kept(&dir, 140, 0o644).unwrap();
    assert_eq!(kept.head, Some(200));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn no_recovery_while_the_stream_is_available() {
    // A 403 on a public stream ends the download as before (no rounds).
    let url = serve_until(12, vec![], 5, None).await;
    let dir = temp_dir("recover-none");
    let s = Session::for_test(recovery_opts(1), Some(FragmentUrl::with_sq_param(&url)), false);
    let (p, _rx) = params(&dir, 12, DownloadState::default());
    let started = std::time::Instant::now();
    let out = download_stream(s, p).await.unwrap();
    assert_eq!(out.skipped, [5, 6, 7, 8, 9]);
    assert!(started.elapsed() < Duration::from_secs(5));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn no_recovery_without_the_flag() {
    // Plain --sqlite: a private stream ends the download as in v0.1.0, even
    // though a working URL is known, and nothing is kept for a later run.
    let (url, hook) = private_from(12, 5).await;
    let older = serve(12, vec![]).await;
    let dir = temp_dir("recover-off");
    let s = Session::for_test(sqlite_opts(1), Some(FragmentUrl::with_sq_param(&url)), false);
    s.remember(DataType::Audio, &FragmentUrl::with_sq_param(&older));
    hook.set(s.clone()).ok().unwrap();
    let (p, _rx) = params(&dir, 12, DownloadState::default());
    let out = download_stream(s, p).await.unwrap();
    assert_eq!(out.skipped, [5]);
    assert_eq!(out.retry_until, None);
    let want: Vec<u8> = (0..5).flat_map(|i| expected(i, i == 0)).collect();
    assert_eq!(std::fs::read(dir.join("out.f140.ts")).unwrap(), want);
    assert!(crate::download::read_kept(&dir, 140, 0o644).is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}
