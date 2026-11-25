use std::collections::HashMap;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::*;
use crate::http::Http;
use crate::types::{DataType, FormatSelection, FragmentUrl, LiveStatus, Options, YtdlpExtraction};

fn session_with_audio_and_video() -> Arc<Session> {
    let opts = Arc::new(Options::default());
    let http = Http::new(&opts, Default::default()).unwrap();
    let selection = FormatSelection {
        label: "1080p60".into(),
        codec: None,
        video_itag: Some(299),
        audio_url: Some(FragmentUrl::with_sq_param("https://old/a?itag=140&noclen=1")),
        video_url: Some(FragmentUrl::with_sq_param("https://old/v?itag=299&noclen=1")),
    };
    Session::new(opts, http, CancellationToken::new(), None, &selection, true, 5)
}

#[test]
fn periodic_refresh_applies_partial_updates_without_discarding_valid_url() {
    let session = session_with_audio_and_video();
    let mut urls = HashMap::new();
    urls.insert(299, FragmentUrl::with_sq_param("https://new/v?itag=299&noclen=1"));
    let info = YtdlpExtraction { urls, live_status: LiveStatus::IsLive, ..Default::default() };

    assert!(!session.apply_periodic_refresh(Ok(info)));
    assert_eq!(session.url(DataType::Video).unwrap().for_seq(1), "https://new/v?itag=299&noclen=1&sq=1");
    assert_eq!(session.url(DataType::Audio).unwrap().for_seq(1), "https://old/a?itag=140&noclen=1&sq=1");
}

#[test]
fn periodic_refresh_skips_finished_stream() {
    let session = session_with_audio_and_video();
    session.set_finished(DataType::Audio);
    let mut urls = HashMap::new();
    urls.insert(299, FragmentUrl::with_sq_param("https://new/v?itag=299&noclen=1"));
    let info = YtdlpExtraction { urls, live_status: LiveStatus::IsLive, ..Default::default() };

    assert!(session.apply_periodic_refresh(Ok(info)));
    assert_eq!(session.url(DataType::Video).unwrap().for_seq(1), "https://new/v?itag=299&noclen=1&sq=1");
    assert_eq!(session.url(DataType::Audio).unwrap().for_seq(1), "https://old/a?itag=140&noclen=1&sq=1");
}

#[test]
fn periodic_refresh_stops_retrying_when_stream_ended_without_live_urls() {
    let session = session_with_audio_and_video();
    let info = YtdlpExtraction { live_status: LiveStatus::WasLive, ..Default::default() };

    assert!(session.apply_periodic_refresh(Ok(info)));
    assert!(!session.is_live());
    assert_eq!(session.url(DataType::Audio).unwrap().for_seq(1), "https://old/a?itag=140&noclen=1&sq=1");
    assert_eq!(session.url(DataType::Video).unwrap().for_seq(1), "https://old/v?itag=299&noclen=1&sq=1");
}
