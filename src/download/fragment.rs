//! Downloading one fragment with Go ytarchive's retry policy.

use std::path::PathBuf;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{CONTENT_TYPE, ORIGIN, REFERER};

use crate::session::Session;
use crate::types::{FragData, FragThreadState, Fragment, FragmentUrl};
use crate::util::write_file_with_mode;
use crate::{log_debug, log_trace, log_warn};

/// The longest wait between attempts that backing off can reach (a longer
/// fragment duration or `Retry-After` still applies).
const MAX_BACKOFF: Duration = Duration::from_secs(15);
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);
/// A fragment of a live stream is given up on after its tries ran out this
/// many times, each time with other fragments downloaded meanwhile, and at
/// least `STUCK_BEHIND` of them since it was first tried.
const STUCK_ROUNDS: u32 = 4;
const STUCK_BEHIND: u64 = 30;

/// A request error for logging: without the URL's query string, which holds
/// the signature and the user's IP address.
pub fn without_query(mut error: reqwest::Error) -> reqwest::Error {
    if let Some(url) = error.url_mut() {
        url.set_query(None);
    }
    error
}

/// Whether a live fragment whose tries just ran out is stuck: it keeps
/// failing while the rest of the stream downloads. `downloaded` is the
/// stream's fragment count now. A connection or server outage stops every
/// fragment, so it never counts.
fn stuck_while_live(state: &mut FragThreadState, downloaded: u64) -> bool {
    if downloaded > state.downloaded_at_round {
        state.stuck_rounds += 1;
    }
    state.downloaded_at_round = downloaded;
    state.stuck_rounds >= STUCK_ROUNDS && downloaded.saturating_sub(state.downloaded_at_start) >= STUCK_BEHIND
}

/// Whether to keep retrying after a failed attempt. When the per-fragment
/// try limit is hit: while live, start over (unless only this fragment is
/// failing); once the stream has ended, give up (or allow a few extra rounds
/// for fragments well behind the head). `downloaded` is how many fragments
/// the stream has downloaded so far.
pub fn continue_fragment_download(session: &Session, state: &mut FragThreadState, downloaded: u64) -> bool {
    if session.is_finished(state.data_type) {
        return false;
    }
    let max = session.opts.frag_max_tries;
    if max == 0 || state.tries < max {
        return true;
    }

    state.full_retries = state.full_retries.saturating_sub(1);
    log_debug!("{}: Fragment {}: {}/{max} retries", state.name, state.seq, state.tries);

    if session.is_live() && !session.is_unavailable() {
        // Go retried forever here, so one fragment YouTube never serves
        // kept everything after it waiting (in memory or fragment files)
        // until the stream ended.
        if stuck_while_live(state, downloaded) {
            log_debug!("{}: Fragment {}: giving up, the rest of the stream is downloading", state.name, state.seq);
            return false;
        }
        log_debug!("{}: Fragment {}: Stream still live, continuing download attempt", state.name, state.seq);
        state.tries = 0;
        return true;
    }

    if state.is_403 {
        if session.is_unavailable() {
            log_warn!(
                "{}: Download link likely expired and stream is privated or members only, cannot continue download",
                state.name
            );
        } else {
            log_warn!(
                "{}: Download link has likely expired and the stream has probably finished processing.",
                state.name
            );
            log_warn!("{}: You might want to use yt-dlp to download instead.", state.name);
        }
        session.set_finished(state.data_type);
        return false;
    }

    let behind_head = state.max_seq.is_some_and(|max| state.seq + 2 < max);
    if behind_head {
        if state.full_retries > 0 {
            log_debug!("{}: More than two fragments away from the highest known fragment", state.name);
            log_debug!("{}: Will try grabbing the fragment {} more times", state.name, state.full_retries);
            // Go didn't reset the count here, so its loop exited immediately
            // and the fragment was dropped without the extra attempts.
            state.tries = 0;
            return true;
        }
        // Give up on this fragment only. Go marked the whole stream finished
        // here, silently dropping everything after one bad fragment.
        return false;
    }

    // Within two of the head after the stream ended: it was probably never
    // created, so the stream is done.
    session.set_finished(state.data_type);
    false
}

/// After a failure that used up the try limit, refresh URLs (unless the
/// limit is 0, i.e. unlimited).
async fn retry_fragment_url(session: &Session, state: &FragThreadState, url: &FragmentUrl) {
    if session.is_finished(state.data_type) || session.is_stopping() {
        return;
    }
    let max = session.opts.frag_max_tries;
    if max > 0 && state.tries >= max {
        session.refresh_url_if_unchanged(state.data_type, url).await;
    }
}

fn handle_http_error(session: &Session, state: &mut FragThreadState, response: &reqwest::Response) {
    let status = response.status();
    log_debug!("{}: HTTP Error for fragment {}: {status}", state.name, state.seq);
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        state.backoff_step += 1;
        state.retry_after = header_u64(response.headers(), "retry-after").map(Duration::from_secs);
    } else {
        state.backoff_step = 0;
    }
    if status == StatusCode::FORBIDDEN {
        state.is_403 = true;
    } else if status == StatusCode::NOT_FOUND
        && !session.is_live()
        && state.max_seq.is_some_and(|max| state.seq + 2 > max)
    {
        log_debug!(
            "{}: Stream has ended and fragment within the last two not found, probably not actually created",
            state.name
        );
        session.set_finished(state.data_type);
    }
}

fn handle_download_error(session: &Session, state: &mut FragThreadState, err: &dyn std::fmt::Display) {
    log_debug!("{}: Error with fragment {}: {err}", state.name, state.seq);
    state.backoff_step += 1;
    if !session.is_live() && state.max_seq.is_some_and(|max| state.seq + 2 >= max) {
        log_debug!(
            "{}: Stream has ended and fragment number is within two of the known max, probably not actually created",
            state.name
        );
        session.set_finished(state.data_type);
    }
}

pub(super) fn header_u64(headers: &reqwest::header::HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

/// The request for fragment `seq` from `url`, with the headers YouTube expects.
pub(super) fn fragment_request(session: &Session, url: &FragmentUrl, seq: u64) -> reqwest::RequestBuilder {
    let mut req = session.http.client.get(url.for_seq(seq)).header(ORIGIN, "https://www.youtube.com");
    if let Some(host) = url.host() {
        req = req.header(REFERER, format!("https://{host}/"));
    }
    req
}

pub(super) fn content_type(headers: &reqwest::header::HeaderMap) -> String {
    headers.get(CONTENT_TYPE).and_then(|value| value.to_str().ok()).unwrap_or("").to_string()
}

/// How long to wait before the next attempt: the fragment duration, as in
/// Go, doubled for each failure in a row that says the server is unreachable
/// or overloaded (Go kept the same pace), or what `Retry-After` asked for.
pub(super) fn retry_delay(state: &FragThreadState) -> Duration {
    if let Some(asked) = state.retry_after {
        return asked.min(MAX_RETRY_AFTER).max(state.sleep);
    }
    let doublings = state.backoff_step.saturating_sub(1).min(8);
    state.sleep.saturating_mul(1 << doublings).min(MAX_BACKOFF).max(state.sleep)
}

async fn sleep_or_stop(session: &Session, state: &FragThreadState) -> bool {
    crate::util::sleep_or_cancel(&session.cancel, retry_delay(state)).await
}

/// Downloads `st.seq`. With `frag_file`, the data goes to that file instead
/// of memory. `downloaded` gives the stream's fragment count so far. Returns
/// `None` if the fragment was given up on or the download is stopping.
pub async fn download_fragment(
    session: &Session,
    state: &mut FragThreadState,
    frag_file: Option<PathBuf>,
    downloaded: &(dyn Fn() -> u64 + Sync),
) -> Option<Fragment> {
    let max = session.opts.frag_max_tries;
    while max == 0 || state.tries < max {
        if session.is_stopping() {
            return None;
        }
        let url = session.url(state.data_type)?;
        state.retry_after = None;
        match fragment_request(session, &url, state.seq).send().await {
            Err(error) => handle_download_error(session, state, &without_query(error)),
            Ok(resp) if resp.status().as_u16() >= 400 => handle_http_error(session, state, &resp),
            Ok(resp) => {
                let head = header_u64(resp.headers(), "x-head-seqnum");
                let mime = content_type(resp.headers());
                match resp.bytes().await {
                    Err(error) => handle_download_error(session, state, &without_query(error)),
                    // Success with no data: the fragment isn't ready yet.
                    Ok(body) if body.is_empty() => state.backoff_step = 0,
                    Ok(body) => {
                        if !mime.ends_with("/mp4") && !mime.ends_with("/webm") {
                            log_trace!("{}: fragment {} has unknown MIME type '{mime}'", state.name, state.seq);
                        }
                        let data = match &frag_file {
                            None => FragData::Memory(body),
                            Some(path) => match write_file_with_mode(path, &body, session.opts.file_mode).await {
                                Ok(()) => FragData::File(path.clone()),
                                Err(error) => {
                                    log_debug!(
                                        "{}: Failed to write fragment {} to file: {error}",
                                        state.name,
                                        state.seq
                                    );
                                    state.tries += 1;
                                    if !continue_fragment_download(session, state, downloaded())
                                        || !sleep_or_stop(session, state).await
                                    {
                                        return None;
                                    }
                                    continue;
                                }
                            },
                        };
                        return Some(Fragment { seq: state.seq, x_head_seqnum: head, data, mime_type: mime });
                    }
                }
            }
        }

        state.tries += 1;
        retry_fragment_url(session, state, &url).await;
        if !continue_fragment_download(session, state, downloaded()) || !sleep_or_stop(session, state).await {
            return None;
        }
    }
    None
}
