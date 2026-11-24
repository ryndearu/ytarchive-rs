//! Resuming `.ts` downloads from a state file.

use std::fs;

use super::*;

#[tokio::test]
async fn keeps_data_that_has_no_usable_state() {
    let url = serve(3, vec![]).await;
    let dir = temp_dir("keep-old");
    fs::write(dir.join("out.f140.ts"), b"earlier download").unwrap();
    let session = Session::for_test(opts(2, false), Some(FragmentUrl::with_sq_param(&url)), false);
    let (stream_params, _receiver) = params(&dir, 3, DownloadState::default());
    let out = download_stream(session, stream_params).await.unwrap();

    assert_eq!(out.state.fragments, 3);
    assert_eq!(fs::read(dir.join("out.f140.ts.old")).unwrap(), b"earlier download");
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn aborts_when_unresumable_ts_cannot_be_moved_aside() {
    let dir = temp_dir("keep-old-fail");
    let data_file = dir.join("out.f140.ts");
    fs::write(&data_file, b"earlier download").unwrap();
    // A directory at `.old` blocks renaming the file on every OS.
    let blocker = dir.join("out.f140.ts.old");
    fs::create_dir(&blocker).unwrap();
    fs::write(blocker.join("keep"), b"x").unwrap();

    let session = Session::for_test(opts(1, false), Some(FragmentUrl::with_sq_param("http://127.0.0.1:1/x?")), false);
    let (stream_params, _receiver) = params(&dir, 3, DownloadState::default());
    let err = download_stream(session, stream_params).await.unwrap_err();
    assert!(matches!(err, Error::OpenStorage(_)), "{err}");
    assert_eq!(fs::read(&data_file).unwrap(), b"earlier download");
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn resumes_from_saved_state() {
    let url = serve(10, vec![]).await;
    let dir = temp_dir("resume");
    let first: Vec<u8> = (0..4).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    // Saved state covers 4 fragments; extra garbage after it must be dropped.
    let mut on_disk = first.clone();
    on_disk.extend_from_slice(b"partial-garbage");
    fs::write(dir.join("out.f140.ts"), &on_disk).unwrap();
    let state = DownloadState { start_frag: 0, fragments: 4, size: first.len() as u64, ..Default::default() };

    let session = Session::for_test(opts(2, false), Some(FragmentUrl::with_sq_param(&url)), false);
    let (stream_params, mut receiver) = params(&dir, 10, state);
    let out = download_stream(session, stream_params).await.unwrap();
    let want: Vec<u8> = (0..10).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(fs::read(dir.join("out.f140.ts")).unwrap(), want);
    assert_eq!(out.state.fragments, 10);
    assert_eq!(progress_counts(&mut receiver), [4, 1, 1, 1, 1, 1, 1]);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn resume_with_missing_file_starts_over() {
    let url = serve(5, vec![]).await;
    let dir = temp_dir("resume-missing");
    let state = DownloadState { start_frag: 0, fragments: 3, size: 100, ..Default::default() };
    let session = Session::for_test(opts(1, false), Some(FragmentUrl::with_sq_param(&url)), false);
    let (stream_params, mut receiver) = params(&dir, 5, state);
    let out = download_stream(session, stream_params).await.unwrap();
    let want: Vec<u8> = (0..5).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(fs::read(dir.join("out.f140.ts")).unwrap(), want);
    assert_eq!(out.state.fragments, 5);
    // The 3 fragments in the state are gone with the file, so the progress
    // line must not count them.
    assert_eq!(progress_counts(&mut receiver), [1; 5]);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn resume_after_skipped_fragment_does_not_duplicate() {
    let dir = temp_dir("resume-skipped");

    // First run: fragment 2 permanently fails and is skipped out of 0..6.
    let url = serve(6, vec![2]).await;
    let session = Session::for_test(opts(2, false), Some(FragmentUrl::with_sq_param(&url)), false);
    let (mut stream_params, _rx) = params(&dir, 6, DownloadState::default());
    stream_params.retry_sleep = Duration::from_millis(1);
    let first = download_stream(session, stream_params).await.unwrap();
    assert_eq!(first.skipped, [2]);
    assert_eq!(first.state.fragments, 5);
    assert_eq!(first.state.skipped, 1);

    let saved = load_state(&dir.join("out.f140.state")).unwrap();
    assert_eq!(saved.skipped, 1);

    // Go state files without `Skipped` must still deserialize with `skipped == 0`.
    let legacy: DownloadState =
        serde_json::from_str(r#"{"StartFrag":0,"Fragments":3,"Size":100,"TempDir":""}"#).unwrap();
    assert_eq!(legacy.skipped, 0);

    // Resume to head 8: must start at 6 (not 5) so fragment 5 isn't duplicated.
    let url = serve(8, vec![]).await;
    let session = Session::for_test(opts(2, false), Some(FragmentUrl::with_sq_param(&url)), false);
    let (mut stream_params, _rx) = params(&dir, 8, saved);
    stream_params.retry_sleep = Duration::from_millis(1);
    let second = download_stream(session, stream_params).await.unwrap();
    assert_eq!(second.state.fragments, 7);

    let want: Vec<u8> =
        (0..8).filter(|sequence| *sequence != 2).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(fs::read(dir.join("out.f140.ts")).unwrap(), want);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn resumes_ts_when_output_name_changes() {
    let url = serve(10, vec![]).await;
    let dir = temp_dir("resume-renamed");
    let first: Vec<u8> = (0..4).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    fs::write(dir.join("old-title.f140.ts"), &first).unwrap();
    let state = DownloadState { start_frag: 0, fragments: 4, size: first.len() as u64, ..Default::default() };

    let session = Session::for_test(opts(2, false), Some(FragmentUrl::with_sq_param(&url)), false);
    let (mut stream_params, mut receiver) = params(&dir, 10, state);
    stream_params.data_file = dir.join("new-title.f140.ts");
    let out = download_stream(session, stream_params).await.unwrap();
    let want: Vec<u8> = (0..10).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(fs::read(dir.join("new-title.f140.ts")).unwrap(), want);
    assert!(!dir.join("old-title.f140.ts").exists());
    assert_eq!(out.state.fragments, 10);
    assert_eq!(progress_counts(&mut receiver), [4, 1, 1, 1, 1, 1, 1]);
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn does_not_adopt_renamed_file_when_temp_dir_not_owned() {
    let url = serve(5, vec![]).await;
    let dir = temp_dir("resume-unowned");
    fs::write(dir.join("other-video.f140.ts"), b"other video data").unwrap();
    let state = DownloadState { start_frag: 0, fragments: 3, size: 16, ..Default::default() };

    let session = Session::for_test(opts(1, false), Some(FragmentUrl::with_sq_param(&url)), false);
    let (mut stream_params, _rx) = params(&dir, 5, state);
    stream_params.data_file = dir.join("new-title.f140.ts");
    stream_params.adopt_renamed = false;
    let out = download_stream(session, stream_params).await.unwrap();
    let want: Vec<u8> = (0..5).flat_map(|sequence| expected(sequence, sequence == 0)).collect();
    assert_eq!(fs::read(dir.join("new-title.f140.ts")).unwrap(), want);
    assert_eq!(fs::read(dir.join("other-video.f140.ts")).unwrap(), b"other video data");
    assert_eq!(out.state.fragments, 5);
    fs::remove_dir_all(&dir).unwrap();
}
