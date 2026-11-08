use std::ffi::OsString;
use std::time::Duration;

use clap::{CommandFactory, Parser};

use super::*;
use crate::error::Result;
use crate::types::{Action, IpVersion, LogLevel, Options, StorageMode, VideoCodec};

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn norm(args: &[&str]) -> Vec<String> {
    normalize_args(os(args)).into_iter().map(|a| a.into_string().unwrap()).collect()
}

fn opts(args: &[&str]) -> Result<Options> {
    Cli::try_parse_from(normalize_args(os(args))).expect("clap parse").into_options()
}

#[test]
fn clap_definition_is_valid() {
    Cli::command().debug_assert();
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
    let o = opts(&["y", "https://youtu.be/x", "best"]).unwrap();
    assert_eq!(o.url.as_deref(), Some("https://youtu.be/x"));
    assert_eq!(o.quality.as_deref(), Some("best"));
    assert_eq!(o.threads, 1);
    assert_eq!(o.frag_max_tries, 10);
    assert_eq!(o.file_mode, 0o644);
    assert_eq!(o.dir_mode, 0o755);
    assert_eq!(o.output_format, crate::types::DEFAULT_FILENAME_FORMAT);
    assert_eq!(o.log_level, LogLevel::Warning);
    assert_eq!(o.wait, Action::Ask);
    assert!(o.frag_files);
    assert_eq!(o.storage, StorageMode::DirectTs);
    assert!(!o.recover_private);
    assert_eq!(o.windows_filenames, cfg!(windows));
    assert_eq!(o.codecs, None, "default order: BEST_CODEC_ORDER");
}

#[test]
fn storage_flags() {
    let o = opts(&["y", "--sqlite", "u"]).unwrap();
    assert_eq!((o.storage, o.recover_private), (StorageMode::Sqlite, false));
    let o = opts(&["y", "--recover-private", "u"]).unwrap();
    assert_eq!((o.storage, o.recover_private), (StorageMode::Sqlite, true));
}

#[test]
fn repeated_flags_take_the_last_value() {
    // As with Go's flag package; lists keep every value.
    let o = opts(&[
        "y", "-o", "a", "-o", "b", "--threads", "2", "--threads", "3", "--ytdlp-opts", "-x", "--ytdlp-opts",
        "--js-runtimes node", "-t", "-t", "--metadata", "k=1", "--metadata", "j=2", "u",
    ])
    .unwrap();
    assert_eq!(o.output_format, "b");
    assert_eq!(o.threads, 3);
    assert_eq!(o.ytdlp_opts, ["--js-runtimes", "node"]);
    assert!(o.embed_thumbnail);
    assert_eq!(o.metadata_overrides.len(), 2);
}

#[test]
fn codec_selection() {
    use VideoCodec::*;
    let codecs = |args: &[&str]| opts(args).unwrap().codecs;
    assert_eq!(codecs(&["y", "--codec", "vp9/h264", "u"]), Some(vec![Vp9, H264, Av1]));
    assert_eq!(codecs(&["y", "--codec", "H264", "u"]), Some(vec![H264, Av1, Vp9]));
    assert_eq!(codecs(&["y", "--codec", "best", "u"]), Some(vec![Av1, Vp9, H264]));
    // Go's flags still work: flagged first, av1 > vp9 > h264.
    assert_eq!(codecs(&["y", "--h264", "--vp9", "u"]), Some(vec![Vp9, H264, Av1]));
    assert!(Cli::try_parse_from(os(&["y", "--codec", "hevc", "u"])).is_err());
    assert!(Cli::try_parse_from(os(&["y", "--codec", "vp9", "--h264", "u"])).is_err());
}

#[test]
fn windows_filenames_flags() {
    assert!(opts(&["y", "--windows-filenames", "u"]).unwrap().windows_filenames);
    assert!(!opts(&["y", "--no-windows-filenames", "u"]).unwrap().windows_filenames);
    assert!(Cli::try_parse_from(os(&["y", "--windows-filenames", "--no-windows-filenames"])).is_err());
    assert!(opts(&["y", "--no-path-limit", "u"]).unwrap().no_path_limit);
    assert!(!opts(&["y", "u"]).unwrap().no_path_limit);
}

#[test]
fn retry_stream_clamping() {
    let o = opts(&["y", "-r", "5", "u"]).unwrap();
    assert_eq!(o.retry_stream, Some(Duration::from_secs(15)));
    let o = opts(&["y", "--monitor-channel", "-r", "20", "u", "best"]).unwrap();
    assert_eq!(o.retry_stream, Some(Duration::from_secs(60)));
    assert_eq!(o.merge_on_cancel, Action::Do);
    let o = opts(&["y", "--monitor-channel", "--no-merge", "u", "best"]).unwrap();
    assert_eq!(o.merge_on_cancel, Action::DoNot);
}

#[test]
fn validation_errors() {
    assert!(opts(&["y", "--monitor-channel", "u"]).is_err());
    assert!(opts(&["y", "--start-delay", "1m", "--live-from", "now", "u"]).is_err());
    assert!(opts(&["y", "--metadata", "novalue", "u"]).is_err());
    assert!(opts(&["y", "--ytdlp-info", "--members-only", "u"]).is_err());
    assert!(opts(&["y", "--capture-duration", "nope", "u"]).is_err());
    assert!(Cli::try_parse_from(os(&["y", "--proxy", "ftp://x"])).is_err());
    assert!(Cli::try_parse_from(os(&["y", "-fp", "999"])).is_err());
}

#[test]
fn misc_resolution() {
    let o = opts(&[
        "y", "--ytdlp-info", "--metadata", "title = %(title)s", "--ytdlp-opts",
        "--extractor-args youtube:player_client=web", "--capture-duration", "1h30m", "--vp9",
        "--av1", "--debug", "-6", "--save-state", "--disable-save-state", "u",
    ])
    .unwrap();
    assert_eq!(o.metadata_overrides, vec![("title".into(), "%(title)s".into())]);
    assert_eq!(o.ytdlp_opts, ["--extractor-args", "youtube:player_client=web"]);
    assert_eq!(o.capture_duration, Some(Duration::from_secs(5400)));
    assert_eq!(o.codecs, Some(vec![VideoCodec::Av1, VideoCodec::Vp9, VideoCodec::H264]));
    assert_eq!(o.log_level, LogLevel::Debug);
    assert_eq!(o.ip_version, IpVersion::V6);
    assert_eq!(o.save_state_on_cancel, Action::DoNot);
    let o = opts(&["y", "--save-state", "u"]).unwrap();
    assert_eq!(o.save_state_on_cancel, Action::Do);
}

#[test]
fn googlevideo_flags_override_positionals() {
    let gv = "https://rr1---sn-x.googlevideo.com/videoplayback?itag=299";
    let o = opts(&["y", "--video-url", gv, "--ytdlp-info", "--members-only", "u", "best"]).unwrap();
    assert_eq!(o.url.as_deref(), Some(gv));
    assert_eq!(o.quality, None);
}
