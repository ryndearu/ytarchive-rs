//! Default mode: stream status from the watch page's player response,
//! waiting for scheduled streams as needed.

mod offline;

use std::time::Duration;

use self::offline::{OfflineStep, OfflineWaitState};
use super::{ENDED_MESSAGE, Extractor, MAX_RETRIES, RETRY_DELAY, sleep_until_unix};
use crate::error::Error;
use crate::types::player_response::{PLAYABLE_ERROR, PLAYABLE_OFFLINE, PLAYABLE_OK, PLAYABLE_UNPLAYABLE};
use crate::types::{DEFAULT_POLL_SECS, PlayerResponse};
use crate::util::{sleep_or_cancel, sleep_wall_clock};
use crate::watch_page::{get_video_html, parse_player_response, video_id_from_watch_page};
use crate::{log, log_debug, log_error, log_general, log_info, log_warn};

pub enum Outcome {
    Found(Box<PlayerResponse>),
    /// Couldn't get a player response at all (permanent or unparsed).
    NotFound,
    /// Temporary network/server failure; a running download stays refreshable.
    TransientError,
    /// Got one, but it can't be downloaded (not live, unplayable, user
    /// declined to wait, cancelled, ...).
    NotUsable,
}

fn is_transient_http(error: &Error) -> bool {
    match error {
        Error::Http(_) | Error::Io(_) => true,
        Error::HttpStatus { status, .. } => *status == 429 || *status >= 500,
        _ => false,
    }
}

pub(super) const POLL: Duration = Duration::from_secs(DEFAULT_POLL_SECS);

fn log_unplayable(player_response: &PlayerResponse, ended: bool, in_progress: bool) {
    if ended && !in_progress {
        log_general!("{ENDED_MESSAGE}");
        log_general!("Reason: {}", player_response.playability_status.reason);
        return;
    }
    let logged_in = !player_response.response_context.main_app_web_response_context.logged_out;
    log_error!("Playability status: UNPLAYABLE.");
    log_error!("Reason: {}", player_response.playability_status.reason);
    log_error!("Logged in status: {logged_in}");
    log_error!(
        "If this is a members only stream, you provided a cookies.txt file, and the above 'logged in' status is not True, please try updating your cookies file."
    );
}

impl Extractor {
    /// Restores channel `/live` polling after a resolved watch page turns out
    /// unavailable, offline, or not yet live.
    fn reset_to_channel(&mut self) {
        if let Some(live_url) = self.channel_url.clone() {
            self.page_url = live_url;
            self.is_channel = true;
            self.members_only = self.opts.members_only;
            self.video_id.clear();
        }
    }

    /// Waits one poll interval on an idle channel, asking for quality ahead on
    /// the first wait. Returns `false` if cancelled.
    async fn wait_on_idle_channel(&mut self, live_waited: &mut u64, retry_count: &mut u32, newlines: bool) -> bool {
        let secs = self.retry_secs.unwrap_or(DEFAULT_POLL_SECS);
        if *live_waited == 0 {
            log::finish_inline();
            self.ask_ahead().await;
            log_general!("You have opted to wait for a livestream to be scheduled. Retrying every {secs} seconds.");
        }
        if !sleep_wall_clock(&self.cancel, Duration::from_secs(secs)).await {
            return false;
        }
        *live_waited += secs;
        *retry_count += 1;
        log::retry_status(*retry_count, *live_waited, newlines);
        true
    }

    /// Restores channel `/live` polling and waits one poll interval.
    async fn reset_and_wait_on_channel(&mut self, waited: &mut u64, retries: &mut u32, newlines: bool) -> bool {
        self.reset_to_channel();
        self.wait_on_idle_channel(waited, retries, newlines).await
    }

    /// Reports an unusable resolved watch page once per `(video_id, state)`
    /// while waiting on a channel, returning `true` when polling should reset to `/live`.
    fn report_channel_unusable(&mut self, player_response: &PlayerResponse, not_live: bool) -> bool {
        let status = player_response.playability_status.status.as_str();
        let reason_key = if not_live {
            "NOT_LIVE"
        } else if matches!(status, PLAYABLE_ERROR | PLAYABLE_UNPLAYABLE) {
            status
        } else {
            return false;
        };
        let video_id = &player_response.video_details.video_id;
        let reason = &player_response.playability_status.reason;
        if self.reported_unusable.insert((video_id.clone(), reason_key.to_string())) {
            log::finish_inline();
            if not_live {
                log_warn!("This video ({video_id}) is not a livestream; continuing to check the channel.");
            } else if status == PLAYABLE_ERROR {
                log_error!("Playability status: ERROR. Reason: {reason}");
            } else {
                let ended = !player_response
                    .microformat
                    .player_microformat_renderer
                    .live_broadcast_details
                    .end_timestamp
                    .is_empty();
                log_unplayable(player_response, ended, self.in_progress);
            }
        } else {
            log_debug!("{reason_key} ({reason}); returning to /live");
        }
        true
    }

    pub(super) async fn get_playable_player_response(&mut self) -> Outcome {
        let newlines = self.opts.status_newlines;
        let wait_on_channel = self.is_channel && self.retry_secs.is_some() && !self.in_progress;
        let mut wait_state = OfflineWaitState {
            first_wait: true,
            live_waited: 0,
            retry_count: 0,
            secs_late: 0,
            wait_on_channel,
            newlines,
        };
        let mut response_retries = 0u32;
        let mut all_transient = true;
        let mut details_retries = 0u32;
        let mut last_warned: Option<String> = None;

        loop {
            if self.cancel.is_cancelled() {
                return Outcome::NotUsable;
            }
            let (parsed, transient) = match get_video_html(
                &self.http,
                &self.page_url,
                self.is_channel,
                self.members_only,
                &self.skip_ids,
                &self.cancel,
            )
            .await
            {
                Ok(html_page) => {
                    let page_id = if self.is_channel { video_id_from_watch_page(&html_page) } else { None };
                    match parse_player_response(&html_page) {
                        Ok(player_response) => {
                            if let Some(id) = page_id {
                                self.video_id = id;
                            }
                            (Ok(player_response), false)
                        }
                        Err(error) => (Err(error.to_string()), false),
                    }
                }
                Err(Error::Cancelled) => return Outcome::NotUsable,
                Err(error) => {
                    let transient = is_transient_http(&error);
                    (Err(error.to_string()), transient)
                }
            };

            let player_response = match parsed {
                Ok(player_response) => {
                    response_retries = 0;
                    all_transient = true;
                    player_response
                }
                Err(err) => {
                    if wait_on_channel {
                        // An offline channel still returns a player response, so
                        // this is a mistyped channel, expired cookies, or a page
                        // change: warn once per distinct error instead of
                        // polling silently forever.
                        if transient || last_warned.as_deref() == Some(err.as_str()) {
                            log_debug!("Error while waiting on channel: {err}");
                        } else {
                            log::finish_inline();
                            log_warn!("Error while waiting on channel: {err}");
                            last_warned = Some(err);
                        }
                        if !self
                            .reset_and_wait_on_channel(
                                &mut wait_state.live_waited,
                                &mut wait_state.retry_count,
                                newlines,
                            )
                            .await
                        {
                            return Outcome::NotUsable;
                        }
                        continue;
                    }
                    all_transient &= transient;
                    if response_retries < MAX_RETRIES {
                        response_retries += 1;
                        log_warn!("Error retrieving player response: {err} (Retry {response_retries}/{MAX_RETRIES})");
                        if !sleep_or_cancel(&self.cancel, RETRY_DELAY).await {
                            return Outcome::NotUsable;
                        }
                        continue;
                    }
                    log::finish_inline();
                    if all_transient {
                        if self.in_progress {
                            log_warn!("Temporary error retrieving player response; continuing with current URLs.");
                        } else {
                            log_error!("Error retrieving player response, max retries reached.");
                        }
                        return Outcome::TransientError;
                    }
                    log_error!("Error retrieving player response, max retries reached.");
                    return Outcome::NotFound;
                }
            };

            let details = &player_response.video_details;
            if details.video_id.is_empty() {
                if wait_on_channel {
                    if !self
                        .reset_and_wait_on_channel(&mut wait_state.live_waited, &mut wait_state.retry_count, newlines)
                        .await
                    {
                        return Outcome::NotUsable;
                    }
                    continue;
                }
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
            details_retries = 0;

            if !self.in_progress && self.skip_ids.contains(&details.video_id) {
                self.video_id = details.video_id.clone();
                self.skipped_already_recorded = true;
                log_info!("{} was already downloaded; not downloading it again", details.video_id);
                return Outcome::NotUsable;
            }

            let (author, title) = (details.author.clone(), details.title.clone());
            self.print_info(&author, &title);

            let status = player_response.playability_status.status.as_str();
            let streamability = &player_response.playability_status.live_streamability.live_streamability_renderer;
            let broadcast = &player_response.microformat.player_microformat_renderer.live_broadcast_details;
            let not_live = streamability.video_id.is_empty() && !details.is_live_content;
            if wait_on_channel && self.report_channel_unusable(&player_response, not_live) {
                if !self
                    .reset_and_wait_on_channel(&mut wait_state.live_waited, &mut wait_state.retry_count, newlines)
                    .await
                {
                    return Outcome::NotUsable;
                }
                continue;
            }
            if not_live {
                if self.live {
                    self.live = false;
                } else {
                    log_error!("This video is not a livestream. It would be better to use yt-dlp to download it.");
                }
                return Outcome::NotUsable;
            }

            match status {
                PLAYABLE_ERROR => {
                    if self.in_progress {
                        log_info!("Finishing download");
                    }
                    log_error!("Playability status: ERROR. Reason: {}", player_response.playability_status.reason);
                    self.live = false;
                    if self.in_progress {
                        self.unavailable = true;
                    }
                    return Outcome::NotUsable;
                }
                PLAYABLE_UNPLAYABLE => {
                    let ended = !broadcast.end_timestamp.is_empty();
                    log_unplayable(&player_response, ended, self.in_progress);
                    if !(ended && !self.in_progress) {
                        self.live = false;
                    }
                    self.unavailable = true;
                    return Outcome::NotUsable;
                }
                PLAYABLE_OFFLINE => match self.handle_offline(&player_response, &author, &title, &mut wait_state).await
                {
                    OfflineStep::Continue => continue,
                    OfflineStep::Return(outcome) => return outcome,
                },
                PLAYABLE_OK => {
                    // The /live page's player response is incomplete; reload
                    // via the resolved watch URL.
                    if self.is_channel {
                        if self.video_id.is_empty() {
                            self.video_id = details.video_id.clone();
                        }
                        self.page_url = self.watch_url(&self.video_id);
                        self.members_only = false;
                        self.is_channel = false;
                        continue;
                    }

                    // An ended stream is downloadable as long as its live
                    // fragments still exist; that is decided from yt-dlp's
                    // URLs afterwards, not from the watch page's formats.
                    if !broadcast.is_live_now && !self.in_progress && broadcast.end_timestamp.is_empty() {
                        // Waiting on a channel (always so with --monitor-channel):
                        // go back to /live instead of polling this video, so
                        // whichever stream starts first is found. Go kept
                        // polling the video.
                        if wait_on_channel && self.channel_url.is_some() {
                            let secs = self.retry_secs.unwrap_or(DEFAULT_POLL_SECS);
                            if wait_state.live_waited == 0 {
                                log::finish_inline();
                                self.ask_ahead().await;
                                log_general!(
                                    "The stream on the channel's /live page has not started. Checking the channel every {secs} seconds."
                                );
                            }
                            self.reset_to_channel();
                            if !sleep_wall_clock(&self.cancel, Duration::from_secs(secs)).await {
                                return Outcome::NotUsable;
                            }
                            wait_state.live_waited += secs;
                            wait_state.retry_count += 1;
                            log::retry_status(wait_state.retry_count, wait_state.live_waited, newlines);
                            continue;
                        }
                        log_general!("Livestream is offline, should have started, and does not have an end timestamp.");
                        log_general!("Waiting {DEFAULT_POLL_SECS} seconds and trying again.");
                        if !sleep_wall_clock(&self.cancel, POLL).await {
                            return Outcome::NotUsable;
                        }
                        continue;
                    }
                }
                other => {
                    if wait_state.secs_late > 0 {
                        log::finish_inline();
                    }
                    log_error!("Unknown playability status: {other}");
                    if self.in_progress {
                        self.live = false;
                        self.unavailable = true;
                    }
                    return Outcome::NotUsable;
                }
            }

            if wait_state.secs_late > 0 {
                log::finish_inline();
            }
            return Outcome::Found(Box::new(player_response));
        }
    }
}
