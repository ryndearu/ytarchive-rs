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
        pending_exports: Vec::new(),
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
