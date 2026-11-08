//! State shared by all download tasks of one stream: current fragment URLs,
//! live/unavailable/finished flags, and URL refreshing.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::http::Http;
use crate::types::{DataType, FormatSelection, FragmentUrl, Options, AUDIO_ITAG, URL_REFRESH_INTERVAL_SECS};
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
    urls: [RwLock<Option<FragmentUrl>>; 2],
    /// Every URL used so far, oldest first. Recovery tries them all, since
    /// a URL from an earlier refresh may still work when a newer one doesn't.
    known: [Mutex<Vec<FragmentUrl>>; 2],
    finished: [AtomicBool; 2],
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
        let page_url = extractor.as_ref().map(|e| e.page_url.clone()).unwrap_or_default();
        let s = Self {
            opts,
            http,
            cancel,
            extractor: extractor.map(tokio::sync::Mutex::new),
            page_url,
            video_itag: sel.video_itag,
            live: AtomicBool::new(live),
            unavailable: AtomicBool::new(false),
            target_duration: AtomicU32::new(target_duration.max(1)),
            urls: [RwLock::new(None), RwLock::new(None)],
            known: [Mutex::new(Vec::new()), Mutex::new(Vec::new())],
            finished: [AtomicBool::new(false), AtomicBool::new(false)],
            last_url_refresh: Mutex::new(Instant::now()),
        };
        s.apply_selection(sel);
        Arc::new(s)
    }

    fn apply_selection(&self, sel: &FormatSelection) {
        self.set_url(DataType::Audio, sel.audio_url.clone());
        self.set_url(DataType::Video, sel.video_url.clone());
    }

    fn set_url(&self, dt: DataType, url: Option<FragmentUrl>) {
        if let Some(u) = &url {
            self.remember(dt, u);
        }
        *self.urls[dt.idx()].write().unwrap() = url;
    }

    /// Adds `url` to the URLs recovery can use.
    pub fn remember(&self, dt: DataType, url: &FragmentUrl) {
        let mut known = self.known[dt.idx()].lock().unwrap();
        if !known.contains(url) {
            known.push(url.clone());
        }
    }

    pub fn url(&self, dt: DataType) -> Option<FragmentUrl> {
        self.urls[dt.idx()].read().unwrap().clone()
    }

    /// Every URL used for `dt`, newest first.
    pub fn known_urls(&self, dt: DataType) -> Vec<FragmentUrl> {
        self.known[dt.idx()].lock().unwrap().iter().rev().cloned().collect()
    }

    pub fn is_live(&self) -> bool {
        self.live.load(Ordering::Relaxed)
    }

    pub fn is_unavailable(&self) -> bool {
        self.unavailable.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub fn set_unavailable(&self) {
        self.unavailable.store(true, Ordering::Relaxed);
    }

    pub fn target_duration(&self) -> u32 {
        self.target_duration.load(Ordering::Relaxed)
    }

    pub fn is_finished(&self, dt: DataType) -> bool {
        self.finished[dt.idx()].load(Ordering::Relaxed)
    }

    pub fn set_finished(&self, dt: DataType) {
        self.finished[dt.idx()].store(true, Ordering::Relaxed);
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
        let Some(m) = &self.extractor else {
            return false;
        };
        let Ok(mut ex) = m.try_lock() else {
            return false;
        };
        let sel = ex.get_video_info().await;
        self.live.store(ex.live, Ordering::Relaxed);
        self.unavailable.store(ex.unavailable, Ordering::Relaxed);
        if ex.target_duration > 0 {
            self.target_duration.store(ex.target_duration, Ordering::Relaxed);
        }
        match sel {
            Some(sel) => {
                self.apply_selection(&sel);
                *self.last_url_refresh.lock().unwrap() = Instant::now();
                true
            }
            None => false,
        }
    }

    /// Refreshes unless another worker already replaced `current`.
    pub async fn refresh_url_if_unchanged(&self, dt: DataType, current: &FragmentUrl) {
        if self.url(dt).as_ref() == Some(current) {
            log_debug!("{}: Attempting to retrieve a new download URL", dt.as_str());
            self.refresh_info().await;
        }
    }

    /// Background task replacing URLs every 5 hours (before they expire)
    /// without pausing downloads. A failed refresh keeps the current URLs.
    pub fn spawn_periodic_refresh(self: &Arc<Self>) -> Option<JoinHandle<()>> {
        self.extractor.as_ref()?;
        let s = Arc::clone(self);
        Some(tokio::spawn(async move {
            let interval = Duration::from_secs(URL_REFRESH_INTERVAL_SECS);
            loop {
                tokio::select! {
                    _ = s.cancel.cancelled() => return,
                    _ = tokio::time::sleep(REFRESH_CHECK_INTERVAL) => {}
                }
                if s.last_url_refresh.lock().unwrap().elapsed() < interval {
                    continue;
                }
                s.periodic_refresh_once().await;
            }
        }))
    }

    async fn periodic_refresh_once(&self) {
        let Some(m) = &self.extractor else { return };
        // Don't overlap with a retry-triggered refresh.
        let Ok(_guard) = m.try_lock() else { return };
        log_debug!("Refreshing download URLs in background (5-hour interval)");
        let info = ytdlp::extract(&self.http, &self.opts, &self.page_url, true, &self.cancel).await;
        *self.last_url_refresh.lock().unwrap() = Instant::now();
        if self.is_stopping() || self.is_unavailable() {
            return;
        }

        let fresh = |itag: u32| {
            info.as_ref()
                .ok()
                .filter(|i| i.last_sq.is_some())
                .and_then(|i| i.urls.get(&itag))
                .filter(|u| u.is_fragmented())
                .cloned()
        };
        let audio = (!self.opts.video_only).then(|| fresh(AUDIO_ITAG));
        let video = self.video_itag.map(fresh);
        if audio.as_ref().is_some_and(Option::is_none) || video.as_ref().is_some_and(Option::is_none) {
            log_warn!("Scheduled URL refresh failed; continuing with existing URLs");
            return;
        }
        if let Some(Some(a)) = audio {
            self.set_url(DataType::Audio, Some(a));
        }
        if let Some(Some(v)) = video {
            self.set_url(DataType::Video, Some(v));
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
