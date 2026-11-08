//! `--recover-private`: recovering fragments after a stream goes private or
//! unavailable mid-download (idea from livestream_dl). Download URLs that were already handed out keep
//! working for a while (until their `expire` time, usually about 6 hours
//! after they were issued), even when the watch page no longer gives out new
//! ones. So instead of giving up, every missing fragment is retried in rounds
//! with every URL the session has used, until all are stored, the URLs expire,
//! or rounds stop making progress. Needs SQLite storage, since recovered
//! fragments arrive out of order.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::task::JoinSet;

use super::db_sink::DbSink;
use super::fragment::{content_type, fragment_request, header_u64};
use super::sink::Event;
use crate::error::Result;
use crate::session::Session;
use crate::stream_meta::local_unix_timestamp;
use crate::types::{DataType, FragData, Fragment, FragmentUrl};
use crate::{log_debug, log_info, log_warn};

/// Time between rounds. livestream_dl waits a few minutes too: right after
/// a stream goes private, requests tend to fail and later ones succeed.
#[cfg(not(test))]
const ROUND_INTERVAL: Duration = Duration::from_secs(5 * 60);
#[cfg(test)]
const ROUND_INTERVAL: Duration = Duration::from_millis(20);
/// Rounds in a row without recovering anything before giving up (an hour).
const MAX_IDLE_ROUNDS: u32 = 12;

/// Fragments `start..` up to the head (and before `end`, from
/// `--capture-duration`) that the download should have.
#[derive(Debug, Clone, Copy)]
pub struct Span {
    pub start: u64,
    /// Newest known fragment.
    pub head: u64,
    pub end: Option<u64>,
}

impl Span {
    /// Not stored yet. The head itself is excluded, as the downloader never
    /// fetches it once a stream has ended (same as Go).
    fn unstored(&self, stored: &HashSet<u64>) -> Vec<u64> {
        let limit = self.end.map_or(self.head, |e| e.min(self.head));
        (self.start..limit).filter(|q| !stored.contains(q)).collect()
    }

    /// Not stored and really missing: the last two before the head of an
    /// ended stream were often never created (see `continue_fragment_download`),
    /// so they're tried but not required.
    pub fn missing(&self, stored: &HashSet<u64>) -> Vec<u64> {
        self.unstored(stored).into_iter().filter(|&q| q + 2 < self.head).collect()
    }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Tries `seq` once with each URL, starting at a different one each round.
async fn fetch(s: Arc<Session>, urls: Arc<Vec<FragmentUrl>>, seq: u64, offset: usize) -> (u64, Option<Fragment>) {
    for i in 0..urls.len() {
        if s.is_stopping() {
            break;
        }
        let url = &urls[(offset + i) % urls.len()];
        let resp = match fragment_request(&s, url, seq).send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                log_debug!("Recovery of fragment {seq}: HTTP {}", r.status());
                continue;
            }
            Err(e) => {
                log_debug!("Recovery of fragment {seq}: {e}");
                continue;
            }
        };
        let head = header_u64(resp.headers(), "x-head-seqnum");
        let mime_type = content_type(resp.headers());
        if let Ok(body) = resp.bytes().await
            && !body.is_empty()
        {
            let data = FragData::Memory(body);
            return (seq, Some(Fragment { seq, x_head_seqnum: head, data, mime_type }));
        }
    }
    (seq, None)
}

/// Runs recovery rounds for `span`, storing fragments in `db`. `on_stored`
/// gets the size of each fragment stored. Returns the span with the newest
/// known head.
pub async fn recover(
    s: &Arc<Session>,
    dt: DataType,
    db: &mut DbSink,
    mut span: Span,
    mut on_stored: impl FnMut(u64),
) -> Result<Span> {
    let name = format!("{}-recovery", dt.as_str());
    let threads = s.opts.threads.max(1);
    let (mut round, mut idle) = (0usize, 0u32);
    loop {
        let stored = db.stored()?;
        let missing = span.missing(&stored);
        if missing.is_empty() {
            if round > 0 {
                log_info!("{name}: All missing fragments were recovered");
            }
            break;
        }
        if round > 0 {
            if idle >= MAX_IDLE_ROUNDS {
                log_warn!("{name}: Nothing was recovered in {idle} rounds; giving up on {} fragment(s)", missing.len());
                break;
            }
            tokio::select! {
                _ = s.cancel.cancelled() => break,
                _ = tokio::time::sleep(ROUND_INTERVAL) => {}
            }
        }
        let now = unix_now();
        let urls: Vec<FragmentUrl> = s.known_urls(dt).into_iter().filter(|u| !u.is_expired(now)).collect();
        if urls.is_empty() {
            log_warn!("{name}: The download URLs have expired; {} fragment(s) can't be recovered", missing.len());
            break;
        }
        if round == 0 {
            let until = urls.iter().filter_map(FragmentUrl::expires_at).max();
            let until = until.map_or(String::new(), |t| {
                format!(" (they work until about {})", local_unix_timestamp(t as i64))
            });
            log_warn!(
                "{name}: The stream is no longer available. Trying to recover {} missing fragment(s) with the download URLs already in use{until}",
                missing.len()
            );
        }
        round += 1;

        let targets = span.unstored(&stored);
        log_debug!("{name}: Round {round}: {} fragment(s), {} URL(s)", targets.len(), urls.len());
        let urls = Arc::new(urls);
        let mut queue = targets.into_iter();
        let mut tasks = JoinSet::new();
        let mut recovered = 0u64;
        loop {
            while tasks.len() < threads
                && let Some(seq) = queue.next()
            {
                tasks.spawn(fetch(s.clone(), urls.clone(), seq, round));
            }
            let Some(res) = tasks.join_next().await else { break };
            let Ok((seq, Some(frag))) = res else { continue };
            if let Some(h) = frag.x_head_seqnum {
                span.head = span.head.max(h);
            }
            for ev in db.push(seq, Some(frag)).await? {
                if let Event::Wrote(n) = ev {
                    recovered += 1;
                    on_stored(n);
                }
            }
        }
        if s.is_stopping() {
            break;
        }
        if recovered > 0 {
            idle = 0;
            log_info!("{name}: Recovered {recovered} fragment(s) in round {round}");
        } else {
            idle += 1;
        }
    }
    Ok(span)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_targets() {
        let span = Span { start: 2, head: 10, end: None };
        let stored = HashSet::from([2, 3, 5]);
        assert_eq!(span.unstored(&stored), [4, 6, 7, 8, 9]);
        // 8 and 9 are within two of the head: tried, not required.
        assert_eq!(span.missing(&stored), [4, 6, 7]);
        let capped = Span { end: Some(6), ..span };
        assert_eq!(capped.unstored(&stored), [4]);
    }
}
