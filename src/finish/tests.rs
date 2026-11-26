use std::fs;
use std::path::Path;

use super::*;
use crate::mux::MuxInput;

/// A finished audio download with a thumbnail in a temp folder, and its
/// state file.
fn downloaded(name: &str) -> (OutputPlan, PathBuf) {
    let final_dir = std::env::temp_dir().join(format!("ytarchive-rs-finish-{name}-{}", std::process::id()));
    let temp_dir = final_dir.join("vid__1a2b");
    fs::create_dir_all(&temp_dir).unwrap();
    let plan = OutputPlan { final_dir: final_dir.clone(), file_name: "out".into(), temp_dir };
    let state_file = final_dir.join("vid.f140.state");
    fs::write(&state_file, b"{}").unwrap();
    fs::write(plan.temp(".f140.ts"), b"audio").unwrap();
    fs::write(plan.temp(".jpg"), b"thumbnail").unwrap();
    (plan, state_file)
}

fn finish_of<'a>(opts: &'a Options, plan: &'a OutputPlan, state_file: &Path) -> Finish<'a> {
    let moved = |suffix: &str| Moved { temp: plan.temp(suffix), dest: plan.final_path(suffix) };
    let audio = plan.final_path(".f140.ts");
    let final_dir = &plan.final_dir;
    Finish {
        opts,
        plan,
        video_id: "vid",
        audio_ts: Some(moved(".f140.ts")),
        video_ts: Some(moved(".f299.ts")),
        thumbnail: Some(moved(".jpg")),
        description: None,
        mux_file: moved(".ffmpeg.txt"),
        mux: mux::ffmpeg_args(&MuxInput {
            audio: Some(&audio),
            video: None,
            thumbnail: None,
            mkv: false,
            metadata: None,
            out_dir: final_dir,
            file_name: "out",
        }),
        audio_mux: None,
        state_files: vec![state_file.to_path_buf()],
        audio_frags: Some(1),
        video_frags: None,
        keep_for_retry: None,
        keep_other_streams: false,
        pending_exports: Vec::new(),
        interrupt: None,
    }
}

/// Ctrl-C with "keep the files": the downloaded data and thumbnail move
/// to the final folder, a file that was never created is skipped, and
/// the temp folder and state file are removed.
#[tokio::test]
async fn cancel_keeping_files_moves_them_and_cleans_up() {
    let (plan, state_file) = downloaded("cancel");
    let opts = Options { merge_on_cancel: Action::DoNot, save_files_on_cancel: Action::Do, ..Default::default() };
    let mut finish = finish_of(&opts, &plan, &state_file);

    assert_eq!(finish.on_cancel().await, Some(2));
    assert_eq!(fs::read(plan.final_path(".f140.ts")).unwrap(), b"audio");
    assert_eq!(fs::read(plan.final_path(".jpg")).unwrap(), b"thumbnail");
    assert!(!plan.final_path(".f299.ts").exists());
    assert!(!plan.temp_dir.exists());
    assert!(!state_file.exists());
    fs::remove_dir_all(&plan.final_dir).unwrap();
}

/// A storage error after data was stored leaves the data and its state
/// where a later run resumes from; with nothing stored, both are removed.
#[test]
fn storage_error_keeps_what_can_be_resumed() {
    let (plan, state_file) = downloaded("storage");
    let opts = Options::default();
    let mut finish = finish_of(&opts, &plan, &state_file);

    assert_eq!(finish.on_storage_error(true), 1);
    assert_eq!(fs::read(plan.temp(".f140.ts")).unwrap(), b"audio");
    assert!(state_file.exists());
    assert!(!plan.final_path(".f140.ts").exists());

    assert_eq!(finish.on_storage_error(false), 1);
    assert!(!plan.temp_dir.exists());
    assert!(!state_file.exists());
    fs::remove_dir_all(&plan.final_dir).unwrap();
}

#[tokio::test]
async fn cancel_discard_skips_pending_sqlite_export() {
    let (plan, state_file) = downloaded("cancel-discard-db");
    fs::remove_file(plan.temp(".f140.ts")).unwrap();
    let pending = PendingExport::for_test(&plan.temp(".f140.sqlite"), plan.temp(".f140.ts")).await;
    let opts = Options {
        merge_on_cancel: Action::DoNot,
        save_files_on_cancel: Action::DoNot,
        save_state_on_cancel: Action::DoNot,
        ..Default::default()
    };
    let mut finish = finish_of(&opts, &plan, &state_file);
    finish.pending_exports.push(pending);

    assert_eq!(finish.on_cancel().await, Some(2));
    assert!(!plan.temp(".f140.ts").exists(), "discarded cancel must not export .ts");
    assert!(!plan.final_path(".f140.ts").exists());
    assert!(!plan.temp_dir.exists());
    assert!(!state_file.exists());
    fs::remove_dir_all(&plan.final_dir).unwrap();
}

#[tokio::test]
async fn terminate_keeps_temp_files_and_state_files() {
    let (plan, state_file) = downloaded("terminate");
    let pending = PendingExport::for_test(&plan.temp(".f140.sqlite"), plan.temp(".f140.ts.exported")).await;
    let opts = Options::default();
    let mut finish = finish_of(&opts, &plan, &state_file);
    finish.pending_exports.push(pending);

    assert_eq!(finish.on_terminate(), 143);
    assert!(plan.temp_dir.exists());
    assert_eq!(fs::read(plan.temp(".f140.ts")).unwrap(), b"audio");
    assert!(state_file.exists());
    assert!(!plan.temp(".f140.ts.exported").exists());
    assert!(!plan.final_path(".f140.ts").exists());
    fs::remove_dir_all(&plan.final_dir).unwrap();
}

#[tokio::test]
async fn write_mux_file_updates_separate_audio_command() {
    let (plan, state_file) = downloaded("mux-separate-audio");
    fs::write(plan.temp(".f299.ts"), b"video").unwrap();
    let audio = plan.final_path(".f140.ts");
    let video = plan.final_path(".f299.ts");
    let main_mux = mux::ffmpeg_args(&MuxInput {
        audio: Some(&audio),
        video: Some(&video),
        thumbnail: None,
        mkv: false,
        metadata: None,
        out_dir: &plan.final_dir,
        file_name: "out",
    });
    let audio_mux = mux::ffmpeg_args(&MuxInput {
        audio: Some(&audio),
        video: None,
        thumbnail: None,
        mkv: false,
        metadata: None,
        out_dir: &plan.final_dir,
        file_name: "out",
    });
    let initial = mux::mux_file_contents("ffmpeg", &main_mux, Some(&audio_mux));
    fs::write(plan.temp(".ffmpeg.txt"), &initial).unwrap();
    // Simulate out.m4a appearing before finish, requiring -1 on the audio command.
    fs::write(plan.final_path(".m4a"), b"existing").unwrap();

    let opts = Options {
        write_mux_file: true,
        separate_audio: true,
        ffmpeg_path: "ffmpeg".into(),
        file_mode: 0o644,
        ..Default::default()
    };
    let mut finish = finish_of(&opts, &plan, &state_file);
    finish.mux = main_mux;
    finish.audio_mux = Some(audio_mux);

    assert_eq!(finish.finish().await, 0);
    let written = fs::read_to_string(plan.final_path(".ffmpeg.txt")).unwrap();
    let lines: Vec<&str> = written.lines().collect();
    assert_eq!(lines.len(), 2, "{written}");
    assert!(lines[0].contains("out.mp4"), "{written}");
    assert!(lines[1].contains("out-1.m4a"), "{written}");
    fs::remove_dir_all(&plan.final_dir).unwrap();
}

#[tokio::test]
async fn failed_second_sqlite_export_removes_first_exported_ts() {
    let (plan, state_file) = downloaded("export-partial-cleanup");
    fs::remove_file(plan.temp(".f140.ts")).unwrap();
    let first_out = plan.temp(".f140.ts");
    let blocked_out = plan.temp(".f299.ts");
    // A directory at the export target forces File::create to fail.
    fs::create_dir(&blocked_out).unwrap();
    let first = PendingExport::for_test(&plan.temp(".f140.sqlite"), first_out.clone()).await;
    let second = PendingExport::for_test(&plan.temp(".f299.sqlite"), blocked_out).await;

    let opts = Options::default();
    let mut finish = finish_of(&opts, &plan, &state_file);
    finish.pending_exports.push(first);
    finish.pending_exports.push(second);

    assert_eq!(finish.finish().await, 1);
    assert!(!first_out.exists(), "earlier exported .ts must be deleted when a later export fails");
    assert!(plan.temp(".f140.sqlite").exists());
    assert!(plan.temp(".f299.sqlite").exists());
    assert!(state_file.exists());
    fs::remove_dir_all(&plan.final_dir).unwrap();
}

#[tokio::test]
async fn finish_skips_muxing_when_move_fails() {
    let (plan, state_file) = downloaded("move-fail-skip-mux");
    // Block moving .f140.ts by placing a non-empty directory at its destination.
    fs::create_dir(plan.final_path(".f140.ts")).unwrap();
    fs::write(plan.final_path(".f140.ts").join("blocker"), b"x").unwrap();

    let opts = Options { ffmpeg_path: "nonexistent-ffmpeg-binary".into(), ..Default::default() };
    let mut finish = finish_of(&opts, &plan, &state_file);

    assert_eq!(finish.finish().await, 1);
    assert!(plan.temp(".f140.ts").exists(), "temp file must remain intact after failed move");
    assert!(state_file.exists(), "state file must remain intact after failed move");
    fs::remove_dir_all(&plan.final_dir).unwrap();
}

#[tokio::test]
async fn zero_fragments_skip_mux_and_keep_only_what_a_retry_needs() {
    for keep_for_retry in [None, Some(u64::MAX)] {
        let (plan, state_file) = downloaded(&format!("zero-frags-{}", keep_for_retry.is_some()));
        let pending = PendingExport::for_test(&plan.temp(".f140.sqlite"), plan.temp(".f140.ts.exported")).await;
        let opts = Options { ffmpeg_path: "nonexistent-ffmpeg-binary".into(), ..Default::default() };
        let mut finish = finish_of(&opts, &plan, &state_file);
        finish.audio_frags = Some(0);
        finish.video_frags = Some(0);
        finish.keep_for_retry = keep_for_retry;
        finish.pending_exports.push(pending);

        assert_eq!(finish.finish().await, 1);
        assert!(!plan.temp(".f140.ts.exported").exists(), "0-fragment finish must not export SQLite");
        assert!(!plan.final_path(".f140.ts").exists(), "0-fragment finish must not move files");
        let kept = keep_for_retry.is_some();
        assert_eq!(plan.temp(".f140.sqlite").exists(), kept, "database kept for a retry");
        assert_eq!(plan.temp_dir.exists(), kept, "temp dir kept only for a retry");
        assert_eq!(state_file.exists(), kept, "state file kept only for a retry");
        fs::remove_dir_all(&plan.final_dir).unwrap();
    }
}

#[tokio::test]
async fn keeps_temp_dir_when_other_kept_stream_was_skipped() {
    let (plan, state_file) = downloaded("keep-other-finish");
    fs::write(plan.temp(".f140.sqlite"), b"processed-audio").unwrap();
    fs::write(plan.temp(".f140.sqlite-wal"), b"processed-wal").unwrap();
    fs::write(plan.temp(".f299.sqlite"), b"kept-video").unwrap();
    let opts = Options { write_mux_file: true, ..Default::default() };
    let mut finish = finish_of(&opts, &plan, &state_file);
    finish.video_ts = None;
    finish.keep_other_streams = true;
    assert_eq!(finish.finish().await, 0);
    assert!(plan.temp_dir.exists());
    assert!(!plan.temp(".f140.ts").exists());
    assert!(!plan.temp(".f140.sqlite").exists());
    assert!(!plan.temp(".f140.sqlite-wal").exists());
    assert_eq!(fs::read(plan.temp(".f299.sqlite")).unwrap(), b"kept-video");
    assert!(!state_file.exists());
    fs::remove_dir_all(&plan.final_dir).unwrap();

    let (plan, state_file) = downloaded("keep-other-cancel");
    fs::write(plan.temp(".f140.sqlite"), b"processed-audio").unwrap();
    fs::write(plan.temp(".f140.sqlite-wal"), b"processed-wal").unwrap();
    fs::write(plan.temp(".f299.sqlite"), b"kept-video").unwrap();
    let opts = Options {
        merge_on_cancel: Action::DoNot,
        save_files_on_cancel: Action::DoNot,
        save_state_on_cancel: Action::DoNot,
        ..Default::default()
    };
    let mut finish = finish_of(&opts, &plan, &state_file);
    finish.video_ts = None;
    finish.keep_other_streams = true;
    assert_eq!(finish.on_cancel().await, Some(2));
    assert!(plan.temp_dir.exists());
    assert!(!plan.temp(".f140.ts").exists());
    assert!(!plan.temp(".f140.sqlite").exists());
    assert!(!plan.temp(".f140.sqlite-wal").exists());
    assert!(!plan.temp(".jpg").exists());
    assert_eq!(fs::read(plan.temp(".f299.sqlite")).unwrap(), b"kept-video");
    assert!(!state_file.exists());
    fs::remove_dir_all(&plan.final_dir).unwrap();
}
