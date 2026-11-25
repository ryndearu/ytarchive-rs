//! State shared by all download tasks of one stream: current fragment URLs,
//! live/unavailable/finished flags, and URL refreshing.

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::http::Http;
use crate::types::{
    AUDIO_ITAG, DataType, FormatSelection, FragmentUrl, LiveStatus, Options, PerType, URL_REFRESH_INTERVAL_SECS,
    YtdlpExtraction,
};
use crate::util::lock;
use crate::video_info::Extractor;
use crate::{log_debug, log_warn, ytdlp};

const REFRESH_CHECK_INTERVAL: Duration = Duration::from_secs(60);
/// A failed scheduled refresh is tried again this soon rather than after
/// another full interval, by which time the URLs would have expired.
const FAILED_REFRESH_RETRY: Duration = Duration::from_secs(5 * 60);

fn clock_now() -> (SystemTime, Instant) {
    (SystemTime::now(), Instant::now())
}

/// True when either monotonic or wall-clock time has advanced by `interval`:
/// `SystemTime` counts system sleep/suspend, while `Instant` still triggers
/// if the system clock moves backwards.
fn elapsed_at_least((wall, mono): (SystemTime, Instant), interval: Duration) -> bool {
    mono.elapsed() >= interval || wall.elapsed().is_ok_and(|elapsed| elapsed >= interval)
}

#[cfg(test)]
type RefreshHook = Arc<dyn Fn() -> Option<FragmentUrl> + Send + Sync>;

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
    /// Both clocks at the last URL refresh so system sleep/suspend counts
    /// toward the 5-hour interval while backward clock jumps cannot stall it.
    last_url_refresh: Mutex<(SystemTime, Instant)>,
    #[cfg(test)]
    on_refresh: Option<RefreshHook>,
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
            last_url_refresh: Mutex::new(clock_now()),
            #[cfg(test)]
            on_refresh: None,
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

    /// Prepends URLs saved by an earlier run, which are older than the ones
    /// this run obtained.
    pub fn remember_older(&self, data_type: DataType, older: Vec<FragmentUrl>) {
        let mut known = lock(self.known.get(data_type));
        let mut combined: Vec<FragmentUrl> = Vec::with_capacity(older.len() + known.len());
        for url in older {
            if !known.contains(&url) && !combined.contains(&url) {
                combined.push(url);
            }
        }
        combined.extend(known.drain(..));
        *known = combined;
    }

    pub fn url(&self, data_type: DataType) -> Option<FragmentUrl> {
        lock(self.urls.get(data_type)).clone()
    }

    /// Every URL used for `data_type`, newest first.
    pub fn known_urls(&self, data_type: DataType) -> Vec<FragmentUrl> {
        lock(self.known.get(data_type)).iter().rev().cloned().collect()
    }

    /// Every URL used for `data_type`, oldest first (matching SQLite `urls` rowid order).
    pub fn known_urls_oldest_first(&self, data_type: DataType) -> Vec<FragmentUrl> {
        lock(self.known.get(data_type)).clone()
    }

    /// Number of distinct URLs recorded for `data_type`.
    pub fn known_url_count(&self, data_type: DataType) -> usize {
        lock(self.known.get(data_type)).len()
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

    /// Re-runs extraction to update live status and URLs. Returns whether new
    /// URLs were applied.
    async fn run_refresh(&self, extractor: &mut Extractor) -> bool {
        let sel = extractor.refresh_urls().await;
        self.live.store(extractor.live, Ordering::Relaxed);
        self.unavailable.store(extractor.unavailable, Ordering::Relaxed);
        if extractor.target_duration > 0 {
            self.target_duration.store(extractor.target_duration, Ordering::Relaxed);
        }
        match sel {
            Some(sel) => {
                self.apply_selection(&sel);
                *lock(&self.last_url_refresh) = clock_now();
                true
            }
            None => false,
        }
    }

    /// Refreshes unless another worker already replaced `current`. Waits for
    /// an in-flight refresh rather than skipping and burning retries on the
    /// old URL. Returns whether `data_type` now has a different URL.
    pub async fn refresh_url_if_unchanged(&self, data_type: DataType, current: &FragmentUrl) -> bool {
        if self.url(data_type).as_ref() != Some(current) {
            return self.url(data_type).is_some();
        }
        #[cfg(test)]
        if let Some(hook) = &self.on_refresh {
            if let Some(url) = hook() {
                self.set_url(data_type, Some(url));
                *lock(&self.last_url_refresh) = clock_now();
            }
            return self.url(data_type).as_ref().is_some_and(|updated| updated != current);
        }
        let Some(mutex) = &self.extractor else {
            return false;
        };
        if self.is_unavailable() {
            return false;
        }
        let mut extractor = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => return false,
            guard = mutex.lock() => guard,
        };
        if self.url(data_type).as_ref() == Some(current) && !self.is_unavailable() {
            log_debug!("{}: Attempting to retrieve a new download URL", data_type.as_str());
            self.run_refresh(&mut extractor).await;
        }
        self.url(data_type).as_ref().is_some_and(|updated| updated != current)
    }

    /// Background task replacing URLs every 5 hours (before they expire)
    /// without pausing downloads. A failed refresh keeps the current URLs.
    pub fn spawn_periodic_refresh(self: &Arc<Self>) -> Option<JoinHandle<()>> {
        self.extractor.as_ref()?;
        Some(tokio::spawn({
            let session = Arc::clone(self);
            async move {
                let interval = Duration::from_secs(URL_REFRESH_INTERVAL_SECS);
                let mut failed_at: Option<(SystemTime, Instant)> = None;
                loop {
                    tokio::select! {
                        _ = session.cancel.cancelled() => return,
                        _ = tokio::time::sleep(REFRESH_CHECK_INTERVAL) => {}
                    }
                    let last = *lock(&session.last_url_refresh);
                    if !elapsed_at_least(last, interval) {
                        failed_at = None;
                        continue;
                    }
                    if failed_at.is_some_and(|at| !elapsed_at_least(at, FAILED_REFRESH_RETRY)) {
                        continue;
                    }
                    failed_at = if session.periodic_refresh_once().await { None } else { Some(clock_now()) };
                }
            }
        }))
    }

    fn periodic_refresh_targets(&self) -> (bool, bool) {
        let need_audio =
            !self.opts.video_only && self.url(DataType::Audio).is_some() && !self.is_finished(DataType::Audio);
        let need_video =
            self.video_itag.is_some() && self.url(DataType::Video).is_some() && !self.is_finished(DataType::Video);
        (need_audio, need_video)
    }

    /// Returns `false` if the refresh failed and should be tried again soon.
    async fn periodic_refresh_once(&self) -> bool {
        let Some(mutex) = &self.extractor else { return true };
        // Don't overlap with a retry-triggered refresh.
        let Ok(_guard) = mutex.try_lock() else { return true };
        if self.is_stopping() || self.is_unavailable() {
            return true;
        }
        let (need_audio, need_video) = self.periodic_refresh_targets();
        if !need_audio && !need_video {
            return true;
        }
        log_debug!("Refreshing download URLs in background (5-hour interval)");
        let info = ytdlp::extract_urls(&self.opts, &self.page_url, true, &self.cancel).await;
        if self.is_stopping() || self.is_unavailable() {
            return true;
        }
        self.apply_periodic_refresh(info)
    }

    pub(crate) fn apply_periodic_refresh(&self, info: Result<YtdlpExtraction>) -> bool {
        let (need_audio, need_video) = self.periodic_refresh_targets();
        if !need_audio && !need_video {
            return true;
        }
        let Ok(info) = info else {
            log_warn!(
                "Scheduled URL refresh failed; continuing with existing URLs and trying again in {} minutes",
                FAILED_REFRESH_RETRY.as_secs() / 60
            );
            return false;
        };
        let fresh = |itag: u32| info.urls.get(&itag).filter(|url| url.is_fragmented()).cloned();
        let audio_url = need_audio.then(|| fresh(AUDIO_ITAG)).flatten();
        let video_url = need_video.then(|| self.video_itag.and_then(fresh)).flatten();
        let audio_ok = !need_audio || audio_url.is_some();
        let video_ok = !need_video || video_url.is_some();
        let applied_any = audio_url.is_some() || video_url.is_some();
        if let Some(url) = audio_url {
            self.set_url(DataType::Audio, Some(url));
        }
        if let Some(url) = video_url {
            self.set_url(DataType::Video, Some(url));
        }
        if audio_ok && video_ok {
            *lock(&self.last_url_refresh) = clock_now();
            return true;
        }
        if matches!(info.live_status, LiveStatus::WasLive | LiveStatus::PostLive | LiveStatus::NotLive)
            && info.urls.is_empty()
        {
            self.live.store(false, Ordering::Relaxed);
            *lock(&self.last_url_refresh) = clock_now();
            log_debug!("Scheduled URL refresh saw stream ended without live URLs; stopping periodic retries");
            return true;
        }
        let detail = if applied_any {
            "Scheduled URL refresh only partially refreshed active URLs"
        } else {
            "Scheduled URL refresh failed"
        };
        log_warn!(
            "{detail}; continuing with existing URLs and trying again in {} minutes",
            FAILED_REFRESH_RETRY.as_secs() / 60
        );
        false
    }

    /// Session for tests: fixed URLs, no extractor.
    #[cfg(test)]
    pub fn for_test(opts: Options, audio: Option<FragmentUrl>, live: bool) -> Arc<Self> {
        Self::for_test_inner(opts, audio, live, None)
    }

    #[cfg(test)]
    pub fn for_test_with_refresh(
        opts: Options,
        audio: Option<FragmentUrl>,
        live: bool,
        on_refresh: RefreshHook,
    ) -> Arc<Self> {
        Self::for_test_inner(opts, audio, live, Some(on_refresh))
    }

    #[cfg(test)]
    fn for_test_inner(
        opts: Options,
        audio: Option<FragmentUrl>,
        live: bool,
        on_refresh: Option<RefreshHook>,
    ) -> Arc<Self> {
        let sel = FormatSelection {
            label: "audio_only".into(),
            codec: None,
            video_itag: None,
            audio_url: audio,
            video_url: None,
        };
        let http = Http::new(&opts, Default::default()).unwrap();
        let session = Self {
            opts: Arc::new(opts),
            http,
            cancel: CancellationToken::new(),
            extractor: None,
            page_url: String::new(),
            video_itag: sel.video_itag,
            live: AtomicBool::new(live),
            unavailable: AtomicBool::new(false),
            target_duration: AtomicU32::new(1),
            urls: PerType::default(),
            known: PerType::default(),
            finished: PerType::default(),
            last_url_refresh: Mutex::new(clock_now()),
            on_refresh,
        };
        session.apply_selection(&sel);
        Arc::new(session)
    }
}
