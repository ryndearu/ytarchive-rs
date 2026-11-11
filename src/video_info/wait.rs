//! Waiting for scheduled streams.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::Extractor;
use crate::log_general;
use crate::prompt::get_user_input;
use crate::types::{Action, DEFAULT_POLL_SECS};

/// Sleeps unless cancelled first. Returns `false` if cancelled.
pub async fn sleep_or_cancel(cancel: &CancellationToken, dur: Duration) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(dur) => true,
    }
}

/// Sleeps until `unix` (re-checking the clock, as a long sleep can drift).
pub async fn sleep_until_unix(cancel: &CancellationToken, unix: i64) -> bool {
    loop {
        let left = unix - chrono::Utc::now().timestamp();
        if left <= 0 {
            return true;
        }
        if !sleep_or_cancel(cancel, Duration::from_secs(left as u64)).await {
            return false;
        }
    }
}

/// Parses the poll-interval answer; anything invalid or too small becomes the default.
pub fn parse_poll_secs(answer: &str) -> u64 {
    answer.trim().parse::<u64>().ok().filter(|seconds| *seconds >= DEFAULT_POLL_SECS).unwrap_or(DEFAULT_POLL_SECS)
}

#[derive(Debug, PartialEq, Eq)]
pub enum WaitChoice {
    Wait,
    Poll,
    No,
}

/// Reads the scheduled-stream answer. Go asked `wait/poll/[no]`; the numbers
/// are new, the words still work, and anything else (Enter, closed stdin) is
/// "no", as in Go.
pub fn parse_wait_choice(answer: &str) -> WaitChoice {
    let arg = answer.trim().to_lowercase();
    if arg == "1" || arg.starts_with("wait") {
        WaitChoice::Wait
    } else if arg == "2" || arg.starts_with("poll") {
        WaitChoice::Poll
    } else {
        WaitChoice::No
    }
}

impl Extractor {
    /// Asks whether to wait for the scheduled time, poll, or give up.
    pub(super) async fn ask_wait_for_stream(&mut self) -> bool {
        log_general!(
            "This stream is likely a future scheduled livestream.\n\
             Would you like to wait for the scheduled start time, poll until it starts, or not wait?"
        );
        let answer = get_user_input("1) wait  2) poll  3) no (default 3): ").await;
        match parse_wait_choice(&answer) {
            WaitChoice::Wait => {
                self.wait = Action::Do;
                true
            }
            WaitChoice::Poll => {
                let secs =
                    get_user_input(&format!("Input poll interval in seconds (minimum {DEFAULT_POLL_SECS}): ")).await;
                self.retry_secs = Some(parse_poll_secs(&secs));
                true
            }
            WaitChoice::No => false,
        }
    }

    /// How long to wait before checking an upcoming stream again (yt-dlp
    /// mode). `None` = don't wait.
    pub(super) async fn upcoming_wait_interval(&mut self, start_unix: Option<i64>) -> Option<Duration> {
        if self.wait == Action::DoNot {
            return None;
        }
        if let Some(seconds) = self.retry_secs {
            return Some(Duration::from_secs(seconds));
        }
        if self.wait == Action::Ask {
            if !self.ask_wait_for_stream().await {
                return None;
            }
            if let Some(seconds) = self.retry_secs {
                return Some(Duration::from_secs(seconds));
            }
        }
        let until_start = start_unix.map(|time| time - chrono::Utc::now().timestamp()).filter(|seconds| *seconds > 0);
        Some(Duration::from_secs(until_start.map_or(DEFAULT_POLL_SECS, |seconds| seconds as u64)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_choice() {
        use WaitChoice::*;
        for (answer, want) in
            [("1", Wait), ("Wait", Wait), ("2", Poll), ("poll", Poll), ("", No), ("3", No), ("no", No), ("x", No)]
        {
            assert_eq!(parse_wait_choice(answer), want, "{answer:?}");
        }
    }
}
