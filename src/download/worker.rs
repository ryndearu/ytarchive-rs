//! A fragment download worker: takes sequence numbers from the dispatcher
//! and sends the results to the stream's writer.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;

use super::dispatch::{Dispatch, Take};
use super::fragment::download_fragment;
use crate::log_debug;
use crate::session::Session;
use crate::types::{DataType, FragThreadState, Fragment};
use crate::util::lock;

/// How long a worker waits when it's ahead of the newest fragment.
const AHEAD_WAIT: Duration = Duration::from_millis(500);

pub(super) enum Msg {
    Done(Fragment),
    /// Given up on; the sink skips it instead of stalling forever.
    Missing(u64),
}

pub(super) async fn worker(
    session: Arc<Session>,
    data_type: DataType,
    name: String,
    dispatch: Arc<Mutex<Dispatch>>,
    sender: mpsc::Sender<Msg>,
    frag_prefix: Option<PathBuf>,
    retry_sleep: Duration,
) {
    let mut state = FragThreadState::new(name.clone(), data_type, retry_sleep);
    loop {
        if session.is_stopping() || session.is_finished(data_type) {
            break;
        }
        let take = lock(&dispatch).take(session.is_live());
        let (seq, max) = match take {
            // Only this worker stops. Go marked the whole stream finished here,
            // which made other workers abandon fragments they were retrying.
            Take::Done => {
                log_debug!("{name}: Stream is finished and highest sequence reached");
                break;
            }
            Take::Wait => {
                tokio::select! {
                    _ = session.cancel.cancelled() => break,
                    _ = tokio::time::sleep(AHEAD_WAIT) => continue,
                }
            }
            Take::Seq(seq, max) => (seq, max),
        };
        state.reset_for(seq, max);
        let frag_file = frag_prefix.as_ref().map(|path| PathBuf::from(format!("{}.frag{seq}.ts", path.display())));
        let frag = download_fragment(&session, &mut state, frag_file).await;
        lock(&dispatch).complete(frag.as_ref().and_then(|fragment| fragment.x_head_seqnum));
        let msg = match frag {
            Some(fragment) => Msg::Done(fragment),
            None => Msg::Missing(seq),
        };
        if sender.send(msg).await.is_err() {
            break;
        }
    }
    log_debug!("{name}: exiting");
}
