//! Downloader tests against a local fake googlevideo server.

mod recovery;
mod resume;
mod retry;
mod sqlite;

use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::error::Error;
use crate::http::Http;
use crate::session::Session;
use crate::types::{DataType, DownloadState, FragmentUrl, Options, ProgressInfo};
use crate::{mp4_atoms, test_http, ytdlp};

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
    serve_observed(head, fail, forbidden, on_forbidden, None).await
}

/// Called by the fake server with the sequence of every request.
type OnRequest = Arc<dyn Fn(u64) + Send + Sync>;

async fn serve_observed(
    head: u64,
    fail: Vec<u64>,
    forbidden: u64,
    on_forbidden: Option<OnForbidden>,
    on_request: Option<OnRequest>,
) -> String {
    serve_with_status(head, fail, "500 Internal Server Error", forbidden, on_forbidden, on_request).await
}

async fn serve_status(head: u64, fail: Vec<u64>, fail_status: &'static str, on_request: Option<OnRequest>) -> String {
    serve_with_status(head, fail, fail_status, u64::MAX, None, on_request).await
}

async fn serve_with_status(
    head: u64,
    fail: Vec<u64>,
    fail_status: &'static str,
    forbidden: u64,
    on_forbidden: Option<OnForbidden>,
    on_request: Option<OnRequest>,
) -> String {
    let addr = test_http::serve(move |request| {
        let seq = request_sequence(&request);
        if let Some(callback) = &on_request {
            callback(seq);
        }
        let (status, body) = if seq >= forbidden && seq != u64::MAX {
            if let Some(callback) = &on_forbidden {
                callback();
            }
            ("403 Forbidden", Vec::new())
        } else if fail.contains(&seq) {
            (fail_status, Vec::new())
        } else if seq > head {
            ("404 Not Found", Vec::new())
        } else {
            ("200 OK", fragment_body(seq))
        };
        let response = fragment_response(status, head, &body);
        async move { Some(response) }
    })
    .await;
    fragment_url(addr)
}

/// The `sq` of a request to the fake server; `u64::MAX` without one.
fn request_sequence(request: &str) -> u64 {
    test_http::path(request).rsplit("sq=").next().and_then(|text| text.parse().ok()).unwrap_or(u64::MAX)
}

fn fragment_response(status: &str, head: u64, body: &[u8]) -> Vec<u8> {
    test_http::response(status, &format!("content-type: video/mp4\r\nx-head-seqnum: {head}\r\n"), body)
}

fn fragment_url(addr: SocketAddr) -> String {
    format!("http://{addr}/videoplayback?itag=140&noclen=1")
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ytarchive-rs-dl-{tag}-{}", std::process::id()));
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn params(dir: &Path, last_sq: u64, state: DownloadState) -> (StreamParams, mpsc::UnboundedReceiver<ProgressInfo>) {
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
            memory_limit: MEMORY_LIMIT,
            adopt_renamed: true,
        },
        receiver,
    )
}

/// The fragment count of each progress message sent so far.
fn progress_counts(receiver: &mut mpsc::UnboundedReceiver<ProgressInfo>) -> Vec<u64> {
    std::iter::from_fn(|| receiver.try_recv().ok()).map(|progress| progress.fragments).collect()
}

fn opts(threads: usize, frag_files: bool) -> Options {
    Options { threads, frag_max_tries: 2, frag_files, file_mode: 0o644, ..Default::default() }
}

/// For downloads that must fail before any request.
const UNREACHABLE: &str = "http://127.0.0.1:1/x?";

fn session_for(options: Options, url: &str, live: bool) -> Arc<Session> {
    Session::for_test(options, Some(FragmentUrl::with_sq_param(url)), live)
}

/// Downloads up to `head` into `dir` with 1 ms retries; a download that
/// doesn't end within 10 s fails the test.
async fn download_quickly(session: Arc<Session>, dir: &Path, head: u64) -> StreamOutcome {
    let (mut stream_params, _receiver) = params(dir, head, DownloadState::default());
    stream_params.retry_sleep = Duration::from_millis(1);
    tokio::time::timeout(Duration::from_secs(10), download_stream(session, stream_params))
        .await
        .expect("download hangs")
        .unwrap()
}

/// Asserts that `0..fragments` were all stored, in order.
fn assert_complete(out: &StreamOutcome, dir: &Path, fragments: u64) {
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    assert_eq!(out.state.fragments, fragments);
    assert_output(&dir.join("out.f140.ts"), 0..fragments);
}

/// The output after storing `sequences` in order; the first keeps `ftyp`.
fn expected_file(sequences: impl IntoIterator<Item = u64>) -> Vec<u8> {
    sequences.into_iter().enumerate().flat_map(|(index, sequence)| expected(sequence, index == 0)).collect()
}

fn assert_output(path: &Path, sequences: impl IntoIterator<Item = u64>) {
    assert_eq!(fs::read(path).unwrap(), expected_file(sequences), "{}", path.display());
}

/// A database already holding `sequences`, as an earlier run leaves it.
async fn stored_database(path: &Path, sequences: impl IntoIterator<Item = u64>) {
    use crate::types::{FragData, Fragment};

    let mut database = DbSink::open(path, 0o644).unwrap();
    for seq in sequences {
        let data = FragData::Memory(bytes::Bytes::from(fragment_body(seq)));
        let fragment = Fragment { seq, x_head_seqnum: None, data, mime_type: "video/mp4".into() };
        database.push(seq, Some(fragment)).await.unwrap();
    }
}

/// Makes inserts into the database fail, like a full disk; `when` is an
/// optional SQL `WHEN` clause.
fn fail_inserts(path: &Path, when: &str) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute_batch(&format!(
            "CREATE TRIGGER fail_insert BEFORE INSERT ON segments {when} BEGIN
                 SELECT RAISE(ABORT, 'database or disk is full');
             END;"
        ))
        .unwrap();
}

/// A server whose fragments from `forbidden` on get 403; the first 403 calls
/// `on_forbidden` with the session registered in the returned cell.
async fn forbidden_from(
    head: u64,
    forbidden: u64,
    on_forbidden: fn(&Session),
) -> (String, Arc<OnceLock<Arc<Session>>>) {
    let cell: Arc<OnceLock<Arc<Session>>> = Arc::default();
    let callback: OnForbidden = Arc::new({
        let cell = cell.clone();
        move || {
            if let Some(session) = cell.get() {
                on_forbidden(session);
            }
        }
    });
    (serve_until(head, vec![], forbidden, Some(callback)).await, cell)
}

#[tokio::test]
async fn downloads_ended_stream_in_order() {
    for frag_files in [false, true] {
        let url = serve(20, vec![]).await;
        let dir = temp_dir(&format!("order-{frag_files}"));
        let session = session_for(opts(4, frag_files), &url, false);
        let (stream_params, mut receiver) = params(&dir, 20, DownloadState::default());
        let out = download_stream(session, stream_params).await.unwrap();

        // Not live: stops before the head sequence, as Go does.
        assert_output(&dir.join("out.f140.ts"), 0..20);
        assert_eq!(out.state.fragments, 20);
        assert_eq!(out.state.size, expected_file(0..20).len() as u64);
        assert!(out.skipped.is_empty());

        let mut progress = 0;
        while receiver.try_recv().is_ok() {
            progress += 1;
        }
        assert_eq!(progress, 20);
        // State file reflects the final state; no fragment files left behind.
        let saved = load_state(&dir.join("out.f140.state")).unwrap();
        assert_eq!(saved.fragments, 20);
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".frag"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[tokio::test]
async fn skips_permanently_failing_fragment() {
    // A skipped first fragment still leaves `ftyp` at the start.
    for failing in [3, 0] {
        let url = serve(15, vec![failing]).await;
        let dir = temp_dir(&format!("skip-{failing}"));
        let (stream_params, _rx) = params(&dir, 15, DownloadState::default());
        let out = download_stream(session_for(opts(3, false), &url, false), stream_params).await.unwrap();
        assert_eq!(out.skipped, [failing]);
        assert_output(&dir.join("out.f140.ts"), (0..15).filter(|sequence| *sequence != failing));
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[tokio::test]
async fn unopenable_storage_is_reported_as_such() {
    // A data file in a folder that doesn't exist, like a name Windows rejects.
    let dir = temp_dir("no-storage");
    let session = session_for(opts(1, false), UNREACHABLE, false);
    let (mut stream_params, _rx) = params(&dir, 5, DownloadState::default());
    stream_params.data_file = dir.join("missing").join("out.f140.ts");
    let err = download_stream(session, stream_params).await.unwrap_err();
    assert!(matches!(err, Error::OpenStorage(_)), "{err}");
    fs::remove_dir_all(&dir).unwrap();
}

/// `/dev/full` takes no data, like a full disk; only Linux has it.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_fragment_that_does_not_reach_a_full_disk_is_not_counted() {
    let url = serve(10, vec![]).await;
    let dir = temp_dir("full-disk");
    let mut options = opts(2, false);
    // The mode /dev/full already has, so applying it changes nothing.
    options.file_mode = 0o666;
    let session = session_for(options, &url, false);
    let (mut stream_params, mut receiver) = params(&dir, 10, DownloadState::default());
    stream_params.data_file = PathBuf::from("/dev/full");
    let err = download_stream(session.clone(), stream_params).await.unwrap_err();

    assert!(matches!(err, Error::Io(_)), "{err}");
    assert!(session.is_stopping());
    assert!(receiver.try_recv().is_err(), "progress was reported for an unwritten fragment");
    let saved = load_state(&dir.join("out.f140.state")).unwrap();
    assert_eq!((saved.fragments, saved.size), (0, 0));
    fs::remove_dir_all(&dir).unwrap();
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
    let http = Http::new(&options, Default::default()).unwrap();
    let info =
        ytdlp::extract(&http, &options, "https://www.youtube.com/@LofiGirl/live", true, &CancellationToken::new())
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
    let data = fs::read(dir.join("out.f140.ts")).unwrap();
    assert_eq!(&data[4..8], b"ftyp", "first fragment keeps ftyp");
    let names: Vec<_> = mp4_atoms::atoms(&data).iter().map(|atom| atom.0.to_string()).collect();
    assert!(!names.contains(&"sidx".to_string()), "{names:?}");
    assert_eq!(names.iter().filter(|name| *name == "ftyp").count(), 1, "{names:?}");
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn capture_duration_and_start_sq() {
    let url = serve(100, vec![]).await;
    let dir = temp_dir("capture");
    let (mut stream_params, _rx) = params(&dir, 100, DownloadState::default());
    stream_params.start_sq = Some(50);
    // Target duration is 1s in tests: 5s = 5 fragments.
    stream_params.capture_duration = Some(Duration::from_secs(5));
    let out = download_stream(session_for(opts(2, false), &url, true), stream_params).await.unwrap();
    assert_output(&dir.join("out.f140.ts"), 50..55);
    assert_eq!(out.state.start_frag, 50);
    fs::remove_dir_all(&dir).unwrap();
}
