//! Resuming `.ts` downloads from a state file.

use std::fs;

use super::*;
use crate::types::StorageMode;

#[tokio::test]
async fn keeps_data_that_has_no_usable_state() {
    let url = serve(3, vec![]).await;
    let dir = temp_dir("keep-old");
    fs::write(dir.join("out.f140.ts"), b"earlier download").unwrap();
    let (stream_params, _receiver) = params(&dir, 3, DownloadState::default());
    let out = download_stream(session_for(opts(2, false), &url, false), stream_params).await.unwrap();

    assert_eq!(out.state.fragments, 3);
    assert_eq!(fs::read(dir.join("out.f140.ts.old")).unwrap(), b"earlier download");
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn aborts_when_unresumable_data_cannot_be_moved_aside() {
    for storage in [StorageMode::DirectTs, StorageMode::Sqlite] {
        let dir = temp_dir(&format!("keep-old-fail-{storage:?}"));
        let data_file = if storage == StorageMode::Sqlite {
            let database = dir.join("out.f140.sqlite");
            stored_database(&database, [0]).await;
            database
        } else {
            let data_file = dir.join("out.f140.ts");
            fs::write(&data_file, b"earlier download").unwrap();
            data_file
        };
        let before = fs::read(&data_file).unwrap();
        // A directory at `.old` blocks renaming the file on every OS.
        let blocker = dir.join(format!("{}.old", data_file.file_name().unwrap().to_string_lossy()));
        fs::create_dir(&blocker).unwrap();
        fs::write(blocker.join("keep"), b"x").unwrap();

        let options = Options { storage, ..opts(1, storage == StorageMode::Sqlite) };
        let (stream_params, _receiver) = params(&dir, 6, DownloadState::default());
        let err = download_stream(session_for(options, UNREACHABLE, false), stream_params).await.unwrap_err();
        assert!(matches!(err, Error::OpenStorage(_)), "{storage:?}: {err}");
        assert_eq!(fs::read(&data_file).unwrap(), before, "{storage:?}");
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[tokio::test]
async fn resumes_from_saved_state() {
    // Extra data after what the state covers must be dropped, and a file
    // saved under an earlier title is picked up under the new one.
    for (old_name, new_name, trailing) in
        [("out", "out", &b"partial-garbage"[..]), ("old-title", "new-title", &b""[..])]
    {
        let url = serve(10, vec![]).await;
        let dir = temp_dir(&format!("resume-{new_name}"));
        let first = expected_file(0..4);
        fs::write(dir.join(format!("{old_name}.f140.ts")), [&first[..], trailing].concat()).unwrap();
        let state = DownloadState { start_frag: 0, fragments: 4, size: first.len() as u64, ..Default::default() };

        let (mut stream_params, mut receiver) = params(&dir, 10, state);
        stream_params.data_file = dir.join(format!("{new_name}.f140.ts"));
        let out = download_stream(session_for(opts(2, false), &url, false), stream_params).await.unwrap();
        assert_output(&dir.join(format!("{new_name}.f140.ts")), 0..10);
        assert!(old_name == new_name || !dir.join(format!("{old_name}.f140.ts")).exists());
        assert_eq!(out.state.fragments, 10);
        assert_eq!(progress_counts(&mut receiver), [4, 1, 1, 1, 1, 1, 1]);
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[tokio::test]
async fn resume_with_missing_file_starts_over() {
    let url = serve(5, vec![]).await;
    let dir = temp_dir("resume-missing");
    let state = DownloadState { start_frag: 0, fragments: 3, size: 100, ..Default::default() };
    let (stream_params, mut receiver) = params(&dir, 5, state);
    let out = download_stream(session_for(opts(1, false), &url, false), stream_params).await.unwrap();
    assert_output(&dir.join("out.f140.ts"), 0..5);
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
    let (mut stream_params, _rx) = params(&dir, 6, DownloadState::default());
    stream_params.retry_sleep = Duration::from_millis(1);
    let first = download_stream(session_for(opts(2, false), &url, false), stream_params).await.unwrap();
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
    let (mut stream_params, _rx) = params(&dir, 8, saved);
    stream_params.retry_sleep = Duration::from_millis(1);
    let second = download_stream(session_for(opts(2, false), &url, false), stream_params).await.unwrap();
    assert_eq!(second.state.fragments, 7);
    assert_output(&dir.join("out.f140.ts"), (0..8).filter(|sequence| *sequence != 2));
    fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn does_not_adopt_renamed_file_when_temp_dir_not_owned() {
    let url = serve(5, vec![]).await;
    let dir = temp_dir("resume-unowned");
    fs::write(dir.join("other-video.f140.ts"), b"other video data").unwrap();
    let state = DownloadState { start_frag: 0, fragments: 3, size: 16, ..Default::default() };

    let (mut stream_params, _rx) = params(&dir, 5, state);
    stream_params.data_file = dir.join("new-title.f140.ts");
    stream_params.adopt_renamed = false;
    let out = download_stream(session_for(opts(1, false), &url, false), stream_params).await.unwrap();
    assert_output(&dir.join("new-title.f140.ts"), 0..5);
    assert_eq!(fs::read(dir.join("other-video.f140.ts")).unwrap(), b"other video data");
    assert_eq!(out.state.fragments, 5);
    fs::remove_dir_all(&dir).unwrap();
}
