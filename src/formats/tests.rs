use super::*;
use crate::types::complete_codec_order;
use VideoCodec::*;

fn urls(itags: &[u32]) -> UrlMap {
    itags
        .iter()
        .map(|i| (*i, FragmentUrl::with_sq_param(&format!("https://r.googlevideo.com/v?itag={i}&noclen=1"))))
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
    assert_eq!(complete_codec_order(&[]), [Av1, Vp9, H264]);
    assert_eq!(complete_codec_order(&[H264]), [H264, Av1, Vp9]);
    assert_eq!(complete_codec_order(&[Vp9, H264]), [Vp9, H264, Av1]);
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
fn codecs_offered_per_label() {
    let u = urls(&[140, 136, 247, 398, 303]);
    assert_eq!(codecs_for_label(&u, "720p"), [Av1, Vp9, H264]);
    // 398 is the 720p AV1 itag; with base 720p in H264/VP9 it isn't 720p60.
    assert_eq!(codecs_for_label(&u, "720p60"), Vec::<VideoCodec>::new());
    assert_eq!(codecs_for_label(&u, "1080p60"), [Vp9]);
    assert!(codecs_for_label(&u, "nope").is_empty());
}

#[test]
fn picks_best_with_codec_priority() {
    let u = urls(&[140, 136, 247, 398]);
    let s = try_select(&u, &sel(&["best"]), Some(&BEST_CODEC_ORDER), false).unwrap();
    // 398 is 720p AV1; base 720p has h264/vp9 so "720p60" is not offered; best = 720p.
    assert_eq!(s.label, "720p");
    assert_eq!(s.codec, Some(Av1));
    assert_eq!(s.video_itag, Some(398));
    assert!(s.audio_url.is_some());

    let s = try_select(&u, &sel(&["720p"]), Some(&complete_codec_order(&[Vp9])), false).unwrap();
    assert_eq!(s.video_itag, Some(247));
}

#[test]
fn best_picks_the_quality_and_codec_picks_the_codec() {
    // --codec h264 with best: the highest quality, in H.264.
    let h264 = complete_codec_order(&[H264]);
    let s = try_select(&urls(&[140, 136, 247, 398]), &sel(&["best"]), Some(&h264), false).unwrap();
    assert_eq!((s.label.as_str(), s.codec), ("720p", Some(H264)));
    // H.264 not offered at the best quality: that quality in the next codec,
    // not a lower quality in H.264 (as in Go).
    let s = try_select(&urls(&[140, 136, 248]), &sel(&["best"]), Some(&h264), false).unwrap();
    assert_eq!((s.label.as_str(), s.codec), ("1080p", Some(Vp9)));
}

#[test]
fn default_codec_order_is_av1_vp9_h264() {
    let u = urls(&[140, 136, 247, 398]);
    let s = try_select(&u, &sel(&["720p"]), None, false).unwrap();
    assert_eq!(s.codec, Some(Av1));
    let s = try_select(&urls(&[140, 136, 247]), &sel(&["720p"]), None, false).unwrap();
    assert_eq!(s.codec, Some(Vp9));
    let s = try_select(&urls(&[140, 136]), &sel(&["720p"]), None, false).unwrap();
    assert_eq!(s.codec, Some(H264));
}

#[test]
fn av1_not_used_for_60fps_when_base_has_other_codecs() {
    let u = urls(&[140, 136, 398]);
    assert!(try_select(&u, &sel(&["720p60"]), None, false).is_none());
    let s = try_select(&u, &sel(&["720p60", "720p"]), None, false).unwrap();
    assert_eq!(s.label, "720p");
}

#[test]
fn audio_only_and_video_only() {
    let u = urls(&[140, 136]);
    let s = try_select(&u, &sel(&["audio"]), None, false).unwrap();
    assert!(s.is_audio_only());
    assert!(s.audio_url.is_some());
    assert!(validate(&s, false).is_ok());

    let s = try_select(&u, &sel(&["720p"]), None, true).unwrap();
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

#[tokio::test]
async fn codec_is_not_asked_for() {
    let u = urls(&[140, 136, 247, 398]);
    let s = select_interactive(&u, sel(&["720p"]), None, false).await.unwrap();
    assert_eq!(s.video_itag, Some(398));
}

#[tokio::test]
async fn unavailable_quality_falls_back_to_best() {
    let u = urls(&[140, 136, 247]);
    let s = select_interactive(&u, sel(&["1080p"]), None, false).await.unwrap();
    assert_eq!((s.label.as_str(), s.codec), ("720p", Some(Vp9)));
}

#[tokio::test]
async fn given_codecs_are_used() {
    let u = urls(&[140, 136, 247, 398]);
    let codecs = complete_codec_order(&[Vp9]);
    let s = select_interactive(&u, sel(&["720p"]), Some(&codecs), false).await.unwrap();
    assert_eq!(s.codec, Some(Vp9));
}
