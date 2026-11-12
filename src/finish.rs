//! After downloading: Ctrl-C choices (merge / keep files / keep resume
//! state), moving files to the final directory, and muxing.

use std::path::PathBuf;

use crate::download::local_time;
use crate::mux::{self, MuxCommand};
use crate::output::OutputPlan;
use crate::prompt::get_yes_no;
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
}

async fn decide(action: Action, question: &str) -> bool {
    match action {
        Action::Do => true,
        Action::DoNot => false,
        Action::Ask => get_yes_no(question).await,
    }
}

impl Finish<'_> {
    async fn move_all(&self, items: &[&Option<Moved>]) -> bool {
        let mut ok = true;
        for moved in items.iter().copied().flatten() {
            // A move to another filesystem copies the whole file.
            let result = tokio::task::spawn_blocking({
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

    fn cleanup_temp(&self, moves_ok: bool) {
        if moves_ok {
            self.plan.remove_temp_dir(self.video_id);
        } else {
            log_error!("At least one error occurred when moving files. Will not delete them.");
        }
    }

    /// A stream's storage couldn't be opened. Go still said "Download
    /// Finished" and ran ffmpeg on the missing file; this doesn't merge. With
    /// nothing downloaded (now or by an earlier run) the files are removed,
    /// otherwise left for resuming once the cause is fixed. Returns the exit code.
    pub fn on_storage_error(&self, had_data: bool) -> i32 {
        if had_data {
            log_error!(
                "Not merging because a download's storage couldn't be opened. The files were left in {} to resume later.",
                self.plan.temp_dir.display()
            );
        } else {
            log_error!("Nothing was downloaded because a download's storage couldn't be opened.");
            self.plan.remove_temp_dir(self.video_id);
            self.delete_state_files();
        }
        1
    }

    /// Handles a user interrupt. Returns `Some(exit code)` if done, `None`
    /// if the user chose to merge what was downloaded.
    pub async fn on_cancel(&self) -> Option<i32> {
        let options = self.opts;
        if decide(
            options.merge_on_cancel,
            "\nDownload stopped prematurely. Would you like to merge the currently downloaded data?",
        )
        .await
        {
            return None;
        }
        let save_files = decide(options.save_files_on_cancel, "\nWould you like to save any created files?").await;
        let save_state = !save_files
            && decide(options.save_state_on_cancel, "\nWould you like to leave files to resume downloading later?")
                .await;

        if save_files {
            let ok = self.move_all(&[&self.audio_ts, &self.video_ts, &self.thumbnail, &self.description]).await;
            self.cleanup_temp(ok);
            self.delete_state_files();
        } else if !save_state {
            self.plan.remove_temp_dir(self.video_id);
            self.delete_state_files();
        }
        Some(2)
    }

    /// Moves files into place and muxes. Returns the exit code.
    pub async fn finish(&self) -> i32 {
        let options = self.opts;
        if self.retry_until().is_none() {
            self.delete_state_files();
        }
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

        // Go kept the output name it chose before the download started.
        let mux = self.mux.with_unused_output();
        let audio_mux = self.audio_mux.as_ref().map(MuxCommand::with_unused_output);
        if mux.output != self.mux.output {
            let command = mux::command_line(&options.ffmpeg_path, &mux.args);
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

        log_general!("Muxing final file...");
        let mut code = mux::execute(&options.ffmpeg_path, &mux.args).await;
        if code != 0 {
            log_error!("ffmpeg returned code {code}. Something must have gone wrong with ffmpeg.");
            log_error!("The .ts files will not be deleted in case the final file is broken.");
            log_error!("The ffmpeg command was written to {}", self.mux_file.dest.display());
        }
        if let Some(audio_mux) = &audio_mux {
            log_general!("Creating separate audio file...");
            let audio_code = mux::execute(&options.ffmpeg_path, &audio_mux.args).await;
            if audio_code != 0 {
                code = audio_code;
                log_error!("ffmpeg returned code {audio_code} for the separate audio file.");
                log_error!("The .ts files will not be deleted in case the final file is broken.");
            }
        }

        self.cleanup_or_keep(moves_ok);
        if code != 0 {
            return code;
        }
        for file in &to_delete {
            try_delete(file);
        }
        // ffmpeg creates the final files with the default mode, so apply
        // --file-permissions afterwards. Go never did.
        let outputs = std::iter::once(&mux).chain(&audio_mux);
        for out in outputs.map(|command| &command.output) {
            apply_mode(out, options.file_mode);
        }
        log_general!("Final file: {}", mux.output.display());
        if let Some(audio_mux) = &audio_mux {
            log_general!("Final audio file: {}", audio_mux.output.display());
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::MuxInput;

    /// Ctrl-C with "keep the files": the downloaded data and thumbnail move
    /// to the final folder, a file that was never created is skipped, and
    /// the temp folder and state file are removed.
    #[tokio::test]
    async fn cancel_keeping_files_moves_them_and_cleans_up() {
        let final_dir = std::env::temp_dir().join(format!("ytarchive-rs-finish-{}", std::process::id()));
        let temp_dir = final_dir.join("vid__1a2b");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let plan = OutputPlan { final_dir: final_dir.clone(), file_name: "out".into(), temp_dir };
        let moved = |suffix: &str| Moved { temp: plan.temp(suffix), dest: plan.final_path(suffix) };
        let state_file = final_dir.join("vid.f140.state");
        std::fs::write(&state_file, b"{}").unwrap();
        std::fs::write(plan.temp(".f140.ts"), b"audio").unwrap();
        std::fs::write(plan.temp(".jpg"), b"thumbnail").unwrap();

        let opts = Options { merge_on_cancel: Action::DoNot, save_files_on_cancel: Action::Do, ..Default::default() };
        let audio = plan.final_path(".f140.ts");
        let finish = Finish {
            opts: &opts,
            plan: &plan,
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
                out_dir: &final_dir,
                file_name: "out",
            }),
            audio_mux: None,
            state_files: vec![state_file.clone()],
            audio_frags: Some(1),
            video_frags: None,
            keep_for_retry: None,
        };

        assert_eq!(finish.on_cancel().await, Some(2));
        assert_eq!(std::fs::read(&audio).unwrap(), b"audio");
        assert_eq!(std::fs::read(plan.final_path(".jpg")).unwrap(), b"thumbnail");
        assert!(!plan.final_path(".f299.ts").exists());
        assert!(!plan.temp_dir.exists());
        assert!(!state_file.exists());
        std::fs::remove_dir_all(&final_dir).unwrap();
    }
}
