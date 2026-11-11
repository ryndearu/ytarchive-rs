//! Validating flag combinations and resolving them into `Options`.

use std::time::Duration;

use super::Cli;
use crate::duration::parse_duration_or_time;
use crate::error::{Error, Result};
use crate::types::{
    Action, DEFAULT_MONITOR_SECS, DEFAULT_POLL_SECS, IpVersion, LogLevel, MINIMUM_MONITOR_SECS, Options, StorageMode,
    VideoCodec, complete_codec_order,
};

fn is_googlevideo(url: &str) -> bool {
    url::Url::parse(url).ok().and_then(|u| u.host_str().map(|h| h.ends_with(".googlevideo.com"))).unwrap_or(false)
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
                .filter_map(|(c, on)| on.then_some(c))
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
            Some(u) => (Some(u), None),
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
        for kv in &self.metadata {
            let (k, v) = kv
                .split_once('=')
                .ok_or_else(|| Error::InvalidArg(format!("--metadata expects KEY=VALUE, got '{kv}'")))?;
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

        let start_delay =
            self.start_delay.as_deref().map(|s| parse_duration_or_time("--start-delay", s)).transpose()?;
        let capture_duration =
            self.capture_duration.as_deref().map(|s| parse_duration_or_time("--capture-duration", s)).transpose()?;

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
            ytdlp_opts: self.ytdlp_opts.map(|s| s.split_whitespace().map(String::from).collect()).unwrap_or_default(),
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
