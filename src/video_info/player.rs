//! Default mode: stream status from the watch page's player response,
//! waiting for scheduled streams as needed.

use std::time::Duration;

use super::wait::sleep_until_unix;
use super::{ENDED_MESSAGE, Extractor, MAX_RETRIES, RETRY_DELAY};
use crate::stream_meta::local_timestamp;
use crate::types::player_response::{PLAYABLE_ERROR, PLAYABLE_OFFLINE, PLAYABLE_OK, PLAYABLE_UNPLAYABLE};
use crate::types::{Action, DEFAULT_POLL_SECS, PlayerResponse};
use crate::util::sleep_or_cancel;
use crate::watch_page::{get_video_html, parse_player_response, video_id_from_watch_page};
use crate::{log, log_debug, log_error, log_general, log_info, log_warn};

pub enum Outcome {
    Found(Box<PlayerResponse>),
    /// Couldn't get a player response at all.
    NotFound,
    /// Got one, but it can't be downloaded (not live, unplayable, user
    /// declined to wait, cancelled, ...).
    NotUsable,
}

const POLL: Duration = Duration::from_secs(DEFAULT_POLL_SECS);

impl Extractor {
    pub(super) async fn get_playable_player_response(&mut self) -> Outcome {
        let newlines = self.opts.status_newlines;
        let wait_on_channel = self.is_channel && self.retry_secs.is_some() && !self.in_progress;
        let mut first_wait = true;
        let mut live_waited = 0u64;
        let mut retry_count = 0u32;
        let mut response_retries = 0u32;
        let mut details_retries = 0u32;
        let mut secs_late = 0u64;

        loop {
            if self.cancel.is_cancelled() {
                return Outcome::NotUsable;
            }
            let html = get_video_html(&self.http, &self.page_url, self.is_channel, self.members_only).await;
            let parsed = match &html {
                Some(host) => parse_player_response(host).map_err(|error| error.to_string()),
                None => Err("unable to retrieve data from video page".to_string()),
            };

            let player_response = match parsed {
                Ok(player_response) => player_response,
                Err(err) => {
                    if wait_on_channel {
                        let secs = self.retry_secs.unwrap_or(DEFAULT_POLL_SECS);
                        if live_waited == 0 {
                            eprintln!();
                            self.ask_ahead().await;
                        }
                        if live_waited == 0 {
                            log_general!(
                                "You have opted to wait for a livestream to be scheduled. Retrying every {secs} seconds."
                            );
                        }
                        if !sleep_or_cancel(&self.cancel, Duration::from_secs(secs)).await {
                            return Outcome::NotUsable;
                        }
                        live_waited += secs;
                        retry_count += 1;
                        log::retry_status(retry_count, live_waited, newlines);
                        continue;
                    }
                    if response_retries < MAX_RETRIES {
                        response_retries += 1;
                        log_warn!("Error retrieving player response: {err} (Retry {response_retries}/{MAX_RETRIES})");
                        if !sleep_or_cancel(&self.cancel, RETRY_DELAY).await {
                            return Outcome::NotUsable;
                        }
                        continue;
                    }
                    eprintln!();
                    log_error!("Error retrieving player response, max retries reached.");
                    return Outcome::NotFound;
                }
            };

            if self.is_channel
                && let Some(id) = html.as_deref().and_then(video_id_from_watch_page)
            {
                self.video_id = id;
            }

            let details = &player_response.video_details;
            if details.video_id.is_empty() {
                if details_retries < MAX_RETRIES {
                    details_retries += 1;
                    log_warn!(
                        "Video Details not found, video may be private or does not exist. (Retry {details_retries}/{MAX_RETRIES})"
                    );
                    if !sleep_or_cancel(&self.cancel, RETRY_DELAY).await {
                        return Outcome::NotUsable;
                    }
                    continue;
                }
                if self.in_progress {
                    log_warn!("Video details no longer available mid download.");
                    log_warn!("Stream was likely privated after finishing.");
                    log_warn!("We will continue to download, but if it starts to fail, nothing can be done.");
                }
                log_error!("Video Details not found, max retries reached.");
                self.live = false;
                self.unavailable = true;
                return Outcome::NotUsable;
            }

            let (author, title) = (details.author.clone(), details.title.clone());
            self.print_info(&author, &title);

            let streamability_id =
                &player_response.playability_status.live_streamability.live_streamability_renderer.video_id;
            if streamability_id.is_empty() && !details.is_live_content {
                if self.live {
                    self.live = false;
                } else {
                    log_error!("This video is not a livestream. It would be better to use yt-dlp to download it.");
                }
                return Outcome::NotUsable;
            }

            let status = player_response.playability_status.status.as_str();
            match status {
                PLAYABLE_ERROR => {
                    if self.in_progress {
                        log_info!("Finishing download");
                    }
                    log_error!("Playability status: ERROR. Reason: {}", player_response.playability_status.reason);
                    self.live = false;
                    return Outcome::NotUsable;
                }
                PLAYABLE_UNPLAYABLE => {
                    // An ended stream is unplayable while YouTube processes
                    // it, and its live download is gone by then.
                    let ended = !player_response
                        .microformat
                        .player_microformat_renderer
                        .live_broadcast_details
                        .end_timestamp
                        .is_empty();
                    if ended && !self.in_progress {
                        log_general!("{ENDED_MESSAGE}");
                        log_general!("Reason: {}", player_response.playability_status.reason);
                        self.unavailable = true;
                        return Outcome::NotUsable;
                    }
                    let logged_in = !player_response.response_context.main_app_web_response_context.logged_out;
                    log_error!("Playability status: UNPLAYABLE.");
                    log_error!("Reason: {}", player_response.playability_status.reason);
                    log_error!("Logged in status: {logged_in}");
                    log_error!(
                        "If this is a members only stream, you provided a cookies.txt file, and the above 'logged in' status is not True, please try updating your cookies file."
                    );
                    self.unavailable = true;
                    return Outcome::NotUsable;
                }
                PLAYABLE_OFFLINE => {
                    if self.in_progress {
                        log_debug!("Livestream status is {PLAYABLE_OFFLINE} mid-download");
                        return Outcome::NotUsable;
                    }
                    if self.opts.live_from.as_deref().is_some_and(|value| value.trim().starts_with('-')) {
                        log_error!("Option --live-from with a negative duration is not valid for a scheduled stream.");
                        return Outcome::NotUsable;
                    }
                    if self.wait == Action::DoNot {
                        log_error!("Stream has not started, and you have opted not to wait.");
                        return Outcome::NotUsable;
                    }
                    if first_wait && self.wait == Action::Ask && self.retry_secs.is_none() {
                        self.print_info(&author, &title);
                        if !self.ask_wait_for_stream().await {
                            return Outcome::NotUsable;
                        }
                    }
                    if first_wait {
                        self.ask_ahead().await;
                    }

                    if let Some(secs) = self.retry_secs {
                        if first_wait {
                            first_wait = false;
                            log_general!("Waiting for stream, retrying every {secs} seconds...");
                        }
                        if !sleep_or_cancel(&self.cancel, Duration::from_secs(secs)).await {
                            return Outcome::NotUsable;
                        }
                        live_waited += secs;
                        retry_count += 1;
                        log::retry_status(retry_count, live_waited, newlines);
                        continue;
                    }

                    let scheduled = player_response
                        .playability_status
                        .live_streamability
                        .live_streamability_renderer
                        .offline_slate
                        .live_stream_offline_slate_renderer
                        .scheduled_start_time
                        .parse::<i64>();
                    let Ok(scheduled) = scheduled else {
                        log_warn!("Failed to get stream start time. Falling back to polling.");
                        self.retry_secs = Some(DEFAULT_POLL_SECS);
                        if !sleep_or_cancel(&self.cancel, POLL).await {
                            return Outcome::NotUsable;
                        }
                        continue;
                    };

                    let until = scheduled - chrono::Utc::now().timestamp();
                    if until > 0 {
                        if !first_wait {
                            log_general!("Stream rescheduled.");
                        }
                        first_wait = false;
                        secs_late = 0;
                        log_general!(
                            "Stream starts at {} in {until} seconds. Waiting for this time to elapse...",
                            local_timestamp(
                                &player_response
                                    .microformat
                                    .player_microformat_renderer
                                    .live_broadcast_details
                                    .start_timestamp
                            )
                        );
                        if !sleep_until_unix(&self.cancel, scheduled).await {
                            return Outcome::NotUsable;
                        }
                        continue;
                    }

                    if first_wait {
                        log_general!("Stream should have started. Checking back every {DEFAULT_POLL_SECS} seconds");
                        first_wait = false;
                    }
                    if !sleep_or_cancel(&self.cancel, POLL).await {
                        return Outcome::NotUsable;
                    }
                    secs_late += DEFAULT_POLL_SECS;
                    log_general!("Stream is {secs_late} seconds late...");
                    continue;
                }
                PLAYABLE_OK => {
                    // The /live page's player response is incomplete; reload
                    // via the resolved watch URL.
                    if self.is_channel {
                        if self.video_id.is_empty() {
                            self.video_id = details.video_id.clone();
                        }
                        self.page_url = format!("https://www.youtube.com/watch?v={}", self.video_id);
                        self.members_only = false;
                        self.is_channel = false;
                        continue;
                    }

                    let details = &player_response.microformat.player_microformat_renderer.live_broadcast_details;
                    // An ended stream is downloadable as long as its live
                    // fragments still exist; that is decided from yt-dlp's
                    // URLs afterwards, not from the watch page's formats.
                    if !details.is_live_now && !self.in_progress && details.end_timestamp.is_empty() {
                        // Waiting on a channel (always so with --monitor-channel):
                        // go back to /live instead of polling this video, so
                        // whichever stream starts first is found. Go kept
                        // polling the video.
                        if wait_on_channel && let Some(live_url) = self.channel_url.clone() {
                            let secs = self.retry_secs.unwrap_or(DEFAULT_POLL_SECS);
                            if live_waited == 0 {
                                log_general!(
                                    "The stream on the channel's /live page has not started. Checking the channel every {secs} seconds."
                                );
                            }
                            self.page_url = live_url;
                            self.is_channel = true;
                            self.members_only = self.opts.members_only;
                            self.video_id.clear();
                            if !sleep_or_cancel(&self.cancel, Duration::from_secs(secs)).await {
                                return Outcome::NotUsable;
                            }
                            live_waited += secs;
                            retry_count += 1;
                            log::retry_status(retry_count, live_waited, newlines);
                            continue;
                        }
                        log_general!("Livestream is offline, should have started, and does not have an end timestamp.");
                        log_general!("Waiting {DEFAULT_POLL_SECS} seconds and trying again.");
                        if !sleep_or_cancel(&self.cancel, POLL).await {
                            return Outcome::NotUsable;
                        }
                        continue;
                    }
                }
                other => {
                    if secs_late > 0 {
                        eprintln!();
                    }
                    log_error!("Unknown playability status: {other}");
                    if self.in_progress {
                        self.live = false;
                    }
                    return Outcome::NotUsable;
                }
            }

            if secs_late > 0 {
                eprintln!();
            }
            return Outcome::Found(Box::new(player_response));
        }
    }
}
