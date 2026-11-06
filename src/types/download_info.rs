//! Shared mutable runtime state for one stream download.
//! Go used one big struct behind a RWMutex; here options are separated out
//! (see `options.rs`) and the mutable parts are grouped behind locks.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use super::consts::DEFAULT_TARGET_DURATION_SECS;
use super::download_state::DownloadState;
use super::fragment::DataType;
use super::media::MediaDlInfo;
use super::options::Options;
use super::ytcfg::YtCfg;

/// Values exposed to the `%(key)s` filename template.
pub type FormatInfo = BTreeMap<String, String>;
/// All template keys present with empty values, so templates can be
/// validated before extraction fills them in.
pub fn new_format_info() -> FormatInfo {
    super::consts::FORMAT_INFO_KEYS
        .iter()
        .map(|k| (k.to_string(), String::new()))
        .collect()
}

/// Metadata written to the final file.
pub type MetaInfo = BTreeMap<String, String>;

/// Stream/video facts discovered during extraction.
#[derive(Debug, Clone, Default)]
pub struct StreamInfo {
    pub video_id: String,
    pub status: String,
    pub live: bool,
    pub unavailable: bool,
    pub selected_quality: String,
    /// Selected video itag; `None` until chosen, `Some(0)` = audio only.
    pub quality: Option<u32>,
    pub thumbnail: String,
    pub target_duration: u32,
    pub last_sq: Option<u64>,
    pub live_from_sq: Option<u64>,
    pub format_info: FormatInfo,
    pub metadata: MetaInfo,
    pub ytcfg: Option<YtCfg>,
}

/// Download progress and URL bookkeeping.
#[derive(Debug, Default)]
pub struct DownloadProgress {
    pub media: HashMap<DataType, MediaDlInfo>,
    /// itag -> resumable state.
    pub states: HashMap<u32, DownloadState>,
    pub in_progress: bool,
    pub last_updated: Option<Instant>,
    pub last_url_refresh: Option<Instant>,
}

#[derive(Debug)]
pub struct DownloadInfo {
    pub options: Arc<Options>,
    pub stream: RwLock<StreamInfo>,
    pub progress: RwLock<DownloadProgress>,
    /// Serializes URL refreshes so concurrent 403s trigger only one.
    pub refresh_lock: Mutex<()>,
    /// Cancelled on Ctrl-C or when capture duration elapses.
    pub cancel: CancellationToken,
}

impl DownloadInfo {
    pub fn new(options: Arc<Options>) -> Arc<Self> {
        let mut media = HashMap::new();
        media.insert(DataType::Audio, MediaDlInfo::new(DataType::Audio));
        media.insert(DataType::Video, MediaDlInfo::new(DataType::Video));

        Arc::new(Self {
            options,
            stream: RwLock::new(StreamInfo {
                target_duration: DEFAULT_TARGET_DURATION_SECS,
                format_info: new_format_info(),
                ..Default::default()
            }),
            progress: RwLock::new(DownloadProgress {
                media,
                ..Default::default()
            }),
            refresh_lock: Mutex::new(()),
            cancel: CancellationToken::new(),
        })
    }

    pub fn is_stopping(&self) -> bool {
        self.cancel.is_cancelled()
    }
}
