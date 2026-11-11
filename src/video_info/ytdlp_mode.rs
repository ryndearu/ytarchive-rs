//! `--ytdlp-info` mode: yt-dlp provides status, metadata, and URLs.

use std::time::Instant;

use super::wait::sleep_or_cancel;
use super::{ENDED_MESSAGE, Extractor, MAX_RETRIES};
use crate::error::Error;
use crate::stream_meta::{apply_metadata, default_metadata, format_info_from_ytdlp, local_unix_timestamp};
use crate::types::{FormatSelection, LiveStatus, YtdlpExtraction};
use crate::util::seconds_to_duration_and_time_str;
use crate::{log, log_debug, log_error, log_general, log_warn, ytdlp};

enum Step {
    Retry,
    Fail,
}

impl Extractor {
    /// Marks the stream unavailable if a download is running; logs either way.
    fn fail_info<T>(&mut self, message: &str) -> Option<T> {
        if self.in_progress {
            self.live = false;
            self.unavailable = true;
            log_warn!("{message}");
        } else {
            log_error!("{message}");
        }
        None
    }

    /// Ended stream without a live download: a running download just finishes.
    fn finish_live<T>(&mut self, message: &str) -> Option<T> {
        if self.in_progress {
            log_debug!("{message}");
            self.live = false;
        } else {
            log_general!("{message}");
        }
        None
    }

    fn retry_or_fail(&self, retries: &mut u32, message: &str) -> Step {
        if *retries >= MAX_RETRIES {
            return Step::Fail;
        }
        *retries += 1;
        log_warn!("{message} (Retry {retries}/{MAX_RETRIES})");
        Step::Retry
    }

    /// Runs yt-dlp until the stream is no longer upcoming (waiting as
    /// configured) and has usable URLs and a sequence boundary.
    async fn extract_usable(&mut self) -> Option<YtdlpExtraction> {
        let newlines = self.opts.status_newlines;
        let mut retries = 0u32;
        let mut poll_count = 0u32;
        let mut total_waited = 0u64;
        let mut polling_announced = false;

        loop {
            let info = match ytdlp::extract(&self.http, &self.opts, &self.page_url, true, &self.cancel).await {
                Ok(i) => i,
                // Unparsable output is retried; yt-dlp itself failing (after its
                // own retries) means the stream is gone, as in Go.
                Err(Error::Json(e)) => {
                    self.last_updated = Some(Instant::now());
                    let msg = format!("Failed to parse yt-dlp JSON: {e}");
                    match self.retry_or_fail(&mut retries, &msg) {
                        Step::Retry => continue,
                        Step::Fail => return self.fail_info(&msg),
                    }
                }
                Err(e) => {
                    self.last_updated = Some(Instant::now());
                    if !self.cancel.is_cancelled() {
                        log_debug!("yt-dlp extraction failed: {e}");
                        self.live = false;
                        self.unavailable = true;
                    }
                    return None;
                }
            };
            self.last_updated = Some(Instant::now());
            if self.cancel.is_cancelled() {
                return None;
            }

            if info.live_status == LiveStatus::IsUpcoming {
                self.print_info(&info.channel, &info.title);
                let interval = self.upcoming_wait_interval(info.start_unix).await?;
                if !polling_announced {
                    self.ask_ahead().await;
                }
                let secs = interval.as_secs().max(1);
                let mut polling = true;
                if self.retry_secs.is_some() {
                    if !polling_announced {
                        log_general!("Waiting for stream, retrying every {secs} seconds...");
                        polling_announced = true;
                    }
                } else if let Some(start) = info.start_unix {
                    if start > chrono::Utc::now().timestamp() {
                        log_general!(
                            "Stream starts at {} in {secs} seconds. Waiting for this time to elapse...",
                            local_unix_timestamp(start)
                        );
                        polling = false;
                    } else if !polling_announced {
                        log_general!("Stream should have started. Checking back every {secs} seconds");
                        polling_announced = true;
                    }
                } else {
                    log_general!("Waiting {} before retrying yt-dlp...", seconds_to_duration_and_time_str(secs));
                }
                if !sleep_or_cancel(&self.cancel, interval).await {
                    return None;
                }
                if polling {
                    poll_count += 1;
                    total_waited += secs;
                    log::retry_status(poll_count, total_waited, newlines);
                }
                continue;
            }

            self.print_info(&info.channel, &info.title);

            if info.urls.is_empty() {
                match info.live_status {
                    LiveStatus::NotLive => {
                        return self.fail_info("This video is not a livestream. Use yt-dlp instead.");
                    }
                    LiveStatus::WasLive | LiveStatus::PostLive => return self.finish_live(ENDED_MESSAGE),
                    _ => {}
                }
                match self.retry_or_fail(&mut retries, "No fragmented download URLs found") {
                    Step::Retry => continue,
                    Step::Fail => return self.fail_info("No fragmented download URLs found"),
                }
            }

            if info.last_sq.is_none() {
                let msg = match info.live_status {
                    LiveStatus::IsLive | LiveStatus::PostLive => "X-Head-Seqnum probe failed",
                    LiveStatus::NotLive => {
                        return self.fail_info("This video is not a livestream. Use yt-dlp instead.");
                    }
                    LiveStatus::WasLive => return self.finish_live(ENDED_MESSAGE),
                    _ if info.is_live => "X-Head-Seqnum probe failed",
                    _ => "No fragment range found",
                };
                match self.retry_or_fail(&mut retries, msg) {
                    Step::Retry => continue,
                    Step::Fail => return self.fail_info(msg),
                }
            }
            return Some(info);
        }
    }

    pub(super) async fn get_video_info_ytdlp(&mut self) -> Option<FormatSelection> {
        let info = self.extract_usable().await?;
        if self.cancel.is_cancelled() || self.unavailable {
            return None;
        }
        if !info.id.is_empty() {
            self.video_id = info.id.clone();
        }
        if self.video_id.is_empty() {
            log_error!("No video ID found");
            return None;
        }
        // Pin a channel's /live URL to the stream it resolved to, so URL
        // refreshes keep following this stream even if the channel starts
        // another one. The channel monitor starts each run from /live again.
        if self.is_channel {
            self.page_url = format!("https://www.youtube.com/watch?v={}", self.video_id);
            self.is_channel = false;
            log_debug!("Pinned the download to {}", self.page_url);
        }
        self.last_sq = info.last_sq;
        if info.target_duration > 0 {
            self.target_duration = info.target_duration;
        }
        if !self.in_progress
            && let Some(start) = info.start_unix
        {
            log_general!("Stream started at time {}", local_unix_timestamp(start));
        }

        let sel = self.select(&info.urls).await?;
        if !self.in_progress {
            let page = format!("https://www.youtube.com/watch?v={}", self.video_id);
            self.format_info = format_info_from_ytdlp(&info, &page);
            self.metadata = apply_metadata(&default_metadata(&self.opts.metadata_overrides), &self.format_info);
            self.thumbnail = info.thumbnail.clone();
            self.in_progress = true;
        }
        self.live = info.is_live;
        Some(sel)
    }
}
