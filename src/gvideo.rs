//! Direct googlevideo URL downloads: no extraction, URLs come from the
//! command line (prompting for the other stream if only one was given).

use crate::input_url::parse_gvideo_url;
use crate::prompt::get_user_input;
use crate::types::{DataType, FormatSelection, FragmentUrl, Options, AUDIO_ITAG};
use crate::{log_error, log_general};

async fn prompt_url(dt: DataType) -> Option<(FragmentUrl, u32)> {
    loop {
        let raw = get_user_input(&format!(
            "Please enter the {} url, or nothing to skip: ",
            dt.as_str()
        ))
        .await;
        if raw.is_empty() {
            return None;
        }
        match parse_gvideo_url(&raw, dt) {
            Ok(v) => return Some(v),
            Err(e) => log_general!("{e}"),
        }
    }
}

fn from_flag(raw: Option<&str>, dt: DataType) -> Option<(FragmentUrl, u32)> {
    let raw = raw?;
    parse_gvideo_url(raw, dt)
        .inspect_err(|e| log_error!("--{}-url: {e}", dt.as_str()))
        .ok()
}

/// Builds the selection from the input URL plus `--audio-url`/`--video-url`.
pub async fn selection(opts: &Options, input_itag: u32, input_url: FragmentUrl) -> Option<FormatSelection> {
    let mut audio = from_flag(opts.audio_url.as_deref(), DataType::Audio);
    let mut video = from_flag(opts.video_url.as_deref(), DataType::Video);
    if input_itag == AUDIO_ITAG {
        audio.get_or_insert((input_url, input_itag));
    } else {
        video.get_or_insert((input_url, input_itag));
    }

    if audio.is_none() && !opts.video_only {
        audio = prompt_url(DataType::Audio).await;
    }
    if video.is_none() && !opts.audio_only {
        video = prompt_url(DataType::Video).await;
    }
    if opts.video_only {
        audio = None;
    }
    if opts.audio_only {
        video = None;
    }
    if audio.is_none() && video.is_none() {
        log_error!("Neither an audio nor a video URL was given.");
        return None;
    }
    Some(FormatSelection {
        label: String::new(),
        codec: None,
        video_itag: video.as_ref().map(|v| v.1),
        audio_url: audio.map(|a| a.0),
        video_url: video.map(|v| v.0),
    })
}
