//! `--ytdlp-info` mode: yt-dlp provides status, metadata, and URLs.

use std::time::Duration;

use super::{ENDED_MESSAGE, Extractor, MAX_RETRIES, RETRY_DELAY};
use crate::error::Error;
use crate::stream_meta::{apply_metadata, default_metadata, format_info_from_ytdlp, local_unix_timestamp};
use crate::types::{FormatSelection, LiveStatus, YtdlpExtraction};
use crate::util::seconds_to_duration_and_time_str;
use crate::util::{sleep_or_cancel, sleep_wall_clock};
use crate::{log, log_debug, log_error, log_general, log_warn, ytdlp};

enum Step {
    Retry,
    Fail,
    Cancelled,
}

fn is_transient_error(error: &Error) -> bool {
    match error {
        Error::Http(_) | Error::Io(_) => true,
        Error::HttpStatus { status, .. } => *status == 429 || *status >= 500,
        Error::Ytdlp(message) => {
            let lower = message.to_ascii_lowercase();
            [
                "timed out",
                "unable to download webpage",
                "failed to resolve",
                "temporary failure in name resolution",
                "connection refused",
                "connection reset",
                "network is unreachable",
                "no route to host",
                "http error 5",
                "http error 429",
            ]
            .iter()
            .any(|needle| lower.contains(needle))
        }
        _ => false,
    }
}

fn is_channel_offline_error(error: &Error) -> bool {
    let Error::Ytdlp(message) = error else {
        return false;
    };
    let lower = message.to_ascii_lowercase();
    ["not currently live", "not live", "no live", "live event will begin"].iter().any(|needle| lower.contains(needle))
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

    async fn retry_or_fail(&self, retries: &mut u32, message: &str) -> Step {
        if *retries >= MAX_RETRIES {
            return Step::Fail;
        }
        *retries += 1;
        log_warn!("{message} (Retry {retries}/{MAX_RETRIES})");
        if sleep_or_cancel(&self.cancel, RETRY_DELAY).await { Step::Retry } else { Step::Cancelled }
    }

    async fn run_ytdlp_extraction(&self, probe_head: bool, waiting_on_channel: bool) -> Result<YtdlpExtraction, Error> {
        if !probe_head {
            ytdlp::extract_urls(&self.opts, &self.page_url, true, &self.cancel).await
        } else if waiting_on_channel {
            ytdlp::extract_once(&self.http, &self.opts, &self.page_url, true, &self.cancel).await
        } else {
            ytdlp::extract(&self.http, &self.opts, &self.page_url, true, &self.cancel).await
        }
    }

    /// Runs yt-dlp until the stream is no longer upcoming (waiting as
    /// configured) and has usable URLs and a sequence boundary.
    /// Waits one poll interval on a channel, announcing the wait the first
    /// time. Returns `false` if cancelled.
    async fn wait_channel_poll(
        &mut self,
        secs: u64,
        poll_count: &mut u32,
        total_waited: &mut u64,
        newlines: bool,
    ) -> bool {
        if *poll_count == 0 {
            log::finish_inline();
            self.ask_ahead().await;
            log_general!("You have opted to wait for a livestream to be scheduled. Retrying every {secs} seconds.");
        }
        if !sleep_wall_clock(&self.cancel, Duration::from_secs(secs)).await {
            return false;
        }
        *poll_count += 1;
        *total_waited += secs;
        log::retry_status(*poll_count, *total_waited, newlines);
        true
    }

    async fn extract_usable(&mut self, probe_head: bool) -> Option<YtdlpExtraction> {
        let newlines = self.opts.status_newlines;
        let waiting_on_channel = self.is_channel && !self.in_progress && self.retry_secs.is_some();
        let mut retries = 0u32;
        let mut poll_count = 0u32;
        let mut total_waited = 0u64;
        let mut polling_announced = false;
        let mut last_warned: Option<String> = None;

        loop {
            let extracted = self.run_ytdlp_extraction(probe_head, waiting_on_channel).await;
            let info = match extracted {
                Ok(info) => info,
                // Unparsable output is retried; yt-dlp itself failing (after its
                // own retries) means the stream is gone, as in Go.
                Err(Error::Json(error)) => {
                    let msg = format!("Failed to parse yt-dlp JSON: {error}");
                    match self.retry_or_fail(&mut retries, &msg).await {
                        Step::Retry => continue,
                        Step::Fail => return self.fail_info(&msg),
                        Step::Cancelled => return None,
                    }
                }
                Err(error) => {
                    if self.cancel.is_cancelled() {
                        return None;
                    }
                    if waiting_on_channel && let Some(secs) = self.retry_secs {
                        if ytdlp::is_start_error(&error) {
                            log_error!("yt-dlp extraction failed: {error}");
                            return None;
                        }
                        if !is_channel_offline_error(&error) && !is_transient_error(&error) {
                            let text = error.to_string();
                            if last_warned.as_deref() != Some(text.as_str()) {
                                log_warn!("yt-dlp failed while waiting on channel: {text}");
                                last_warned = Some(text);
                            } else {
                                log_debug!("yt-dlp extraction failed while waiting on channel: {text}");
                            }
                        } else {
                            log_debug!("yt-dlp extraction failed while waiting on channel: {error}");
                        }
                        if !self.wait_channel_poll(secs, &mut poll_count, &mut total_waited, newlines).await {
                            return None;
                        }
                        continue;
                    }
                    if is_transient_error(&error) {
                        log_warn!("Temporary yt-dlp extraction failure: {error}");
                    } else {
                        if self.in_progress {
                            log_debug!("yt-dlp extraction failed: {error}");
                        } else {
                            log_error!("yt-dlp extraction failed: {error}");
                        }
                        self.live = false;
                        self.unavailable = true;
                    }
                    return None;
                }
            };
            if self.cancel.is_cancelled() {
                return None;
            }

            if info.live_status == LiveStatus::IsUpcoming {
                if self.in_progress {
                    log_debug!("Livestream status is upcoming mid-download");
                    return None;
                }
                if self.reject_negative_live_from() {
                    return None;
                }
                retries = 0;
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
                let waited = if let Some(start) = (!polling).then_some(info.start_unix).flatten() {
                    super::sleep_until_unix(&self.cancel, start).await
                } else {
                    sleep_wall_clock(&self.cancel, interval).await
                };
                if !waited {
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

            // /live can still show the stream that just ended; wait for the
            // next one instead.
            if waiting_on_channel
                && let Some(secs) = self.retry_secs
                && matches!(info.live_status, LiveStatus::WasLive | LiveStatus::PostLive | LiveStatus::NotLive)
            {
                let text = format!(
                    "The video on the channel's /live page ({}) has ended or is not a livestream; continuing to check the channel.",
                    info.id
                );
                if last_warned.as_deref() != Some(text.as_str()) {
                    log::finish_inline();
                    log_general!("{text}");
                    last_warned = Some(text);
                }
                if !self.wait_channel_poll(secs, &mut poll_count, &mut total_waited, newlines).await {
                    return None;
                }
                continue;
            }

            if info.urls.is_empty() {
                match info.live_status {
                    LiveStatus::NotLive => {
                        return self.fail_info("This video is not a livestream. Use yt-dlp instead.");
                    }
                    LiveStatus::WasLive | LiveStatus::PostLive => return self.finish_live(ENDED_MESSAGE),
                    _ => {}
                }
                match self.retry_or_fail(&mut retries, "No fragmented download URLs found").await {
                    Step::Retry => continue,
                    Step::Fail => return self.fail_info("No fragmented download URLs found"),
                    Step::Cancelled => return None,
                }
            }

            if probe_head && info.last_sq.is_none() {
                let msg = match info.live_status {
                    LiveStatus::IsLive | LiveStatus::PostLive => "X-Head-Seqnum probe failed",
                    LiveStatus::NotLive => {
                        return self.fail_info("This video is not a livestream. Use yt-dlp instead.");
                    }
                    LiveStatus::WasLive => return self.finish_live(ENDED_MESSAGE),
                    _ if info.is_live => "X-Head-Seqnum probe failed",
                    _ => "No fragment range found",
                };
                match self.retry_or_fail(&mut retries, msg).await {
                    Step::Retry => continue,
                    Step::Fail => return self.fail_info(msg),
                    Step::Cancelled => return None,
                }
            }
            return Some(info);
        }
    }

    pub(super) async fn get_video_info_ytdlp(&mut self, probe_head: bool) -> Option<FormatSelection> {
        let info = self.extract_usable(probe_head).await?;
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
        if let Some(sequence) = info.last_sq {
            self.last_sq = Some(sequence);
        }
        if info.target_duration > 0 {
            self.target_duration = info.target_duration;
        }
        if !self.in_progress
            && let Some(start) = info.start_unix
        {
            log_general!("Stream started at time {}", local_unix_timestamp(start));
        }

        if self.in_progress {
            self.live = info.is_live;
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
