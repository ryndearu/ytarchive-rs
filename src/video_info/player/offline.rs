use std::time::Duration;

use chrono::Utc;

use super::{Extractor, Outcome, POLL, sleep_until_unix};
use crate::stream_meta::local_timestamp;
use crate::types::player_response::PLAYABLE_OFFLINE;
use crate::types::{Action, DEFAULT_POLL_SECS, PlayerResponse};
use crate::util::sleep_wall_clock;
use crate::{log, log_debug, log_error, log_general, log_warn};

pub(super) struct OfflineWaitState {
    pub first_wait: bool,
    pub live_waited: u64,
    pub retry_count: u32,
    pub secs_late: u64,
    pub wait_on_channel: bool,
    pub newlines: bool,
}

pub(super) enum OfflineStep {
    Continue,
    Return(Outcome),
}

impl Extractor {
    /// Waits for a `PLAYABLE_OFFLINE` (scheduled) stream according to `--wait`
    /// / `--retry-stream` or its scheduled start time.
    pub(super) async fn handle_offline(
        &mut self,
        player_response: &PlayerResponse,
        author: &str,
        title: &str,
        state: &mut OfflineWaitState,
    ) -> OfflineStep {
        if self.in_progress {
            log_debug!("Livestream status is {PLAYABLE_OFFLINE} mid-download");
            return OfflineStep::Return(Outcome::NotUsable);
        }
        if self.reject_negative_live_from() {
            return OfflineStep::Return(Outcome::NotUsable);
        }
        if self.wait == Action::DoNot {
            log_error!("Stream has not started, and you have opted not to wait.");
            return OfflineStep::Return(Outcome::NotUsable);
        }
        if state.first_wait && self.wait == Action::Ask && self.retry_secs.is_none() {
            self.print_info(author, title);
            if !self.ask_wait_for_stream().await {
                return OfflineStep::Return(Outcome::NotUsable);
            }
        }
        if state.first_wait {
            self.ask_ahead().await;
        }

        if let Some(secs) = self.retry_secs {
            if state.wait_on_channel {
                self.reset_to_channel();
            }
            if state.first_wait {
                state.first_wait = false;
                log_general!("Waiting for stream, retrying every {secs} seconds...");
            }
            if !sleep_wall_clock(&self.cancel, Duration::from_secs(secs)).await {
                return OfflineStep::Return(Outcome::NotUsable);
            }
            state.live_waited += secs;
            state.retry_count += 1;
            log::retry_status(state.retry_count, state.live_waited, state.newlines);
            return OfflineStep::Continue;
        }

        let streamability = &player_response.playability_status.live_streamability.live_streamability_renderer;
        let broadcast = &player_response.microformat.player_microformat_renderer.live_broadcast_details;
        let scheduled =
            streamability.offline_slate.live_stream_offline_slate_renderer.scheduled_start_time.parse::<i64>();
        let Ok(scheduled) = scheduled else {
            log_warn!("Failed to get stream start time. Falling back to polling.");
            self.retry_secs = Some(DEFAULT_POLL_SECS);
            if !sleep_wall_clock(&self.cancel, POLL).await {
                return OfflineStep::Return(Outcome::NotUsable);
            }
            return OfflineStep::Continue;
        };

        let until = scheduled - Utc::now().timestamp();
        if until > 0 {
            if !state.first_wait {
                log_general!("Stream rescheduled.");
            }
            state.first_wait = false;
            state.secs_late = 0;
            log_general!(
                "Stream starts at {} in {until} seconds. Waiting for this time to elapse...",
                local_timestamp(&broadcast.start_timestamp)
            );
            if !sleep_until_unix(&self.cancel, scheduled).await {
                return OfflineStep::Return(Outcome::NotUsable);
            }
            return OfflineStep::Continue;
        }

        if state.first_wait {
            log_general!("Stream should have started. Checking back every {DEFAULT_POLL_SECS} seconds");
            state.first_wait = false;
        }
        if !sleep_wall_clock(&self.cancel, POLL).await {
            return OfflineStep::Return(Outcome::NotUsable);
        }
        state.secs_late += DEFAULT_POLL_SECS;
        log_general!("Stream is {} seconds late...", state.secs_late);
        OfflineStep::Continue
    }
}
