//! Retrieving stream status, metadata, and download URLs, including waiting
//! for scheduled streams. Called once before downloading and again by the
//! downloader to refresh URLs / check whether the stream is still live.

mod player;
#[cfg(test)]
mod tests;
mod ytdlp_mode;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::error::Error;
use crate::formats::{self, UrlMap};
use crate::http::Http;
use crate::input_url::InputUrl;
use crate::prompt::get_user_input;
use crate::stream_meta::{apply_metadata, default_metadata, format_info_from_player_response, local_timestamp};
use crate::types::{
    Action, DEFAULT_POLL_SECS, DEFAULT_TARGET_DURATION_SECS, FormatInfo, FormatSelection, MetaInfo, Options,
    VideoCodec, new_format_info,
};
use crate::util::sleep_or_cancel;
use crate::{log_error, log_general, log_warn, ytdlp};

/// Minimum time between two extractions; little changes faster than this.
const MIN_UPDATE_INTERVAL: Duration = Duration::from_secs(DEFAULT_POLL_SECS);
const MAX_RETRIES: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_secs(2);

/// Shown when a stream is over and its live (fragmented) download is gone,
/// whether or not YouTube has finished processing it.
const ENDED_MESSAGE: &str = "Livestream has already ended; its live download no longer exists. Use yt-dlp instead.";

pub struct Extractor {
    pub opts: Arc<Options>,
    pub http: Http,
    pub cancel: CancellationToken,

    /// Page to query: a watch URL, or a channel's /live URL until resolved.
    pub page_url: String,
    pub is_channel: bool,
    /// The channel's /live URL when the input was a channel.
    pub channel_url: Option<String>,
    pub members_only: bool,
    pub video_id: String,
    /// Video IDs already recorded by `--monitor-channel`, skipped before yt-dlp.
    pub skip_ids: HashSet<String>,
    pub skipped_already_recorded: bool,

    pub wait: Action,
    /// Poll interval while waiting; `None` = use the scheduled start time.
    pub retry_secs: Option<u64>,
    /// Qualities from the CLI (or prompted while waiting).
    pub selected: Vec<String>,
    /// Codec preference from the CLI; `None` = default order.
    pub codecs: Option<Vec<VideoCodec>>,
    /// Video itag once chosen: `Some(None)` = audio only. Later refreshes
    /// reuse it instead of selecting again.
    pub fixed_itag: Option<Option<u32>>,

    printed_info: Option<(String, String)>,
    pub in_progress: bool,
    pub live: bool,
    pub unavailable: bool,
    last_updated: Option<Instant>,

    pub target_duration: u32,
    pub last_sq: Option<u64>,
    pub format_info: FormatInfo,
    pub metadata: MetaInfo,
    pub thumbnail: String,
    #[cfg(test)]
    pub(super) test_base_url: Option<String>,
}

impl Extractor {
    pub fn new(
        opts: Arc<Options>,
        http: Http,
        cancel: CancellationToken,
        input: &InputUrl,
        skip_ids: &HashSet<String>,
    ) -> Self {
        let (is_channel, video_id) = match input {
            InputUrl::Channel { .. } => (true, String::new()),
            InputUrl::Video { id } | InputUrl::GoogleVideo { id, .. } => (false, id.clone()),
        };
        let selected = opts.quality.as_deref().map(formats::parse_quality_selection).unwrap_or_default();
        Self {
            page_url: input.page_url(),
            is_channel,
            channel_url: is_channel.then(|| input.page_url()),
            members_only: opts.members_only && is_channel,
            video_id,
            skip_ids: skip_ids.clone(),
            skipped_already_recorded: false,
            wait: opts.wait,
            retry_secs: opts.retry_stream.map(|duration| duration.as_secs()),
            selected,
            codecs: opts.codecs.clone(),
            fixed_itag: opts.audio_only.then_some(None),
            printed_info: None,
            in_progress: false,
            live: false,
            unavailable: false,
            last_updated: None,
            target_duration: DEFAULT_TARGET_DURATION_SECS,
            last_sq: None,
            format_info: new_format_info(),
            metadata: default_metadata(&opts.metadata_overrides),
            thumbnail: String::new(),
            #[cfg(test)]
            test_base_url: None,
            opts,
            http,
            cancel,
        }
    }

    pub(super) fn watch_url(&self, video_id: &str) -> String {
        #[cfg(test)]
        if let Some(base) = &self.test_base_url {
            return format!("{base}/watch?v={video_id}");
        }
        format!("https://www.youtube.com/watch?v={video_id}")
    }

    fn due(&self) -> bool {
        !self.unavailable
            && !self.cancel.is_cancelled()
            && self.last_updated.is_none_or(|updated| updated.elapsed() >= MIN_UPDATE_INTERVAL)
    }

    /// Before waiting for a scheduled stream, ask for the quality so nobody
    /// needs to be around when it starts.
    async fn ask_ahead(&mut self) {
        if self.selected.is_empty() && self.fixed_itag.is_none() {
            self.selected = formats::prompt_quality(&formats::selectable_qualities(self.opts.video_only), true).await;
        }
    }

    fn print_info(&mut self, channel: &str, title: &str) {
        if self.in_progress || channel.is_empty() || title.is_empty() {
            return;
        }
        if self
            .printed_info
            .as_ref()
            .is_some_and(|(prev_channel, prev_title)| prev_channel == channel && prev_title == title)
        {
            return;
        }
        log_general!("Channel: {channel}");
        log_general!("Video Title: {title}");
        self.printed_info = Some((channel.to_string(), title.to_string()));
    }

    pub(super) fn reject_negative_live_from(&self) -> bool {
        if self.opts.live_from.as_deref().is_some_and(|value| value.trim().starts_with('-')) {
            log_error!("Option --live-from with a negative duration is not valid for a scheduled stream.");
            return true;
        }
        false
    }

    /// Fetches stream info and selects download URLs. Returns `None` if
    /// nothing usable was found, it's too soon since the last update, or the
    /// user chose not to wait; check `live` / `unavailable` afterwards.
    pub async fn get_video_info(&mut self) -> Option<FormatSelection> {
        if !self.due() {
            return None;
        }
        let selection = if self.opts.ytdlp_info {
            self.get_video_info_ytdlp().await
        } else {
            self.get_video_info_player_response().await
        };
        // A failed extraction can take minutes, and the workers queued on the
        // refresh lock would each run another one right away.
        self.last_updated = Some(Instant::now());
        selection
    }

    async fn get_video_info_player_response(&mut self) -> Option<FormatSelection> {
        let outcome = self.get_playable_player_response().await;
        let player_response = match outcome {
            player::Outcome::Found(player_response) => player_response,
            player::Outcome::NotFound => {
                self.live = false;
                self.unavailable = true;
                return None;
            }
            player::Outcome::TransientError | player::Outcome::NotUsable => return None,
        };

        let details = &player_response.microformat.player_microformat_renderer.live_broadcast_details;
        if !self.in_progress {
            log_general!("Stream started at time {}", local_timestamp(&details.start_timestamp));
        }
        if let Some(format) = player_response.streaming_data.adaptive_formats.first()
            && format.target_duration_sec > 0.0
        {
            self.target_duration = format.target_duration_sec.round() as u32;
        }

        let info = match ytdlp::extract(&self.http, &self.opts, &self.page_url, false, &self.cancel).await {
            Ok(info) => info,
            Err(Error::Cancelled) => return None,
            Err(error) => {
                if self.in_progress {
                    log_warn!("yt-dlp extraction failed; continuing with current URLs: {error}");
                } else {
                    log_error!("yt-dlp extraction failed: {error}");
                }
                return None;
            }
        };
        if let Some(sequence) = info.last_sq.filter(|last| *last > 0) {
            self.last_sq = Some(sequence);
        }
        if info.target_duration > 0 {
            self.target_duration = info.target_duration;
        }
        if !self.in_progress && !details.is_live_now && info.urls.is_empty() {
            log_general!("{ENDED_MESSAGE}");
            return None;
        }

        let sel = self.select(&info.urls).await?;
        if !self.in_progress {
            self.format_info = format_info_from_player_response(&player_response);
            self.metadata = apply_metadata(&default_metadata(&self.opts.metadata_overrides), &self.format_info);
            if let Some(updated) = player_response.microformat.player_microformat_renderer.thumbnail.thumbnails.first()
            {
                self.thumbnail = updated.url.clone();
            }
            self.in_progress = true;
        }
        self.live = details.is_live_now;
        Some(sel)
    }

    /// Chooses formats: interactively the first time, then the same itag on
    /// later refreshes.
    async fn select(&mut self, urls: &UrlMap) -> Option<FormatSelection> {
        if urls.is_empty() {
            log_error!("No download URLs found");
            return None;
        }
        let video_only = self.opts.video_only;
        let sel = match self.fixed_itag {
            Some(itag) => formats::select_fixed(urls, itag, video_only),
            None => {
                let sel = formats::select_interactive(
                    urls,
                    std::mem::take(&mut self.selected),
                    self.codecs.as_deref(),
                    video_only,
                )
                .await
                .inspect_err(|error| log_error!("{error}"))
                .ok()?;
                self.fixed_itag = Some(sel.video_itag);
                sel
            }
        };
        match formats::validate(&sel, video_only) {
            Ok(()) => Some(sel),
            Err(error) => {
                log_error!("{error}");
                None
            }
        }
    }

    /// Asks whether to wait for the scheduled time, poll, or give up.
    async fn ask_wait_for_stream(&mut self) -> bool {
        log_general!(
            "This stream is likely a future scheduled livestream.\n\
             Would you like to wait for the scheduled start time, poll until it starts, or not wait?"
        );
        let answer = get_user_input("1) wait  2) poll  3) no (default 3): ").await;
        match parse_wait_choice(&answer) {
            WaitChoice::Wait => {
                self.wait = Action::Do;
                true
            }
            WaitChoice::Poll => {
                let secs =
                    get_user_input(&format!("Input poll interval in seconds (minimum {DEFAULT_POLL_SECS}): ")).await;
                self.retry_secs = Some(parse_poll_secs(&secs));
                true
            }
            WaitChoice::No => false,
        }
    }

    /// How long to wait before checking an upcoming stream again (yt-dlp
    /// mode). `None` = don't wait.
    async fn upcoming_wait_interval(&mut self, start_unix: Option<i64>) -> Option<Duration> {
        if self.wait == Action::DoNot {
            return None;
        }
        if let Some(seconds) = self.retry_secs {
            return Some(Duration::from_secs(seconds));
        }
        if self.wait == Action::Ask {
            if !self.ask_wait_for_stream().await {
                return None;
            }
            if let Some(seconds) = self.retry_secs {
                return Some(Duration::from_secs(seconds));
            }
        }
        let until_start = start_unix.map(|time| time - chrono::Utc::now().timestamp()).filter(|seconds| *seconds > 0);
        Some(Duration::from_secs(until_start.map_or(DEFAULT_POLL_SECS, |seconds| seconds as u64)))
    }
}

/// Sleeps until `unix` in chunks of at most 60 s, re-checking the wall clock
/// because tokio's timer stops while the system is suspended.
async fn sleep_until_unix(cancel: &CancellationToken, unix: i64) -> bool {
    loop {
        let left = unix - chrono::Utc::now().timestamp();
        if left <= 0 {
            return true;
        }
        if !sleep_or_cancel(cancel, Duration::from_secs((left as u64).min(60))).await {
            return false;
        }
    }
}

/// Parses the poll-interval answer; anything invalid or too small becomes the default.
pub fn parse_poll_secs(answer: &str) -> u64 {
    answer.trim().parse::<u64>().ok().filter(|seconds| *seconds >= DEFAULT_POLL_SECS).unwrap_or(DEFAULT_POLL_SECS)
}

#[derive(Debug, PartialEq, Eq)]
pub enum WaitChoice {
    Wait,
    Poll,
    No,
}

/// Reads the scheduled-stream answer. Go asked `wait/poll/[no]`; the numbers
/// are new, the words still work, and anything else (Enter, closed stdin) is
/// "no", as in Go.
pub fn parse_wait_choice(answer: &str) -> WaitChoice {
    let arg = answer.trim().to_lowercase();
    if arg == "1" || arg.starts_with("wait") {
        WaitChoice::Wait
    } else if arg == "2" || arg.starts_with("poll") {
        WaitChoice::Poll
    } else {
        WaitChoice::No
    }
}
