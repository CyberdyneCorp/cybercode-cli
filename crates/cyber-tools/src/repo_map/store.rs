//! The local index cache `<cache>/repo-map/index.db` (`code-intelligence` → Repository
//! map index storage): tags by content hash and extractor version, one row per checkout
//! and one per walked file. It is a rebuildable cache: a schema mismatch drops it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use super::extract::Tags;
use super::lang::hex;

pub const SCHEMA_VERSION: i64 = 1;
/// Checkouts unused this long are dropped.
pub const UNUSED_CHECKOUT: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// Above this size, least recently used checkouts are evicted.
pub const SIZE_CAP: u64 = 512 * 1024 * 1024;
/// Marker of a file listed by path only because its parse timed out.
pub const PARSE_TIMEOUT: &str = "parse_timeout";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS blobs (
    content_sha256 TEXT NOT NULL,
    extractor_version TEXT NOT NULL,
    lang TEXT NOT NULL,
    tags BLOB NOT NULL,
    PRIMARY KEY (content_sha256, extractor_version)
);
CREATE TABLE IF NOT EXISTS checkouts (
    root_id TEXT PRIMARY KEY,
    canonical_root TEXT NOT NULL,
    last_used INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'empty',
    refreshed_at INTEGER,
    build_ms INTEGER,
    generation INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS files (
    root_id TEXT NOT NULL,
    path TEXT NOT NULL,
    size INTEGER NOT NULL,
    mtime_ns INTEGER NOT NULL,
    content_sha256 TEXT,
    lang TEXT,
    excluded_reason TEXT,
    PRIMARY KEY (root_id, path)
);
CREATE INDEX IF NOT EXISTS files_by_blob ON files (content_sha256);
";

/// One walked file as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    pub path: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub sha: Option<String>,
    pub lang: Option<String>,
    pub excluded: Option<String>,
}

/// A checkout's index state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckoutRow {
    pub root_id: String,
    pub status: String,
    pub refreshed_at: Option<i64>,
    pub build_ms: Option<i64>,
    pub generation: i64,
}

/// An indexed file and its tags (`None` for a path-only file).
#[derive(Debug, Clone)]
pub struct IndexedFile {
    pub path: String,
    pub lang: String,
    pub tags: Option<Arc<Tags>>,
    pub parse_timeout: bool,
}

pub struct Store {
    conn: Connection,
    path: PathBuf,
}

pub fn index_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("repo-map").join("index.db")
}

pub fn root_id(canonical_root: &Path) -> String {
    let digest = Sha256::digest(canonical_root.to_string_lossy().as_bytes());
    hex(&digest[..8])
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Store {
    /// Open (or create) the cache; a schema version mismatch drops and recreates it.
    pub fn open(cache_dir: &Path) -> Result<Self, String> {
        let path = index_path(cache_dir);
        let dir = path.parent().ok_or("invalid cache directory")?;
        create_private_dir(dir)?;
        match Self::open_at(&path)? {
            Some(store) => Ok(store),
            None => {
                remove_database(&path);
                Self::open_at(&path)?.ok_or_else(|| "repository index schema mismatch".into())
            }
        }
    }

    /// `None` when the stored schema version differs.
    fn open_at(path: &Path) -> Result<Option<Self>, String> {
        create_private_file(path)?;
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        // Only a new database is set up, so readers never write; concurrent creators
        // retry while one of them holds the lock.
        for attempt in 0.. {
            let version: i64 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .map_err(|e| e.to_string())?;
            if version == SCHEMA_VERSION {
                break;
            }
            if version != 0 {
                return Ok(None);
            }
            match setup(&conn) {
                Ok(()) => break,
                Err(_) if attempt < 50 => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => return Err(error),
            }
        }
        Ok(Some(Self {
            conn,
            path: path.to_path_buf(),
        }))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The checkout row of `root`, created when missing; marks it used now.
    pub fn checkout(&self, root: &Path) -> Result<CheckoutRow, String> {
        let id = root_id(root);
        self.conn
            .execute(
                "INSERT INTO checkouts (root_id, canonical_root, last_used) VALUES (?1, ?2, ?3)
                 ON CONFLICT(root_id) DO UPDATE SET last_used = excluded.last_used",
                params![id, root.to_string_lossy(), now_ms()],
            )
            .map_err(|e| e.to_string())?;
        self.checkout_row(&id)
            .and_then(|row| row.ok_or_else(|| "checkout row missing".into()))
    }

    pub fn checkout_row(&self, id: &str) -> Result<Option<CheckoutRow>, String> {
        self.conn
            .query_row(
                "SELECT status, refreshed_at, build_ms, generation FROM checkouts WHERE root_id = ?1",
                [id],
                |row| {
                    Ok(CheckoutRow {
                        root_id: id.to_owned(),
                        status: row.get(0)?,
                        refreshed_at: row.get(1)?,
                        build_ms: row.get(2)?,
                        generation: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(|e| e.to_string())
    }

    pub fn file_rows(&self, id: &str) -> Result<HashMap<String, FileRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT path, size, mtime_ns, content_sha256, lang, excluded_reason
                 FROM files WHERE root_id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([id], |row| {
                Ok(FileRow {
                    path: row.get(0)?,
                    size: row.get::<_, i64>(1)? as u64,
                    mtime_ns: row.get(2)?,
                    sha: row.get(3)?,
                    lang: row.get(4)?,
                    excluded: row.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.map(|row| row.map(|r| (r.path.clone(), r)))
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())
    }

    pub fn has_blob(&self, sha: &str, version: &str) -> Result<bool, String> {
        self.conn
            .query_row(
                "SELECT 1 FROM blobs WHERE content_sha256 = ?1 AND extractor_version = ?2",
                params![sha, version],
                |_| Ok(()),
            )
            .optional()
            .map(|found| found.is_some())
            .map_err(|e| e.to_string())
    }

    /// Commit one batch of file rows, new blobs and removed paths in one transaction, and
    /// bump the checkout's generation.
    pub fn commit(
        &mut self,
        id: &str,
        rows: &[FileRow],
        blobs: &[(String, String, String, Vec<u8>)],
        removed: &[String],
    ) -> Result<(), String> {
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        {
            let mut blob = tx
                .prepare_cached(
                    "INSERT OR REPLACE INTO blobs (content_sha256, extractor_version, lang, tags)
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(|e| e.to_string())?;
            for (sha, version, lang, tags) in blobs {
                blob.execute(params![sha, version, lang, tags])
                    .map_err(|e| e.to_string())?;
            }
            let mut file = tx
                .prepare_cached(
                    "INSERT OR REPLACE INTO files
                     (root_id, path, size, mtime_ns, content_sha256, lang, excluded_reason)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )
                .map_err(|e| e.to_string())?;
            for row in rows {
                file.execute(params![
                    id,
                    row.path,
                    row.size as i64,
                    row.mtime_ns,
                    row.sha,
                    row.lang,
                    row.excluded
                ])
                .map_err(|e| e.to_string())?;
            }
            let mut delete = tx
                .prepare_cached("DELETE FROM files WHERE root_id = ?1 AND path = ?2")
                .map_err(|e| e.to_string())?;
            for path in removed {
                delete
                    .execute(params![id, path])
                    .map_err(|e| e.to_string())?;
            }
            tx.execute(
                "UPDATE checkouts SET generation = generation + 1 WHERE root_id = ?1",
                [id],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn finish(&self, id: &str, status: &str, build_ms: i64) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE checkouts SET status = ?2, refreshed_at = ?3, build_ms = ?4,
                 generation = generation + 1 WHERE root_id = ?1",
                params![id, status, now_ms(), build_ms],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Indexed files of a checkout in path order, with their tags.
    pub fn indexed(&self, id: &str) -> Result<Vec<IndexedFile>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT f.path, f.lang, f.excluded_reason, b.tags FROM files f
                 LEFT JOIN blobs b ON b.content_sha256 = f.content_sha256
                   AND b.lang = f.lang
                 WHERE f.root_id = ?1
                   AND (f.excluded_reason IS NULL OR f.excluded_reason = ?2)
                 ORDER BY f.path",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![id, PARSE_TIMEOUT], |row| {
                let tags: Option<Vec<u8>> = row.get(3)?;
                let excluded: Option<String> = row.get(2)?;
                Ok(IndexedFile {
                    path: row.get(0)?,
                    lang: row
                        .get::<_, Option<String>>(1)?
                        .unwrap_or_else(|| super::lang::OTHER.into()),
                    tags: tags
                        .and_then(|bytes| serde_json::from_slice::<Tags>(&bytes).ok())
                        .map(Arc::new),
                    parse_timeout: excluded.is_some(),
                })
            })
            .map_err(|e| e.to_string())?;
        let mut files: Vec<IndexedFile> =
            rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
        // One blob per (hash, version): keep the first joined row per path.
        files.dedup_by(|a, b| a.path == b.path);
        Ok(files)
    }

    /// Excluded file counts by reason.
    pub fn excluded(&self, id: &str) -> Result<Vec<(String, u64)>, String> {
        self.counts(
            "SELECT excluded_reason, COUNT(*) FROM files WHERE root_id = ?1
             AND excluded_reason IS NOT NULL GROUP BY excluded_reason ORDER BY excluded_reason",
            id,
        )
    }

    fn counts(&self, sql: &str, id: &str) -> Result<Vec<(String, u64)>, String> {
        let mut stmt = self.conn.prepare(sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([id], |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as u64)))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    /// Delete a checkout's rows.
    pub fn clear(&self, id: &str) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM files WHERE root_id = ?1", [id])
            .and_then(|_| {
                self.conn
                    .execute("DELETE FROM checkouts WHERE root_id = ?1", [id])
            })
            .map(|_| ())
            .map_err(|e| e.to_string())?;
        self.drop_unreferenced_blobs()
    }

    /// Re-extract every file of a checkout: forget its stored stats.
    pub fn forget_stats(&self, id: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE files SET size = -1, mtime_ns = -1 WHERE root_id = ?1",
                [id],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Unreferenced blobs, checkouts unused for 30 days, then least recently used
    /// checkouts while the database is above `cap` bytes.
    pub fn gc(&self, cap: u64, keep: &str) -> Result<(), String> {
        let cutoff = now_ms() - UNUSED_CHECKOUT.as_millis() as i64;
        let stale: Vec<String> = self.ids(
            "SELECT root_id FROM checkouts WHERE last_used < ?1 ORDER BY root_id",
            cutoff,
        )?;
        for id in stale.iter().filter(|id| *id != keep) {
            self.clear(id)?;
        }
        self.drop_unreferenced_blobs()?;
        while self.used_bytes()? > cap {
            let lru = self.ids(
                "SELECT root_id FROM checkouts WHERE last_used >= ?1
                 ORDER BY last_used, root_id",
                i64::MIN,
            )?;
            let Some(victim) = lru.into_iter().find(|id| id != keep) else {
                break;
            };
            self.clear(&victim)?;
        }
        Ok(())
    }

    fn ids(&self, sql: &str, bound: i64) -> Result<Vec<String>, String> {
        let mut stmt = self.conn.prepare(sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([bound], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    fn drop_unreferenced_blobs(&self) -> Result<(), String> {
        let versions: Vec<String> = super::lang::ALL
            .iter()
            .map(|lang| format!("'{}'", lang.extractor_version()))
            .collect();
        self.conn
            .execute(
                &format!(
                    "DELETE FROM blobs WHERE extractor_version NOT IN ({})
                     OR NOT EXISTS (SELECT 1 FROM files
                       WHERE files.content_sha256 = blobs.content_sha256)",
                    versions.join(", ")
                ),
                [],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Bytes in use: allocated pages minus free ones.
    pub fn used_bytes(&self) -> Result<u64, String> {
        let pragma = |name: &str| -> Result<i64, String> {
            self.conn
                .query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))
                .map_err(|e| e.to_string())
        };
        let pages = pragma("page_count")? - pragma("freelist_count")?;
        Ok((pages.max(0) * pragma("page_size")?) as u64)
    }

    /// Mark a checkout as last used at `at` (milliseconds), for tests of eviction.
    #[cfg(test)]
    pub fn touch(&self, id: &str, at: i64) {
        self.conn
            .execute(
                "UPDATE checkouts SET last_used = ?2 WHERE root_id = ?1",
                params![id, at],
            )
            .unwrap();
    }
}

fn setup(conn: &Connection) -> Result<(), String> {
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| e.to_string())?;
    conn.execute_batch(&format!(
        "BEGIN IMMEDIATE; {SCHEMA} PRAGMA user_version = {SCHEMA_VERSION}; COMMIT;"
    ))
    .map_err(|e| e.to_string())
}

fn remove_database(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(name));
    }
}

fn create_private_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// Create the database file owner-only before SQLite opens it.
fn create_private_file(path: &Path) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map(|_| ())
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
