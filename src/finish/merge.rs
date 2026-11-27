//! The normal end of a download: moving files into place and muxing.

use tokio_util::sync::CancellationToken;

use super::Finish;
use crate::interrupt::Interrupt;
use crate::mux::{self, MuxCommand, MuxExit};
use crate::util::{apply_mode, try_delete, write_file_with_mode};
use crate::{log_debug, log_error, log_general, log_warn};

impl Finish<'_> {
    /// Moves files into place and muxes. Returns the exit code.
    pub async fn finish(&mut self) -> i32 {
        if self.total_frags() == 0 {
            log_error!("No fragments were downloaded.");
            self.discard_empty();
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
        // A retry run reuses it: the thumbnail URL often 404s once the stream
        // is private. That includes the run for another kept stream.
        if (self.retry_until().is_some() || self.keep_other_streams)
            && let Some(thumbnail) = &self.thumbnail
            && let Err(error) = tokio::fs::copy(&thumbnail.dest, &thumbnail.temp).await
        {
            log_debug!("Could not keep the thumbnail for a retry: {error}");
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
