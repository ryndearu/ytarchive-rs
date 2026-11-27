//! Waiting on a channel's `/live` page: polling an idle channel and going
//! back to `/live` when the stream it showed can't be used.

use std::time::Duration;

use super::log_unplayable;
use crate::types::player_response::{PLAYABLE_ERROR, PLAYABLE_OFFLINE, PLAYABLE_OK, PLAYABLE_UNPLAYABLE};
use crate::types::{DEFAULT_POLL_SECS, PlayerResponse};
use crate::util::sleep_wall_clock;
use crate::video_info::Extractor;
use crate::{log, log_debug, log_error, log_general, log_warn};

impl Extractor {
    /// Restores channel `/live` polling after a resolved watch page turns out
    /// unavailable, offline, or not yet live.
    pub(super) fn reset_to_channel(&mut self) {
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
    pub(super) async fn reset_and_wait_on_channel(
        &mut self,
        waited: &mut u64,
        retries: &mut u32,
        newlines: bool,
    ) -> bool {
        self.reset_to_channel();
        self.wait_on_idle_channel(waited, retries, newlines).await
    }

    /// Reports an unusable resolved watch page once per `(video_id, state)`
    /// while waiting on a channel, returning `true` when polling should reset to `/live`.
    pub(super) fn report_channel_unusable(&mut self, player_response: &PlayerResponse, not_live: bool) -> bool {
        let status = player_response.playability_status.status.as_str();
        let broadcast = &player_response.microformat.player_microformat_renderer.live_broadcast_details;
        // /live can still show the stream that just ended (only judged on
        // the resolved watch page; /live's own response is incomplete).
        let ended =
            !self.is_channel && status == PLAYABLE_OK && !broadcast.is_live_now && !broadcast.end_timestamp.is_empty();
        let reason_key = if not_live {
            "NOT_LIVE"
        } else if ended {
            "ENDED"
        } else if !matches!(status, PLAYABLE_OK | PLAYABLE_OFFLINE) {
            status
        } else {
            return false;
        };
        let video_id = &player_response.video_details.video_id;
        let reason = &player_response.playability_status.reason;
        // Or /streams keeps offering it first, and another stream that is
        // live on the channel at the same time is never tried.
        self.channel_skip_ids.insert(video_id.clone());
        if self.reported_unusable.insert((video_id.clone(), reason_key.to_string())) {
            log::finish_inline();
            if not_live {
                log_warn!("This video ({video_id}) is not a livestream; continuing to check the channel.");
            } else if ended {
                log_general!(
                    "The stream on the channel's /live page ({video_id}) has ended; continuing to check the channel."
                );
            } else if status == PLAYABLE_UNPLAYABLE {
                log_unplayable(player_response, !broadcast.end_timestamp.is_empty(), self.in_progress);
            } else if status == PLAYABLE_ERROR {
                log_error!("Playability status: ERROR. Reason: {reason}");
            } else {
                // LOGIN_REQUIRED, AGE_CHECK_REQUIRED, and the like.
                log_warn!("Playability status: {status}. Reason: {reason}; continuing to check the channel.");
            }
        } else {
            log_debug!("{reason_key} ({reason}); returning to /live");
        }
        true
    }
}
