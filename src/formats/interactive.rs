//! Asking the user for a quality and a codec.

use super::{
    codecs_for_label, is_audio_only_selection, parse_quality_selection, quality_list_string,
    try_select, UrlMap, DEFAULT_QUALITY,
};
use crate::error::{Error, Result};
use crate::log_general;
use crate::prompt::get_user_input;
use crate::types::{parse_codec_list, FormatSelection, VideoCodec, DEFAULT_CODEC_ORDER};

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
        let mut q = get_user_input("Enter desired video quality: ").await;
        if q.is_empty() {
            q = DEFAULT_QUALITY.into();
        }
        let sel = parse_quality_selection(&q);
        if !sel.is_empty() {
            return sel;
        }
        println!("No valid qualities selected");
    }
}

/// Prompts for a codec preference among `available` (all codecs while
/// waiting). Empty input keeps the default order (av1/vp9/h264).
pub async fn prompt_codecs(available: &[VideoCodec], waiting: bool) -> Vec<VideoCodec> {
    if waiting {
        println!("Which codecs the stream will offer isn't known until it starts; codecs you list that aren't available are skipped.");
    }
    let names: Vec<&str> = available.iter().map(|c| c.arg()).collect();
    println!("Available video codecs: {}", names.join(", "));
    let default: Vec<&str> = DEFAULT_CODEC_ORDER.iter().map(|c| c.arg()).collect();
    loop {
        let answer = get_user_input(&format!(
            "Enter preferred video codec(s), e.g. vp9/h264 (default {}): ",
            default.join("/")
        ))
        .await;
        match parse_codec_list(&answer) {
            Some(order) => return order,
            None => println!("No valid codecs selected (valid: av1, vp9, h264)"),
        }
    }
}

/// Picks formats from `selected`, prompting for a quality when nothing was
/// given or none of the given qualities are available, and for a codec when
/// `codecs` is `None` and more than one is offered (the answer is stored in
/// `codecs`). With `ask == false` (`--info-only`), nothing is asked: `best`
/// and the default codec order are used.
pub async fn select_interactive(
    urls: &UrlMap,
    mut selected: Vec<String>,
    codecs: &mut Option<Vec<VideoCodec>>,
    video_only: bool,
    ask: bool,
) -> Result<FormatSelection> {
    if urls.is_empty() {
        return Err(Error::NoUrls("this stream".into()));
    }
    let available = super::available_qualities(urls);
    if !ask {
        if selected.is_empty() {
            selected = vec![DEFAULT_QUALITY.into()];
        }
        let order = codecs.clone().unwrap_or_else(|| DEFAULT_CODEC_ORDER.to_vec());
        return try_select(urls, &selected, &order, video_only)
            .ok_or_else(|| Error::NoUrls("the selected qualities".into()));
    }
    loop {
        if selected.is_empty() {
            selected = prompt_quality(&available, false).await;
        }
        // Find the quality first, then settle the codec for it.
        let Some(found) = try_select(urls, &selected, &DEFAULT_CODEC_ORDER, video_only) else {
            log_general!("The qualities you selected ended up unavailable for this stream");
            log_general!("You will now have the option to select from the available qualities");
            selected.clear();
            continue;
        };
        if found.is_audio_only() || is_audio_only_selection(&selected) {
            return Ok(found);
        }
        if codecs.is_none() {
            let offered = codecs_for_label(urls, &found.label);
            *codecs = Some(if offered.len() > 1 {
                prompt_codecs(&offered, false).await
            } else {
                DEFAULT_CODEC_ORDER.to_vec()
            });
        }
        let order = codecs.clone().unwrap_or_else(|| DEFAULT_CODEC_ORDER.to_vec());
        let sel = try_select(urls, std::slice::from_ref(&found.label), &order, video_only).unwrap_or(found);
        if let Some(codec) = sel.codec {
            log_general!("Selected quality: {} ({})", sel.label, codec.name());
        }
        return Ok(sel);
    }
}
