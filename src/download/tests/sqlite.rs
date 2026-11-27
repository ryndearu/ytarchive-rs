//! SQLite storage tests.

use std::fs;

use rusqlite::Connection;
use tokio::time::timeout;

use super::*;
use crate::error::Error;
use crate::types::StorageMode;

pub(super) fn sqlite_opts(threads: usize) -> Options {
    Options { storage: StorageMode::Sqlite, ..opts(threads, true) }
}

#[tokio::test]
async fn sqlite_resume_fills_gap() {
    // The second run also has a new title: the database follows the name.
    for (old_name, new_name) in [("out", "out"), ("old-title", "new-title")] {
        let dir = temp_dir(&format!("sqlite-resume-{new_name}"));

        // First run: fragment 3 always fails and is skipped.
        let url = serve(10, vec![3]).await;
        let (mut stream_params, _rx) = params(&dir, 10, DownloadState::default());
        stream_params.data_file = dir.join(format!("{old_name}.f140.ts"));
        let first = download_stream(session_for(sqlite_opts(2), &url, false), stream_params).await.unwrap();
        assert_eq!(first.skipped, [3]);
        assert_eq!(first.state.fragments, 9);
        assert!(dir.join(format!("{old_name}.f140.sqlite")).exists());
        // Dropping PendingExport closes the DB without exporting yet.
        drop(first.pending_export);
        // No temp fragment files are used in SQLite mode.
        assert!(
            !fs::read_dir(&dir).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().contains(".frag"))
        );

        // Second run against a healthy server resumes from the saved state and
        // only needs the missing fragment.
        let url = serve(10, vec![]).await;
        let saved = load_state(&dir.join("out.f140.state")).unwrap();
        let (mut stream_params, mut receiver) = params(&dir, 10, saved);
        stream_params.data_file = dir.join(format!("{new_name}.f140.ts"));
        let second = download_stream(session_for(sqlite_opts(2), &url, false), stream_params).await.unwrap();
        assert!(second.skipped.is_empty());
        assert_eq!(second.state.fragments, 10);
        // The 9 stored fragments, then only the gap downloaded again.
        assert_eq!(progress_counts(&mut receiver), [9, 1]);
        assert!(old_name == new_name || !dir.join(format!("{old_name}.f140.sqlite")).exists());
        assert!(dir.join(format!("{new_name}.f140.sqlite")).exists());

        second.pending_export.unwrap().export().await.unwrap();
        assert_output(&dir.join(format!("{new_name}.f140.ts")), 0..10);
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[tokio::test]
async fn resumed_capture_finishes_when_its_last_missing_fragment_is_stuck() {
    // Only fragment 5 is missing, and it never downloads. Its one worker is
    // the only one left, so nothing else downloads to count against it.
    let dir = temp_dir("sqlite-capture-stuck");
    stored_database(&dir.join("out.f140.sqlite"), (0..10).filter(|sequence| *sequence != 5)).await;
    let url = serve(200, vec![5]).await;
    let state = DownloadState { start_frag: 0, fragments: 9, ..Default::default() };
    let (mut stream_params, _rx) = params(&dir, 200, state);
    stream_params.start_sq = Some(0);
    stream_params.capture_duration = Some(Duration::from_secs(10));
    stream_params.retry_sleep = Duration::from_millis(1);
    let download = download_stream(session_for(sqlite_opts(1), &url, true), stream_params);
    let out = timeout(Duration::from_secs(30), download).await.expect("download hangs").unwrap();
    assert_eq!(out.skipped, [5]);
    out.pending_export.unwrap().export().await.unwrap();
    assert_output(&dir.join("out.f140.ts"), (0..10).filter(|sequence| *sequence != 5));
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn sqlite_resumes_a_ts_download_as_ts() {
    let url = serve(8, vec![]).await;
    let dir = temp_dir("sqlite-resume-ts");
    // Started with --no-sqlite (or by Go): 3 fragments in the .ts, no database.
    let first = expected_file(0..3);
    fs::write(dir.join("out.f140.ts"), &first).unwrap();
    let state = DownloadState { start_frag: 0, fragments: 3, size: first.len() as u64, ..Default::default() };

    let (stream_params, _rx) = params(&dir, 8, state);
    let out = download_stream(session_for(sqlite_opts(2), &url, false), stream_params).await.unwrap();
    assert_output(&dir.join("out.f140.ts"), 0..8);
    assert_eq!(out.state.fragments, 8);
    assert!(!dir.join("out.f140.sqlite").exists());
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn sqlite_sets_aside_unresumable_database_when_state_is_empty() {
    let dir = temp_dir("sqlite-unresumable");
    stored_database(&dir.join("out.f140.sqlite"), [0]).await;

    let url = serve(6, vec![]).await;
    let (mut stream_params, _rx) = params(&dir, 6, DownloadState::default());
    stream_params.start_sq = Some(4);
    let out = download_stream(session_for(sqlite_opts(1), &url, false), stream_params).await.unwrap();
    assert_eq!(out.state.start_frag, 4);
    assert_eq!(out.state.fragments, 2);
    assert!(dir.join("out.f140.sqlite.old").exists(), "old database should be kept as .old");

    out.pending_export.unwrap().export().await.unwrap();
    assert_output(&dir.join("out.f140.ts"), 4..6);
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
async fn sqlite_mid_download_write_failure_resumes_cleanly() {
    let dir = temp_dir("sqlite-full-resume");
    let database_path = dir.join("out.f140.sqlite");
    stored_database(&database_path, []).await;
    fail_inserts(&database_path, "WHEN NEW.seq >= 3");

    // First run stores fragments 0..3, then fails on fragment 3.
    let url = serve(6, vec![]).await;
    let session = session_for(sqlite_opts(1), &url, false);
    let (stream_params, _rx) = params(&dir, 6, DownloadState::default());
    let err = download_stream(session.clone(), stream_params).await.unwrap_err();
    assert!(matches!(err, Error::Sqlite(_)), "{err}");
    assert!(session.is_stopping());
    assert!(!dir.join("out.f140.ts").exists(), "failed download must not export .ts");
    let saved = load_state(&dir.join("out.f140.state")).unwrap();
    assert_eq!(saved.fragments, 3);

    // Space freed: drop the trigger and resume; only fragments 3..6 are fetched.
    let conn = Connection::open(&database_path).unwrap();
    conn.execute_batch("DROP TRIGGER fail_insert;").unwrap();
    drop(conn);

    let (stream_params, mut receiver) = params(&dir, 6, saved);
    let resumed = download_stream(session_for(sqlite_opts(1), &url, false), stream_params).await.unwrap();
    assert_eq!(resumed.state.fragments, 6);
    assert_eq!(progress_counts(&mut receiver), [3, 1, 1, 1]);

    resumed.pending_export.unwrap().export().await.unwrap();
    assert_output(&dir.join("out.f140.ts"), 0..6);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn sqlite_failed_export_keeps_database_for_retry() {
    let dir = temp_dir("sqlite-export-retry");
    let url = serve(4, vec![]).await;
    let (stream_params, _rx) = params(&dir, 4, DownloadState::default());
    let out = download_stream(session_for(sqlite_opts(1), &url, false), stream_params).await.unwrap();

    // Block the .ts target with a directory so export fails.
    let blocked = dir.join("out.f140.ts");
    fs::create_dir(&blocked).unwrap();
    assert!(out.pending_export.unwrap().export().await.is_err());
    fs::remove_dir(&blocked).unwrap();

    // The database remains intact and exports cleanly on retry.
    let database = DbSink::open(&dir.join("out.f140.sqlite"), 0o644).unwrap();
    let (count, _) = database.export(&blocked, 0o644).unwrap();
    assert_eq!(count, 4);
    drop(database);
    assert_output(&blocked, 0..4);
    fs::remove_dir_all(&dir).unwrap();
}
