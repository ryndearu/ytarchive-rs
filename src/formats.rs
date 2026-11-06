//! Quality parsing and audio/video format selection.

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::prompt::get_user_input;
use crate::types::{
    itags_for_label, CodecPrefs, FormatSelection, FragmentUrl, VideoCodec, VideoItag, AUDIO_ITAG,
    VIDEO_QUALITIES,
};
use crate::{log_debug, log_general};

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

/// Labels available in `urls`, always starting with `audio_only`.
pub fn available_qualities(urls: &UrlMap) -> Vec<&'static str> {
    let mut out = vec!["audio_only"];
    for (label, itags) in VIDEO_QUALITIES.iter().skip(1) {
        let av1 = urls.contains_key(&itags.av1) && !av1_ambiguous(label, *itags, urls);
        if av1 || urls.contains_key(&itags.vp9) || urls.contains_key(&itags.h264) {
            out.push(label);
        }
    }
    out
}

/// First of `selected` that is available. Returns `None` if none are.
pub fn try_select(
    urls: &UrlMap,
    selected: &[String],
    prefs: CodecPrefs,
    video_only: bool,
) -> Option<FormatSelection> {
    let available = available_qualities(urls);
    let audio_url = if video_only {
        None
    } else {
        urls.get(&AUDIO_ITAG).cloned()
    };
    let order = prefs.priority_order();
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
        for &codec in &order {
            if codec == VideoCodec::Av1 && av1_ambiguous(label, itags, urls) {
                log_debug!("Treating {label} AV1 itag={} as unavailable", itags.av1);
                continue;
            }
            let itag = itag_for(itags, codec);
            if let Some(url) = urls.get(&itag) {
                return Some(FormatSelection {
                    label: label.into(),
                    codec: Some(codec),
                    video_itag: Some(itag),
                    audio_url,
                    video_url: Some(url.clone()),
                });
            }
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

/// Picks formats from `selected`, prompting when nothing was given or none of
/// the given qualities are available.
pub async fn select_interactive(
    urls: &UrlMap,
    mut selected: Vec<String>,
    prefs: CodecPrefs,
    video_only: bool,
) -> Result<FormatSelection> {
    if urls.is_empty() {
        return Err(Error::NoUrls("this stream".into()));
    }
    let available = available_qualities(urls);
    loop {
        if selected.is_empty() {
            selected = prompt_quality(&available, false).await;
        }
        if let Some(sel) = try_select(urls, &selected, prefs, video_only) {
            if let Some(codec) = sel.codec {
                log_general!("Selected quality: {} ({})", sel.label, codec.name());
            }
            return Ok(sel);
        }
        log_general!("The qualities you selected ended up unavailable for this stream");
        log_general!("You will now have the option to select from the available qualities");
        selected.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(itags: &[u32]) -> UrlMap {
        itags
            .iter()
            .map(|i| {
                (
                    *i,
                    FragmentUrl::with_sq_param(&format!("https://r.googlevideo.com/v?itag={i}&noclen=1")),
                )
            })
            .collect()
    }

    fn sel(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parse_selection() {
        assert_eq!(parse_quality_selection(" 1080P60 / bogus/720p/best"), ["1080p60", "720p", "best"]);
        assert_eq!(parse_quality_selection("audio"), ["audio"]);
        assert!(parse_quality_selection("nope").is_empty());
        assert_eq!(quality_list_string(&["audio_only", "720p"]), "audio_only, 720p, best");
    }

    #[test]
    fn codec_order() {
        use VideoCodec::*;
        assert_eq!(CodecPrefs::default().priority_order(), [Av1, Vp9, H264]);
        let p = CodecPrefs { h264: true, ..Default::default() };
        assert_eq!(p.priority_order(), [H264, Av1, Vp9]);
        let p = CodecPrefs { h264: true, vp9: true, av1: false };
        assert_eq!(p.priority_order(), [Vp9, H264, Av1]);
    }

    #[test]
    fn available_list() {
        // 720p h264+vp9, 1080p60 vp9; 399 AV1 also present.
        let u = urls(&[140, 136, 247, 303, 399]);
        assert_eq!(available_qualities(&u), ["audio_only", "720p", "1080p", "1080p60"]);
        // 399 alone (no base 1080p h264/vp9) counts for 1080p60 too.
        let u = urls(&[140, 399]);
        assert_eq!(available_qualities(&u), ["audio_only", "1080p", "1080p60"]);
    }

    #[test]
    fn picks_best_with_codec_priority() {
        let u = urls(&[140, 136, 247, 398]);
        let s = try_select(&u, &sel(&["best"]), CodecPrefs::default(), false).unwrap();
        // 398 is 720p AV1; base 720p has h264/vp9 so "720p60" is not offered; best = 720p.
        assert_eq!(s.label, "720p");
        assert_eq!(s.codec, Some(VideoCodec::Av1));
        assert_eq!(s.video_itag, Some(398));
        assert!(s.audio_url.is_some());

        let s = try_select(&u, &sel(&["720p"]), CodecPrefs { h264: true, ..Default::default() }, false).unwrap();
        assert_eq!(s.video_itag, Some(136));
    }

    #[test]
    fn av1_not_used_for_60fps_when_base_has_other_codecs() {
        let u = urls(&[140, 136, 398]);
        assert!(try_select(&u, &sel(&["720p60"]), CodecPrefs::default(), false).is_none());
        let s = try_select(&u, &sel(&["720p60", "720p"]), CodecPrefs::default(), false).unwrap();
        assert_eq!(s.label, "720p");
    }

    #[test]
    fn audio_only_and_video_only() {
        let u = urls(&[140, 136]);
        let s = try_select(&u, &sel(&["audio"]), CodecPrefs::default(), false).unwrap();
        assert!(s.is_audio_only());
        assert!(s.audio_url.is_some());
        assert!(validate(&s, false).is_ok());

        let s = try_select(&u, &sel(&["720p"]), CodecPrefs::default(), true).unwrap();
        assert!(s.audio_url.is_none());
        assert!(validate(&s, true).is_ok());
        assert!(validate(&s, false).is_err());
    }

    #[test]
    fn fixed_selection() {
        let u = urls(&[140, 299]);
        let s = select_fixed(&u, Some(299), false);
        assert!(s.video_url.is_some() && s.audio_url.is_some());
        assert!(validate(&s, false).is_ok());
        let s = select_fixed(&u, Some(137), false);
        assert!(validate(&s, false).is_err());
        let s = select_fixed(&u, None, false);
        assert!(s.is_audio_only() && validate(&s, false).is_ok());
    }
}
