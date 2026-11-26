use std::fs;

use super::*;

/// One folder per test with no shared parent, so removing it leaves nothing
/// behind and can't touch a test running in parallel.
fn dir(test: &str) -> PathBuf {
    std::env::temp_dir().join(format!("ytarchive-rs-mux-{}-{test}", std::process::id()))
}

#[test]
fn thumbnail_and_container_per_output() {
    let meta: MetaInfo = [("title".to_string(), "T".to_string()), ("date".into(), String::new())].into();
    // Audio only is always m4a with the thumbnail as a picture, even with --mkv.
    for (case, video, mkv, metadata, output, present, absent) in [
        (
            "mp4",
            true,
            false,
            Some(&meta),
            "out.mp4",
            &[
                "-hide_banner -nostdin -loglevel fatal -stats -i t.jpg -seekable 0",
                "-seekable 0 -i a.ts -seekable 0 -i v.ts -movflags faststart -map 0 -map 1 -map 2 -c copy -disposition:v:0 attached_pic",
                "-metadata TITLE=T",
            ][..],
            &["DATE"][..],
        ),
        ("mkv", true, true, None, "out.mkv", &["-attach t.jpg"][..], &["-i t.jpg", "faststart"][..]),
        (
            "m4a",
            false,
            true,
            None,
            "out.m4a",
            &["-i t.jpg", "-disposition:v:0 attached_pic"][..],
            &["-attach", "-movflags"][..],
        ),
    ] {
        let out_dir = dir(case);
        let cmd = ffmpeg_args(&MuxInput {
            audio: Some(Path::new("a.ts")),
            video: video.then(|| Path::new("v.ts")),
            thumbnail: Some(Path::new("t.jpg")),
            mkv,
            metadata,
            out_dir: &out_dir,
            file_name: "out",
        });
        let joined = cmd.args.join(" ");
        assert_eq!(cmd.output, out_dir.join(output));
        assert!(present.iter().all(|part| joined.contains(part)), "{case}: {joined}");
        assert!(!absent.iter().any(|part| joined.contains(part)), "{case}: {joined}");
        // ffmpeg 9 rejects -thread_queue_size before an input (exit code 234).
        assert!(!joined.contains("-thread_queue_size"), "{case}: {joined}");
    }
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
fn quoting() {
    let posix: fn(&str) -> String = posix_quote;
    let windows: fn(&str) -> String = windows_quote;
    let cases = [
        ("posix", posix, "a.ts", "a.ts"),
        ("posix", posix, "my file's.ts", r"'my file'\''s.ts'"),
        ("posix", posix, "COMMENT=line1\nline2", "'COMMENT=line1 line2'"),
        ("posix", posix, "COMMENT=line1\r\nline2\rline3", "'COMMENT=line1 line2 line3'"),
        ("posix", posix, "", "''"),
        ("windows", windows, r"C:\dir\a.ts", r"C:\dir\a.ts"),
        ("windows", windows, r"C:\my dir\a.ts", r#""C:\my dir\a.ts""#),
        ("windows", windows, r"C:\my dir\", r#""C:\my dir\\""#),
        ("windows", windows, r#"say "hi""#, r#""say \"hi\"""#),
        ("windows", windows, "TITLE=a&b", r#""TITLE=a&b""#),
        ("windows", windows, "COMMENT=line1\nline2", r#""COMMENT=line1 line2""#),
        ("windows", windows, "COMMENT=line1\r\nline2\rline3", r#""COMMENT=line1 line2 line3""#),
        ("windows", windows, "a\\\nb", r#""a\ b""#),
        ("windows", windows, "", r#""""#),
    ];
    for (style, quote, input, expected) in cases {
        assert_eq!(quote(input), expected, "{style}: {input:?}");
    }
}

#[tokio::test]
async fn execute_returns_cancelled_when_token_is_cancelled() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(execute("nonexistent-ffmpeg-binary", &[], &cancel).await, MuxExit::Cancelled);
}
