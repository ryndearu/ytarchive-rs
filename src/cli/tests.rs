use std::ffi::OsString;
use std::time::Duration;

use clap::Parser;

use super::*;
use crate::error::Result;
use crate::types::{Action, DEFAULT_FILENAME_FORMAT, IpVersion, LogLevel, Options, StorageMode, VideoCodec};

fn os_args(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn norm(args: &[&str]) -> Vec<String> {
    normalize_args(os_args(args)).into_iter().map(|arg| arg.into_string().unwrap()).collect()
}

fn opts(args: &[&str]) -> Result<Options> {
    Cli::try_parse_from(normalize_args(os_args(args))).expect("clap parse").into_options()
}

#[test]
fn normalizes_go_style_flags() {
    assert_eq!(
        norm(&["y", "-threads", "4", "-td", "/tmp", "-dp=0700", "-w", "URL"]),
        ["y", "--threads", "4", "--temporary-dir", "/tmp", "--directory-permissions=0700", "-w", "URL"]
    );
}

#[test]
fn normalize_leaves_values_alone() {
    // `-verbose` is a value of --ytdlp-opts, `-30m` a value of --live-from.
    assert_eq!(
        norm(&["y", "--ytdlp-opts", "-verbose", "-live-from", "-30m", "-o", "-debug"]),
        ["y", "--ytdlp-opts", "-verbose", "--live-from", "-30m", "-o", "-debug"]
    );
    assert_eq!(norm(&["y", "--", "-threads"]), ["y", "--", "-threads"]);
}

#[test]
fn defaults() {
    let options = opts(&["y", "https://youtu.be/x", "best"]).unwrap();
    assert_eq!(options.url.as_deref(), Some("https://youtu.be/x"));
    assert_eq!(options.quality.as_deref(), Some("best"));
    assert_eq!(options.threads, 1);
    assert_eq!(options.frag_max_tries, 10);
    assert_eq!(options.file_mode, 0o644);
    assert_eq!(options.dir_mode, 0o755);
    assert_eq!(options.output_format, DEFAULT_FILENAME_FORMAT);
    assert_eq!(options.log_level, LogLevel::Warning);
    assert_eq!(options.wait, Action::Ask);
    assert!(options.frag_files);
    assert_eq!(options.storage, StorageMode::DirectTs);
    assert!(!options.recover_private);
    assert_eq!(options.windows_filenames, cfg!(windows));
    assert_eq!(options.codecs, None, "default order: BEST_CODEC_ORDER");
}

#[test]
fn storage_flags() {
    let options = opts(&["y", "--sqlite", "u"]).unwrap();
    assert_eq!((options.storage, options.recover_private), (StorageMode::Sqlite, false));
    let options = opts(&["y", "--recover-private", "u"]).unwrap();
    assert_eq!((options.storage, options.recover_private), (StorageMode::Sqlite, true));
}

#[test]
fn repeated_flags_take_the_last_value() {
    // As with Go's flag package; lists keep every value.
    let options = opts(&[
        "y",
        "-o",
        "a",
        "-o",
        "b",
        "--threads",
        "2",
        "--threads",
        "3",
        "--ytdlp-opts",
        "-x",
        "--ytdlp-opts",
        "--js-runtimes node",
        "-t",
        "-t",
        "--metadata",
        "k=1",
        "--metadata",
        "j=2",
        "u",
    ])
    .unwrap();
    assert_eq!(options.output_format, "b");
    assert_eq!(options.threads, 3);
    assert_eq!(options.ytdlp_opts, ["--js-runtimes", "node"]);
    assert!(options.embed_thumbnail);
    assert_eq!(options.metadata_overrides.len(), 2);
}

#[test]
fn codec_selection() {
    use VideoCodec::*;
    let codecs = |args: &[&str]| opts(args).unwrap().codecs;
    assert_eq!(codecs(&["y", "--codec", "vp9/h264", "u"]), Some(vec![Vp9, H264, Av1]));
    assert_eq!(codecs(&["y", "--codec", "vp9/best", "u"]), Some(vec![Vp9, Av1, H264]));
    assert_eq!(codecs(&["y", "--codec", "h264/best", "u"]), Some(vec![H264, Av1, Vp9]));
    assert_eq!(codecs(&["y", "--codec", "H264", "u"]), Some(vec![H264, Av1, Vp9]));
    assert_eq!(codecs(&["y", "--codec", "best", "u"]), Some(vec![Av1, Vp9, H264]));
    // Go's flags still work: flagged first, av1 > vp9 > h264.
    assert_eq!(codecs(&["y", "--h264", "--vp9", "u"]), Some(vec![Vp9, H264, Av1]));
    assert!(Cli::try_parse_from(os_args(&["y", "--codec", "hevc", "u"])).is_err());
    assert!(Cli::try_parse_from(os_args(&["y", "--codec", "hevc/best", "u"])).is_err());
    assert!(Cli::try_parse_from(os_args(&["y", "--codec", "best/hevc", "u"])).is_err());
    assert!(Cli::try_parse_from(os_args(&["y", "--codec", "vp9", "--h264", "u"])).is_err());
}

#[test]
fn windows_filenames_flags() {
    assert!(opts(&["y", "--windows-filenames", "u"]).unwrap().windows_filenames);
    assert!(!opts(&["y", "--no-windows-filenames", "u"]).unwrap().windows_filenames);
    assert!(Cli::try_parse_from(os_args(&["y", "--windows-filenames", "--no-windows-filenames"])).is_err());
    assert!(opts(&["y", "--no-path-limit", "u"]).unwrap().no_path_limit);
    assert!(!opts(&["y", "u"]).unwrap().no_path_limit);
}

#[test]
fn retry_stream_clamping() {
    let options = opts(&["y", "-r", "5", "u"]).unwrap();
    assert_eq!(options.retry_stream, Some(Duration::from_secs(15)));
    let options = opts(&["y", "--monitor-channel", "-r", "20", "u", "best"]).unwrap();
    assert_eq!(options.retry_stream, Some(Duration::from_secs(60)));
    assert_eq!(options.merge_on_cancel, Action::Do);
    let options = opts(&["y", "--monitor-channel", "--no-merge", "u", "best"]).unwrap();
    assert_eq!(options.merge_on_cancel, Action::DoNot);
}

#[test]
fn validation_errors() {
    assert!(opts(&["y", "--monitor-channel", "u"]).is_err());
    assert!(opts(&["y", "--start-delay", "1m", "--live-from", "now", "u"]).is_err());
    assert!(opts(&["y", "--metadata", "novalue", "u"]).is_err());
    assert!(opts(&["y", "--ytdlp-info", "--members-only", "u"]).is_err());
    assert!(opts(&["y", "--capture-duration", "nope", "u"]).is_err());
    assert!(opts(&["y", "--live-from", "garbage", "u"]).is_err());
    assert!(opts(&["y", "--live-from", "-8d", "u"]).is_err());
    assert!(opts(&["y", "--ytdlp-opts", "--cookies \"unclosed", "u"]).is_err());
    assert!(Cli::try_parse_from(os_args(&["y", "--proxy", "ftp://x"])).is_err());
    assert!(Cli::try_parse_from(os_args(&["y", "-fp", "999"])).is_err());
}

#[test]
fn misc_resolution() {
    let options = opts(&[
        "y",
        "--ytdlp-info",
        "--metadata",
        "title = %(title)s",
        "--ytdlp-opts",
        "--extractor-args youtube:player_client=web",
        "--capture-duration",
        "1h30m",
        "--vp9",
        "--av1",
        "--debug",
        "-6",
        "--save-state",
        "--disable-save-state",
        "u",
    ])
    .unwrap();
    assert_eq!(options.metadata_overrides, vec![("title".into(), "%(title)s".into())]);
    assert_eq!(options.ytdlp_opts, ["--extractor-args", "youtube:player_client=web"]);
    assert_eq!(options.capture_duration, Some(Duration::from_secs(5400)));
    assert_eq!(options.codecs, Some(vec![VideoCodec::Av1, VideoCodec::Vp9, VideoCodec::H264]));
    assert_eq!(options.log_level, LogLevel::Debug);
    assert_eq!(options.ip_version, IpVersion::V6);
    assert_eq!(options.save_state_on_cancel, Action::DoNot);
    let options = opts(&["y", "--save-state", "u"]).unwrap();
    assert_eq!(options.save_state_on_cancel, Action::Do);
}

#[test]
fn ytdlp_opts_parses_quotes_and_windows_backslashes() {
    let options =
        opts(&["y", "--ytdlp-opts", r#"--cookies "C:\My Folder\cookies.txt" --extractor-args 'youtube:a b' """#, "u"])
            .unwrap();
    assert_eq!(options.ytdlp_opts, ["--cookies", r"C:\My Folder\cookies.txt", "--extractor-args", "youtube:a b", ""]);
}

#[test]
fn googlevideo_flags_override_positionals() {
    let video_url = "https://rr1---sn-x.googlevideo.com/videoplayback?itag=299";
    let options = opts(&["y", "--video-url", video_url, "--ytdlp-info", "--members-only", "u", "best"]).unwrap();
    assert_eq!(options.url.as_deref(), Some(video_url));
    assert_eq!(options.quality, None);
}

#[cfg(unix)]
#[test]
fn preserves_non_utf8_path_arguments() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;

    let raw_bytes = b"/tmp/dir-\xff/cookies.txt";
    let raw_os = OsStr::from_bytes(raw_bytes);
    let expected = PathBuf::from(raw_os);

    let parse = |args: Vec<OsString>| Cli::try_parse_from(normalize_args(args)).unwrap().into_options().unwrap();

    let mut inline_cookies = b"--cookies=".to_vec();
    inline_cookies.extend_from_slice(raw_bytes);
    let mut single_dash_cookies = b"-cookies=".to_vec();
    single_dash_cookies.extend_from_slice(raw_bytes);
    let mut attached_short = b"-c".to_vec();
    attached_short.extend_from_slice(raw_bytes);
    let mut inline_td = b"-td=".to_vec();
    inline_td.extend_from_slice(raw_bytes);
    let mut inline_temp_dir = b"-temporary-dir=".to_vec();
    inline_temp_dir.extend_from_slice(raw_bytes);

    for cookie_args in [
        vec![OsString::from("y"), OsString::from("--cookies"), raw_os.to_os_string(), OsString::from("u")],
        vec![OsString::from("y"), OsString::from("-c"), raw_os.to_os_string(), OsString::from("u")],
        vec![OsString::from("y"), OsString::from("-cookies"), raw_os.to_os_string(), OsString::from("u")],
        vec![OsString::from("y"), OsStr::from_bytes(&inline_cookies).to_os_string(), OsString::from("u")],
        vec![OsString::from("y"), OsStr::from_bytes(&single_dash_cookies).to_os_string(), OsString::from("u")],
        vec![OsString::from("y"), OsStr::from_bytes(&attached_short).to_os_string(), OsString::from("u")],
    ] {
        let options = parse(cookie_args);
        assert_eq!(options.cookies_file.as_ref(), Some(&expected));
    }

    for temp_args in [
        vec![OsString::from("y"), OsString::from("-td"), raw_os.to_os_string(), OsString::from("u")],
        vec![OsString::from("y"), OsStr::from_bytes(&inline_td).to_os_string(), OsString::from("u")],
        vec![OsString::from("y"), OsStr::from_bytes(&inline_temp_dir).to_os_string(), OsString::from("u")],
    ] {
        let options = parse(temp_args);
        assert_eq!(options.temp_dir.as_ref(), Some(&expected));
    }

    // Non-path text arguments still convert lossily rather than failing parse.
    let lossy_out = OsStr::from_bytes(b"out-\xff").to_os_string();
    let options = parse(vec![OsString::from("y"), OsString::from("-o"), lossy_out, OsString::from("u")]);
    assert_eq!(options.output_format, "out-\u{fffd}");
}
