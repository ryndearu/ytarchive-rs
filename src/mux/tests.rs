use std::fs;

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
    fs::create_dir_all(&out_dir).unwrap();
    fs::write(out_dir.join("out.mp4"), b"").unwrap();
    assert_eq!(unused_output(&out_dir, "out", "mp4"), out_dir.join("out-1.mp4"));
    fs::remove_dir_all(&out_dir).unwrap();
}

#[test]
fn output_name_is_picked_again_before_muxing() {
    let out_dir = dir("taken-later");
    fs::create_dir_all(&out_dir).unwrap();
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

    fs::write(&cmd.output, b"").unwrap();
    let later = cmd.with_unused_output();
    assert_eq!(later.output, out_dir.join("out-1.mp4"));
    assert_eq!(later.args.last(), Some(&path_text(&later.output)));
    assert_eq!(later.args[..later.args.len() - 1], cmd.args[..cmd.args.len() - 1]);
    fs::remove_dir_all(&out_dir).unwrap();
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
    assert_eq!(windows_quote("COMMENT=line1\nline2"), r#""COMMENT=line1 line2""#);
    assert_eq!(windows_quote("COMMENT=line1\r\nline2\rline3"), r#""COMMENT=line1 line2 line3""#);
    assert_eq!(windows_quote(""), r#""""#);
}

#[tokio::test]
async fn execute_returns_cancelled_when_token_is_cancelled() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(execute("nonexistent-ffmpeg-binary", &[], &cancel).await, MuxExit::Cancelled);
}
