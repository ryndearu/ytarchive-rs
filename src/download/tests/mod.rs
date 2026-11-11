//! Downloader tests against a local fake googlevideo server.

mod retry;
mod sqlite;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use super::*;
use crate::session::Session;
use crate::types::{DataType, DownloadState, FragmentUrl, Options};

fn mp4_box(name: &str, payload: &[u8]) -> Vec<u8> {
    let mut bytes = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
    bytes.extend_from_slice(name.as_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

/// What the fake server returns for a sequence number.
fn fragment_body(seq: u64) -> Vec<u8> {
    [
        mp4_box("ftyp", b"iso6"),
        mp4_box("sidx", b"index"),
        mp4_box("moof", &seq.to_be_bytes()),
        mp4_box("mdat", format!("data{seq}").as_bytes()),
    ]
    .concat()
}

/// Bytes expected in the output for `seq` (first fragment keeps `ftyp`).
fn expected(seq: u64, first: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    if first {
        bytes.extend(mp4_box("ftyp", b"iso6"));
    }
    bytes.extend(mp4_box("moof", &seq.to_be_bytes()));
    bytes.extend(mp4_box("mdat", format!("data{seq}").as_bytes()));
    bytes
}

/// Serves fragments `0..=head`; `fail` lists sequences that always 500.
async fn serve(head: u64, fail: Vec<u64>) -> String {
    serve_until(head, fail, u64::MAX, None).await
}

/// Called by the fake server on its first 403.
type OnForbidden = Arc<dyn Fn() + Send + Sync>;

/// Like `serve`, but sequences from `forbidden` on get 403, as for a stream
/// that went private.
async fn serve_until(head: u64, fail: Vec<u64>, forbidden: u64, on_forbidden: Option<OnForbidden>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let fail = Arc::new(fail);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let fail = fail.clone();
            let on_forbidden = on_forbidden.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
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
                let (status, body) = if seq >= forbidden && seq != u64::MAX {
                    if let Some(callback) = &on_forbidden {
                        callback();
                    }
                    ("403 Forbidden", Vec::new())
                } else if fail.contains(&seq) {
                    ("500 Internal Server Error", Vec::new())
                } else if seq > head {
                    ("404 Not Found", Vec::new())
                } else {
                    ("200 OK", fragment_body(seq))
                };
                let hdr = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: video/mp4\r\nx-head-seqnum: {head}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                // The client may hang up first (a cancelled request).
                sock.write_all(hdr.as_bytes()).await.ok();
                sock.write_all(&body).await.ok();
            });
        }
    });
    format!("http://127.0.0.1:{}/videoplayback?itag=140&noclen=1", addr.port())
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ytarchive-rs-dl-{tag}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn params(
    dir: &Path,
    last_sq: u64,
    state: DownloadState,
) -> (StreamParams, mpsc::UnboundedReceiver<crate::types::ProgressInfo>) {
    let (sender, receiver) = mpsc::unbounded_channel();
    (
        StreamParams {
            data_type: DataType::Audio,
            data_file: dir.join("out.f140.ts"),
            frag_prefix: dir.join("out.f140"),
            state,
            state_file: Some(dir.join("out.f140.state")),
            last_sq: Some(last_sq),
            start_sq: None,
            capture_duration: None,
            retry_sleep: Duration::from_millis(10),
            progress: sender,
            saved: None,
        },
        receiver,
    )
}

fn opts(threads: usize, frag_files: bool) -> Options {
    Options { threads, frag_max_tries: 2, frag_files, file_mode: 0o644, ..Default::default() }
}

#[tokio::test]
async fn downloads_ended_stream_in_order() {
    for frag_files in [false, true] {
        let url = serve(20, vec![]).await;
        let dir = temp_dir(&format!("order-{frag_files}"));
        let session = Session::for_test(opts(4, frag_files), Some(FragmentUrl::with_sq_param(&url)), false);
        let (stream_params, mut receiver) = params(&dir, 20, DownloadState::default());
        let out = download_stream(session, stream_params).await.unwrap();

        // Not live: stops before the head sequence, as Go does.
        let want: Vec<u8> = (0..20).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
        assert_eq!(std::fs::read(dir.join("out.f140.ts")).unwrap(), want);
        assert_eq!(out.state.fragments, 20);
        assert_eq!(out.state.size, want.len() as u64);
        assert!(out.skipped.is_empty());

        let mut progress = 0;
        while receiver.try_recv().is_ok() {
            progress += 1;
        }
        assert_eq!(progress, 20);
        // State file reflects the final state; no fragment files left behind.
        let saved = load_state(&dir.join("out.f140.state")).unwrap();
        assert_eq!(saved.fragments, 20);
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".frag"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[tokio::test]
async fn keeps_data_that_has_no_usable_state() {
    let url = serve(3, vec![]).await;
    let dir = temp_dir("keep-old");
    std::fs::write(dir.join("out.f140.ts"), b"earlier download").unwrap();
    let session = Session::for_test(opts(2, false), Some(FragmentUrl::with_sq_param(&url)), false);
    let (stream_params, _receiver) = params(&dir, 3, DownloadState::default());
    let out = download_stream(session, stream_params).await.unwrap();

    assert_eq!(out.state.fragments, 3);
    assert_eq!(std::fs::read(dir.join("out.f140.ts.old")).unwrap(), b"earlier download");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn resumes_from_saved_state() {
    let url = serve(10, vec![]).await;
    let dir = temp_dir("resume");
    let first: Vec<u8> = (0..4).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    // Saved state covers 4 fragments; extra garbage after it must be dropped.
    let mut on_disk = first.clone();
    on_disk.extend_from_slice(b"partial-garbage");
    std::fs::write(dir.join("out.f140.ts"), &on_disk).unwrap();
    let state = DownloadState { start_frag: 0, fragments: 4, size: first.len() as u64, ..Default::default() };

    let session = Session::for_test(opts(2, false), Some(FragmentUrl::with_sq_param(&url)), false);
    let (stream_params, _rx) = params(&dir, 10, state);
    let out = download_stream(session, stream_params).await.unwrap();
    let want: Vec<u8> = (0..10).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(std::fs::read(dir.join("out.f140.ts")).unwrap(), want);
    assert_eq!(out.state.fragments, 10);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn resume_with_missing_file_starts_over() {
    let url = serve(5, vec![]).await;
    let dir = temp_dir("resume-missing");
    let state = DownloadState { start_frag: 0, fragments: 3, size: 100, ..Default::default() };
    let session = Session::for_test(opts(1, false), Some(FragmentUrl::with_sq_param(&url)), false);
    let (stream_params, _rx) = params(&dir, 5, state);
    let out = download_stream(session, stream_params).await.unwrap();
    let want: Vec<u8> = (0..5).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(std::fs::read(dir.join("out.f140.ts")).unwrap(), want);
    assert_eq!(out.state.fragments, 5);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn skips_permanently_failing_fragment() {
    let url = serve(15, vec![3]).await;
    let dir = temp_dir("skip");
    let session = Session::for_test(opts(3, false), Some(FragmentUrl::with_sq_param(&url)), false);
    let (stream_params, _rx) = params(&dir, 15, DownloadState::default());
    let out = download_stream(session, stream_params).await.unwrap();
    assert_eq!(out.skipped, [3]);
    let want: Vec<u8> =
        (0..15).filter(|sequence| *sequence != 3).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(std::fs::read(dir.join("out.f140.ts")).unwrap(), want);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn unopenable_storage_is_reported_as_such() {
    // A data file in a folder that doesn't exist, like a name Windows rejects.
    let dir = temp_dir("no-storage");
    let session = Session::for_test(opts(1, false), Some(FragmentUrl::with_sq_param("http://127.0.0.1:1/x?")), false);
    let (mut stream_params, _rx) = params(&dir, 5, DownloadState::default());
    stream_params.data_file = dir.join("missing").join("out.f140.ts");
    let err = download_stream(session, stream_params).await.unwrap_err();
    assert!(matches!(err, crate::error::Error::OpenStorage(_)), "{err}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Hits YouTube: downloads 3 real audio fragments from a 24/7 stream.
/// Run with `YTDLP_TEST_OPTS="--js-runtimes node" cargo test -- --ignored live_fragments`.
#[tokio::test]
#[ignore]
async fn live_fragments() {
    let mut options = opts(2, true);
    options.ytdlp_path = "yt-dlp".into();
    options.ytdlp_opts =
        std::env::var("YTDLP_TEST_OPTS").unwrap_or_default().split_whitespace().map(String::from).collect();
    let http = crate::http::Http::new(&options, Default::default()).unwrap();
    let info = crate::ytdlp::extract(
        &http,
        &options,
        "https://www.youtube.com/@LofiGirl/live",
        true,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    let last = info.last_sq.unwrap();

    let dir = temp_dir("live");
    let session = Session::for_test(options, info.urls.get(&140).cloned(), true);
    let (mut stream_params, _rx) = params(&dir, last, DownloadState::default());
    stream_params.start_sq = Some(last - 3);
    stream_params.capture_duration = Some(Duration::from_secs(3)); // 3 fragments at the test's 1s target
    stream_params.retry_sleep = Duration::from_secs(1);
    let out = download_stream(session, stream_params).await.unwrap();
    assert_eq!(out.state.fragments, 3);
    let data = std::fs::read(dir.join("out.f140.ts")).unwrap();
    assert_eq!(&data[4..8], b"ftyp", "first fragment keeps ftyp");
    let names: Vec<_> = crate::mp4_atoms::atoms(&data).iter().map(|atom| atom.0.to_string()).collect();
    assert!(!names.contains(&"sidx".to_string()), "{names:?}");
    assert_eq!(names.iter().filter(|name| *name == "ftyp").count(), 1, "{names:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn capture_duration_and_start_sq() {
    let url = serve(100, vec![]).await;
    let dir = temp_dir("capture");
    let session = Session::for_test(opts(2, false), Some(FragmentUrl::with_sq_param(&url)), true);
    let (mut stream_params, _rx) = params(&dir, 100, DownloadState::default());
    stream_params.start_sq = Some(50);
    // Target duration is 1s in tests: 5s = 5 fragments.
    stream_params.capture_duration = Some(Duration::from_secs(5));
    let out = download_stream(session, stream_params).await.unwrap();
    let want: Vec<u8> = (50..55).flat_map(|sequence| expected(sequence, sequence == 50)).collect();
    assert_eq!(std::fs::read(dir.join("out.f140.ts")).unwrap(), want);
    assert_eq!(out.state.start_frag, 50);
    std::fs::remove_dir_all(&dir).unwrap();
}
