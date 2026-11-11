//! Quality parsing and audio/video format selection.

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::prompt::get_user_input;
use crate::types::{
    AUDIO_ITAG, BEST_CODEC_ORDER, FormatSelection, FragmentUrl, VIDEO_QUALITIES, VideoCodec, VideoItag, itags_for_label,
};
use crate::{log_debug, log_general};

pub const DEFAULT_QUALITY: &str = "best";

pub type UrlMap = HashMap<u32, FragmentUrl>;

/// All labels from the itag table, lowest first (used while waiting, when
/// nothing is known about the stream yet).
pub fn all_qualities() -> Vec<&'static str> {
    VIDEO_QUALITIES.iter().map(|(line, _)| *line).collect()
}

/// `a, b, c, best`
pub fn quality_list_string(qualities: &[&str]) -> String {
    let mut text: String = qualities.iter().map(|quality| format!("{quality}, ")).collect();
    text.push_str(DEFAULT_QUALITY);
    text
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
        .filter(|quality| *quality == "best" || *quality == "audio" || known.contains(quality))
        .map(String::from)
        .collect()
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
    BEST_CODEC_ORDER
        .into_iter()
        .filter(|&codec| !(codec == VideoCodec::Av1 && av1_ambiguous(label, itags, urls)))
        .filter(|&codec| urls.contains_key(&itag_for(itags, codec)))
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

/// First of `selected` that is available. The codec comes from `codecs`
/// (the user's order) or, if `None`, `BEST_CODEC_ORDER`. Returns `None` if none are available.
pub fn try_select(
    urls: &UrlMap,
    selected: &[String],
    codecs: Option<&[VideoCodec]>,
    video_only: bool,
) -> Option<FormatSelection> {
    let available = available_qualities(urls);
    let audio_url = if video_only { None } else { urls.get(&AUDIO_ITAG).cloned() };

    for quality in selected {
        let order = codecs.unwrap_or(&BEST_CODEC_ORDER);
        let label = match quality.trim() {
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
        log_debug!(
            "Codec priority order for {label}: {}",
            order.iter().map(|codec| codec.name()).collect::<Vec<_>>().join(", ")
        );
        if let Some(&codec) = order.iter().find(|candidate| offered.contains(candidate)) {
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
    let fragmented = |itag: &u32| urls.get(itag).filter(|url| url.is_fragmented()).cloned();
    FormatSelection {
        label: if video_itag.is_none() { "audio_only".into() } else { String::new() },
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

/// Prompts until the user enters at least one valid quality.
/// `waiting`: the stream hasn't started, so every possible quality is offered.
pub async fn prompt_quality(qualities: &[&str], waiting: bool) -> Vec<String> {
    if waiting {
        println!(
            "Since you are going to wait for the stream, you must pre-emptively select a video quality.\n\
             There is no way to know which qualities will be available before the stream starts, so a list of all possible stream qualities will be presented.\n\
             You can use youtube-dl style selection (slash-delimited first to last preference). Default is 'best'\n"
        );
    }
    println!("Available video qualities: {}", quality_list_string(qualities));
    loop {
        let mut quality = get_user_input("Enter desired video quality: ").await;
        if quality.is_empty() {
            quality = DEFAULT_QUALITY.into();
        }
        let sel = parse_quality_selection(&quality);
        if !sel.is_empty() {
            return sel;
        }
        println!("No valid qualities selected");
    }
}

/// Picks formats from `selected`, prompting for a quality when nothing was
/// given. If none of the given qualities are available, `best` is used
/// without asking. The codec is never asked for: `codecs` (from the CLI) or
/// the default order is used.
pub async fn select_interactive(
    urls: &UrlMap,
    mut selected: Vec<String>,
    codecs: Option<&[VideoCodec]>,
    video_only: bool,
) -> Result<FormatSelection> {
    if urls.is_empty() {
        return Err(Error::NoUrls("this stream".into()));
    }
    if selected.is_empty() {
        selected = prompt_quality(&available_qualities(urls), false).await;
    }
    let sel = match try_select(urls, &selected, codecs, video_only) {
        Some(sel) => sel,
        // Go asked again here, which stalls a download chosen ahead of time
        // with nobody watching; fall back to the default instead.
        None => {
            log_general!(
                "The qualities you selected ended up unavailable for this stream; using {DEFAULT_QUALITY} instead"
            );
            try_select(urls, &[DEFAULT_QUALITY.into()], codecs, video_only)
                .ok_or_else(|| Error::NoUrls("this stream".into()))?
        }
    };
    if let Some(codec) = sel.codec {
        log_general!("Selected quality: {} ({})", sel.label, codec.name());
    }
    Ok(sel)
}
