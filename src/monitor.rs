//! Entry point for downloads: one run, or with `--monitor-channel` a loop
//! that keeps recording a channel's streams.

use std::collections::HashSet;
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

pub async fn start(opts: Arc<Options>, http: Http, interrupt: Arc<Interrupt>) -> ExitCode {
    let raw_url = match &opts.url {
        Some(u) => u.clone(),
        None => get_user_input("Enter a youtube livestream URL: ").await,
    };
    let input = match parse_input_url(&raw_url) {
        Ok(i) => i,
        Err(e) => {
            log_error!("{e}");
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
    let mut recorded = HashSet::new();

    loop {
        let started = Instant::now();
        let r = run(opts.clone(), http.clone(), &input, interrupt.clone(), &recorded).await;
        if interrupt.is_set() || !monitor {
            return exit_code(r.code);
        }
        if r.downloaded
            && let Some(id) = r.video_id
        {
            log_general!("Finished with {id}; continuing to monitor the channel.");
            recorded.insert(id);
        }
        if started.elapsed() < interval {
            log_debug!("Last run exited before the set wait time. Waiting before running again...");
            if !interrupt.sleep(interval).await {
                return exit_code(2);
            }
        }
    }
}
