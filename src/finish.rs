//! After downloading: Ctrl-C choices (merge / keep files / keep resume
//! state), moving files to the final directory, and muxing.

use std::path::PathBuf;

use crate::mux::{self, MuxCommand};
use crate::output::OutputPlan;
use crate::prompt::get_yes_no;
use crate::types::{Action, Options};
use crate::util::{try_delete, try_move};
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
}

async fn decide(action: Action, question: &str) -> bool {
    match action {
        Action::Do => true,
        Action::DoNot => false,
        Action::Ask => get_yes_no(question).await,
    }
}

impl Finish<'_> {
    fn move_all(&self, items: &[&Option<Moved>]) -> bool {
        let mut ok = true;
        for m in items.iter().copied().flatten() {
            ok &= try_move(&m.temp, &m.dest).is_ok();
        }
        ok
    }

    fn delete_state_files(&self) {
        if !self.opts.disable_save_state {
            for f in &self.state_files {
                try_delete(f);
            }
        }
    }

    fn cleanup_temp(&self, moves_ok: bool) {
        if moves_ok {
            self.plan.remove_temp_dir(self.video_id);
        } else {
            log_error!("At least one error occurred when moving files. Will not delete them.");
        }
    }

    /// Handles a user interrupt. Returns `Some(exit code)` if done, `None`
    /// if the user chose to merge what was downloaded.
    pub async fn on_cancel(&self) -> Option<i32> {
        let o = self.opts;
        if decide(o.merge_on_cancel, "\nDownload stopped prematurely. Would you like to merge the currently downloaded data?").await {
            return None;
        }
        let save_files = decide(o.save_files_on_cancel, "\nWould you like to save any created files?").await;
        let save_state = !save_files
            && decide(o.save_state_on_cancel, "\nWould you like to leave files to resume downloading later?").await;

        if save_files {
            let ok = self.move_all(&[&self.audio_ts, &self.video_ts, &self.thumbnail, &self.description]);
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
        let o = self.opts;
        self.delete_state_files();
        log_general!("Download Finished");

        if let (Some(a), Some(v)) = (self.audio_frags, self.video_frags)
            && a != v
        {
            log_warn!("Mismatched number of video ({v}) and audio ({a}) fragments.");
            log_warn!("The files should still be mergeable but data might be missing.");
        }

        let mux_file = Some(self.mux_file.clone());
        let moves_ok =
            self.move_all(&[&self.audio_ts, &self.video_ts, &self.thumbnail, &self.description, &mux_file]);

        let mut to_delete = vec![self.mux_file.dest.clone()];
        if !o.keep_ts_files {
            to_delete.extend(self.audio_ts.iter().chain(&self.video_ts).map(|m| m.dest.clone()));
        }
        if !o.write_thumbnail {
            to_delete.extend(self.thumbnail.iter().map(|m| m.dest.clone()));
        }

        if o.write_mux_file {
            self.cleanup_temp(moves_ok);
            return if moves_ok { 0 } else { 1 };
        }

        if !mux::program_exists(&o.ffmpeg_path) {
            log_error!(
                "{} not found. Please install ffmpeg or provide a location using --ffmpeg-path",
                o.ffmpeg_path
            );
            self.cleanup_temp(moves_ok);
            return 1;
        }

        log_general!("Muxing final file...");
        let mut code = mux::execute(&o.ffmpeg_path, &self.mux.args).await;
        if code != 0 {
            log_error!("ffmpeg returned code {code}. Something must have gone wrong with ffmpeg.");
            log_error!("The .ts files will not be deleted in case the final file is broken.");
            log_error!("The ffmpeg command was written to {}", self.mux_file.dest.display());
        }
        if let Some(am) = &self.audio_mux {
            log_general!("Creating separate audio file...");
            let ac = mux::execute(&o.ffmpeg_path, &am.args).await;
            if ac != 0 {
                code = ac;
                log_error!("ffmpeg returned code {ac} for the separate audio file.");
                log_error!("The .ts files will not be deleted in case the final file is broken.");
            }
        }

        self.cleanup_temp(moves_ok);
        if code != 0 {
            return code;
        }
        for f in &to_delete {
            try_delete(f);
        }
        log_general!("Final file: {}", self.mux.output.display());
        if let Some(am) = &self.audio_mux {
            log_general!("Final audio file: {}", am.output.display());
        }
        0
    }
}
