//! Per-stream (audio/video) download URL and job bookkeeping.

use super::fragment::DataType;

#[derive(Debug, Clone)]
pub struct MediaDlInfo {
    pub data_type: DataType,
    pub download_url: String,
    pub url_host: String,
    pub base_path: String,
    pub active_jobs: usize,
    pub finished: bool,
}

impl MediaDlInfo {
    pub fn new(data_type: DataType) -> Self {
        Self {
            data_type,
            download_url: String::new(),
            url_host: String::new(),
            base_path: String::new(),
            active_jobs: 0,
            finished: false,
        }
    }
}
