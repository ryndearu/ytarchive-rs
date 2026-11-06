//! Per-stream (audio/video) download URL and job bookkeeping.

use super::fragment::DataType;
use super::fragment_url::FragmentUrl;

#[derive(Debug, Clone)]
pub struct MediaDlInfo {
    pub data_type: DataType,
    pub download_url: Option<FragmentUrl>,
    pub url_host: String,
    pub base_path: String,
    pub active_jobs: usize,
    pub finished: bool,
}

impl MediaDlInfo {
    pub fn new(data_type: DataType) -> Self {
        Self {
            data_type,
            download_url: None,
            url_host: String::new(),
            base_path: String::new(),
            active_jobs: 0,
            finished: false,
        }
    }

    pub fn set_url(&mut self, url: Option<FragmentUrl>) {
        self.url_host = url.as_ref().and_then(FragmentUrl::host).unwrap_or_default();
        self.download_url = url;
    }
}
