//! Rewriting Go-style flags for clap, then validating flag combinations and
//! resolving them into `Options`.

use std::ffi::OsString;
use std::time::Duration;

use clap::CommandFactory;

use super::Cli;
use crate::duration::parse_duration_or_time;
use crate::error::{Error, Result};
use crate::types::{
    Action, DEFAULT_MONITOR_SECS, DEFAULT_POLL_SECS, IpVersion, LogLevel, MINIMUM_MONITOR_SECS, Options, StorageMode,
    VideoCodec, complete_codec_order,
};

fn is_googlevideo(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(|host| host.ends_with(".googlevideo.com")))
        .unwrap_or(false)
}

/// Go flag aliases that are multi-character but single-dash.
const GO_SHORT_ALIASES: &[(&str, &str)] =
    &[("td", "temporary-dir"), ("dp", "directory-permissions"), ("fp", "file-permissions")];

/// Rewrites Go-style single-dash long options (`-threads 4`, `-td DIR`) to
/// `--flag` so clap accepts them. Values following a value-taking flag are
/// left untouched.
pub fn normalize_args<I>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = OsString>,
{
    let cmd = Cli::command();
    let mut long_takes_value = std::collections::HashMap::new();
    let mut short_takes_value = std::collections::HashSet::new();
    for arg in cmd.get_arguments() {
        let takes = arg.get_action().takes_values();
        if let Some(line) = arg.get_long() {
            long_takes_value.insert(line.to_string(), takes);
        }
        if let Some(character) = arg.get_short()
            && takes
        {
            short_takes_value.insert(character);
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
        let Some(text) = arg.to_str() else {
            out.push(arg);
            continue;
        };
        if text == "--" {
            passthrough = true;
            out.push(arg);
            continue;
        }

        let (rewritten, name) = if let Some(rest) = text.strip_prefix("--") {
            (None, Some(rest.split('=').next().unwrap_or(rest).to_string()))
        } else if let Some(rest) = text.strip_prefix('-').filter(|stripped| stripped.len() > 1) {
            let name = rest.split('=').next().unwrap_or(rest);
            let mapped = GO_SHORT_ALIASES
                .iter()
                .find(|(arg, _)| *arg == name)
                .map(|(_, line)| *line)
                .or_else(|| long_takes_value.contains_key(name).then_some(name));
            match mapped {
                Some(long) => {
                    let suffix = &rest[name.len()..];
                    (Some(format!("--{long}{suffix}")), Some(long.to_string()))
                }
                None => (None, None),
            }
        } else if let Some(character) = text.strip_prefix('-').and_then(|rest| rest.chars().next()) {
            // Plain single-char short flag like `-o`.
            if short_takes_value.contains(&character) && text.len() == 2 {
                skip_next = true;
            }
            (None, None)
        } else {
            (None, None)
        };

        let final_arg = rewritten.unwrap_or_else(|| text.to_string());
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

impl Cli {
    /// `--codec`, else Go's `--av1`/`--vp9`/`--h264` flags (flagged codecs
    /// first in av1 > vp9 > h264 order), else `None` (ask later).
    fn codec_order(&self) -> Option<Vec<VideoCodec>> {
        if let Some(list) = &self.codec {
            return Some(list.0.clone());
        }
        let flagged: Vec<VideoCodec> =
            [(VideoCodec::Av1, self.av1), (VideoCodec::Vp9, self.vp9), (VideoCodec::H264, self.h264)]
                .into_iter()
                .filter_map(|(codec, on)| on.then_some(codec))
                .collect();
        (!flagged.is_empty()).then(|| complete_codec_order(&flagged))
    }

    /// Validates flag combinations and resolves them into `Options`.
    pub fn into_options(self) -> Result<Options> {
        if self.monitor_channel && (self.url.is_none() || self.quality.is_none()) {
            return Err(Error::InvalidArg(
                "you must specify a channel AND quality when choosing to monitor a channel".into(),
            ));
        }
        if self.start_delay.is_some() && self.live_from.is_some() {
            return Err(Error::InvalidArg("you cannot use both --start-delay and --live-from at the same time".into()));
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
            Some(url) => (Some(url), None),
            None => (self.url.clone(), self.quality.clone()),
        };
        let direct_download =
            self.video_url.is_some() || self.audio_url.is_some() || url.as_deref().is_some_and(is_googlevideo);

        if self.ytdlp_info && !direct_download {
            if self.monitor_channel {
                return Err(Error::InvalidArg("--ytdlp-info is not compatible with --monitor-channel".into()));
            }
            if self.members_only {
                return Err(Error::InvalidArg(
                    "--ytdlp-info is not compatible with --members-only; use cookies and a direct member stream URL instead".into(),
                ));
            }
        }

        let mut metadata_overrides = Vec::with_capacity(self.metadata.len());
        for pair in &self.metadata {
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| Error::InvalidArg(format!("--metadata expects KEY=VALUE, got '{pair}'")))?;
            let key = key.trim();
            if key.is_empty() {
                return Err(Error::InvalidArg(format!("--metadata key is empty in '{pair}'")));
            }
            metadata_overrides.push((key.to_string(), value.trim().to_string()));
        }

        let mut retry_stream =
            self.retry_stream.filter(|seconds| *seconds > 0).map(|seconds| seconds.max(DEFAULT_POLL_SECS));
        if self.monitor_channel && retry_stream.is_none_or(|seconds| seconds < MINIMUM_MONITOR_SECS) {
            retry_stream = Some(DEFAULT_MONITOR_SECS);
        }

        let start_delay =
            self.start_delay.as_deref().map(|text| parse_duration_or_time("--start-delay", text)).transpose()?;
        let capture_duration = self
            .capture_duration
            .as_deref()
            .map(|text| parse_duration_or_time("--capture-duration", text))
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
        let codecs = self.codec_order();

        let options = Options {
            url,
            quality,
            video_url: self.video_url,
            audio_url: self.audio_url,
            cookies_file: self.cookies,
            ytdlp_cookies_file: None,
            ytdlp_path: self.ytdlp_path,
            ytdlp_opts: self
                .ytdlp_opts
                .map(|text| text.split_whitespace().map(String::from).collect())
                .unwrap_or_default(),
            ytdlp_info: self.ytdlp_info,
            codecs,
            mkv: self.mkv,
            audio_only: self.no_video,
            video_only: self.no_audio,
            output_format: self.output,
            temp_dir: self.temporary_dir,
            lookalike_chars: self.lookalike_chars,
            // Explicit flag wins; otherwise on for Windows builds only.
            windows_filenames: self.windows_filenames || (!self.no_windows_filenames && cfg!(windows)),
            no_path_limit: self.no_path_limit,
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
            // Recovery stores fragments out of order, which needs SQLite.
            storage: if self.sqlite || self.recover_private { StorageMode::Sqlite } else { StorageMode::DirectTs },
            recover_private: self.recover_private,
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
            log_level,
            status_newlines: self.newline,
        };

        Ok(options)
    }
}
