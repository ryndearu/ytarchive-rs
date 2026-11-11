use super::*;
use crate::types::complete_codec_order;
use VideoCodec::*;

fn urls(itags: &[u32]) -> UrlMap {
    itags
        .iter()
        .map(|itag| (*itag, FragmentUrl::with_sq_param(&format!("https://r.googlevideo.com/v?itag={itag}&noclen=1"))))
        .collect()
}

fn sel(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

#[test]
fn parse_selection() {
    assert_eq!(parse_quality_selection(" 1080P60 / bogus/720p/best"), ["1080p60", "720p", "best"]);
    assert_eq!(parse_quality_selection("audio"), ["audio"]);
    assert!(parse_quality_selection("nope").is_empty());
    assert_eq!(quality_list_string(&["audio_only", "720p"]), "audio_only, 720p, best");
}

#[test]
fn available_list() {
    // 720p h264+vp9, 1080p60 vp9; 399 AV1 also present.
    let offered = urls(&[140, 136, 247, 303, 399]);
    assert_eq!(available_qualities(&offered), ["audio_only", "720p", "1080p", "1080p60"]);
    // 399 alone (no base 1080p h264/vp9) counts for 1080p60 too.
    let offered = urls(&[140, 399]);
    assert_eq!(available_qualities(&offered), ["audio_only", "1080p", "1080p60"]);
}

#[test]
fn codecs_offered_per_label() {
    let offered = urls(&[140, 136, 247, 398, 303]);
    assert_eq!(codecs_for_label(&offered, "720p"), [Av1, Vp9, H264]);
    // 398 is the 720p AV1 itag; with base 720p in H264/VP9 it isn't 720p60.
    assert_eq!(codecs_for_label(&offered, "720p60"), Vec::<VideoCodec>::new());
    assert_eq!(codecs_for_label(&offered, "1080p60"), [Vp9]);
    assert!(codecs_for_label(&offered, "nope").is_empty());
}

#[test]
fn picks_best_with_codec_priority() {
    let offered = urls(&[140, 136, 247, 398]);
    let selection = try_select(&offered, &sel(&["best"]), Some(&BEST_CODEC_ORDER), false).unwrap();
    // 398 is 720p AV1; base 720p has h264/vp9 so "720p60" is not offered; best = 720p.
    assert_eq!(selection.label, "720p");
    assert_eq!(selection.codec, Some(Av1));
    assert_eq!(selection.video_itag, Some(398));
    assert!(selection.audio_url.is_some());

    let selection = try_select(&offered, &sel(&["720p"]), Some(&complete_codec_order(&[Vp9])), false).unwrap();
    assert_eq!(selection.video_itag, Some(247));
}

#[test]
fn best_picks_the_quality_and_codec_picks_the_codec() {
    // --codec h264 with best: the highest quality, in H.264.
    let h264 = complete_codec_order(&[H264]);
    let selection = try_select(&urls(&[140, 136, 247, 398]), &sel(&["best"]), Some(&h264), false).unwrap();
    assert_eq!((selection.label.as_str(), selection.codec), ("720p", Some(H264)));
    // H.264 not offered at the best quality: that quality in the next codec,
    // not a lower quality in H.264 (as in Go).
    let selection = try_select(&urls(&[140, 136, 248]), &sel(&["best"]), Some(&h264), false).unwrap();
    assert_eq!((selection.label.as_str(), selection.codec), ("1080p", Some(Vp9)));
}

#[test]
fn default_codec_order_is_av1_vp9_h264() {
    let offered = urls(&[140, 136, 247, 398]);
    let selection = try_select(&offered, &sel(&["720p"]), None, false).unwrap();
    assert_eq!(selection.codec, Some(Av1));
    let selection = try_select(&urls(&[140, 136, 247]), &sel(&["720p"]), None, false).unwrap();
    assert_eq!(selection.codec, Some(Vp9));
    let selection = try_select(&urls(&[140, 136]), &sel(&["720p"]), None, false).unwrap();
    assert_eq!(selection.codec, Some(H264));
}

#[test]
fn av1_not_used_for_60fps_when_base_has_other_codecs() {
    let offered = urls(&[140, 136, 398]);
    assert!(try_select(&offered, &sel(&["720p60"]), None, false).is_none());
    let selection = try_select(&offered, &sel(&["720p60", "720p"]), None, false).unwrap();
    assert_eq!(selection.label, "720p");
}

#[test]
fn audio_only_and_video_only() {
    let offered = urls(&[140, 136]);
    let selection = try_select(&offered, &sel(&["audio"]), None, false).unwrap();
    assert!(selection.is_audio_only());
    assert!(selection.audio_url.is_some());
    assert!(validate(&selection, false).is_ok());

    let selection = try_select(&offered, &sel(&["720p"]), None, true).unwrap();
    assert!(selection.audio_url.is_none());
    assert!(validate(&selection, true).is_ok());
    assert!(validate(&selection, false).is_err());
}

#[test]
fn fixed_selection() {
    let offered = urls(&[140, 299]);
    let selection = select_fixed(&offered, Some(299), false);
    assert!(selection.video_url.is_some() && selection.audio_url.is_some());
    assert!(validate(&selection, false).is_ok());
    let selection = select_fixed(&offered, Some(137), false);
    assert!(validate(&selection, false).is_err());
    let selection = select_fixed(&offered, None, false);
    assert!(selection.is_audio_only() && validate(&selection, false).is_ok());
}

#[tokio::test]
async fn codec_is_not_asked_for() {
    let offered = urls(&[140, 136, 247, 398]);
    let selection = select_interactive(&offered, sel(&["720p"]), None, false).await.unwrap();
    assert_eq!(selection.video_itag, Some(398));
}

#[tokio::test]
async fn unavailable_quality_falls_back_to_best() {
    let offered = urls(&[140, 136, 247]);
    let selection = select_interactive(&offered, sel(&["1080p"]), None, false).await.unwrap();
    assert_eq!((selection.label.as_str(), selection.codec), ("720p", Some(Vp9)));
}

#[tokio::test]
async fn given_codecs_are_used() {
    let offered = urls(&[140, 136, 247, 398]);
    let codecs = complete_codec_order(&[Vp9]);
    let selection = select_interactive(&offered, sel(&["720p"]), Some(&codecs), false).await.unwrap();
    assert_eq!(selection.codec, Some(Vp9));
}
