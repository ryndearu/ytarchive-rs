//! After downloading: Ctrl-C choices (merge / keep files / keep resume
//! state), moving files to the final directory, and muxing.

mod merge;

use std::mem;
use std::path::{Path, PathBuf};

use tokio::task::spawn_blocking;

use crate::download::{PendingExport, local_time};
use crate::interrupt::Interrupt;
use crate::mux::MuxCommand;
use crate::output::OutputPlan;
use crate::prompt::read_yes_no;
use crate::types::{Action, Options};
use crate::util::{try_delete, try_move};
use crate::{log_error, log_warn};

/// A file produced in the temp dir and where it ends up.
#[derive(Debug, Clone)]
pub struct Moved {
    pub temp: PathBuf,
    pub dest: PathBuf,
}

pub struct Finish<'a> {
    pub opts: &'a Options,
    pub plan: &'a OutputPlan,
    pub video_id: &'a str,
    pub audio_ts: Option<Moved>,
    pub video_ts: Option<Moved>,
    pub thumbnail: Option<Moved>,
    pub description: Option<Moved>,
    pub mux_file: Moved,
    pub mux: MuxCommand,
    pub audio_mux: Option<MuxCommand>,
    pub state_files: Vec<PathBuf>,
    /// Fragment counts, for the audio/video mismatch warning.
    pub audio_frags: Option<u64>,
    pub video_frags: Option<u64>,
    /// Fragments are missing but can be retried by running again until
    /// this time (Unix seconds, `u64::MAX` if unknown): keep the temp dir
    /// with the database, and the state files that lead to it.
    pub keep_for_retry: Option<u64>,
    /// Another kept stream in the temp dir was skipped (`--no-audio`/`--no-video`),
    /// so cleanup must leave the temp folder intact.
    pub keep_other_streams: bool,
    /// Deferred SQLite exports when interrupted so cancel prompts run first.
    pub pending_exports: Vec<PendingExport>,
    /// Shared signal state, used to cancel ffmpeg if interrupted during muxing.
    pub interrupt: Option<&'a Interrupt>,
}

async fn decide(action: Action, question: &str) -> Option<bool> {
    match action {
        Action::Do => Some(true),
        Action::DoNot => Some(false),
        Action::Ask => read_yes_no(question).await,
    }
}

impl Finish<'_> {
    async fn export_pending(&mut self) -> bool {
        let mut exported: Vec<PathBuf> = Vec::new();
        for pending in mem::take(&mut self.pending_exports) {
            let out = pending.out().to_path_buf();
            if let Err(error) = pending.export().await {
                log_error!("Failed to export downloaded data: {error}");
                // Free any earlier stream's exported .ts so a full disk isn't
                // left holding duplicates of the SQLite databases.
                for path in exported {
                    try_delete(&path);
                }
                return false;
            }
            exported.push(out);
        }
        true
    }

    async fn move_all(&self, items: &[&Option<Moved>]) -> bool {
        let mut ok = true;
        for moved in items.iter().copied().flatten() {
            // A move to another filesystem copies the whole file.
            let result = spawn_blocking({
                let moved = moved.clone();
                move || try_move(&moved.temp, &moved.dest)
            })
            .await;
            ok &= matches!(result, Ok(Ok(())));
        }
        ok
    }

    fn delete_state_files(&self) {
        if !self.opts.disable_save_state {
            for file in &self.state_files {
                try_delete(file);
            }
        }
    }

    /// `--disable-save-state` can't be combined with `--recover-private`, so
    /// a later run can always find kept data through the state files.
    fn retry_until(&self) -> Option<u64> {
        self.keep_for_retry
    }

    /// After merging: removes the temp dir, or keeps it for retrying.
    fn cleanup_or_keep(&self, moves_ok: bool) {
        let Some(until) = self.retry_until() else {
            self.cleanup_temp(moves_ok);
            return;
        };
        let until = if until == u64::MAX {
            String::new()
        } else {
            format!(" (before about {}, when the download URLs expire)", local_time(until))
        };
        log_warn!(
            "Some fragments are still missing. The downloaded data was kept in {} so they can be retried: run the same command again{until}.",
            self.plan.temp_dir.display()
        );
        log_warn!("Delete that folder if you don't want to retry.");
    }

    fn remove_temp_dir(&self) {
        if self.keep_other_streams {
            let ts: Vec<&Path> = self.audio_ts.iter().chain(&self.video_ts).map(|moved| moved.temp.as_path()).collect();
            // The thumbnail stays for the other stream's retry.
            let extra: Vec<&Path> = self
                .description
                .iter()
                .map(|moved| moved.temp.as_path())
                .chain([self.mux_file.temp.as_path()])
                .collect();
            self.plan.remove_stream_temp_files(self.video_id, &ts, &extra);
            return;
        }
        self.plan.remove_temp_dir(self.video_id);
    }

    fn cleanup_temp(&self, moves_ok: bool) {
        if moves_ok {
            self.remove_temp_dir();
        } else {
            log_error!("At least one error occurred when moving files. Will not delete them.");
        }
    }

    /// A stream's storage couldn't be opened, written to, or exported. Go
    /// still said "Download Finished" and ran ffmpeg; this doesn't merge,
    /// which would move the data away from its resume state and delete that
    /// state. With nothing downloaded (now or by an earlier run) the files are
    /// removed, otherwise left for resuming once the cause is fixed. Returns
    /// the exit code.
    pub fn on_storage_error(&mut self, had_data: bool) -> i32 {
        self.pending_exports.clear();
        if had_data {
            log_error!(
                "Not merging because a download's data couldn't be stored. The files were left in {} to resume later.",
                self.plan.temp_dir.display()
            );
        } else {
            log_error!("Nothing was downloaded because a download's storage couldn't be opened.");
            self.remove_temp_dir();
            self.delete_state_files();
        }
        1
    }

    pub fn on_terminate(&mut self) -> i32 {
        self.pending_exports.clear();
        log_warn!("Termination signal received. Files were left in {} to resume later.", self.plan.temp_dir.display());
        143
    }

    fn total_frags(&self) -> u64 {
        self.audio_frags.into_iter().chain(self.video_frags).sum()
    }

    /// Cleans up after a download that stored nothing, keeping only what a
    /// `--recover-private` retry needs.
    fn discard_empty(&mut self) {
        self.pending_exports.clear();
        if self.retry_until().is_some() {
            self.cleanup_or_keep(true);
        } else {
            self.remove_temp_dir();
            self.delete_state_files();
        }
    }

    fn keep_for_resume(&mut self) -> i32 {
        self.pending_exports.clear();
        log_warn!(
            "No answer could be read because stdin is closed. Files were left in {} to resume later.",
            self.plan.temp_dir.display()
        );
        2
    }

    /// Handles a user interrupt. Returns `Some(exit code)` if done, `None`
    /// if the user chose to merge what was downloaded.
    pub async fn on_cancel(&mut self) -> Option<i32> {
        // Nothing to merge or save, so there's nothing to ask; --merge would
        // otherwise report "No fragments were downloaded" and exit 1.
        if self.total_frags() == 0 {
            log_warn!("Download stopped before any fragment was downloaded.");
            self.discard_empty();
            return Some(2);
        }
        let options = self.opts;
        let Some(merge) = decide(
            options.merge_on_cancel,
            "\nDownload stopped prematurely. Would you like to merge the currently downloaded data?",
        )
        .await
        else {
            return Some(self.keep_for_resume());
        };
        if merge {
            return None;
        }
        let Some(save_files) =
            decide(options.save_files_on_cancel, "\nWould you like to save any created files?").await
        else {
            return Some(self.keep_for_resume());
        };
        let save_state = if save_files {
            false
        } else {
            let Some(answer) =
                decide(options.save_state_on_cancel, "\nWould you like to leave files to resume downloading later?")
                    .await
            else {
                return Some(self.keep_for_resume());
            };
            answer
        };

        if save_files {
            if !self.export_pending().await {
                log_error!(
                    "Not deleting temporary files because exporting failed. The files were left in {} to resume later.",
                    self.plan.temp_dir.display()
                );
                return Some(1);
            }
            let mux_file = Some(self.mux_file.clone());
            let ok =
                self.move_all(&[&self.audio_ts, &self.video_ts, &self.thumbnail, &self.description, &mux_file]).await;
            self.cleanup_temp(ok);
            if ok {
                self.delete_state_files();
            }
            return Some(if ok { 2 } else { 1 });
        } else {
            self.pending_exports.clear();
            if !save_state {
                self.remove_temp_dir();
                self.delete_state_files();
            }
        }
        Some(2)
    }
}

#[cfg(test)]
mod tests;
