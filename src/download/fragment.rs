//! Downloading one fragment with Go ytarchive's retry policy.

use std::path::PathBuf;

use reqwest::StatusCode;
use reqwest::header::{CONTENT_TYPE, ORIGIN, REFERER};

use crate::session::Session;
use crate::types::{FragData, FragThreadState, Fragment, FragmentUrl};
use crate::util::write_file_with_mode;
use crate::{log_debug, log_trace, log_warn};

/// Whether to keep retrying after a failed attempt. When the per-fragment
/// try limit is hit: while live, start over; once the stream has ended,
/// give up (or allow a few extra rounds for fragments well behind the head).
pub fn continue_fragment_download(s: &Session, st: &mut FragThreadState) -> bool {
    if s.is_finished(st.data_type) {
        return false;
    }
    let max = s.opts.frag_max_tries;
    if max == 0 || st.tries < max {
        return true;
    }

    st.full_retries = st.full_retries.saturating_sub(1);
    log_debug!("{}: Fragment {}: {}/{max} retries", st.name, st.seq, st.tries);

    if s.is_live() && !s.is_unavailable() {
        log_debug!("{}: Fragment {}: Stream still live, continuing download attempt", st.name, st.seq);
        st.tries = 0;
        return true;
    }

    if st.is_403 {
        if s.is_unavailable() {
            log_warn!(
                "{}: Download link likely expired and stream is privated or members only, cannot continue download",
                st.name
            );
        } else {
            log_warn!("{}: Download link has likely expired and the stream has probably finished processing.", st.name);
            log_warn!("{}: You might want to use yt-dlp to download instead.", st.name);
        }
        s.set_finished(st.data_type);
        return false;
    }

    let behind_head = st.max_seq.is_some_and(|m| st.seq + 2 < m);
    if behind_head {
        if st.full_retries > 0 {
            log_debug!("{}: More than two fragments away from the highest known fragment", st.name);
            log_debug!("{}: Will try grabbing the fragment {} more times", st.name, st.full_retries);
            // Go didn't reset the count here, so its loop exited immediately
            // and the fragment was dropped without the extra attempts.
            st.tries = 0;
            return true;
        }
        // Give up on this fragment only. Go marked the whole stream finished
        // here, silently dropping everything after one bad fragment.
        return false;
    }

    // Within two of the head after the stream ended: it was probably never
    // created, so the stream is done.
    s.set_finished(st.data_type);
    false
}

/// After a failure that used up the try limit, refresh URLs (unless the
/// limit is 0, i.e. unlimited).
async fn retry_fragment_url(s: &Session, st: &FragThreadState, url: &FragmentUrl) {
    if s.is_finished(st.data_type) || s.is_stopping() {
        return;
    }
    let max = s.opts.frag_max_tries;
    if max > 0 && st.tries >= max {
        s.refresh_url_if_unchanged(st.data_type, url).await;
    }
}

fn handle_http_error(s: &Session, st: &mut FragThreadState, status: StatusCode) {
    log_debug!("{}: HTTP Error for fragment {}: {status}", st.name, st.seq);
    if status == StatusCode::FORBIDDEN {
        st.is_403 = true;
    } else if status == StatusCode::NOT_FOUND && !s.is_live() && st.max_seq.is_some_and(|m| st.seq + 2 > m) {
        log_debug!(
            "{}: Stream has ended and fragment within the last two not found, probably not actually created",
            st.name
        );
        s.set_finished(st.data_type);
    }
}

fn handle_download_error(s: &Session, st: &FragThreadState, err: &dyn std::fmt::Display) {
    log_debug!("{}: Error with fragment {}: {err}", st.name, st.seq);
    if !s.is_live() && st.max_seq.is_some_and(|m| st.seq + 2 >= m) {
        log_debug!(
            "{}: Stream has ended and fragment number is within two of the known max, probably not actually created",
            st.name
        );
        s.set_finished(st.data_type);
    }
}

pub(super) fn header_u64(h: &reqwest::header::HeaderMap, name: &str) -> Option<u64> {
    h.get(name)?.to_str().ok()?.trim().parse().ok()
}

/// The request for fragment `seq` from `url`, with the headers YouTube expects.
pub(super) fn fragment_request(s: &Session, url: &FragmentUrl, seq: u64) -> reqwest::RequestBuilder {
    let mut req = s.http.client.get(url.for_seq(seq)).header(ORIGIN, "https://www.youtube.com");
    if let Some(host) = url.host() {
        req = req.header(REFERER, format!("https://{host}/"));
    }
    req
}

pub(super) fn content_type(h: &reqwest::header::HeaderMap) -> String {
    h.get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string()
}

async fn sleep_or_stop(s: &Session, st: &FragThreadState) -> bool {
    tokio::select! {
        _ = s.cancel.cancelled() => false,
        _ = tokio::time::sleep(st.sleep) => true,
    }
}

/// Downloads `st.seq`. With `frag_file`, the data goes to that file instead
/// of memory. Returns `None` if the fragment was given up on or the
/// download is stopping.
pub async fn download_fragment(s: &Session, st: &mut FragThreadState, frag_file: Option<PathBuf>) -> Option<Fragment> {
    let max = s.opts.frag_max_tries;
    while max == 0 || st.tries < max {
        if s.is_stopping() {
            return None;
        }
        let url = s.url(st.data_type)?;
        match fragment_request(s, &url, st.seq).send().await {
            Err(e) => handle_download_error(s, st, &e),
            Ok(resp) if resp.status().as_u16() >= 400 => handle_http_error(s, st, resp.status()),
            Ok(resp) => {
                let head = header_u64(resp.headers(), "x-head-seqnum");
                let mime = content_type(resp.headers());
                match resp.bytes().await {
                    Err(e) => handle_download_error(s, st, &e),
                    // Success with no data: the fragment isn't ready yet.
                    Ok(body) if body.is_empty() => {}
                    Ok(body) => {
                        if !mime.ends_with("/mp4") && !mime.ends_with("/webm") {
                            log_trace!("{}: fragment {} has unknown MIME type '{mime}'", st.name, st.seq);
                        }
                        let data = match &frag_file {
                            None => FragData::Memory(body),
                            Some(path) => match write_file_with_mode(path, &body, s.opts.file_mode).await {
                                Ok(()) => FragData::File(path.clone()),
                                Err(e) => {
                                    log_debug!("{}: Failed to write fragment {} to file: {e}", st.name, st.seq);
                                    st.tries += 1;
                                    if !continue_fragment_download(s, st) || !sleep_or_stop(s, st).await {
                                        return None;
                                    }
                                    continue;
                                }
                            },
                        };
                        return Some(Fragment { seq: st.seq, x_head_seqnum: head, data, mime_type: mime });
                    }
                }
            }
        }

        st.tries += 1;
        retry_fragment_url(s, st, &url).await;
        if !continue_fragment_download(s, st) || !sleep_or_stop(s, st).await {
            return None;
        }
    }
    None
}
