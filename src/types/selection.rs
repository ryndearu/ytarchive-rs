//! Result of choosing which audio/video streams to download.

use super::fragment_url::FragmentUrl;
use super::options::VideoCodec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatSelection {
    /// Quality label that matched, e.g. `1080p60` or `audio_only`.
    pub label: String,
    /// `None` for audio only.
    pub codec: Option<VideoCodec>,
    /// Video itag; `None` for audio only.
    pub video_itag: Option<u32>,
    pub audio_url: Option<FragmentUrl>,
    pub video_url: Option<FragmentUrl>,
}

impl FormatSelection {
    pub fn is_audio_only(&self) -> bool {
        self.video_itag.is_none()
    }
}
