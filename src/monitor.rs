//! Entry point for downloads: one run, or with `--monitor-channel` a loop
//! that keeps recording a channel's streams.

use std::collections::{HashMap, HashSet};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::http::Http;
use crate::input_url::{InputUrl, parse_input_url};
use crate::interrupt::Interrupt;
use crate::prompt::get_user_input;
use crate::run::{exit_code, run};
use crate::types::{DEFAULT_MONITOR_SECS, Options};
use crate::{log_debug, log_error, log_general, log_warn};

const MAX_MONITOR_FAILURES: u32 = 2;

#[derive(Debug, PartialEq, Eq)]
enum MonitorOutcome {
    Finished,
    Retry,
    GaveUp(u32),
}

#[derive(Default)]
struct MonitorTracker {
    recorded: HashSet<String>,
    failures: HashMap<String, u32>,
}

impl MonitorTracker {
    fn recorded(&self) -> &HashSet<String> {
        &self.recorded
    }

    fn record_result(&mut self, id: String, code: i32) -> MonitorOutcome {
        if code == 0 {
            self.failures.remove(&id);
            self.recorded.insert(id);
            return MonitorOutcome::Finished;
        }
        let attempts = {
            let count = self.failures.entry(id.clone()).or_insert(0);
            *count += 1;
            *count
        };
        if attempts >= MAX_MONITOR_FAILURES {
            self.failures.remove(&id);
            self.recorded.insert(id);
            return MonitorOutcome::GaveUp(attempts);
        }
        MonitorOutcome::Retry
    }
}

pub async fn start(opts: Arc<Options>, http: Http, interrupt: Arc<Interrupt>) -> ExitCode {
    let raw_url = match &opts.url {
        Some(url) => url.clone(),
        None => get_user_input("Enter a youtube livestream URL: ").await,
    };
    let input = match parse_input_url(&raw_url) {
        Ok(input) => input,
        Err(error) => {
            log_error!("{error}");
            return ExitCode::FAILURE;
        }
    };

    let monitor = opts.monitor_channel && matches!(input, InputUrl::Channel { .. });
    if opts.monitor_channel && !monitor {
        log_warn!("--monitor-channel only works with channel URLs; downloading this video once.");
    }
    let interval = opts.retry_stream.unwrap_or(Duration::from_secs(DEFAULT_MONITOR_SECS));
    // Go re-ran /live right after a stream ended and could download the same
    // stream again; remember what was recorded instead.
    let mut tracker = MonitorTracker::default();

    loop {
        let started = Instant::now();
        let outcome = run(opts.clone(), http.clone(), &input, interrupt.clone(), tracker.recorded()).await;
        if interrupt.is_set() || !monitor {
            return exit_code(outcome.code);
        }
        if outcome.downloaded
            && let Some(id) = outcome.video_id
        {
            let display_id = id.clone();
            match tracker.record_result(id, outcome.code) {
                MonitorOutcome::Finished => {
                    log_general!("Finished with {display_id}; continuing to monitor the channel.");
                }
                MonitorOutcome::Retry => {
                    log_warn!(
                        "Download of {display_id} exited with code {}; continuing to monitor the channel.",
                        outcome.code
                    );
                }
                MonitorOutcome::GaveUp(attempts) => {
                    log_warn!(
                        "Giving up on {display_id} after {attempts} failed attempts; continuing to monitor the channel."
                    );
                }
            }
        }
        if started.elapsed() < interval {
            log_debug!("Last run exited before the set wait time. Waiting before running again...");
            if !interrupt.sleep(interval).await {
                return exit_code(if interrupt.terminated() { 143 } else { 2 });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_retries_once_then_gives_up() {
        let mut tracker = MonitorTracker::default();
        assert_eq!(tracker.record_result("ok".into(), 0), MonitorOutcome::Finished);
        assert!(tracker.recorded().contains("ok"));

        assert_eq!(tracker.record_result("bad".into(), 1), MonitorOutcome::Retry);
        assert!(!tracker.recorded().contains("bad"));
        assert_eq!(tracker.record_result("bad".into(), 1), MonitorOutcome::GaveUp(2));
        assert!(tracker.recorded().contains("bad"));

        assert_eq!(tracker.record_result("flaky".into(), 1), MonitorOutcome::Retry);
        assert_eq!(tracker.record_result("flaky".into(), 0), MonitorOutcome::Finished);
        assert!(tracker.recorded().contains("flaky"));
    }
}
