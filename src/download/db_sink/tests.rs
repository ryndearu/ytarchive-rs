use std::env;
use std::process;
use std::sync::mpsc;
use std::time::Duration;

use super::*;
use crate::types::FragData;

fn frag(seq: u64, body: &[u8]) -> Fragment {
    Fragment {
        seq,
        x_head_seqnum: None,
        data: FragData::Memory(bytes::Bytes::copy_from_slice(body)),
        mime_type: "video/webm".into(),
    }
}

#[tokio::test]
async fn stores_out_of_order_and_exports_in_order() {
    let dir = env::temp_dir().join(format!("ytarchive-rs-db-{}", process::id()));
    fs::create_dir_all(&dir).unwrap();
    let database_path = db_path(&dir.join("x.f140.ts"));
    assert!(database_path.ends_with("x.f140.sqlite"));

    let mut sink = DbSink::open(&database_path, 0o644).unwrap();
    for seq in [3u64, 1, 2] {
        let events = sink.push(seq, Some(frag(seq, format!("<{seq}>").as_bytes()))).await.unwrap();
        assert!(matches!(events[..], [Event::Wrote(3)]));
    }
    assert!(matches!(sink.push(4, None).await.unwrap()[..], [Event::Gap(4)]));
    drop(sink);

    // Reopen: data persisted, duplicates replaced.
    let mut sink = DbSink::open(&database_path, 0o644).unwrap();
    assert_eq!(sink.stored().unwrap(), HashSet::from([1, 2, 3]));
    sink.push(2, Some(frag(2, b"<2>"))).await.unwrap();
    assert_eq!(sink.totals().unwrap(), (3, 9));

    let out = dir.join("x.f140.ts");
    assert_eq!(sink.export(&out, 0o644).unwrap(), (3, 9));
    assert_eq!(fs::read(&out).unwrap(), b"<1><2><3>");

    let first = FragmentUrl::with_sq_param("https://a/videoplayback?itag=140");
    let second = FragmentUrl::with_sq_param("https://b/videoplayback?itag=140");
    sink.save_urls(&[first.clone(), second.clone()]).unwrap();
    sink.save_urls(std::slice::from_ref(&first)).unwrap();
    assert_eq!(sink.urls().unwrap(), [first, second]);
    assert_eq!(sink.meta("head").unwrap(), None);
    sink.set_meta("head", "9").unwrap();
    sink.set_meta("head", "10").unwrap();
    assert_eq!(sink.meta("head").unwrap().as_deref(), Some("10"));
    // Windows can't delete the database while the connection is open.
    drop(sink);
    fs::remove_dir_all(&dir).unwrap();
}

/// Another thread holds the database, as a slow insert would. On the
/// test's single runtime thread the release can only be sent while the
/// insert waits elsewhere.
#[tokio::test]
async fn insert_leaves_the_runtime_thread_free() {
    let dir = env::temp_dir().join(format!("ytarchive-rs-db-free-{}", process::id()));
    fs::create_dir_all(&dir).unwrap();
    let mut sink = DbSink::open(&dir.join("x.f140.sqlite"), 0o644).unwrap();

    let (held_sender, held_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel::<()>();
    let holder = std::thread::spawn({
        let conn = sink.conn.clone();
        move || {
            let guard = lock(&conn);
            held_sender.send(()).unwrap();
            let released = release_receiver.recv_timeout(Duration::from_secs(5)).is_ok();
            drop(guard);
            released
        }
    });
    held_receiver.recv().unwrap();

    let (events, ()) = tokio::join!(sink.push(1, Some(frag(1, b"<1>"))), async { release_sender.send(()).unwrap() });
    assert!(matches!(events.unwrap()[..], [Event::Wrote(3)]));
    assert!(holder.join().unwrap(), "the insert held the runtime thread until the database was free");
    assert_eq!(sink.stored().unwrap(), HashSet::from([1]));
    drop(sink);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn read_kept_recovers_wal_without_shm() {
    let base = env::temp_dir().join(format!("ytarchive-rs-db-wal-{}", process::id()));
    let live_dir = base.join("live");
    let crash_dir = base.join("crash");
    fs::create_dir_all(&live_dir).unwrap();
    fs::create_dir_all(&crash_dir).unwrap();

    let db = live_dir.join("stream.f140.sqlite");
    let sink = DbSink::open(&db, 0o644).unwrap();
    let saved = SavedDownload { video_id: "vid123".into(), target_duration: 5, ..Default::default() };
    sink.set_meta("download", &serde_json::to_string(&saved).unwrap()).unwrap();
    sink.set_meta("head", "42").unwrap();
    let url = FragmentUrl::with_sq_param("https://r.googlevideo.com/v?itag=140&noclen=1");
    sink.save_urls(std::slice::from_ref(&url)).unwrap();

    // Copy the DB and its uncheckpointed -wal without -shm (simulating a crash).
    fs::copy(&db, crash_dir.join("stream.f140.sqlite")).unwrap();
    fs::copy(wal_path(&db), crash_dir.join("stream.f140.sqlite-wal")).unwrap();
    drop(sink);

    let kept = read_kept(&crash_dir, 140).expect("should recover kept metadata from WAL");
    assert_eq!(kept.info.video_id, "vid123");
    assert_eq!(kept.head, Some(42));
    assert_eq!(kept.urls, [url]);
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn sets_busy_timeout_and_read_kept_ignores_directories() {
    let dir = env::temp_dir().join(format!("ytarchive-rs-db-kept-{}", process::id()));
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(dir.join("bad.f140.sqlite")).unwrap();

    let db = dir.join("good.f140.sqlite");
    let sink = DbSink::open(&db, 0o644).unwrap();
    let timeout_ms: i64 = lock(&sink.conn).query_row("PRAGMA busy_timeout", [], |row| row.get(0)).unwrap();
    assert_eq!(timeout_ms, 5000);

    let saved = SavedDownload { video_id: "vid456".into(), target_duration: 5, ..Default::default() };
    sink.set_meta("download", &serde_json::to_string(&saved).unwrap()).unwrap();
    drop(sink);

    let kept = read_kept(&dir, 140).expect("should ignore directory and read valid sqlite file");
    assert_eq!(kept.info.video_id, "vid456");
    fs::remove_dir_all(&dir).unwrap();
}
