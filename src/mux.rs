//! Building and running the ffmpeg command that muxes the downloaded
//! audio/video into the final file.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::AsyncReadExt;

use crate::types::MetaInfo;
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
        args.extend(["-seekable", "0", "-thread_queue_size", "1024", "-i"].map(String::from));
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

/// POSIX shell quoting, for the human-readable `.ffmpeg.txt` command on
/// Linux/macOS.
pub fn posix_quote(arg: &str) -> String {
    let safe = !arg.is_empty()
        && arg.chars().all(|character| character.is_ascii_alphanumeric() || "-_./=:,+@%".contains(character));
    if safe { arg.to_string() } else { format!("'{}'", arg.replace('\'', r"'\''")) }
}

/// Windows command-line quoting (the rules ffmpeg's argument parser uses),
/// so the `.ffmpeg.txt` command can be pasted into cmd or PowerShell.
pub fn windows_quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"', '&', '|', '<', '>', '^']) {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0;
    for character in arg.chars() {
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

/// Whether `program` can be run: a path that exists, a file next to the
/// ytarchive executable, or a name on PATH.
pub fn program_exists(program: &str) -> bool {
    resolve_program(program).is_some()
}

/// Runs ffmpeg, passing its stderr (progress stats) through. Returns the exit
/// code, or -1 if it couldn't be run.
pub async fn execute(program: &str, args: &[String]) -> i32 {
    log_debug!("Executing command: {}", command_line(program, args));
    let executable = resolve_program(program).unwrap_or_else(|| PathBuf::from(program));
    let mut child = match tokio::process::Command::new(&executable)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            log_error!("{error}");
            return -1;
        }
    };
    if let Some(mut err) = child.stderr.take() {
        let mut buf = [0u8; 2048];
        while let Ok(count) = err.read(&mut buf).await {
            if count == 0 {
                break;
            }
            eprint!("{}", String::from_utf8_lossy(buf.get(..count).unwrap_or(&buf)));
        }
    }
    match child.wait().await {
        Ok(status) => status.code().unwrap_or(-1),
        Err(error) => {
            log_error!("{error}");
            -1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One folder per test with no shared parent, so removing it leaves nothing
    /// behind and can't touch a test running in parallel.
    fn dir(test: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ytarchive-rs-mux-{}-{test}", std::process::id()))
    }

    #[test]
    fn audio_and_video_mp4_with_thumbnail_and_metadata() {
        let out_dir = dir("mp4");
        let meta: MetaInfo = [("title".to_string(), "T".to_string()), ("date".into(), String::new())].into();
        let cmd = ffmpeg_args(&MuxInput {
            audio: Some(Path::new("a.ts")),
            video: Some(Path::new("v.ts")),
            thumbnail: Some(Path::new("t.jpg")),
            mkv: false,
            metadata: Some(&meta),
            out_dir: &out_dir,
            file_name: "out",
        });
        let joined = cmd.args.join(" ");
        assert!(joined.starts_with("-hide_banner -nostdin -loglevel fatal -stats -i t.jpg -seekable 0"));
        assert!(joined.contains("-i a.ts -seekable 0 -thread_queue_size 1024 -i v.ts -movflags faststart -map 0 -map 1 -map 2 -c copy -disposition:v:0 attached_pic"));
        assert!(joined.contains("-metadata TITLE=T"));
        assert!(!joined.contains("DATE"));
        assert_eq!(cmd.output, out_dir.join("out.mp4"));
    }

    #[test]
    fn mkv_attaches_thumbnail() {
        let out_dir = dir("mkv");
        let cmd = ffmpeg_args(&MuxInput {
            audio: Some(Path::new("a.ts")),
            video: Some(Path::new("v.ts")),
            thumbnail: Some(Path::new("t.jpg")),
            mkv: true,
            metadata: None,
            out_dir: &out_dir,
            file_name: "out",
        });
        let joined = cmd.args.join(" ");
        assert!(!joined.contains("-i t.jpg"));
        assert!(!joined.contains("faststart"));
        assert!(joined.contains("-attach t.jpg"));
        assert!(cmd.output.ends_with("out.mkv"));
    }

    #[test]
    fn audio_only_is_m4a() {
        let out_dir = dir("m4a");
        let cmd = ffmpeg_args(&MuxInput {
            audio: Some(Path::new("a.ts")),
            video: None,
            thumbnail: Some(Path::new("t.jpg")),
            mkv: true,
            metadata: None,
            out_dir: &out_dir,
            file_name: "out",
        });
        assert!(cmd.output.ends_with("out.m4a"));
        let joined = cmd.args.join(" ");
        assert!(!cmd.args.contains(&"-movflags".to_string()));
        assert!(joined.contains("-i t.jpg"));
        assert!(joined.contains("-disposition:v:0 attached_pic"));
        assert!(!joined.contains("-attach"));
    }

    #[test]
    fn avoids_existing_output() {
        let out_dir = dir("exists");
        std::fs::create_dir_all(&out_dir).unwrap();
        std::fs::write(out_dir.join("out.mp4"), b"").unwrap();
        assert_eq!(unused_output(&out_dir, "out", "mp4"), out_dir.join("out-1.mp4"));
        std::fs::remove_dir_all(&out_dir).unwrap();
    }

    #[test]
    fn output_name_is_picked_again_before_muxing() {
        let out_dir = dir("taken-later");
        std::fs::create_dir_all(&out_dir).unwrap();
        let cmd = ffmpeg_args(&MuxInput {
            audio: Some(Path::new("a.ts")),
            video: Some(Path::new("v.ts")),
            thumbnail: None,
            mkv: false,
            metadata: None,
            out_dir: &out_dir,
            file_name: "out",
        });
        assert_eq!(cmd.with_unused_output().args, cmd.args);

        std::fs::write(&cmd.output, b"").unwrap();
        let later = cmd.with_unused_output();
        assert_eq!(later.output, out_dir.join("out-1.mp4"));
        assert_eq!(later.args.last(), Some(&path_text(&later.output)));
        assert_eq!(later.args[..later.args.len() - 1], cmd.args[..cmd.args.len() - 1]);
        std::fs::remove_dir_all(&out_dir).unwrap();
    }

    #[test]
    fn posix_quoting() {
        assert_eq!(posix_quote("a.ts"), "a.ts");
        assert_eq!(posix_quote("my file's.ts"), r"'my file'\''s.ts'");
        assert_eq!(posix_quote(""), "''");
    }

    #[test]
    fn windows_quoting() {
        assert_eq!(windows_quote(r"C:\dir\a.ts"), r"C:\dir\a.ts");
        assert_eq!(windows_quote(r"C:\my dir\a.ts"), r#""C:\my dir\a.ts""#);
        assert_eq!(windows_quote(r"C:\my dir\"), r#""C:\my dir\\""#);
        assert_eq!(windows_quote(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(windows_quote("TITLE=a&b"), r#""TITLE=a&b""#);
        assert_eq!(windows_quote(""), r#""""#);
    }
}
