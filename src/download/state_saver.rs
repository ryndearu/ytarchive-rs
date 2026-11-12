//! Saves a stream's resume state on a blocking thread, so a slow disk doesn't
//! hold up the download.

use std::path::PathBuf;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::log_warn;
use crate::types::DownloadState;
use crate::util::write_atomic;

/// Only the newest state is written: while one save runs, later ones replace
/// each other. The file can be behind the stored data but never ahead of it,
/// and resuming cuts the data file back to what the state says.
pub struct StateSaver {
    sender: watch::Sender<Option<Vec<u8>>>,
    task: JoinHandle<()>,
}

impl StateSaver {
    pub fn start(path: PathBuf, mode: u32) -> Self {
        let (sender, mut receiver) = watch::channel(None::<Vec<u8>>);
        let task = tokio::spawn(async move {
            while receiver.changed().await.is_ok() {
                let Some(data) = receiver.borrow_and_update().clone() else { continue };
                let written = tokio::task::spawn_blocking({
                    let path = path.clone();
                    move || write_atomic(&path, &data, mode)
                })
                .await
                .unwrap_or_else(|error| Err(std::io::Error::other(error)));
                if let Err(error) = written {
                    log_warn!("Error when saving state: {error}");
                }
            }
        });
        Self { sender, task }
    }

    pub fn save(&self, state: &DownloadState) {
        match serde_json::to_vec(state) {
            Ok(data) => {
                self.sender.send_replace(Some(data));
            }
            Err(error) => log_warn!("Error when saving state: {error}"),
        }
    }

    /// Waits until the last state given to `save` is on disk.
    pub async fn finish(self) {
        drop(self.sender);
        if let Err(error) = self.task.await {
            log_warn!("Error when saving state: {error}");
        }
    }
}
