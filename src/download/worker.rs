//! A fragment download worker: takes sequence numbers from the dispatcher
//! and sends the results to the stream's writer.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;

use super::fragment::download_fragment;
use crate::log_debug;
use crate::session::Session;
use crate::types::{DataType, FragThreadState, Fragment};
use crate::util::lock;

/// How long a worker waits when it's ahead of the newest fragment.
const AHEAD_WAIT: Duration = Duration::from_millis(500);

/// Hands out sequence numbers to workers, never running more than one
/// fragment past the newest known one.
#[derive(Debug)]
pub(super) struct Dispatch {
    pub next: u64,
    pub max_seq: Option<u64>,
    pub end_seq: Option<u64>,
    pub in_flight: usize,
    /// Fragments downloaded so far; shows a worker stuck on one fragment
    /// that the others are still getting through.
    pub downloaded: u64,
    /// Already stored (SQLite resume).
    pub skip: HashSet<u64>,
}

enum Take {
    Seq(u64, Option<u64>),
    Wait,
    Done,
}

impl Dispatch {
    fn take(&mut self, live: bool) -> Take {
        while self.skip.remove(&self.next) {
            self.next += 1;
        }
        if self.end_seq.is_some_and(|end| self.next >= end) {
            log_debug!("Reached the maximum duration specified by --capture-duration.");
            return Take::Done;
        }
        if let Some(max) = self.max_seq {
            if !live && self.next >= max {
                return Take::Done;
            }
            if self.next > max + 1 && self.in_flight > 0 {
                return Take::Wait;
            }
        }
        let seq = self.next;
        self.next += 1;
        self.in_flight += 1;
        Take::Seq(seq, self.max_seq)
    }

    fn complete(&mut self, downloaded: bool, head: Option<u64>) {
        self.in_flight = self.in_flight.saturating_sub(1);
        self.downloaded += u64::from(downloaded);
        if let Some(head_seq) = head {
            self.max_seq = Some(self.max_seq.map_or(head_seq, |max| max.max(head_seq)));
        }
    }
}

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
        let (take, downloaded) = {
            let mut dispatch = lock(&dispatch);
            (dispatch.take(session.is_live()), dispatch.downloaded)
        };
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
        state.reset_for(seq, max, downloaded);
        let frag_file = frag_prefix.as_ref().map(|path| PathBuf::from(format!("{}.frag{seq}.ts", path.display())));
        let frag = download_fragment(&session, &mut state, frag_file, &|| lock(&dispatch).downloaded).await;
        lock(&dispatch).complete(frag.is_some(), frag.as_ref().and_then(|fragment| fragment.x_head_seqnum));
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
