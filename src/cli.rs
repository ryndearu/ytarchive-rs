//! Command-line parsing. Flag names match Go ytarchive so existing scripts work,
//! including Go-style single-dash long flags (`-threads 4`, `-td DIR`).

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use clap::{CommandFactory, Parser};

use crate::duration::parse_duration_or_time;
use crate::error::{Error, Result};
use crate::types::{
    Action, CodecPrefs, IpVersion, LogLevel, Options, StorageMode, DEFAULT_FILENAME_FORMAT,
    DEFAULT_FRAG_MAX_TRIES, DEFAULT_MONITOR_SECS, DEFAULT_POLL_SECS, MINIMUM_MONITOR_SECS,
};

const AFTER_HELP: &str = "\
QUALITY is a slash-delimited list from most to least wanted, e.g. 1080p60/720p/best.
Valid values: audio_only, 144p, 240p, 360p, 480p, 720p, 720p60, 1080p, 1080p60,
1440p, 1440p60, 2160p, 2160p60, best

Durations accept units (1d8h10m, 90s) or time strings (01:30:00).
--live-from also accepts negative durations (-30m) and 'now'.";

/// Go flag aliases that are multi-character but single-dash.
const GO_SHORT_ALIASES: &[(&str, &str)] = &[
    ("td", "temporary-dir"),
    ("dp", "directory-permissions"),
    ("fp", "file-permissions"),
];

#[derive(Debug, Parser)]
#[command(
    name = "ytarchive",
    version = crate::version::VERSION,
    about = "Archive a YouTube livestream from the start",
    after_help = AFTER_HELP
)]
pub struct Cli {
    /// YouTube livestream, channel, or googlevideo URL. Prompted for if omitted.
    pub url: Option<String>,
    /// Slash-delimited quality list. Prompted for if omitted.
    pub quality: Option<String>,

    // ---- waiting / cancellation behaviour ----
    /// Wait for a scheduled stream to start.
    #[arg(short = 'w', long)]
    pub wait: bool,
    /// Do not wait for a scheduled stream to start.
    #[arg(short = 'n', long)]
    pub no_wait: bool,
    /// Seconds between stream status checks while waiting (min 15; 30 when monitoring).
    #[arg(short = 'r', long, value_name = "SECS")]
    pub retry_stream: Option<u64>,
    /// Automatically merge files when the download is cancelled.
    #[arg(long)]
    pub merge: bool,
    /// Skip merging files when the download is cancelled.
    #[arg(long)]
    pub no_merge: bool,
    /// Keep downloaded files on cancel if not merging.
    #[arg(long)]
    pub save: bool,
    /// Delete downloaded files on cancel if not merging.
    #[arg(long)]
    pub no_save: bool,
    /// Leave files for resuming later on cancel, if not saving files.
    #[arg(long)]
    pub save_state: bool,
    /// Do not leave files for resuming later on cancel.
    #[arg(long)]
    pub no_save_state: bool,
    /// Disable resumable download state entirely.
    #[arg(long)]
    pub disable_save_state: bool,

    // ---- input / auth ----
    /// Netscape-format cookies file (needed for members-only streams).
    #[arg(short = 'c', long, value_name = "FILE")]
    pub cookies: Option<PathBuf>,
    /// Ignored (accepted for compatibility); pass PO tokens to yt-dlp via --ytdlp-opts.
    #[arg(long, value_name = "TOKEN")]
    pub potoken: Option<String>,
    /// Ignored (accepted for compatibility); pass visitor data to yt-dlp via --ytdlp-opts.
    #[arg(long, value_name = "VISITOR_DATA")]
    pub visitor_data: Option<String>,
    /// Googlevideo URL to use for the video stream.
    #[arg(long, value_name = "URL")]
    pub video_url: Option<String>,
    /// Googlevideo URL to use for the audio stream (itag 140).
    #[arg(long, value_name = "URL")]
    pub audio_url: Option<String>,
    /// yt-dlp executable used for URL extraction.
    #[arg(long, value_name = "PATH", default_value = "yt-dlp")]
    pub ytdlp_path: String,
    /// Extra arguments for yt-dlp, e.g. '--extractor-args youtube:player_client=web'.
    #[arg(long, value_name = "OPTIONS", allow_hyphen_values = true)]
    pub ytdlp_opts: Option<String>,
    /// Let yt-dlp extract stream info; ytarchive only downloads fragments.
    #[arg(long)]
    pub ytdlp_info: bool,

    // ---- format selection ----
    /// Prefer VP9 video when available.
    #[arg(long)]
    pub vp9: bool,
    /// Prefer AV1 video when available (takes priority).
    #[arg(long)]
    pub av1: bool,
    /// Prefer H.264 video.
    #[arg(long)]
    pub h264: bool,
    /// Only download the audio stream.
    #[arg(long)]
    pub no_video: bool,
    /// Only download the video stream.
    #[arg(long)]
    pub no_audio: bool,

    // ---- output ----
    /// Output filename template, e.g. '%(channel)s/%(upload_date)s %(title)s'.
    #[arg(short = 'o', long, value_name = "FORMAT", default_value = DEFAULT_FILENAME_FORMAT)]
    pub output: String,
    /// Directory for temporary files.
    #[arg(long = "temporary-dir", value_name = "DIR")]
    pub temporary_dir: Option<PathBuf>,
    /// Mux into mkv instead of mp4 (ignored for audio only).
    #[arg(long)]
    pub mkv: bool,
    /// Use lookalike characters in place of forbidden filename characters.
    #[arg(short = 'l', long)]
    pub lookalike_chars: bool,
    /// Force filenames to be Windows-compatible (default on Windows).
    #[arg(long, conflicts_with = "no_windows_filenames")]
    pub windows_filenames: bool,
    /// Sanitize filenames only minimally (default on other than Windows).
    #[arg(long)]
    pub no_windows_filenames: bool,
    /// Write basic metadata to the final file.
    #[arg(long)]
    pub add_metadata: bool,
    /// Override metadata, KEY=VALUE (VALUE is a format template; empty omits the key).
    #[arg(long, value_name = "KEY=VALUE")]
    pub metadata: Vec<String>,
    /// Write the description to a separate file.
    #[arg(long)]
    pub write_description: bool,
    /// Write the thumbnail to a separate file.
    #[arg(long)]
    pub write_thumbnail: bool,
    /// Embed the thumbnail into the final file.
    #[arg(short = 't', long)]
    pub thumbnail: bool,
    /// Write the ffmpeg mux command to a file instead of muxing.
    #[arg(long)]
    pub write_mux_file: bool,
    /// Keep the raw .ts files after muxing.
    #[arg(short = 'k', long)]
    pub keep_ts_files: bool,
    /// Also save the audio as a separate file.
    #[arg(long)]
    pub separate_audio: bool,
    /// ffmpeg executable to use.
    #[arg(long, value_name = "PATH", default_value = "ffmpeg")]
    pub ffmpeg_path: String,
    /// Permissions for created files (octal).
    #[arg(long = "file-permissions", value_name = "MODE", default_value = "0644", value_parser = parse_octal)]
    pub file_permissions: u32,
    /// Permissions for created directories (octal).
    #[arg(long = "directory-permissions", value_name = "MODE", default_value = "0755", value_parser = parse_octal)]
    pub directory_permissions: u32,

    // ---- download behaviour ----
    /// Download threads per stream type.
    #[arg(long, value_name = "N", default_value_t = 1)]
    pub threads: usize,
    /// Attempts per fragment before refreshing URLs / giving up (0 = unlimited).
    #[arg(long, value_name = "ATTEMPTS", default_value_t = DEFAULT_FRAG_MAX_TRIES)]
    pub retry_frags: u32,
    /// Keep out-of-order fragments in memory instead of temp files.
    #[arg(long)]
    pub no_frag_files: bool,
    /// Store fragments in a SQLite database (more robust to gaps).
    #[arg(long)]
    pub sqlite: bool,
    /// Start from a point in the stream: duration (-30m), time string, or 'now'.
    #[arg(long, value_name = "WHEN", allow_hyphen_values = true)]
    pub live_from: Option<String>,
    /// Wait this long after the stream starts before capturing.
    #[arg(long, value_name = "DURATION")]
    pub start_delay: Option<String>,
    /// Capture this much of the stream, then finish.
    #[arg(long, value_name = "DURATION")]
    pub capture_duration: Option<String>,

    // ---- monitoring ----
    /// Continually monitor a channel URL for new streams (requires URL and QUALITY).
    #[arg(long)]
    pub monitor_channel: bool,
    /// Only download members-only streams (channel URLs; requires cookies).
    #[arg(long)]
    pub members_only: bool,

    // ---- network ----
    /// Force IPv4.
    #[arg(short = '4', long, conflicts_with = "ipv6")]
    pub ipv4: bool,
    /// Force IPv6.
    #[arg(short = '6', long)]
    pub ipv6: bool,
    /// Proxy URL (http, https, or socks5).
    #[arg(long, value_name = "URL", value_parser = parse_proxy)]
    pub proxy: Option<url::Url>,

    // ---- logging ----
    /// Print stream information (title, selected quality, start time, duration) and exit.
    #[arg(long)]
    pub info_only: bool,
    /// Print nothing except prompts.
    #[arg(short = 'q', long)]
    pub quiet: bool,
    /// Print only errors.
    #[arg(long)]
    pub error: bool,
    /// Print warnings and errors (default).
    #[arg(long)]
    pub warn: bool,
    /// Verbose output.
    #[arg(short = 'v', long)]
    pub verbose: bool,
    /// Debug output.
    #[arg(long)]
    pub debug: bool,
    /// Trace output.
    #[arg(long)]
    pub trace: bool,
    /// Print progress on new lines instead of updating one line.
    #[arg(long)]
    pub newline: bool,
}

fn parse_octal(s: &str) -> std::result::Result<u32, String> {
    let digits = s.strip_prefix("0o").unwrap_or(s);
    u32::from_str_radix(digits, 8)
        .ok()
        .filter(|m| *m <= 0o7777)
        .ok_or_else(|| format!("'{s}' is not an octal permission mode like 0644"))
}

fn parse_proxy(s: &str) -> std::result::Result<url::Url, String> {
    let u = url::Url::parse(s).map_err(|_| "invalid proxy URL".to_string())?;
    match u.scheme() {
        "http" | "https" | "socks5" => Ok(u),
        _ => Err("the proxy URL scheme must be http, https, or socks5".into()),
    }
}

/// Rewrites Go-style `-flag` long options to `--flag` so clap accepts them.
/// Values following a value-taking flag are left untouched.
pub fn normalize_args<I>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = OsString>,
{
    let cmd = Cli::command();
    let mut long_takes_value = std::collections::HashMap::new();
    let mut short_takes_value = std::collections::HashSet::new();
    for arg in cmd.get_arguments() {
        let takes = arg.get_action().takes_values();
        if let Some(l) = arg.get_long() {
            long_takes_value.insert(l.to_string(), takes);
        }
        if let Some(s) = arg.get_short()
            && takes
        {
            short_takes_value.insert(s);
        }
    }

    let mut out = Vec::new();
    let mut iter = args.into_iter();
    if let Some(prog) = iter.next() {
        out.push(prog);
    }

    let mut skip_next = false;
    let mut passthrough = false;
    for arg in iter {
        if passthrough || skip_next {
            skip_next = false;
            out.push(arg);
            continue;
        }
        let Some(s) = arg.to_str() else {
            out.push(arg);
            continue;
        };
        if s == "--" {
            passthrough = true;
            out.push(arg);
            continue;
        }

        let (rewritten, name) = if let Some(rest) = s.strip_prefix("--") {
            (None, Some(rest.split('=').next().unwrap_or(rest).to_string()))
        } else if let Some(rest) = s.strip_prefix('-').filter(|r| r.len() > 1) {
            let name = rest.split('=').next().unwrap_or(rest);
            let mapped = GO_SHORT_ALIASES
                .iter()
                .find(|(a, _)| *a == name)
                .map(|(_, l)| *l)
                .or_else(|| long_takes_value.contains_key(name).then_some(name));
            match mapped {
                Some(long) => {
                    let suffix = &rest[name.len()..];
                    (Some(format!("--{long}{suffix}")), Some(long.to_string()))
                }
                None => (None, None),
            }
        } else if let Some(c) = s.strip_prefix('-').and_then(|r| r.chars().next()) {
            // Plain single-char short flag like `-o`.
            if short_takes_value.contains(&c) && s.len() == 2 {
                skip_next = true;
            }
            (None, None)
        } else {
            (None, None)
        };

        let final_arg = rewritten.unwrap_or_else(|| s.to_string());
        if let Some(name) = name {
            let has_inline_value = final_arg.contains('=');
            if !has_inline_value && long_takes_value.get(&name).copied().unwrap_or(false) {
                skip_next = true;
            }
        }
        out.push(OsString::from(final_arg));
    }
    out
}

fn is_googlevideo(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.ends_with(".googlevideo.com")))
        .unwrap_or(false)
}

impl Cli {
    pub fn parse_normalized() -> Self {
        Cli::parse_from(normalize_args(std::env::args_os()))
    }

    /// Validates flag combinations and resolves them into `Options`.
    /// Returns any non-fatal warnings alongside.
    pub fn into_options(self) -> Result<(Options, Vec<String>)> {
        let mut warnings = Vec::new();

        if self.monitor_channel && (self.url.is_none() || self.quality.is_none()) {
            return Err(Error::InvalidArg(
                "you must specify a channel AND quality when choosing to monitor a channel".into(),
            ));
        }
        if self.start_delay.is_some() && self.live_from.is_some() {
            return Err(Error::InvalidArg(
                "you cannot use both --start-delay and --live-from at the same time".into(),
            ));
        }
        if self.no_video && self.no_audio {
            return Err(Error::InvalidArg("--no-video and --no-audio cannot be combined".into()));
        }
        if self.threads == 0 {
            return Err(Error::InvalidArg("--threads must be at least 1".into()));
        }

        // Googlevideo URLs given via flags take precedence over positionals (as in Go).
        let gv_url = self.video_url.clone().or_else(|| self.audio_url.clone());
        let (url, quality) = match gv_url {
            Some(u) => (Some(u), None),
            None => (self.url, self.quality),
        };
        let direct_download =
            self.video_url.is_some() || self.audio_url.is_some() || url.as_deref().is_some_and(is_googlevideo);

        if self.ytdlp_info && !direct_download {
            if self.monitor_channel {
                return Err(Error::InvalidArg(
                    "--ytdlp-info is not compatible with --monitor-channel".into(),
                ));
            }
            if self.members_only {
                return Err(Error::InvalidArg(
                    "--ytdlp-info is not compatible with --members-only; use cookies and a direct member stream URL instead".into(),
                ));
            }
        }
        // dreammu accepted these but never used them (its InnerTube request
        // code is unreachable); yt-dlp does the extraction now.
        if self.potoken.is_some() || self.visitor_data.is_some() {
            warnings.push(
                "--potoken and --visitor-data are ignored; pass PO token options to yt-dlp through --ytdlp-opts, e.g. --ytdlp-opts '--extractor-args youtube:po_token=web.gvs+TOKEN'".into(),
            );
        }

        let mut metadata_overrides = Vec::with_capacity(self.metadata.len());
        for kv in &self.metadata {
            let (k, v) = kv.split_once('=').ok_or_else(|| {
                Error::InvalidArg(format!("--metadata expects KEY=VALUE, got '{kv}'"))
            })?;
            let k = k.trim();
            if k.is_empty() {
                return Err(Error::InvalidArg(format!("--metadata key is empty in '{kv}'")));
            }
            metadata_overrides.push((k.to_string(), v.trim().to_string()));
        }

        let mut retry_stream = self.retry_stream.filter(|s| *s > 0).map(|s| s.max(DEFAULT_POLL_SECS));
        if self.monitor_channel && retry_stream.is_none_or(|s| s < MINIMUM_MONITOR_SECS) {
            retry_stream = Some(DEFAULT_MONITOR_SECS);
        }

        let start_delay = self
            .start_delay
            .as_deref()
            .map(|s| parse_duration_or_time("--start-delay", s))
            .transpose()?;
        let capture_duration = self
            .capture_duration
            .as_deref()
            .map(|s| parse_duration_or_time("--capture-duration", s))
            .transpose()?;

        let log_level = if self.trace {
            LogLevel::Trace
        } else if self.debug {
            LogLevel::Debug
        } else if self.verbose {
            LogLevel::Info
        } else if self.warn {
            LogLevel::Warning
        } else if self.error {
            LogLevel::Error
        } else if self.quiet {
            LogLevel::Quiet
        } else {
            LogLevel::Warning
        };

        // Monitoring is unattended, so merge unless explicitly told not to.
        let merge = self.merge || (self.monitor_channel && !self.no_merge);

        let options = Options {
            url,
            quality,
            video_url: self.video_url,
            audio_url: self.audio_url,
            cookies_file: self.cookies,
            ytdlp_path: self.ytdlp_path,
            ytdlp_opts: self
                .ytdlp_opts
                .map(|s| s.split_whitespace().map(String::from).collect())
                .unwrap_or_default(),
            ytdlp_info: self.ytdlp_info,
            codecs: CodecPrefs { av1: self.av1, vp9: self.vp9, h264: self.h264 },
            mkv: self.mkv,
            audio_only: self.no_video,
            video_only: self.no_audio,
            output_format: self.output,
            temp_dir: self.temporary_dir,
            lookalike_chars: self.lookalike_chars,
            // Explicit flag wins; otherwise on for Windows builds only.
            windows_filenames: self.windows_filenames
                || (!self.no_windows_filenames && cfg!(windows)),
            add_metadata: self.add_metadata,
            metadata_overrides,
            write_description: self.write_description,
            write_thumbnail: self.write_thumbnail,
            embed_thumbnail: self.thumbnail,
            write_mux_file: self.write_mux_file,
            keep_ts_files: self.keep_ts_files,
            separate_audio: self.separate_audio,
            ffmpeg_path: self.ffmpeg_path,
            file_mode: self.file_permissions,
            dir_mode: self.directory_permissions,
            threads: self.threads,
            frag_max_tries: self.retry_frags,
            storage: if self.sqlite { StorageMode::Sqlite } else { StorageMode::DirectTs },
            frag_files: !self.no_frag_files,
            disable_save_state: self.disable_save_state,
            wait: Action::from_flags(self.wait, self.no_wait),
            merge_on_cancel: Action::from_flags(merge, self.no_merge),
            save_files_on_cancel: Action::from_flags(self.save, self.no_save),
            // Unlike the other pairs, "don't" wins here (matches Go).
            save_state_on_cancel: if self.no_save_state || self.disable_save_state {
                Action::DoNot
            } else {
                Action::from_flags(self.save_state, false)
            },
            retry_stream: retry_stream.map(Duration::from_secs),
            live_from: self.live_from,
            start_delay,
            capture_duration,
            monitor_channel: self.monitor_channel,
            members_only: self.members_only,
            ip_version: if self.ipv4 {
                IpVersion::V4
            } else if self.ipv6 {
                IpVersion::V6
            } else {
                IpVersion::Any
            },
            proxy: self.proxy,
            info_only: self.info_only,
            log_level,
            status_newlines: self.newline,
        };

        Ok((options, warnings))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn norm(args: &[&str]) -> Vec<String> {
        normalize_args(os(args)).into_iter().map(|a| a.into_string().unwrap()).collect()
    }

    fn opts(args: &[&str]) -> Result<(Options, Vec<String>)> {
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
        let (o, w) = opts(&["y", "https://youtu.be/x", "best"]).unwrap();
        assert!(w.is_empty());
        assert_eq!(o.url.as_deref(), Some("https://youtu.be/x"));
        assert_eq!(o.quality.as_deref(), Some("best"));
        assert_eq!(o.threads, 1);
        assert_eq!(o.frag_max_tries, 10);
        assert_eq!(o.file_mode, 0o644);
        assert_eq!(o.dir_mode, 0o755);
        assert_eq!(o.output_format, DEFAULT_FILENAME_FORMAT);
        assert_eq!(o.log_level, LogLevel::Warning);
        assert_eq!(o.wait, Action::Ask);
        assert!(o.frag_files);
        assert_eq!(o.storage, StorageMode::DirectTs);
        assert_eq!(o.windows_filenames, cfg!(windows));
    }

    #[test]
    fn windows_filenames_flags() {
        assert!(opts(&["y", "--windows-filenames", "u"]).unwrap().0.windows_filenames);
        assert!(!opts(&["y", "--no-windows-filenames", "u"]).unwrap().0.windows_filenames);
        assert!(Cli::try_parse_from(os(&["y", "--windows-filenames", "--no-windows-filenames"])).is_err());
    }

    #[test]
    fn retry_stream_clamping() {
        let (o, _) = opts(&["y", "-r", "5", "u"]).unwrap();
        assert_eq!(o.retry_stream, Some(Duration::from_secs(15)));
        let (o, _) = opts(&["y", "--monitor-channel", "-r", "20", "u", "best"]).unwrap();
        assert_eq!(o.retry_stream, Some(Duration::from_secs(60)));
        assert_eq!(o.merge_on_cancel, Action::Do);
        let (o, _) = opts(&["y", "--monitor-channel", "--no-merge", "u", "best"]).unwrap();
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
        let (o, w) = opts(&[
            "y", "--ytdlp-info", "--potoken", "p", "--metadata", "title = %(title)s", "--ytdlp-opts",
            "--extractor-args youtube:player_client=web", "--capture-duration", "1h30m", "--vp9",
            "--av1", "--debug", "-6", "--save-state", "--disable-save-state", "u",
        ])
        .unwrap();
        assert_eq!(w.len(), 1);
        assert_eq!(o.metadata_overrides, vec![("title".into(), "%(title)s".into())]);
        assert_eq!(o.ytdlp_opts, ["--extractor-args", "youtube:player_client=web"]);
        assert_eq!(o.capture_duration, Some(Duration::from_secs(5400)));
        assert_eq!(o.codecs, CodecPrefs { av1: true, vp9: true, h264: false });
        assert_eq!(o.log_level, LogLevel::Debug);
        assert_eq!(o.ip_version, IpVersion::V6);
        assert_eq!(o.save_state_on_cancel, Action::DoNot);
        let (o, _) = opts(&["y", "--save-state", "u"]).unwrap();
        assert_eq!(o.save_state_on_cancel, Action::Do);
    }

    #[test]
    fn googlevideo_flags_override_positionals() {
        let gv = "https://rr1---sn-x.googlevideo.com/videoplayback?itag=299";
        let (o, _) = opts(&["y", "--video-url", gv, "--ytdlp-info", "--members-only", "u", "best"]).unwrap();
        assert_eq!(o.url.as_deref(), Some(gv));
        assert_eq!(o.quality, None);
    }
}
