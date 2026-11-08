//! Building and running the ffmpeg command that muxes the downloaded
//! audio/video into the final file.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::AsyncReadExt;

use crate::types::MetaInfo;
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
    pub args: Vec<String>,
    pub output: PathBuf,
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// First of `name.ext`, `name-1.ext` ... `name-10.ext` that doesn't exist.
fn unused_output(dir: &Path, name: &str, ext: &str) -> PathBuf {
    let mut out = dir.join(format!("{name}.{ext}"));
    let mut n = 0;
    while out.exists() && n < 10 {
        n += 1;
        out = dir.join(format!("{name}-{n}.{ext}"));
    }
    out
}

pub fn ffmpeg_args(m: &MuxInput<'_>) -> MuxCommand {
    let audio_only = m.video.is_none();
    let mut a: Vec<String> = ["-hide_banner", "-nostdin", "-loglevel", "fatal", "-stats"]
        .map(String::from)
        .to_vec();

    // mp4/m4a take the thumbnail as an input stream; mkv attaches it.
    let thumb_input = m.thumbnail.filter(|_| !m.mkv);
    if let Some(t) = thumb_input {
        a.extend(["-i".into(), s(t)]);
    }

    let ext = if audio_only {
        "m4a"
    } else if m.mkv {
        "mkv"
    } else {
        "mp4"
    };
    let output = unused_output(m.out_dir, m.file_name, ext);

    for input in [m.audio, m.video].into_iter().flatten() {
        a.extend(["-seekable", "0", "-thread_queue_size", "1024", "-i"].map(String::from));
        a.push(s(input));
    }
    if m.video.is_some() {
        if !m.mkv {
            a.extend(["-movflags".into(), "faststart".into()]);
        }
        if thumb_input.is_some() {
            let inputs = 1 + usize::from(m.audio.is_some()) + 1;
            for i in 0..inputs {
                a.extend(["-map".into(), i.to_string()]);
            }
        }
    }

    a.extend(["-c".into(), "copy".into()]);
    if let Some(t) = m.thumbnail {
        if m.mkv {
            a.extend([
                "-attach".into(),
                s(t),
                "-metadata:s:t".into(),
                "filename=cover_land.jpg".into(),
                "-metadata:s:t".into(),
                "mimetype=image/jpeg".into(),
            ]);
        } else {
            a.extend(["-disposition:v:0".into(), "attached_pic".into()]);
        }
    }
    if let Some(meta) = m.metadata {
        for (k, v) in meta.iter().filter(|(_, v)| !v.is_empty()) {
            a.extend(["-metadata".into(), format!("{}={v}", k.to_uppercase())]);
        }
    }
    a.push(s(&output));
    MuxCommand { args: a, output }
}

/// POSIX shell quoting, for the human-readable `.ffmpeg.txt` command on
/// Linux/macOS.
pub fn posix_quote(arg: &str) -> String {
    let safe = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c));
    if safe {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// Windows command-line quoting (the rules ffmpeg's argument parser uses),
/// so the `.ffmpeg.txt` command can be pasted into cmd or PowerShell.
pub fn windows_quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"', '&', '|', '<', '>', '^']) {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(backslashes));
                out.push(c);
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
    std::iter::once(program)
        .chain(args.iter().map(String::as_str))
        .map(quote)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `program` can be run: a path that exists, a name on PATH, or (as
/// Windows users often do) a file next to the ytarchive executable.
pub fn program_exists(program: &str) -> bool {
    let p = Path::new(program);
    if p.components().count() > 1 {
        return p.is_file() || cfg!(windows) && p.with_extension("exe").is_file();
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf));
    let path_dirs = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .unwrap_or_default();
    exe_dir.into_iter().chain(path_dirs).any(|dir| {
        let c = dir.join(program);
        c.is_file() || cfg!(windows) && c.with_extension("exe").is_file()
    }) || p.is_file()
}

/// Runs ffmpeg, passing its stderr (progress stats) through. Returns the exit
/// code, or -1 if it couldn't be run.
pub async fn execute(program: &str, args: &[String]) -> i32 {
    log_debug!("Executing command: {}", command_line(program, args));
    let mut child = match tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            log_error!("{e}");
            return -1;
        }
    };
    if let Some(mut err) = child.stderr.take() {
        let mut buf = [0u8; 2048];
        while let Ok(n) = err.read(&mut buf).await {
            if n == 0 {
                break;
            }
            eprint!("{}", String::from_utf8_lossy(&buf[..n]));
        }
    }
    match child.wait().await {
        Ok(st) => st.code().unwrap_or(-1),
        Err(e) => {
            log_error!("{e}");
            -1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        std::env::temp_dir().join(format!("ytarchive-rs-mux-{}", std::process::id()))
    }

    #[test]
    fn audio_and_video_mp4_with_thumbnail_and_metadata() {
        let d = dir();
        let meta: MetaInfo = [("title".to_string(), "T".to_string()), ("date".into(), String::new())].into();
        let cmd = ffmpeg_args(&MuxInput {
            audio: Some(Path::new("a.ts")),
            video: Some(Path::new("v.ts")),
            thumbnail: Some(Path::new("t.jpg")),
            mkv: false,
            metadata: Some(&meta),
            out_dir: &d,
            file_name: "out",
        });
        let a = cmd.args.join(" ");
        assert!(a.starts_with("-hide_banner -nostdin -loglevel fatal -stats -i t.jpg -seekable 0"));
        assert!(a.contains("-i a.ts -seekable 0 -thread_queue_size 1024 -i v.ts -movflags faststart -map 0 -map 1 -map 2 -c copy -disposition:v:0 attached_pic"));
        assert!(a.contains("-metadata TITLE=T"));
        assert!(!a.contains("DATE"));
        assert_eq!(cmd.output, d.join("out.mp4"));
    }

    #[test]
    fn mkv_attaches_thumbnail() {
        let d = dir();
        let cmd = ffmpeg_args(&MuxInput {
            audio: Some(Path::new("a.ts")),
            video: Some(Path::new("v.ts")),
            thumbnail: Some(Path::new("t.jpg")),
            mkv: true,
            metadata: None,
            out_dir: &d,
            file_name: "out",
        });
        let a = cmd.args.join(" ");
        assert!(!a.contains("-i t.jpg"));
        assert!(!a.contains("faststart"));
        assert!(a.contains("-attach t.jpg"));
        assert!(cmd.output.ends_with("out.mkv"));
    }

    #[test]
    fn audio_only_is_m4a() {
        let d = dir();
        let cmd = ffmpeg_args(&MuxInput {
            audio: Some(Path::new("a.ts")),
            video: None,
            thumbnail: None,
            mkv: true,
            metadata: None,
            out_dir: &d,
            file_name: "out",
        });
        assert!(cmd.output.ends_with("out.m4a"));
        assert!(!cmd.args.contains(&"-movflags".to_string()));
    }

    #[test]
    fn avoids_existing_output() {
        let d = dir().join("exists");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("out.mp4"), b"").unwrap();
        assert_eq!(unused_output(&d, "out", "mp4"), d.join("out-1.mp4"));
        std::fs::remove_dir_all(&d).unwrap();
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
