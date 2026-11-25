use std::collections::HashMap;
use std::time::Duration;

use tokio::task::JoinSet;
use tokio::time::timeout;

use super::tasks::collect_stream_tasks;
use super::*;
use crate::download::StreamOutcome;
use crate::types::{DownloadState, Options};

#[tokio::test]
async fn task_panic_stops_sibling_stream_immediately() {
    let session = Session::for_test(Options::default(), None, true);
    let mut tasks = JoinSet::new();
    let mut task_types = HashMap::new();
    let audio_handle = tasks.spawn(async move {
        if true {
            panic!("boom");
        }
        (
            DataType::Audio,
            Ok(StreamOutcome {
                state: DownloadState::default(),
                skipped: Vec::new(),
                retry_until: None,
                pending_export: None,
            }),
        )
    });
    task_types.insert(audio_handle.id(), DataType::Audio);
    let video_handle = tasks.spawn({
        let session = session.clone();
        async move {
            session.cancel.cancelled().await;
            (
                DataType::Video,
                Ok(StreamOutcome {
                    state: DownloadState::default(),
                    skipped: Vec::new(),
                    retry_until: None,
                    pending_export: None,
                }),
            )
        }
    });
    task_types.insert(video_handle.id(), DataType::Video);

    let collected = timeout(Duration::from_secs(2), collect_stream_tasks(&session, &mut tasks, &task_types))
        .await
        .expect("sibling stream should stop immediately after task panic");
    assert!(session.is_stopping());
    assert!(collected.storage_failed);
    assert!(collected.stored_before_failing);
}
