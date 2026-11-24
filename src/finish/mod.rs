//! After downloading: Ctrl-C choices (merge / keep files / keep resume
//! state), moving files to the final directory, and muxing.

use std::mem;
use std::path::PathBuf;

use tokio::task::spawn_blocking;
use tokio_util::sync::CancellationToken;

use crate::download::{PendingExport, local_time};
use crate::interrupt::Interrupt;
use crate::mux::{self, MuxCommand, MuxExit};
use crate::output::{OutputPlan, other_state_uses_temp_dir};
use crate::prompt::read_yes_no;
use crate::types::{Action, Options};
use crate::util::{apply_mode, try_delete, try_move, write_file_with_mode};
use crate::{log_error, log_general, log_warn};

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

    /// `keep_for_retry`, unless state files are disabled (a later run
    /// couldn't find the data then).
    fn retry_until(&self) -> Option<u64> {
        self.keep_for_retry.filter(|_| !self.opts.disable_save_state)
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
        let mut dirs = vec![self.plan.temp_dir.clone()];
        for moved in [&self.audio_ts, &self.video_ts].into_iter().flatten() {
            if let Some(parent) = moved.temp.parent()
                && !dirs.iter().any(|dir| dir == parent)
            {
                dirs.push(parent.to_path_buf());
            }
        }
        for dir in dirs {
            if other_state_uses_temp_dir(self.opts, self.video_id, &dir, &self.state_files) {
                log_warn!("Leaving {} because another stream's resume state still references it.", dir.display());
                continue;
            }
            OutputPlan { temp_dir: dir, ..self.plan.clone() }.remove_temp_dir(self.video_id);
        }
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

    /// Moves files into place and muxes. Returns the exit code.
    pub async fn finish(&mut self) -> i32 {
        let total_frags: u64 = self.audio_frags.into_iter().chain(self.video_frags).sum();
        if total_frags == 0 {
            self.pending_exports.clear();
            log_error!("No fragments were downloaded.");
            if self.retry_until().is_some() {
                self.cleanup_or_keep(true);
            } else {
                self.remove_temp_dir();
                self.delete_state_files();
            }
            return 1;
        }
        if !self.export_pending().await {
            return self.on_storage_error(true);
        }
        let options = self.opts;
        log_general!("Download Finished");

        if let (Some(audio), Some(value)) = (self.audio_frags, self.video_frags)
            && audio != value
        {
            log_warn!("Mismatched number of video ({value}) and audio ({audio}) fragments.");
            log_warn!("The files should still be mergeable but data might be missing.");
        }

        let mux_file = Some(self.mux_file.clone());
        let moves_ok =
            self.move_all(&[&self.audio_ts, &self.video_ts, &self.thumbnail, &self.description, &mux_file]).await;
        if self.retry_until().is_none() && moves_ok {
            self.delete_state_files();
        }

        // Go kept the output name it chose before the download started.
        let mux = self.mux.with_unused_output();
        let audio_mux = self.audio_mux.as_ref().map(MuxCommand::with_unused_output);
        let output_changed = mux.output != self.mux.output
            || audio_mux.as_ref().zip(self.audio_mux.as_ref()).is_some_and(|(next, prev)| next.output != prev.output);
        if output_changed {
            let command = mux::mux_file_contents(&options.ffmpeg_path, &mux, audio_mux.as_ref());
            if let Err(error) = write_file_with_mode(&self.mux_file.dest, command.as_bytes(), options.file_mode).await {
                log_warn!("Failed to update the mux file: {error}");
            }
        }

        let mut to_delete = vec![self.mux_file.dest.clone()];
        if !options.keep_ts_files {
            to_delete.extend(self.audio_ts.iter().chain(&self.video_ts).map(|moved| moved.dest.clone()));
        }
        if !options.write_thumbnail {
            to_delete.extend(self.thumbnail.iter().map(|moved| moved.dest.clone()));
        }

        if options.write_mux_file {
            self.cleanup_or_keep(moves_ok);
            return if moves_ok { 0 } else { 1 };
        }
        if !moves_ok {
            log_error!("Not muxing because moving downloaded files failed.");
            self.cleanup_or_keep(false);
            return 1;
        }

        if !mux::program_exists(&options.ffmpeg_path) {
            log_error!(
                "{} not found. Please install ffmpeg or provide a location using --ffmpeg-path",
                options.ffmpeg_path
            );
            self.cleanup_or_keep(moves_ok);
            return 1;
        }

        // Go ran ffmpeg anyway; it refuses to overwrite and only reports
        // an exit code.
        let outputs = std::iter::once(&mux).chain(&audio_mux);
        if let Some(taken) = outputs.map(|command| &command.output).find(|output| output.exists()) {
            log_error!(
                "Not muxing: {} and every numbered name before it already exist. Move or delete one of them.",
                taken.display()
            );
            log_error!("The .ts files were kept. The ffmpeg command was written to {}", self.mux_file.dest.display());
            self.cleanup_or_keep(moves_ok);
            return 1;
        }

        // When merging after a user Ctrl-C, the shared token is already
        // cancelled; only cancel muxing on a fresh signal during muxing.
        let mux_cancel = match self.interrupt {
            Some(interrupt) if !interrupt.is_set() => interrupt.child_token(),
            _ => CancellationToken::new(),
        };
        let cancel_code = || self.interrupt.map_or(2, Interrupt::cancel_code);

        log_general!("Muxing final file...");
        let mut code = match mux::execute(&options.ffmpeg_path, &mux.args, &mux_cancel).await {
            MuxExit::Cancelled => {
                log_warn!("Muxing interrupted; keeping .ts files.");
                try_delete(&mux.output);
                self.cleanup_or_keep(moves_ok);
                return cancel_code();
            }
            MuxExit::Code(code) => code,
        };
        if code == 0 {
            // ffmpeg creates the final files with the default mode, so apply
            // --file-permissions afterwards. Go never did.
            apply_mode(&mux.output, options.file_mode);
        } else {
            log_error!("ffmpeg returned code {code}. Something must have gone wrong with ffmpeg.");
            log_error!("The .ts files will not be deleted in case the final file is broken.");
            log_error!("The ffmpeg command was written to {}", self.mux_file.dest.display());
        }
        if let Some(audio_mux) = &audio_mux {
            log_general!("Creating separate audio file...");
            let audio_code = match mux::execute(&options.ffmpeg_path, &audio_mux.args, &mux_cancel).await {
                MuxExit::Cancelled => {
                    log_warn!("Muxing interrupted; keeping .ts files.");
                    try_delete(&audio_mux.output);
                    self.cleanup_or_keep(moves_ok);
                    return cancel_code();
                }
                MuxExit::Code(audio_code) => audio_code,
            };
            if audio_code == 0 {
                apply_mode(&audio_mux.output, options.file_mode);
            } else {
                code = audio_code;
                log_error!("ffmpeg returned code {audio_code} for the separate audio file.");
                log_error!("The .ts files will not be deleted in case the final file is broken.");
                let commands = mux::mux_file_contents(&options.ffmpeg_path, &mux, Some(audio_mux));
                if let Err(error) =
                    write_file_with_mode(&self.mux_file.dest, commands.as_bytes(), options.file_mode).await
                {
                    log_warn!("Failed to update the mux file: {error}");
                }
                log_error!("The ffmpeg commands were written to {}", self.mux_file.dest.display());
            }
        }

        self.cleanup_or_keep(moves_ok);
        if code != 0 {
            return code;
        }
        for file in &to_delete {
            try_delete(file);
        }
        log_general!("Final file: {}", mux.output.display());
        if let Some(audio_mux) = &audio_mux {
            log_general!("Final audio file: {}", audio_mux.output.display());
        }
        0
    }
}

#[cfg(test)]
mod tests;
