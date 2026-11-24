//! SQLite storage tests.

use std::fs;
use std::slice;

use rusqlite::Connection;

use super::*;
use crate::error::Error;
use crate::types::StorageMode;

pub(super) fn sqlite_opts(threads: usize) -> Options {
    Options { storage: StorageMode::Sqlite, ..opts(threads, true) }
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

    keep_existing_db(&target_db, "audio").await.unwrap();

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

#[tokio::test]
async fn aborts_when_unresumable_sqlite_cannot_be_moved_aside() {
    use super::super::db_sink::DbSink;
    use crate::types::{FragData, Fragment};

    let dir = temp_dir("sqlite-keep-old-fail");
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

    let blocker = dir.join("out.f140.sqlite.old");
    fs::create_dir(&blocker).unwrap();
    fs::write(blocker.join("keep"), b"x").unwrap();

    let session = Session::for_test(sqlite_opts(1), Some(FragmentUrl::with_sq_param("http://127.0.0.1:1/x?")), false);
    let (stream_params, _rx) = params(&dir, 6, DownloadState::default());
    let err = download_stream(session, stream_params).await.unwrap_err();
    assert!(matches!(err, Error::OpenStorage(_)), "{err}");

    let conn = Connection::open(&database_path).unwrap();
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM segments", [], |row| row.get(0)).unwrap();
    assert_eq!(count, 1);
    drop(conn);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn sqlite_resumes_when_output_name_changes() {
    let dir = temp_dir("sqlite-resume-renamed");
    let url = serve(6, vec![2]).await;
    let session = Session::for_test(sqlite_opts(2), Some(FragmentUrl::with_sq_param(&url)), false);
    let (mut stream_params, _rx) = params(&dir, 6, DownloadState::default());
    stream_params.data_file = dir.join("old-title.f140.ts");
    let first = download_stream(session, stream_params).await.unwrap();
    assert_eq!(first.skipped, [2]);
    drop(first.pending_export);

    let url = serve(6, vec![]).await;
    let saved = load_state(&dir.join("out.f140.state")).unwrap();
    let session = Session::for_test(sqlite_opts(2), Some(FragmentUrl::with_sq_param(&url)), false);
    let (mut stream_params, mut receiver) = params(&dir, 6, saved);
    stream_params.data_file = dir.join("new-title.f140.ts");
    let second = download_stream(session, stream_params).await.unwrap();
    assert!(second.skipped.is_empty());
    assert_eq!(second.state.fragments, 6);
    assert_eq!(progress_counts(&mut receiver), [5, 1]);
    assert!(!dir.join("old-title.f140.sqlite").exists());
    assert!(dir.join("new-title.f140.sqlite").exists());

    second.pending_export.unwrap().export().await.unwrap();
    let want: Vec<u8> = (0..6).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(fs::read(dir.join("new-title.f140.ts")).unwrap(), want);
    fs::remove_dir_all(&dir).unwrap();
}
