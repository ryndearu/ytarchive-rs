//! Getting from an input URL to selected formats and stream facts.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::gvideo;
use crate::http::Http;
use crate::input_url::InputUrl;
use crate::types::{DEFAULT_TARGET_DURATION_SECS, FormatInfo, FormatSelection, MetaInfo, Options, new_format_info};
use crate::video_info::Extractor;

/// Everything known about the stream once formats are selected.
pub struct Prepared {
    /// `None` for direct googlevideo URLs.
    pub extractor: Option<Extractor>,
    pub sel: FormatSelection,
    pub video_id: String,
    pub live: bool,
    pub last_sq: Option<u64>,
    pub target_duration: u32,
    pub format_info: FormatInfo,
    pub metadata: MetaInfo,
    pub thumbnail_url: String,
    /// Rebuilt from a kept download because the stream can no longer be
    /// looked up; missing fragments go straight to recovery.
    pub unavailable: bool,
}

/// `None` if nothing downloadable was found (already logged).
pub async fn prepare(
    opts: &Arc<Options>,
    http: &Http,
    cancel: &CancellationToken,
    input: &InputUrl,
) -> Option<Prepared> {
    if let InputUrl::GoogleVideo { id, itag, url } = input {
        let sel = gvideo::selection(opts, *itag, url.clone()).await?;
        let mut format_info = new_format_info();
        format_info.insert("id".into(), id.clone());
        return Some(Prepared {
            extractor: None,
            sel,
            video_id: id.clone(),
            live: false,
            last_sq: None,
            target_duration: DEFAULT_TARGET_DURATION_SECS,
            format_info,
            metadata: MetaInfo::new(),
            thumbnail_url: String::new(),
            unavailable: false,
        });
    }

    let mut ex = Extractor::new(opts.clone(), http.clone(), cancel.clone(), input);
    let sel = ex.get_video_info().await?;
    Some(Prepared {
        sel,
        video_id: ex.video_id.clone(),
        live: ex.live,
        last_sq: ex.last_sq,
        target_duration: ex.target_duration,
        format_info: ex.format_info.clone(),
        metadata: ex.metadata.clone(),
        thumbnail_url: ex.thumbnail.clone(),
        unavailable: false,
        extractor: Some(ex),
    })
}
