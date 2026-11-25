use std::collections::HashMap;

use tokio::task::{Id, JoinSet};

use crate::download::{PendingExport, StreamOutcome};
use crate::error::{Error, Result};
use crate::session::Session;
use crate::types::{DataType, PerType};
use crate::{log_error, log_warn};

#[derive(Default)]
pub(super) struct StreamCollection {
    pub frags: PerType<Option<u64>>,
    pub storage_failed: bool,
    pub stored_before_failing: bool,
    pub retry_until: Option<u64>,
    pub pending_exports: Vec<PendingExport>,
}

pub(super) async fn collect_stream_tasks(
    session: &Session,
    tasks: &mut JoinSet<(DataType, Result<StreamOutcome>)>,
    task_types: &HashMap<Id, DataType>,
) -> StreamCollection {
    let mut collected = StreamCollection::default();
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((data_type, Ok(outcome))) => {
                if !outcome.skipped.is_empty() {
                    log_warn!(
                        "{}: {} fragment(s) could not be downloaded and are missing: {:?}",
                        data_type.as_str(),
                        outcome.skipped.len(),
                        outcome.skipped
                    );
                }
                *collected.frags.get_mut(data_type) = Some(outcome.state.fragments);
                if let Some(time) = outcome.retry_until {
                    collected.retry_until = Some(collected.retry_until.map_or(time, |until| until.max(time)));
                }
                if let Some(export) = outcome.pending_export {
                    collected.pending_exports.push(export);
                }
            }
            Ok((data_type, Err(error))) => {
                // Every error a download returns is about its storage. One
                // that isn't from opening it came after data was stored.
                collected.storage_failed = true;
                collected.stored_before_failing |= !matches!(error, Error::OpenStorage(_));
                session.stop();
                log_error!("{} download failed: {error}", data_type.as_str());
            }
            Err(error) => {
                collected.storage_failed = true;
                collected.stored_before_failing = true;
                session.stop();
                match task_types.get(&error.id()) {
                    Some(data_type) => log_error!("{} download task panicked: {error}", data_type.as_str()),
                    None => log_error!("Download task panicked: {error}"),
                }
            }
        }
    }
    collected
}
