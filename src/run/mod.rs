//! Top-level flow for one stream: resolve the URL, fetch info, download
//! (output paths, the audio/video download tasks), and finish (merge or
//! handle an interrupt).

mod download;

use std::collections::HashSet;
use std::process::ExitCode;
use std::sync::Arc;

use download::download;

use crate::http::Http;
use crate::input_url::InputUrl;
use crate::interrupt::Interrupt;
use crate::log_info;
use crate::prepare::{PrepareOutcome, prepare, prepared_from_kept};
use crate::types::Options;

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
    let prepared = match prepare(&opts, &http, &cancel, input, skip_ids).await {
        PrepareOutcome::Ready(prepared) => *prepared,
        PrepareOutcome::Skipped(id) => {
            return RunResult { code: 0, video_id: Some(id), downloaded: false };
        }
        PrepareOutcome::Failed(failed_id) => {
            // A download kept for retrying missing fragments can still go on
            // with its saved URLs after the stream went private.
            if !interrupt.is_set()
                && let InputUrl::Video { id } = input
                && let Some(prepared) = prepared_from_kept(&opts, id)
            {
                let code = download(opts.clone(), http, cancel, prepared, interrupt).await;
                return RunResult { code, video_id: Some(id.clone()), downloaded: true };
            }
            if interrupt.is_set() {
                return RunResult::code(interrupt.cancel_code());
            }
            // A stream that resolved but can't be downloaded counts as a
            // failed attempt, so the channel monitor gives up on it.
            return RunResult { code: 1, downloaded: failed_id.is_some(), video_id: failed_id };
        }
    };
    if skip_ids.contains(&prepared.video_id) {
        log_info!("{} was already downloaded; not downloading it again", prepared.video_id);
        return RunResult { code: 0, video_id: Some(prepared.video_id), downloaded: false };
    }
    let video_id = Some(prepared.video_id.clone());
    let code = download(opts, http, cancel, prepared, interrupt).await;
    RunResult { code, video_id, downloaded: true }
}
