//! Top-level flow for one stream: resolve the URL, fetch info, download,
//! and finish (merge or handle an interrupt).

use std::collections::HashSet;
use std::process::ExitCode;
use std::sync::Arc;

use crate::http::Http;
use crate::input_url::InputUrl;
use crate::interrupt::Interrupt;
use crate::kept::prepared_from_kept;
use crate::live_from::resolve_live_from;
use crate::prepare::{prepare, print_info, print_upcoming, PrepareError};
use crate::types::Options;
use crate::{log_error, log_general, log_info};

mod download;
use download::download;

pub fn exit_code(code: i32) -> ExitCode {
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

/// Result of one run: exit code, the video it was for (if resolved), and
/// whether a download was attempted.
pub struct RunResult {
    pub code: i32,
    pub video_id: Option<String>,
    pub downloaded: bool,
}

impl RunResult {
    fn code(code: i32) -> Self {
        Self { code, video_id: None, downloaded: false }
    }
}

/// Downloads one stream. `skip_ids`: videos already recorded by the channel
/// monitor, which are not downloaded again.
pub async fn run(
    opts: Arc<Options>,
    http: Http,
    input: &InputUrl,
    interrupt: Arc<Interrupt>,
    skip_ids: &HashSet<String>,
) -> RunResult {
    let cancel = interrupt.child_token();
    let prepared = match prepare(&opts, &http, &cancel, input).await {
        Ok(p) => p,
        Err(PrepareError::Upcoming(u)) => {
            print_upcoming(&u);
            return RunResult { code: 0, video_id: Some(u.video_id), downloaded: false };
        }
        Err(PrepareError::Failed) => {
            // A download kept for retrying missing fragments can still go on
            // with its saved URLs after the stream went private.
            if !interrupt.is_set()
                && !opts.info_only
                && let InputUrl::Video { id } = input
                && let Some(p) = prepared_from_kept(&opts, id)
            {
                let code = download(opts.clone(), http, cancel, p, None, interrupt).await;
                return RunResult { code, video_id: Some(id.clone()), downloaded: true };
            }
            return RunResult::code(if interrupt.is_set() { 2 } else { 1 });
        }
    };
    if skip_ids.contains(&prepared.video_id) {
        log_info!("{} was already downloaded; not downloading it again", prepared.video_id);
        return RunResult { code: 0, video_id: Some(prepared.video_id), downloaded: false };
    }
    let video_id = Some(prepared.video_id.clone());

    let live_from_sq = match (&opts.live_from, prepared.last_sq) {
        (None, _) => None,
        (Some(v), Some(last)) => match resolve_live_from(v, last, prepared.target_duration) {
            Ok(lf) => {
                log_general!("{}", lf.message);
                Some(lf.start_sq)
            }
            Err(e) => {
                log_error!("{e}");
                return RunResult::code(1);
            }
        },
        (Some(_), None) => {
            log_error!("--live-from needs the current stream position, which could not be determined");
            return RunResult::code(1);
        }
    };

    if opts.info_only {
        print_info(&prepared, input);
        return RunResult { code: 0, video_id, downloaded: false };
    }
    let code = download(opts, http, cancel, prepared, live_from_sq, interrupt).await;
    RunResult { code, video_id, downloaded: true }
}

