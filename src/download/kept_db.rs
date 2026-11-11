//! What an SQLite download keeps for retrying missing fragments in a later
//! run: saving it during the download, and reading it back.

use std::path::Path;

use super::db_sink::DbSink;
use super::sink::Sink;
use crate::error::Result;
use crate::session::Session;
use crate::types::{DataType, FragmentUrl, SavedDownload};

/// Stores what a later run needs, and adds the URLs saved by an earlier run
/// to the ones recovery can use. Returns how many URLs are saved.
pub(super) fn save_info(
    session: &Session,
    data_type: DataType,
    database: &DbSink,
    saved: Option<&SavedDownload>,
) -> Result<usize> {
    for url in database.urls()? {
        session.remember(data_type, &url);
    }
    let urls = session.known_urls(data_type);
    database.save_urls(&urls)?;
    if let Some(info) = saved {
        database.set_meta("download", &serde_json::to_string(info)?)?;
    }
    Ok(urls.len())
}

/// What `save_progress` has already written, so it writes only changes.
#[derive(Default)]
pub(super) struct Saved {
    pub urls: usize,
    pub head: Option<u64>,
}

/// Saves URLs added by a refresh and the newest known fragment since the
/// last call. The head is saved as it moves, not only by `save_end`, so a
/// download that is killed (taskkill, a closed console) still has it.
pub(super) fn save_progress(session: &Session, data_type: DataType, sink: &Sink, head: Option<u64>, saved: &mut Saved) {
    let Sink::Db(database, _) = sink else { return };
    let urls = session.known_urls(data_type);
    if urls.len() != saved.urls && database.save_urls(&urls).is_ok() {
        saved.urls = urls.len();
    }
    if let Some(head_seq) = head
        && saved.head != head
        && database.set_meta("head", &head_seq.to_string()).is_ok()
    {
        saved.head = head;
    }
}

/// At the end of a download: the URLs and the newest known fragment.
pub(super) fn save_end(session: &Session, data_type: DataType, database: &DbSink, head: Option<u64>) -> Result<()> {
    database.save_urls(&session.known_urls(data_type))?;
    match head {
        Some(head) => database.set_meta("head", &head.to_string()),
        None => Ok(()),
    }
}

/// One stream of a kept download.
pub struct KeptStream {
    pub info: SavedDownload,
    /// Newest known fragment.
    pub head: Option<u64>,
    /// Download URLs used, oldest first.
    pub urls: Vec<FragmentUrl>,
}

/// Reads the `*.f{itag}.sqlite` database in `temp_dir`, if it has the info
/// a later run needs.
pub fn read_kept(temp_dir: &Path, itag: u32, mode: u32) -> Option<KeptStream> {
    let suffix = format!(".f{itag}.sqlite");
    let path = std::fs::read_dir(temp_dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| path.file_name().is_some_and(|name| name.to_string_lossy().ends_with(&suffix)))?;
    // Closed again on return, before the download reopens it (Windows
    // can't share it otherwise).
    let database = DbSink::open(&path, mode).ok()?;
    let info: SavedDownload = serde_json::from_str(&database.meta("download").ok()??).ok()?;
    let head = database.meta("head").ok()?.and_then(|text| text.parse().ok());
    let urls = database.urls().ok()?;
    Some(KeptStream { info, head, urls })
}
