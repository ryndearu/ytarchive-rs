//! Getting from an input URL to selected formats and stream facts.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::gvideo;
use crate::http::Http;
use crate::input_url::InputUrl;
use crate::types::{
    new_format_info, FormatInfo, FormatSelection, MetaInfo, Options, DEFAULT_TARGET_DURATION_SECS,
};
use crate::util::seconds_to_duration_and_time_str;
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
}

/// Returns `None` if nothing downloadable was found (already logged).
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
        extractor: Some(ex),
    })
}

/// `--info-only` output.
pub fn print_info(p: &Prepared, input: &InputUrl) {
    let fi = &p.format_info;
    let sel = &p.sel;
    let quality = match (sel.codec, sel.video_itag) {
        (Some(c), Some(itag)) => format!("{} ({}, itag {itag})", sel.label, c.name()),
        (None, Some(itag)) => format!("itag {itag}"),
        _ => "audio_only".into(),
    };
    println!("Video ID: {}", p.video_id);
    println!("Title: {}", fi.get("title").map_or("", String::as_str));
    println!("Channel: {}", fi.get("channel").map_or("", String::as_str));
    println!("Selected quality: {quality}");
    println!("Live: {}", p.live);
    if let Some(last) = p.last_sq {
        println!(
            "Duration so far: {} ({} fragments of {}s)",
            seconds_to_duration_and_time_str(last * u64::from(p.target_duration)),
            last,
            p.target_duration
        );
    }
    if let InputUrl::Channel { live_url } = input {
        println!("Resolved from: {live_url}");
    }
}
