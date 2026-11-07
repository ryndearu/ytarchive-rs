//! Quality parsing and audio/video format selection.

mod interactive;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

pub use interactive::{prompt_codecs, prompt_quality, select_interactive};

use crate::error::{Error, Result};
use crate::types::{
    itags_for_label, FormatSelection, FragmentUrl, VideoCodec, VideoItag, AUDIO_ITAG,
    DEFAULT_CODEC_ORDER, VIDEO_QUALITIES,
};
use crate::log_debug;

pub const DEFAULT_QUALITY: &str = "best";

pub type UrlMap = HashMap<u32, FragmentUrl>;

/// All labels from the itag table, lowest first (used while waiting, when
/// nothing is known about the stream yet).
pub fn all_qualities() -> Vec<&'static str> {
    VIDEO_QUALITIES.iter().map(|(l, _)| *l).collect()
}

/// `a, b, c, best`
pub fn quality_list_string(qualities: &[&str]) -> String {
    let mut s: String = qualities.iter().map(|q| format!("{q}, ")).collect();
    s.push_str(DEFAULT_QUALITY);
    s
}

/// Splits `1080p60/720p/best` into valid labels (lowercased). `best` and
/// `audio` pass through; unknown labels are dropped.
pub fn parse_quality_selection(quality: &str) -> Vec<String> {
    let known = all_qualities();
    quality
        .trim()
        .to_lowercase()
        .split('/')
        .map(str::trim)
        .filter(|q| *q == "best" || *q == "audio" || known.contains(q))
        .map(String::from)
        .collect()
}

/// Whether a quality list asks for audio only (no codec needed).
pub fn is_audio_only_selection(selected: &[String]) -> bool {
    selected.first().is_some_and(|q| q == "audio" || q == "audio_only")
}

/// 60fps labels share their AV1 itag with the base quality (AV1 itags don't
/// encode frame rate). If the base quality has H264/VP9, an AV1 match for the
/// 60fps label is really the 30fps stream, so treat it as unavailable.
fn av1_ambiguous(label: &str, itags: VideoItag, urls: &UrlMap) -> bool {
    let Some(base_label) = label.strip_suffix("60") else {
        return false;
    };
    let Some(base) = itags_for_label(base_label) else {
        return false;
    };
    base.av1 == itags.av1 && (urls.contains_key(&base.h264) || urls.contains_key(&base.vp9))
}

fn itag_for(itags: VideoItag, codec: VideoCodec) -> u32 {
    match codec {
        VideoCodec::Av1 => itags.av1,
        VideoCodec::Vp9 => itags.vp9,
        VideoCodec::H264 => itags.h264,
    }
}

/// Codecs the stream offers for `label`, in default order.
pub fn codecs_for_label(urls: &UrlMap, label: &str) -> Vec<VideoCodec> {
    let Some(itags) = itags_for_label(label) else {
        return Vec::new();
    };
    DEFAULT_CODEC_ORDER
        .into_iter()
        .filter(|&c| !(c == VideoCodec::Av1 && av1_ambiguous(label, itags, urls)))
        .filter(|&c| urls.contains_key(&itag_for(itags, c)))
        .collect()
}

/// Labels available in `urls`, always starting with `audio_only`.
pub fn available_qualities(urls: &UrlMap) -> Vec<&'static str> {
    let mut out = vec!["audio_only"];
    for (label, _) in VIDEO_QUALITIES.iter().skip(1) {
        if !codecs_for_label(urls, label).is_empty() {
            out.push(label);
        }
    }
    out
}

/// First of `selected` that is available, using `order` to pick the codec.
/// Returns `None` if none are available.
pub fn try_select(
    urls: &UrlMap,
    selected: &[String],
    order: &[VideoCodec],
    video_only: bool,
) -> Option<FormatSelection> {
    let available = available_qualities(urls);
    let audio_url = if video_only {
        None
    } else {
        urls.get(&AUDIO_ITAG).cloned()
    };
    log_debug!(
        "Codec priority order: {}",
        order.iter().map(|c| c.name()).collect::<Vec<_>>().join(", ")
    );

    for q in selected {
        let label = match q.trim() {
            "best" => *available.last()?,
            "audio" => "audio_only",
            other => other,
        };
        let Some(itags) = itags_for_label(label) else {
            continue;
        };
        if label == "audio_only" {
            return Some(FormatSelection {
                label: label.into(),
                codec: None,
                video_itag: None,
                audio_url: urls.get(&AUDIO_ITAG).cloned(),
                video_url: None,
            });
        }
        let offered = codecs_for_label(urls, label);
        if let Some(&codec) = order.iter().find(|c| offered.contains(c)) {
            let itag = itag_for(itags, codec);
            return Some(FormatSelection {
                label: label.into(),
                codec: Some(codec),
                video_itag: Some(itag),
                audio_url,
                video_url: urls.get(&itag).cloned(),
            });
        }
    }
    None
}

/// Selection when the video itag is already fixed (googlevideo URL input or
/// `--no-video`). `video_itag == None` means audio only.
pub fn select_fixed(urls: &UrlMap, video_itag: Option<u32>, video_only: bool) -> FormatSelection {
    let fragmented = |itag: &u32| urls.get(itag).filter(|u| u.is_fragmented()).cloned();
    FormatSelection {
        label: if video_itag.is_none() {
            "audio_only".into()
        } else {
            String::new()
        },
        codec: None,
        video_itag,
        audio_url: if video_only { None } else { fragmented(&AUDIO_ITAG) },
        video_url: video_itag.as_ref().and_then(fragmented),
    }
}

/// Ensures the URLs needed for the requested streams are present.
pub fn validate(sel: &FormatSelection, video_only: bool) -> Result<()> {
    if !video_only && !sel.is_audio_only() && sel.audio_url.is_none() {
        return Err(Error::NoUrls("audio".into()));
    }
    if sel.is_audio_only() && sel.audio_url.is_none() {
        return Err(Error::NoUrls("audio".into()));
    }
    if !sel.is_audio_only() && sel.video_url.is_none() {
        return Err(Error::NoUrls("video".into()));
    }
    Ok(())
}
