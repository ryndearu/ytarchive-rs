//! Asking the user for a quality.

use super::{parse_quality_selection, quality_list_string, try_select, UrlMap, DEFAULT_QUALITY};
use crate::error::{Error, Result};
use crate::log_general;
use crate::prompt::get_user_input;
use crate::types::{FormatSelection, VideoCodec};

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

/// Picks formats from `selected`, prompting for a quality when nothing was
/// given or none of the given qualities are available. The codec is never
/// asked for: `codecs` (from the CLI) or the default order is used.
pub async fn select_interactive(
    urls: &UrlMap,
    mut selected: Vec<String>,
    codecs: Option<&[VideoCodec]>,
    video_only: bool,
) -> Result<FormatSelection> {
    if urls.is_empty() {
        return Err(Error::NoUrls("this stream".into()));
    }
    let available = super::available_qualities(urls);
    loop {
        if selected.is_empty() {
            selected = prompt_quality(&available, false).await;
        }
        let Some(sel) = try_select(urls, &selected, codecs, video_only) else {
            log_general!("The qualities you selected ended up unavailable for this stream");
            log_general!("You will now have the option to select from the available qualities");
            selected.clear();
            continue;
        };
        if let Some(codec) = sel.codec {
            log_general!("Selected quality: {} ({})", sel.label, codec.name());
        }
        return Ok(sel);
    }
}
