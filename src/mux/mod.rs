//! Building and running the ffmpeg command that muxes the downloaded
//! audio/video into the final file.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::log;
use crate::types::{LogLevel, MetaInfo};
use crate::util::resolve_program;
use crate::{log_debug, log_error};

pub struct MuxInput<'a> {
    pub audio: Option<&'a Path>,
    pub video: Option<&'a Path>,
    /// Thumbnail to embed (`--thumbnail`), if downloaded.
    pub thumbnail: Option<&'a Path>,
    pub mkv: bool,
    /// Written when `--add-metadata` is set; empty values are skipped.
    pub metadata: Option<&'a MetaInfo>,
    pub out_dir: &'a Path,
    pub file_name: &'a str,
}

pub struct MuxCommand {
    /// The output path is the last argument.
    pub args: Vec<String>,
    pub output: PathBuf,
    out_dir: PathBuf,
    file_name: String,
    extension: &'static str,
}

impl MuxCommand {
    /// The same command with the output name picked again. The command is
    /// built before the download, which can take hours; a file created under
    /// that name in the meantime would make ffmpeg refuse to write.
    pub fn with_unused_output(&self) -> Self {
        let output = unused_output(&self.out_dir, &self.file_name, self.extension);
        let mut args = self.args.clone();
        if let Some(last) = args.last_mut() {
            *last = path_text(&output);
        }
        Self {
            args,
            output,
            out_dir: self.out_dir.clone(),
            file_name: self.file_name.clone(),
            extension: self.extension,
        }
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// First of `name.ext`, `name-1.ext` ... `name-10.ext` that doesn't exist.
fn unused_output(dir: &Path, name: &str, ext: &str) -> PathBuf {
    let mut out = dir.join(format!("{name}.{ext}"));
    let mut number = 0;
    while out.exists() && number < 10 {
        number += 1;
        out = dir.join(format!("{name}-{number}.{ext}"));
    }
    out
}

pub fn ffmpeg_args(mux_input: &MuxInput<'_>) -> MuxCommand {
    let audio_only = mux_input.video.is_none();
    let mkv = mux_input.mkv && !audio_only;
    let mut args: Vec<String> = ["-hide_banner", "-nostdin", "-loglevel", "fatal", "-stats"].map(String::from).to_vec();

    // mp4/m4a take the thumbnail as an input stream; mkv attaches it.
    let thumb_input = mux_input.thumbnail.filter(|_| !mkv);
    if let Some(thumbnail) = thumb_input {
        args.extend(["-i".into(), path_text(thumbnail)]);
    }

    let ext = if audio_only {
        "m4a"
    } else if mkv {
        "mkv"
    } else {
        "mp4"
    };
    let output = unused_output(mux_input.out_dir, mux_input.file_name, ext);

    for input in [mux_input.audio, mux_input.video].into_iter().flatten() {
        args.extend(["-seekable", "0", "-i"].map(String::from));
        args.push(path_text(input));
    }
    if mux_input.video.is_some() {
        if !mkv {
            args.extend(["-movflags".into(), "faststart".into()]);
        }
        if thumb_input.is_some() {
            let inputs = 1 + usize::from(mux_input.audio.is_some()) + 1;
            for index in 0..inputs {
                args.extend(["-map".into(), index.to_string()]);
            }
        }
    }

    args.extend(["-c".into(), "copy".into()]);
    if let Some(thumbnail) = mux_input.thumbnail {
        if mkv {
            args.extend([
                "-attach".into(),
                path_text(thumbnail),
                "-metadata:s:t".into(),
                "filename=cover_land.jpg".into(),
                "-metadata:s:t".into(),
                "mimetype=image/jpeg".into(),
            ]);
        } else {
            args.extend(["-disposition:v:0".into(), "attached_pic".into()]);
        }
    }
    if let Some(meta) = mux_input.metadata {
        for (key, value) in meta.iter().filter(|(_, value)| !value.is_empty()) {
            args.extend(["-metadata".into(), format!("{}={value}", key.to_uppercase())]);
        }
    }
    args.push(path_text(&output));
    MuxCommand {
        args,
        output,
        out_dir: mux_input.out_dir.to_path_buf(),
        file_name: mux_input.file_name.to_string(),
        extension: ext,
    }
}

/// Replaces `\r\n`, `\r`, and `\n` with spaces so embedded newlines (e.g. in
/// `COMMENT` metadata) stay on one line in `.ffmpeg.txt`.
fn flatten_newlines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_cr = false;
    for character in text.chars() {
        if character == '\n' && prev_cr {
            prev_cr = false;
            continue;
        }
        prev_cr = character == '\r';
        out.push(if matches!(character, '\r' | '\n') { ' ' } else { character });
    }
    out
}

/// POSIX shell quoting, for the human-readable `.ffmpeg.txt` command on
/// Linux/macOS.
pub fn posix_quote(arg: &str) -> String {
    let flat = flatten_newlines(arg);
    let safe = !flat.is_empty()
        && flat.chars().all(|character| character.is_ascii_alphanumeric() || "-_./=:,+@%".contains(character));
    if safe { flat } else { format!("'{}'", flat.replace('\'', r"'\''")) }
}

/// Windows command-line quoting (the rules ffmpeg's argument parser uses),
/// so the `.ffmpeg.txt` command can be pasted into cmd or PowerShell.
pub fn windows_quote(arg: &str) -> String {
    let flat = flatten_newlines(arg);
    if !flat.is_empty() && !flat.contains([' ', '\t', '"', '&', '|', '<', '>', '^']) {
        return flat;
    }
    let mut out = String::from("\"");
    let mut backslashes = 0;
    for character in flat.chars() {
        match character {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(backslashes));
                out.push(character);
                backslashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

pub fn command_line(program: &str, args: &[String]) -> String {
    let quote = if cfg!(windows) { windows_quote } else { posix_quote };
    std::iter::once(program).chain(args.iter().map(String::as_str)).map(quote).collect::<Vec<_>>().join(" ")
}

/// Contents of `.ffmpeg.txt`: the main mux command, plus the separate audio
/// command on a second line when `--separate-audio` is active.
pub fn mux_file_contents(program: &str, mux: &MuxCommand, audio_mux: Option<&MuxCommand>) -> String {
    match audio_mux {
        Some(audio) => format!("{}\n{}", command_line(program, &mux.args), command_line(program, &audio.args)),
        None => command_line(program, &mux.args),
    }
}

/// Whether `program` can be run: a path that exists, a file next to the
/// ytarchive executable, or a name on PATH.
pub fn program_exists(program: &str) -> bool {
    resolve_program(program).is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuxExit {
    /// Process exited with this code (-1 if killed externally or failed to start).
    Code(i32),
    /// Cancelled by an interrupt/termination signal while muxing.
    Cancelled,
}

/// Runs ffmpeg, passing its stderr (progress stats) through (or silencing both
/// stdout and stderr under `--quiet`). Kills ffmpeg immediately if `cancel`
/// triggers.
pub async fn execute(program: &str, args: &[String], cancel: &CancellationToken) -> MuxExit {
    if cancel.is_cancelled() {
        return MuxExit::Cancelled;
    }
    log_debug!("Executing command: {}", command_line(program, args));
    let executable = resolve_program(program).unwrap_or_else(|| PathBuf::from(program));
    let quiet = log::level() <= LogLevel::Quiet;
    let stdout = if quiet { Stdio::null() } else { Stdio::inherit() };
    let stderr = if quiet { Stdio::null() } else { Stdio::inherit() };
    let mut child = match Command::new(&executable)
        .args(args)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            log_error!("{error}");
            return MuxExit::Code(-1);
        }
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            if let Err(error) = child.kill().await {
                log_debug!("Could not kill ffmpeg after interrupt: {error}");
            }
            MuxExit::Cancelled
        }
        status = child.wait() => match status {
            Ok(status) => MuxExit::Code(status.code().unwrap_or(-1)),
            Err(error) => {
                log_error!("{error}");
                MuxExit::Code(-1)
            }
        }
    }
}

#[cfg(test)]
mod tests;
