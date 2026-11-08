//! What an SQLite download stores about the stream, so a later run can retry
//! missing fragments even when the stream can no longer be looked up (it went
//! private, or its live download is gone).

use serde::{Deserialize, Serialize};

use super::format_info::{FormatInfo, MetaInfo};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedDownload {
    pub video_id: String,
    /// Output template values (title, channel, ...), for the file name.
    pub format_info: FormatInfo,
    /// Metadata for the final file and the description.
    pub metadata: MetaInfo,
    pub thumbnail_url: String,
    pub target_duration: u32,
    /// Quality label that was selected, e.g. `1080p60` or `audio_only`.
    pub label: String,
    /// `None` for audio only.
    pub video_itag: Option<u32>,
}
