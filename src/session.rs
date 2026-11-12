//! State shared by all download tasks of one stream: current fragment URLs,
//! live/unavailable/finished flags, and URL refreshing.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::http::Http;
use crate::types::{AUDIO_ITAG, DataType, FormatSelection, FragmentUrl, Options, PerType, URL_REFRESH_INTERVAL_SECS};
use crate::util::lock;
use crate::video_info::Extractor;
use crate::{log_debug, log_warn, ytdlp};

const REFRESH_CHECK_INTERVAL: Duration = Duration::from_secs(60);

pub struct Session {
    pub opts: Arc<Options>,
    pub http: Http,
    pub cancel: CancellationToken,
    /// `None` for direct googlevideo URLs, which can't be refreshed.
    extractor: Option<tokio::sync::Mutex<Extractor>>,
    page_url: String,
    video_itag: Option<u32>,
    live: AtomicBool,
    unavailable: AtomicBool,
    target_duration: AtomicU32,
    urls: PerType<Mutex<Option<FragmentUrl>>>,
    /// Every URL used so far, oldest first. Recovery tries them all, since
    /// a URL from an earlier refresh may still work when a newer one doesn't.
    known: PerType<Mutex<Vec<FragmentUrl>>>,
    finished: PerType<AtomicBool>,
    last_url_refresh: Mutex<Instant>,
}

impl Session {
    pub fn new(
        opts: Arc<Options>,
        http: Http,
        cancel: CancellationToken,
        extractor: Option<Extractor>,
        sel: &FormatSelection,
        live: bool,
        target_duration: u32,
    ) -> Arc<Self> {
        let page_url = extractor.as_ref().map(|inner| inner.page_url.clone()).unwrap_or_default();
        let session = Self {
            opts,
            http,
            cancel,
            extractor: extractor.map(tokio::sync::Mutex::new),
            page_url,
            video_itag: sel.video_itag,
            live: AtomicBool::new(live),
            unavailable: AtomicBool::new(false),
            target_duration: AtomicU32::new(target_duration.max(1)),
            urls: PerType::default(),
            known: PerType::default(),
            finished: PerType::default(),
            last_url_refresh: Mutex::new(Instant::now()),
        };
        session.apply_selection(sel);
        Arc::new(session)
    }

    fn apply_selection(&self, sel: &FormatSelection) {
        self.set_url(DataType::Audio, sel.audio_url.clone());
        self.set_url(DataType::Video, sel.video_url.clone());
    }

    fn set_url(&self, data_type: DataType, url: Option<FragmentUrl>) {
        if let Some(new_url) = &url {
            self.remember(data_type, new_url);
        }
        *lock(self.urls.get(data_type)) = url;
    }

    /// Adds `url` to the URLs recovery can use.
    pub fn remember(&self, data_type: DataType, url: &FragmentUrl) {
        let mut known = lock(self.known.get(data_type));
        if !known.contains(url) {
            known.push(url.clone());
        }
    }

    pub fn url(&self, data_type: DataType) -> Option<FragmentUrl> {
        lock(self.urls.get(data_type)).clone()
    }

    /// Every URL used for `dt`, newest first.
    pub fn known_urls(&self, data_type: DataType) -> Vec<FragmentUrl> {
        lock(self.known.get(data_type)).iter().rev().cloned().collect()
    }

    pub fn is_live(&self) -> bool {
        self.live.load(Ordering::Relaxed)
    }

    /// What a refresh finding the stream ended would do.
    #[cfg(test)]
    pub fn set_live(&self, live: bool) {
        self.live.store(live, Ordering::Relaxed);
    }

    pub fn is_unavailable(&self) -> bool {
        self.unavailable.load(Ordering::Relaxed)
    }

    pub fn set_unavailable(&self) {
        self.unavailable.store(true, Ordering::Relaxed);
    }

    pub fn target_duration(&self) -> u32 {
        self.target_duration.load(Ordering::Relaxed)
    }

    pub fn is_finished(&self, data_type: DataType) -> bool {
        self.finished.get(data_type).load(Ordering::Relaxed)
    }

    pub fn set_finished(&self, data_type: DataType) {
        self.finished.get(data_type).store(true, Ordering::Relaxed);
    }

    pub fn is_stopping(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Stops all downloads (Ctrl-C, fatal write errors, capture end).
    pub fn stop(&self) {
        self.cancel.cancel();
        self.set_finished(DataType::Audio);
        self.set_finished(DataType::Video);
    }

    /// Re-runs extraction to update live status and URLs. Skipped if another
    /// refresh is in progress. Returns whether new URLs were applied.
    pub async fn refresh_info(&self) -> bool {
        let Some(mutex) = &self.extractor else {
            return false;
        };
        let Ok(mut extractor) = mutex.try_lock() else {
            return false;
        };
        let sel = extractor.get_video_info().await;
        self.live.store(extractor.live, Ordering::Relaxed);
        self.unavailable.store(extractor.unavailable, Ordering::Relaxed);
        if extractor.target_duration > 0 {
            self.target_duration.store(extractor.target_duration, Ordering::Relaxed);
        }
        match sel {
            Some(sel) => {
                self.apply_selection(&sel);
                *lock(&self.last_url_refresh) = Instant::now();
                true
            }
            None => false,
        }
    }

    /// Refreshes unless another worker already replaced `current`.
    pub async fn refresh_url_if_unchanged(&self, data_type: DataType, current: &FragmentUrl) {
        if self.url(data_type).as_ref() == Some(current) {
            log_debug!("{}: Attempting to retrieve a new download URL", data_type.as_str());
            self.refresh_info().await;
        }
    }

    /// Background task replacing URLs every 5 hours (before they expire)
    /// without pausing downloads. A failed refresh keeps the current URLs.
    pub fn spawn_periodic_refresh(self: &Arc<Self>) -> Option<JoinHandle<()>> {
        self.extractor.as_ref()?;
        Some(tokio::spawn({
            let session = Arc::clone(self);
            async move {
                let interval = Duration::from_secs(URL_REFRESH_INTERVAL_SECS);
                loop {
                    tokio::select! {
                        _ = session.cancel.cancelled() => return,
                        _ = tokio::time::sleep(REFRESH_CHECK_INTERVAL) => {}
                    }
                    if lock(&session.last_url_refresh).elapsed() < interval {
                        continue;
                    }
                    session.periodic_refresh_once().await;
                }
            }
        }))
    }

    async fn periodic_refresh_once(&self) {
        let Some(mutex) = &self.extractor else { return };
        // Don't overlap with a retry-triggered refresh.
        let Ok(_guard) = mutex.try_lock() else { return };
        log_debug!("Refreshing download URLs in background (5-hour interval)");
        let info = ytdlp::extract(&self.http, &self.opts, &self.page_url, true, &self.cancel).await;
        *lock(&self.last_url_refresh) = Instant::now();
        if self.is_stopping() || self.is_unavailable() {
            return;
        }

        let fresh = |itag: u32| {
            info.as_ref()
                .ok()
                .filter(|info| info.last_sq.is_some())
                .and_then(|info| info.urls.get(&itag))
                .filter(|url| url.is_fragmented())
                .cloned()
        };
        let audio = (!self.opts.video_only).then(|| fresh(AUDIO_ITAG));
        let video = self.video_itag.map(fresh);
        if audio.as_ref().is_some_and(Option::is_none) || video.as_ref().is_some_and(Option::is_none) {
            log_warn!("Scheduled URL refresh failed; continuing with existing URLs");
            return;
        }
        if let Some(Some(url)) = audio {
            self.set_url(DataType::Audio, Some(url));
        }
        if let Some(Some(url)) = video {
            self.set_url(DataType::Video, Some(url));
        }
    }

    /// Session for tests: fixed URLs, no extractor.
    #[cfg(test)]
    pub fn for_test(opts: Options, audio: Option<FragmentUrl>, live: bool) -> Arc<Self> {
        let sel = FormatSelection {
            label: "audio_only".into(),
            codec: None,
            video_itag: None,
            audio_url: audio,
            video_url: None,
        };
        let http = Http::new(&opts, Default::default()).unwrap();
        Self::new(Arc::new(opts), http, CancellationToken::new(), None, &sel, live, 1)
    }
}
