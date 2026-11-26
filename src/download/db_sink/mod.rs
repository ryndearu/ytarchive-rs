//! `--sqlite` storage (from livestream_dl): every fragment goes into a SQLite
//! table keyed by sequence as soon as it arrives, so nothing waits in memory
//! behind a slow or missing fragment. Resuming skips fragments already
//! stored. At the end the table is exported in order to the `.ts` file.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OpenFlags, params};
use tokio::task::spawn_blocking;

use super::sink::{Event, fragment_bytes, remove_frag_file};
use crate::error::{Error, Result};
use crate::log_debug;
use crate::mp4_atoms::strip_for_concat;
use crate::session::Session;
use crate::types::{DataType, Fragment, FragmentUrl, SavedDownload};
use crate::util::{apply_mode, lock};

pub struct DbSink {
    /// Shared so database operations can run on a blocking thread.
    conn: Arc<Mutex<Connection>>,
    path: PathBuf,
}

/// Metadata updates committed alongside a fragment insert inside `push_with_meta`.
#[derive(Default)]
pub struct MetaUpdate {
    pub head: Option<u64>,
    pub urls: Vec<FragmentUrl>,
}

/// `x.f140.ts` -> `x.f140.sqlite`
pub fn db_path(data_file: &Path) -> PathBuf {
    data_file.with_extension("sqlite")
}

async fn run_blocking<T: Send + 'static>(
    conn: &Arc<Mutex<Connection>>,
    func: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
) -> Result<T> {
    let conn = conn.clone();
    spawn_blocking(move || func(&mut lock(&conn))).await.map_err(io::Error::other)?
}

fn stored_with(conn: &Connection) -> Result<HashSet<u64>> {
    let mut stmt = conn.prepare("SELECT seq FROM segments")?;
    let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
    Ok(rows.map(|row| row.map(|sequence| sequence as u64)).collect::<rusqlite::Result<_>>()?)
}

fn stored_and_size_with(conn: &Connection) -> Result<(HashSet<u64>, u64)> {
    let mut stmt = conn.prepare("SELECT seq, LENGTH(data) FROM segments")?;
    let mut rows = stmt.query([])?;
    let mut stored = HashSet::new();
    let mut total_size = 0u64;
    while let Some(row) = rows.next()? {
        let sequence = row.get::<_, i64>(0)? as u64;
        let len = row.get::<_, i64>(1)? as u64;
        stored.insert(sequence);
        total_size += len;
    }
    Ok((stored, total_size))
}

fn save_urls_with(conn: &Connection, urls: &[FragmentUrl]) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare_cached("INSERT OR IGNORE INTO urls (prefix, suffix) VALUES (?1, ?2)")?;
    for url in urls {
        let (prefix, suffix) = url.parts();
        stmt.execute(params![prefix, suffix])?;
    }
    Ok(())
}

fn urls_with(conn: &Connection) -> Result<Vec<FragmentUrl>> {
    let mut stmt = conn.prepare("SELECT prefix, suffix FROM urls ORDER BY rowid")?;
    let rows = stmt.query_map([], |row| Ok(FragmentUrl::from_parts(row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn meta_with(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = ?1")?;
    let mut rows = stmt.query(params![key])?;
    Ok(match rows.next()? {
        Some(row) => Some(row.get(0)?),
        None => None,
    })
}

impl DbSink {
    pub fn open(path: &Path, mode: u32) -> Result<Self> {
        let conn = Connection::open(path)?;
        apply_mode(path, mode);
        conn.execute_batch(
            "PRAGMA busy_timeout = 5000;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             -- SQLite's default of 1000 pages (4 MiB) is about two video
             -- fragments, so nearly every insert also copied the log into
             -- the database and synced it. 8 MiB does that half as often;
             -- 16 MiB made one insert stall for 7 seconds on a slow disk.
             PRAGMA wal_autocheckpoint = 2000;
             CREATE TABLE IF NOT EXISTS segments (
                 seq  INTEGER PRIMARY KEY,
                 mime TEXT NOT NULL,
                 data BLOB NOT NULL
             );
             -- Every download URL used, so a later run can retry missing
             -- fragments after the stream went private.
             CREATE TABLE IF NOT EXISTS urls (
                 prefix TEXT NOT NULL,
                 suffix TEXT NOT NULL,
                 PRIMARY KEY (prefix, suffix)
             );
             -- `download`: SavedDownload JSON; `head`: newest known fragment.
             CREATE TABLE IF NOT EXISTS meta (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );",
        )?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)), path: path.to_path_buf() })
    }

    pub async fn open_async(path: PathBuf, mode: u32) -> Result<Self> {
        spawn_blocking(move || Self::open(&path, mode)).await.map_err(io::Error::other)?
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Sequence numbers already stored.
    pub async fn stored_async(&self) -> Result<HashSet<u64>> {
        run_blocking(&self.conn, |conn| stored_with(conn)).await
    }

    /// Adds download URLs (already stored ones are ignored).
    #[cfg(test)]
    pub fn save_urls(&self, urls: &[FragmentUrl]) -> Result<()> {
        Ok(save_urls_with(&lock(&self.conn), urls)?)
    }

    /// Stored download URLs, oldest first.
    #[cfg(test)]
    pub fn urls(&self) -> Result<Vec<FragmentUrl>> {
        urls_with(&lock(&self.conn))
    }

    #[cfg(test)]
    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        lock(&self.conn).execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", params![key, value])?;
        Ok(())
    }

    #[cfg(test)]
    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        meta_with(&lock(&self.conn), key)
    }

    pub async fn stored_and_size_async(&self) -> Result<(HashSet<u64>, u64)> {
        run_blocking(&self.conn, |conn| stored_and_size_with(conn)).await
    }

    pub async fn push(&mut self, seq: u64, frag: Option<Fragment>) -> Result<Vec<Event>> {
        self.push_with_meta(seq, frag, MetaUpdate::default()).await
    }

    pub async fn push_with_meta(&mut self, seq: u64, frag: Option<Fragment>, meta: MetaUpdate) -> Result<Vec<Event>> {
        let Some(frag) = frag else {
            return Ok(vec![Event::Gap(seq)]);
        };
        let data = fragment_bytes(&frag.data).await;
        remove_frag_file(&frag.data).await;
        let data = data?;
        let stored = data.len() as u64;
        let mime_type = frag.mime_type;
        // Commits segment and any head/URL metadata together on a blocking thread
        // so WAL checkpoints never stall async tasks.
        run_blocking(&self.conn, move |conn| {
            if meta.head.is_none() && meta.urls.is_empty() {
                let mut insert =
                    conn.prepare_cached("INSERT OR REPLACE INTO segments (seq, mime, data) VALUES (?1, ?2, ?3)")?;
                insert.execute(params![seq as i64, mime_type, &data[..]])?;
            } else {
                let tx = conn.transaction()?;
                {
                    let mut insert =
                        tx.prepare_cached("INSERT OR REPLACE INTO segments (seq, mime, data) VALUES (?1, ?2, ?3)")?;
                    insert.execute(params![seq as i64, mime_type, &data[..]])?;
                    if !meta.urls.is_empty() {
                        save_urls_with(&tx, &meta.urls)?;
                    }
                    if let Some(head_seq) = meta.head {
                        tx.execute(
                            "INSERT OR REPLACE INTO meta (key, value) VALUES ('head', ?1)",
                            params![head_seq.to_string()],
                        )?;
                    }
                }
                tx.commit()?;
            }
            Ok(())
        })
        .await?;
        Ok(vec![Event::Wrote(stored)])
    }

    /// Writes all stored fragments in order to `out` (replacing it). The
    /// first fragment keeps its `ftyp` box. Returns (fragments, bytes).
    /// Deletes a partial `out` if writing fails.
    pub fn export(&self, out: &Path, mode: u32) -> Result<(u64, u64)> {
        let res = self.write_export(out, mode);
        if res.is_err()
            && let Err(error) = fs::remove_file(out)
            && error.kind() != io::ErrorKind::NotFound
        {
            log_debug!("Could not delete the partial export {}: {error}", out.display());
        }
        res
    }

    fn write_export(&self, out: &Path, mode: u32) -> Result<(u64, u64)> {
        let mut file = BufWriter::with_capacity(1 << 20, File::create(out)?);
        apply_mode(out, mode);
        let conn = lock(&self.conn);
        let mut stmt = conn.prepare("SELECT mime, data FROM segments ORDER BY seq")?;
        let mut rows = stmt.query([])?;
        let (mut count, mut bytes) = (0u64, 0u64);
        while let Some(row) = rows.next()? {
            let mime: String = row.get(0)?;
            let raw: Vec<u8> = row.get(1)?;
            let data = strip_for_concat(&raw, &mime, count == 0).unwrap_or(raw);
            file.write_all(&data)?;
            count += 1;
            bytes += data.len() as u64;
        }
        file.flush()?;
        Ok((count, bytes))
    }
}

/// Stores what a later run needs to retry missing fragments, and adds the
/// URLs saved by an earlier run to the ones recovery can use. Returns how
/// many URLs are saved.
pub(super) async fn save_info_async(
    session: &Session,
    data_type: DataType,
    database: &DbSink,
    saved: Option<&SavedDownload>,
) -> Result<usize> {
    let older = run_blocking(&database.conn, |conn| urls_with(conn)).await?;
    session.remember_older(data_type, older);
    let urls = session.known_urls_oldest_first(data_type);
    let count = urls.len();
    let download_json = saved.map(serde_json::to_string).transpose()?;
    run_blocking(&database.conn, move |conn| {
        let tx = conn.transaction()?;
        save_urls_with(&tx, &urls)?;
        if let Some(json) = download_json {
            tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('download', ?1)", params![json])?;
        }
        tx.commit()?;
        Ok(())
    })
    .await?;
    Ok(count)
}

/// What `push_with_meta` has already written, so only changes are persisted.
#[derive(Default)]
pub(super) struct Saved {
    pub urls: usize,
    pub head: Option<u64>,
}

/// At the end of a download: the URLs and the newest known fragment.
pub(super) async fn save_end_async(
    session: &Session,
    data_type: DataType,
    database: &DbSink,
    head: Option<u64>,
) -> Result<()> {
    let urls = session.known_urls_oldest_first(data_type);
    run_blocking(&database.conn, move |conn| {
        let tx = conn.transaction()?;
        save_urls_with(&tx, &urls)?;
        if let Some(head_seq) = head {
            tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('head', ?1)", params![head_seq.to_string()])?;
        }
        tx.commit()?;
        Ok(())
    })
    .await
}

/// One stream of a kept download.
pub struct KeptStream {
    pub info: SavedDownload,
    /// Newest known fragment.
    pub head: Option<u64>,
    /// Download URLs used, oldest first.
    pub urls: Vec<FragmentUrl>,
}

fn wal_path(database_path: &Path) -> PathBuf {
    let mut name = database_path.as_os_str().to_os_string();
    name.push("-wal");
    PathBuf::from(name)
}

fn read_kept_from(conn: &Connection) -> Result<Option<KeptStream>> {
    let Some(raw) = meta_with(conn, "download")? else {
        return Ok(None);
    };
    let Ok(info) = serde_json::from_str::<SavedDownload>(&raw) else {
        return Ok(None);
    };
    let head = meta_with(conn, "head")?.and_then(|text| text.parse().ok());
    let urls = urls_with(conn)?;
    Ok(Some(KeptStream { info, head, urls }))
}

/// Reads the `*.f{itag}.sqlite` database in `temp_dir`, if it has the info
/// a later run needs. Opens read-only so inspection never modifies files or
/// permissions, falling back to read-write only when WAL recovery is needed
/// (which SQLite can reject at query time as well as open time).
pub fn read_kept(temp_dir: &Path, itag: u32) -> Option<KeptStream> {
    let suffix = format!(".f{itag}.sqlite");
    let path = fs::read_dir(temp_dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .find(|path| path.file_name().is_some_and(|name| name.to_string_lossy().ends_with(&suffix)))?;
    // Closed again on return, before the download reopens it (Windows
    // can't share it otherwise).
    let read_only = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(Error::from)
        .and_then(|conn| read_kept_from(&conn));
    match read_only {
        Ok(kept) => kept,
        Err(_) if wal_path(&path).exists() => {
            let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE).ok()?;
            read_kept_from(&conn).ok().flatten()
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests;
