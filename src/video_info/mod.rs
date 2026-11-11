//! Retrieving stream status, metadata, and download URLs, including waiting
//! for scheduled streams. Called once before downloading and again by the
//! downloader to refresh URLs / check whether the stream is still live.

mod player;
mod wait;
mod ytdlp_mode;

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::formats::{self, UrlMap};
use crate::http::Http;
use crate::input_url::InputUrl;
use crate::stream_meta::{apply_metadata, default_metadata, format_info_from_player_response, local_timestamp};
use crate::types::{
    Action, DEFAULT_POLL_SECS, FormatInfo, FormatSelection, MetaInfo, Options, VideoCodec, new_format_info,
};
use crate::{log_debug, log_error, log_general, ytdlp};

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

    pub info_printed: bool,
    pub in_progress: bool,
    pub live: bool,
    pub unavailable: bool,
    last_updated: Option<Instant>,

    pub target_duration: u32,
    pub last_sq: Option<u64>,
    pub format_info: FormatInfo,
    pub metadata: MetaInfo,
    pub thumbnail: String,
}

impl Extractor {
    pub fn new(opts: Arc<Options>, http: Http, cancel: CancellationToken, input: &InputUrl) -> Self {
        let (is_channel, video_id) = match input {
            InputUrl::Channel { .. } => (true, String::new()),
            InputUrl::Video { id } | InputUrl::GoogleVideo { id, .. } => (false, id.clone()),
        };
        let selected = opts.quality.as_deref().map(formats::parse_quality_selection).unwrap_or_default();
        Self {
            page_url: input.page_url(),
            is_channel,
            channel_url: is_channel.then(|| input.page_url()),
            members_only: opts.members_only,
            video_id,
            wait: opts.wait,
            retry_secs: opts.retry_stream.map(|duration| duration.as_secs()),
            selected,
            codecs: opts.codecs.clone(),
            fixed_itag: opts.audio_only.then_some(None),
            info_printed: false,
            in_progress: false,
            live: false,
            unavailable: false,
            last_updated: None,
            target_duration: crate::types::DEFAULT_TARGET_DURATION_SECS,
            last_sq: None,
            format_info: new_format_info(),
            metadata: default_metadata(&opts.metadata_overrides),
            thumbnail: String::new(),
            opts,
            http,
            cancel,
        }
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
            self.selected = formats::prompt_quality(&formats::all_qualities(), true).await;
        }
    }

    fn print_info(&mut self, channel: &str, title: &str) {
        if self.info_printed || channel.is_empty() || title.is_empty() {
            return;
        }
        log_general!("Channel: {channel}");
        log_general!("Video Title: {title}");
        self.info_printed = true;
    }

    /// Fetches stream info and selects download URLs. Returns `None` if
    /// nothing usable was found, it's too soon since the last update, or the
    /// user chose not to wait; check `live` / `unavailable` afterwards.
    pub async fn get_video_info(&mut self) -> Option<FormatSelection> {
        if !self.due() {
            return None;
        }
        if self.opts.ytdlp_info {
            self.get_video_info_ytdlp().await
        } else {
            self.get_video_info_player_response().await
        }
    }

    async fn get_video_info_player_response(&mut self) -> Option<FormatSelection> {
        let outcome = self.get_playable_player_response().await;
        self.last_updated = Some(Instant::now());
        let player_response = match outcome {
            player::Outcome::Found(player_response) => player_response,
            player::Outcome::NotFound => {
                self.live = false;
                self.unavailable = true;
                return None;
            }
            player::Outcome::NotUsable => return None,
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
            Err(error) => {
                log_debug!("yt-dlp extraction failed: {error}");
                Default::default()
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
}
