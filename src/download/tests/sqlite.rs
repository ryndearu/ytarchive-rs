//! SQLite storage and private-stream recovery tests.

use std::fs;
use std::slice;
use std::sync::OnceLock;
use std::time::Instant;

use rusqlite::Connection;

use super::*;
use crate::error::Error;
use crate::types::{SavedDownload, StorageMode};

fn sqlite_opts(threads: usize) -> Options {
    Options { storage: StorageMode::Sqlite, ..opts(threads, true) }
}

/// `--recover-private` (which turns on SQLite).
fn recovery_opts(threads: usize) -> Options {
    Options { recover_private: true, ..sqlite_opts(threads) }
}

#[tokio::test]
async fn sqlite_resume_fills_gap() {
    let dir = temp_dir("sqlite-resume");

    // First run: fragment 3 always fails and is skipped.
    let url = serve(10, vec![3]).await;
    let session = Session::for_test(sqlite_opts(2), Some(FragmentUrl::with_sq_param(&url)), false);
    let (stream_params, _rx) = params(&dir, 10, DownloadState::default());
    let first = download_stream(session, stream_params).await.unwrap();
    assert_eq!(first.skipped, [3]);
    assert_eq!(first.state.fragments, 9);
    assert!(dir.join("out.f140.sqlite").exists());
    // Dropping PendingExport closes the DB without exporting yet.
    drop(first.pending_export);
    // No temp fragment files are used in SQLite mode.
    assert!(!fs::read_dir(&dir).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().contains(".frag")));

    // Second run against a healthy server resumes from the saved state and
    // only needs the missing fragment.
    let url = serve(10, vec![]).await;
    let saved = load_state(&dir.join("out.f140.state")).unwrap();
    let session = Session::for_test(sqlite_opts(2), Some(FragmentUrl::with_sq_param(&url)), false);
    let (stream_params, mut receiver) = params(&dir, 10, saved);
    let second = download_stream(session, stream_params).await.unwrap();
    assert!(second.skipped.is_empty());
    assert_eq!(second.state.fragments, 10);
    // The 9 stored fragments, then only the gap downloaded again.
    assert_eq!(progress_counts(&mut receiver), [9, 1]);

    second.pending_export.unwrap().export().await.unwrap();
    let want: Vec<u8> = (0..10).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(fs::read(dir.join("out.f140.ts")).unwrap(), want);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn sqlite_resumes_a_ts_download_as_ts() {
    let url = serve(8, vec![]).await;
    let dir = temp_dir("sqlite-resume-ts");
    // Started with --no-sqlite (or by Go): 3 fragments in the .ts, no database.
    let first: Vec<u8> = (0..3).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    fs::write(dir.join("out.f140.ts"), &first).unwrap();
    let state = DownloadState { start_frag: 0, fragments: 3, size: first.len() as u64, ..Default::default() };

    let session = Session::for_test(sqlite_opts(2), Some(FragmentUrl::with_sq_param(&url)), false);
    let (stream_params, _rx) = params(&dir, 8, state);
    let out = download_stream(session, stream_params).await.unwrap();
    let want: Vec<u8> = (0..8).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(fs::read(dir.join("out.f140.ts")).unwrap(), want);
    assert_eq!(out.state.fragments, 8);
    assert!(!dir.join("out.f140.sqlite").exists());
    fs::remove_dir_all(&dir).unwrap();
}

/// A server whose fragments from `forbidden` on get 403, marking `session`
/// unavailable when that starts (as a refresh finding the stream private
/// would).
async fn private_from(head: u64, forbidden: u64) -> (String, Arc<OnceLock<Arc<Session>>>) {
    let hook: Arc<OnceLock<Arc<Session>>> = Arc::default();
    let shared_hook = hook.clone();
    let on_forbidden: OnForbidden = Arc::new(move || {
        if let Some(session) = shared_hook.get() {
            session.set_unavailable();
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
    let session = Session::for_test(recovery_opts(2), Some(FragmentUrl::with_sq_param(&current)), false);
    session.remember(DataType::Audio, &FragmentUrl::with_sq_param(&older));
    hook.set(session.clone()).ok().unwrap();
    let (stream_params, _rx) = params(&dir, 12, DownloadState::default());
    let out = download_stream(session, stream_params).await.unwrap();
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    assert_eq!(out.retry_until, None);
    assert_eq!(out.state.fragments, 12);
    out.pending_export.unwrap().export().await.unwrap();
    let want: Vec<u8> = (0..12).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(fs::read(dir.join("out.f140.ts")).unwrap(), want);
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
    let (url, hook) = private_from(12, 5).await;
    let session = Session::for_test(recovery_opts(2), Some(FragmentUrl::with_sq_param(&url)), false);
    hook.set(session.clone()).ok().unwrap();
    let (mut stream_params, _rx) = params(&dir, 12, DownloadState::default());
    stream_params.saved = Some(saved.clone());
    let out = download_stream(session, stream_params).await.unwrap();
    // 10 and 11 are within two of the head (12), so they don't count.
    assert_eq!(out.skipped, [5, 6, 7, 8, 9]);
    // The fake server's URLs have no expire parameter.
    assert_eq!(out.retry_until, Some(u64::MAX));
    out.pending_export.unwrap().export().await.unwrap();
    let want: Vec<u8> = (0..5).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(fs::read(dir.join("out.f140.ts")).unwrap(), want);

    // The database has what a later run needs.
    let kept = read_kept(&dir, 140).unwrap();
    assert_eq!(kept.info, *saved);
    assert_eq!(kept.head, Some(12));
    assert_eq!(kept.urls, [FragmentUrl::with_sq_param(&url)]);

    // Later run, the stream still can't be looked up: straight to recovery,
    // where a working URL now fetches only the missing fragments.
    let working = serve(12, vec![]).await;
    let session = Session::for_test(recovery_opts(2), Some(FragmentUrl::with_sq_param(&working)), false);
    session.set_unavailable();
    let state = load_state(&dir.join("out.f140.state")).unwrap();
    let (stream_params, mut receiver) = params(&dir, 12, state);
    let out = download_stream(session, stream_params).await.unwrap();
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    // 0..=4 were stored; 5..=11 are fetched again, nothing else.
    assert_eq!(progress_counts(&mut receiver), [5, 1, 1, 1, 1, 1, 1, 1]);
    out.pending_export.unwrap().export().await.unwrap();
    let want: Vec<u8> = (0..12).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(fs::read(dir.join("out.f140.ts")).unwrap(), want);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn a_killed_download_keeps_the_head() {
    // A kill (taskkill /F, a closed console) skips the end-of-download save,
    // so the head must already be in the database.
    let url = serve(200, vec![]).await;
    let dir = temp_dir("recover-killed");
    let session = Session::for_test(recovery_opts(1), Some(FragmentUrl::with_sq_param(&url)), false);
    let (mut stream_params, mut receiver) = params(&dir, 200, DownloadState::default());
    stream_params.saved = Some(Arc::new(SavedDownload { video_id: "abc".into(), ..Default::default() }));
    let task = tokio::spawn(download_stream(session, stream_params));
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
    let session = Session::for_test(recovery_opts(1), Some(FragmentUrl::with_sq_param(&url)), false);
    let (stream_params, _rx) = params(&dir, 12, DownloadState::default());
    let started = Instant::now();
    let out = download_stream(session, stream_params).await.unwrap();
    assert_eq!(out.skipped, [5, 6, 7, 8, 9]);
    assert!(started.elapsed() < Duration::from_secs(5));
    drop(out);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn no_recovery_without_the_flag() {
    // Plain --sqlite: a private stream ends the download as in v0.1.0, even
    // though a working URL is known, and nothing is kept for a later run.
    let (url, hook) = private_from(12, 5).await;
    let older = serve(12, vec![]).await;
    let dir = temp_dir("recover-off");
    let session = Session::for_test(sqlite_opts(1), Some(FragmentUrl::with_sq_param(&url)), false);
    session.remember(DataType::Audio, &FragmentUrl::with_sq_param(&older));
    hook.set(session.clone()).ok().unwrap();
    let (stream_params, _rx) = params(&dir, 12, DownloadState::default());
    let out = download_stream(session, stream_params).await.unwrap();
    assert_eq!(out.skipped, [5]);
    assert_eq!(out.retry_until, None);
    out.pending_export.unwrap().export().await.unwrap();
    let want: Vec<u8> = (0..5).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(fs::read(dir.join("out.f140.ts")).unwrap(), want);
    assert!(read_kept(&dir, 140).is_none());
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn sqlite_storage_error_creates_no_ts_file() {
    let dir = temp_dir("sqlite-storage-err");
    let database_path = dir.join("out.f140.sqlite");
    let conn = Connection::open(&database_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE segments (seq INTEGER PRIMARY KEY, mime TEXT NOT NULL, data BLOB NOT NULL);
         CREATE TRIGGER fail_insert BEFORE INSERT ON segments BEGIN
             SELECT RAISE(ABORT, 'disk error');
         END;",
    )
    .unwrap();
    drop(conn);

    let url = serve(5, vec![]).await;
    let session = Session::for_test(sqlite_opts(1), Some(FragmentUrl::with_sq_param(&url)), false);
    let (stream_params, _rx) = params(&dir, 5, DownloadState::default());
    let err = download_stream(session, stream_params).await.unwrap_err();
    assert!(matches!(err, Error::Sqlite(_)), "{err}");
    assert!(!dir.join("out.f140.ts").exists(), "partial .ts must not be exported on storage error");
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn push_with_meta_batches_and_skips_empty_updates() {
    use super::super::db_sink::{DbSink, MetaUpdate};
    use crate::types::{FragData, Fragment};

    let dir = temp_dir("push-with-meta");
    let mut sink = DbSink::open(&dir.join("out.f140.sqlite"), 0o644).unwrap();
    let url = FragmentUrl::with_sq_param("https://r1/videoplayback?itag=140");
    let frag = |seq: u64| Fragment {
        seq,
        x_head_seqnum: None,
        data: FragData::Memory(bytes::Bytes::from_static(b"data")),
        mime_type: "video/webm".into(),
    };

    sink.push_with_meta(1, Some(frag(1)), MetaUpdate { head: Some(42), urls: vec![url.clone()] }).await.unwrap();
    assert_eq!(sink.meta("head").unwrap().as_deref(), Some("42"));
    assert_eq!(sink.urls().unwrap(), slice::from_ref(&url));

    sink.push_with_meta(2, Some(frag(2)), MetaUpdate::default()).await.unwrap();
    assert_eq!(sink.meta("head").unwrap().as_deref(), Some("42"));
    assert_eq!(sink.urls().unwrap(), [url]);
    drop(sink);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn sqlite_sets_aside_unresumable_database_when_state_is_empty() {
    use super::super::db_sink::DbSink;
    use crate::types::{FragData, Fragment};

    let dir = temp_dir("sqlite-unresumable");
    let database_path = dir.join("out.f140.sqlite");
    let mut old = DbSink::open(&database_path, 0o644).unwrap();
    let frag = Fragment {
        seq: 0,
        x_head_seqnum: None,
        data: FragData::Memory(bytes::Bytes::from(fragment_body(0))),
        mime_type: "video/mp4".into(),
    };
    old.push(0, Some(frag)).await.unwrap();
    drop(old);

    let url = serve(6, vec![]).await;
    let session = Session::for_test(sqlite_opts(1), Some(FragmentUrl::with_sq_param(&url)), false);
    let (mut stream_params, _rx) = params(&dir, 6, DownloadState::default());
    stream_params.start_sq = Some(4);
    let out = download_stream(session, stream_params).await.unwrap();
    assert_eq!(out.state.start_frag, 4);
    assert_eq!(out.state.fragments, 2);
    assert!(dir.join("out.f140.sqlite.old").exists(), "old database should be kept as .old");

    out.pending_export.unwrap().export().await.unwrap();
    let want: Vec<u8> = (4..6).flat_map(|sequence| expected(sequence, sequence == 4)).collect();
    assert_eq!(fs::read(dir.join("out.f140.ts")).unwrap(), want);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn keep_existing_db_preserves_uncheckpointed_wal() {
    use super::super::sink::keep_existing_db;

    let dir = temp_dir("sqlite-keep-wal");
    let live_db = dir.join("live.sqlite");
    let conn = Connection::open(&live_db).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA wal_autocheckpoint = 0;
         CREATE TABLE segments (seq INTEGER PRIMARY KEY, data BLOB NOT NULL);",
    )
    .unwrap();
    // Flush the schema into the main DB, then write a row that stays in -wal only.
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
    conn.execute("INSERT INTO segments (seq, data) VALUES (7, X'DEADBEEF')", []).unwrap();

    // Snapshot the DB and its uncheckpointed WAL while still open (skip -shm,
    // which SQLite byte-range-locks on Windows and rebuilds from -wal on open).
    let target_db = dir.join("out.f140.sqlite");
    fs::copy(&live_db, &target_db).unwrap();
    fs::copy(dir.join("live.sqlite-wal"), dir.join("out.f140.sqlite-wal")).unwrap();
    drop(conn);

    keep_existing_db(&target_db, "audio").await;

    assert!(!target_db.exists());
    assert!(!dir.join("out.f140.sqlite-wal").exists());
    assert!(dir.join("out.f140.sqlite.old").exists());
    assert!(dir.join("out.f140.sqlite.old-wal").exists());

    // The uncheckpointed row must still be readable from the .old backup.
    let old_conn = Connection::open(dir.join("out.f140.sqlite.old")).unwrap();
    let data: Vec<u8> = old_conn.query_row("SELECT data FROM segments WHERE seq = 7", [], |row| row.get(0)).unwrap();
    assert_eq!(data, [0xDE, 0xAD, 0xBE, 0xEF]);
    drop(old_conn);

    fs::remove_dir_all(&dir).unwrap();
}
