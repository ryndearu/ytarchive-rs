//! Building and running the ffmpeg command that muxes the downloaded
//! audio/video into the final file.

use std::ffi::OsString;
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
    pub args: Vec<OsString>,
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
            *last = output.as_os_str().to_os_string();
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
    let mut args: Vec<OsString> =
        ["-hide_banner", "-nostdin", "-loglevel", "fatal", "-stats"].map(OsString::from).to_vec();

    // mp4/m4a take the thumbnail as an input stream; mkv attaches it.
    let thumb_input = mux_input.thumbnail.filter(|_| !mkv);
    if let Some(thumbnail) = thumb_input {
        args.extend(["-i".into(), thumbnail.as_os_str().to_os_string()]);
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
        args.extend(["-seekable", "0", "-thread_queue_size", "1024", "-i"].map(OsString::from));
        args.push(input.as_os_str().to_os_string());
    }
    if mux_input.video.is_some() {
        if !mkv {
            args.extend(["-movflags".into(), "faststart".into()]);
        }
        if thumb_input.is_some() {
            let inputs = 1 + usize::from(mux_input.audio.is_some()) + 1;
            for index in 0..inputs {
                args.extend(["-map".into(), index.to_string().into()]);
            }
        }
    }

    args.extend(["-c".into(), "copy".into()]);
    if let Some(thumbnail) = mux_input.thumbnail {
        if mkv {
            args.extend([
                "-attach".into(),
                thumbnail.as_os_str().to_os_string(),
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
            args.extend(["-metadata".into(), format!("{}={value}", key.to_uppercase()).into()]);
        }
    }
    args.push(output.as_os_str().to_os_string());
    MuxCommand {
        args,
        output,
        out_dir: mux_input.out_dir.to_path_buf(),
        file_name: mux_input.file_name.to_string(),
        extension: ext,
    }
}

/// POSIX shell quoting, for the human-readable `.ffmpeg.txt` command on
/// Linux/macOS.
pub fn posix_quote(arg: &str) -> String {
    let safe = !arg.is_empty()
        && arg.chars().all(|character| character.is_ascii_alphanumeric() || "-_./=:,+@%".contains(character));
    if safe { arg.to_string() } else { format!("'{}'", arg.replace('\'', r"'\''")) }
}

/// Windows command-line quoting (the rules ffmpeg's argument parser uses),
/// so the `.ffmpeg.txt` command can be pasted into cmd or PowerShell.
/// Embedded newlines (e.g. in `COMMENT` metadata) are flattened to spaces so
/// each command stays on a single line.
pub fn windows_quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\r', '\n', '"', '&', '|', '<', '>', '^']) {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0;
    let mut prev_cr = false;
    for character in arg.chars() {
        if character == '\n' && prev_cr {
            prev_cr = false;
            continue;
        }
        prev_cr = character == '\r';
        match character {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            '\r' | '\n' => {
                out.push_str(&"\\".repeat(backslashes));
                out.push(' ');
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

pub fn command_line(program: &str, args: &[OsString]) -> String {
    let quote = if cfg!(windows) { windows_quote } else { posix_quote };
    std::iter::once(quote(program))
        .chain(args.iter().map(|arg| quote(&arg.to_string_lossy())))
        .collect::<Vec<_>>()
        .join(" ")
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
pub async fn execute(program: &str, args: &[OsString], cancel: &CancellationToken) -> MuxExit {
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
