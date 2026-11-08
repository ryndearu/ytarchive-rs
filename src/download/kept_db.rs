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
pub(super) fn save_info(s: &Session, dt: DataType, db: &DbSink, saved: Option<&SavedDownload>) -> Result<usize> {
    for u in db.urls()? {
        s.remember(dt, &u);
    }
    let urls = s.known_urls(dt);
    db.save_urls(&urls)?;
    if let Some(info) = saved {
        db.set_meta("download", &serde_json::to_string(info)?)?;
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
pub(super) fn save_progress(s: &Session, dt: DataType, sink: &Sink, head: Option<u64>, saved: &mut Saved) {
    let Sink::Db(db, _) = sink else { return };
    let urls = s.known_urls(dt);
    if urls.len() != saved.urls && db.save_urls(&urls).is_ok() {
        saved.urls = urls.len();
    }
    if let Some(h) = head
        && saved.head != head
        && db.set_meta("head", &h.to_string()).is_ok()
    {
        saved.head = head;
    }
}

/// At the end of a download: the URLs and the newest known fragment.
pub(super) fn save_end(s: &Session, dt: DataType, db: &DbSink, head: Option<u64>) -> Result<()> {
    db.save_urls(&s.known_urls(dt))?;
    match head {
        Some(h) => db.set_meta("head", &h.to_string()),
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
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().ends_with(&suffix)))?;
    // Closed again on return, before the download reopens it (Windows
    // can't share it otherwise).
    let db = DbSink::open(&path, mode).ok()?;
    let info: SavedDownload = serde_json::from_str(&db.meta("download").ok()??).ok()?;
    let head = db.meta("head").ok()?.and_then(|h| h.parse().ok());
    let urls = db.urls().ok()?;
    Some(KeptStream { info, head, urls })
}
