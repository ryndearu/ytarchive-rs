//! SQLite storage and private-stream recovery tests.

use super::*;

fn sqlite_opts(threads: usize) -> Options {
    Options {
        storage: crate::types::StorageMode::Sqlite,
        ..opts(threads, true)
    }
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
    assert!(!std::fs::read_dir(&dir)
        .unwrap()
        .any(|e| e.unwrap().file_name().to_string_lossy().contains(".frag")));
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

#[tokio::test]
async fn recovers_fragments_after_the_stream_goes_private() {
    // The URL in use stops working at fragment 5; one from an earlier
    // refresh still serves everything.
    let current = serve_until(12, vec![], 5).await;
    let older = serve(12, vec![]).await;
    let dir = temp_dir("recover");
    let s = Session::for_test(recovery_opts(2), Some(FragmentUrl::with_sq_param(&current)), false);
    s.remember(DataType::Audio, &FragmentUrl::with_sq_param(&older));
    s.set_unavailable();
    let (p, _rx) = params(&dir, 12, DownloadState::default());
    let out = download_stream(s, p).await.unwrap();
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    assert_eq!(out.state.fragments, 12);
    let want: Vec<u8> = (0..12).flat_map(|i| expected(i, i == 0)).collect();
    assert_eq!(std::fs::read(dir.join("out.f140.ts")).unwrap(), want);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn recovery_gives_up_and_reports_missing_fragments() {
    let url = serve_until(12, vec![], 5).await;
    let dir = temp_dir("recover-fail");
    let s = Session::for_test(recovery_opts(2), Some(FragmentUrl::with_sq_param(&url)), false);
    s.set_unavailable();
    let (p, _rx) = params(&dir, 12, DownloadState::default());
    let out = download_stream(s, p).await.unwrap();
    // 10 and 11 are within two of the head (12), so they don't count.
    assert_eq!(out.skipped, [5, 6, 7, 8, 9]);
    let want: Vec<u8> = (0..5).flat_map(|i| expected(i, i == 0)).collect();
    assert_eq!(std::fs::read(dir.join("out.f140.ts")).unwrap(), want);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn no_recovery_while_the_stream_is_available() {
    // A 403 on a public stream ends the download as before (no rounds).
    let url = serve_until(12, vec![], 5).await;
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
    // though a working URL is known.
    let url = serve_until(12, vec![], 5).await;
    let older = serve(12, vec![]).await;
    let dir = temp_dir("recover-off");
    let s = Session::for_test(sqlite_opts(1), Some(FragmentUrl::with_sq_param(&url)), false);
    s.remember(DataType::Audio, &FragmentUrl::with_sq_param(&older));
    s.set_unavailable();
    let (p, _rx) = params(&dir, 12, DownloadState::default());
    let out = download_stream(s, p).await.unwrap();
    assert_eq!(out.skipped, [5]);
    let want: Vec<u8> = (0..5).flat_map(|i| expected(i, i == 0)).collect();
    assert_eq!(std::fs::read(dir.join("out.f140.ts")).unwrap(), want);
    std::fs::remove_dir_all(&dir).unwrap();
}
