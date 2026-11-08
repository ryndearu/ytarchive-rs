//! `--sqlite` storage (from livestream_dl): every fragment goes into a SQLite
//! table keyed by sequence as soon as it arrives, so nothing waits in memory
//! behind a slow or missing fragment. Resuming skips fragments already
//! stored. At the end the table is exported in order to the `.ts` file.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use super::sink::Event;
use super::ts_sink::fragment_bytes;
use crate::error::Result;
use crate::mp4_atoms::strip_for_concat;
use crate::types::{Fragment, FragmentUrl};
use crate::util::set_mode;

pub struct DbSink {
    conn: Connection,
    path: PathBuf,
}

/// `x.f140.ts` -> `x.f140.sqlite`
pub fn db_path(data_file: &Path) -> PathBuf {
    data_file.with_extension("sqlite")
}

impl DbSink {
    // rusqlite is synchronous; inserts are small and fast, so they run inline
    // on the writer task rather than through a blocking pool.
    pub fn open(path: &Path, mode: u32) -> Result<Self> {
        let conn = Connection::open(path)?;
        let _ = set_mode(path, mode);
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
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
        Ok(Self { conn, path: path.to_path_buf() })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Sequence numbers already stored.
    pub fn stored(&self) -> Result<HashSet<u64>> {
        let mut stmt = self.conn.prepare("SELECT seq FROM segments")?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        Ok(rows.filter_map(|r| r.ok()).map(|s| s as u64).collect())
    }

    /// Adds download URLs (already stored ones are ignored).
    pub fn save_urls(&self, urls: &[FragmentUrl]) -> Result<()> {
        let mut stmt = self.conn.prepare_cached("INSERT OR IGNORE INTO urls (prefix, suffix) VALUES (?1, ?2)")?;
        for u in urls {
            let (prefix, suffix) = u.parts();
            stmt.execute(params![prefix, suffix])?;
        }
        Ok(())
    }

    /// Stored download URLs, oldest first.
    pub fn urls(&self) -> Result<Vec<FragmentUrl>> {
        let mut stmt = self.conn.prepare("SELECT prefix, suffix FROM urls ORDER BY rowid")?;
        let rows = stmt.query_map([], |r| Ok(FragmentUrl::from_parts(r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn
            .execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", params![key, value])?;
        Ok(())
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare("SELECT value FROM meta WHERE key = ?1")?;
        let mut rows = stmt.query(params![key])?;
        Ok(match rows.next()? {
            Some(r) => Some(r.get(0)?),
            None => None,
        })
    }

    /// (fragment count, total bytes) stored.
    pub fn totals(&self) -> Result<(u64, u64)> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(LENGTH(data)), 0) FROM segments",
            [],
            |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)),
        )?)
    }

    pub async fn push(&mut self, seq: u64, frag: Option<Fragment>) -> Result<Vec<Event>> {
        let Some(frag) = frag else {
            return Ok(vec![Event::Gap(seq)]);
        };
        let data = fragment_bytes(&frag.data).await;
        super::ts_sink::remove_frag_file(&frag.data).await;
        let data = data?;
        self.conn.execute(
            "INSERT OR REPLACE INTO segments (seq, mime, data) VALUES (?1, ?2, ?3)",
            params![seq as i64, frag.mime_type, data],
        )?;
        Ok(vec![Event::Wrote(data.len() as u64)])
    }

    /// Writes all stored fragments in order to `out` (replacing it). The
    /// first fragment keeps its `ftyp` box. Returns (fragments, bytes).
    pub fn export(&self, out: &Path, mode: u32) -> Result<(u64, u64)> {
        let mut file = std::io::BufWriter::new(std::fs::File::create(out)?);
        let _ = set_mode(out, mode);
        let mut stmt = self.conn.prepare("SELECT mime, data FROM segments ORDER BY seq")?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FragData;

    fn frag(seq: u64, body: &[u8]) -> Fragment {
        Fragment {
            seq,
            x_head_seqnum: None,
            data: FragData::Memory(bytes::Bytes::copy_from_slice(body)),
            mime_type: "video/webm".into(),
        }
    }

    #[tokio::test]
    async fn stores_out_of_order_and_exports_in_order() {
        let dir = std::env::temp_dir().join(format!("ytarchive-rs-db-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = db_path(&dir.join("x.f140.ts"));
        assert!(db.ends_with("x.f140.sqlite"));

        let mut sink = DbSink::open(&db, 0o644).unwrap();
        for seq in [3u64, 1, 2] {
            let ev = sink.push(seq, Some(frag(seq, format!("<{seq}>").as_bytes()))).await.unwrap();
            assert!(matches!(ev[..], [Event::Wrote(3)]));
        }
        assert!(matches!(sink.push(4, None).await.unwrap()[..], [Event::Gap(4)]));
        drop(sink);

        // Reopen: data persisted, duplicates replaced.
        let mut sink = DbSink::open(&db, 0o644).unwrap();
        assert_eq!(sink.stored().unwrap(), HashSet::from([1, 2, 3]));
        sink.push(2, Some(frag(2, b"<2>"))).await.unwrap();
        assert_eq!(sink.totals().unwrap(), (3, 9));

        let out = dir.join("x.f140.ts");
        assert_eq!(sink.export(&out, 0o644).unwrap(), (3, 9));
        assert_eq!(std::fs::read(&out).unwrap(), b"<1><2><3>");

        let a = FragmentUrl::with_sq_param("https://a/videoplayback?itag=140");
        let b = FragmentUrl::with_sq_param("https://b/videoplayback?itag=140");
        sink.save_urls(&[a.clone(), b.clone()]).unwrap();
        sink.save_urls(std::slice::from_ref(&a)).unwrap();
        assert_eq!(sink.urls().unwrap(), [a, b]);
        assert_eq!(sink.meta("head").unwrap(), None);
        sink.set_meta("head", "9").unwrap();
        sink.set_meta("head", "10").unwrap();
        assert_eq!(sink.meta("head").unwrap().as_deref(), Some("10"));
        // Windows can't delete the database while the connection is open.
        drop(sink);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
