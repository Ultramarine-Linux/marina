//! Per-backend SQLite caches for remote store catalogs.
//!
//! Each store backend gets its own SQLite file (`<dir>/<backend_id>.db`),
//! separate from the main library database: dropping or re-syncing one
//! backend's cache must never touch local library state or another backend.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rusqlite::{Connection, params};
use tracing::debug;

use crate::StoreEntry;

/// Cache for a single backend. One file per backend.
#[derive(Debug)]
pub struct StoreCache {
    backend_id: String,
    conn: Mutex<Connection>,
}

impl StoreCache {
    pub fn open(path: impl AsRef<Path>, backend_id: &str) -> Result<Self, rusqlite::Error> {
        let path = path.as_ref();
        if path != Path::new(":memory:") {
            if let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                std::fs::create_dir_all(parent)
                    .map_err(|_| rusqlite::Error::InvalidPath(path.to_owned()))?;
            }
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS store_entries(
               entry_id TEXT PRIMARY KEY,
               title TEXT NOT NULL,
               platform_slug TEXT NOT NULL,
               platform_name TEXT,
               payload_json TEXT NOT NULL DEFAULT '{}');
             CREATE INDEX IF NOT EXISTS idx_store_title ON store_entries(title);
             CREATE INDEX IF NOT EXISTS idx_store_platform ON store_entries(platform_slug);",
        )?;
        Ok(Self {
            backend_id: backend_id.to_owned(),
            conn: Mutex::new(conn),
        })
    }

    pub fn in_memory(backend_id: &str) -> Result<Self, rusqlite::Error> {
        Self::open(":memory:", backend_id)
    }

    pub fn backend_id(&self) -> &str {
        &self.backend_id
    }

    fn entry(&self, row: &rusqlite::Row<'_>) -> Result<StoreEntry, rusqlite::Error> {
        Ok(StoreEntry {
            backend_id: self.backend_id.clone(),
            entry_id: row.get(0)?,
            title: row.get(1)?,
            platform_slug: row.get(2)?,
            platform_name: row.get(3)?,
            payload_json: row.get::<_, String>(4).ok(),
        })
    }

    pub fn upsert_entries(&self, entries: &[StoreEntry]) -> Result<(), rusqlite::Error> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        {
            let mut statement = tx.prepare(
                "INSERT OR REPLACE INTO store_entries
                 (entry_id, title, platform_slug, platform_name, payload_json)
                 VALUES(?,?,?,?,?)",
            )?;
            for entry in entries {
                statement.execute(params![
                    entry.entry_id,
                    entry.title,
                    entry.platform_slug,
                    entry.platform_name,
                    entry.payload_json.as_deref().unwrap_or("{}"),
                ])?;
            }
        }
        tx.commit()?;
        debug!(
            backend_id = %self.backend_id,
            rows = entries.len(),
            "store catalog rows committed to cache"
        );
        Ok(())
    }

    pub fn browse(
        &self,
        platform_slug: Option<&str>,
        search: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<StoreEntry>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT entry_id, title, platform_slug, platform_name, payload_json
             FROM store_entries
             WHERE (?1 IS NULL OR platform_slug=?1)
               AND (?2 IS NULL OR lower(title) LIKE '%'||lower(?2)||'%')
             ORDER BY title, entry_id LIMIT ?3 OFFSET ?4",
        )?;
        statement
            .query_map(
                params![platform_slug, search, limit as i64, offset as i64],
                |row| self.entry(row),
            )?
            .collect()
    }

    pub fn get(&self, entry_id: &str) -> Result<Option<StoreEntry>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT entry_id, title, platform_slug, platform_name, payload_json
             FROM store_entries WHERE entry_id=?1",
        )?;
        let mut rows = statement.query(params![entry_id])?;
        rows.next()?.map(|row| self.entry(row)).transpose()
    }

    pub fn count(&self) -> Result<u64, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM store_entries", [], |row| {
            row.get::<_, u64>(0)
        })
    }
}

/// Lazily-opened set of per-backend cache files under one directory.
#[derive(Debug, Default)]
pub struct StoreCaches {
    dir: PathBuf,
    caches: Mutex<HashMap<String, Arc<StoreCache>>>,
}

impl StoreCaches {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            caches: Mutex::new(HashMap::new()),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn cache_path_for(dir: &Path, backend_id: &str) -> PathBuf {
        let safe: String = backend_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        dir.join(format!("{safe}.db"))
    }

    /// Returns the cache for a backend, opening its own `<backend_id>.db`
    /// file on first use.
    pub fn cache_for(&self, backend_id: &str) -> Result<Arc<StoreCache>, rusqlite::Error> {
        let mut caches = self.caches.lock().unwrap();
        if let Some(cache) = caches.get(backend_id) {
            return Ok(cache.clone());
        }
        let path = Self::cache_path_for(&self.dir, backend_id);
        let cache = Arc::new(StoreCache::open(path, backend_id)?);
        caches.insert(backend_id.to_owned(), cache.clone());
        Ok(cache)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, title: &str, platform: &str) -> StoreEntry {
        StoreEntry {
            backend_id: "romm".into(),
            entry_id: id.into(),
            title: title.into(),
            platform_slug: platform.into(),
            platform_name: None,
            payload_json: Some("{}".into()),
        }
    }

    #[test]
    fn cache_round_trips_and_filters() {
        let cache = StoreCache::in_memory("romm").unwrap();
        cache
            .upsert_entries(&[entry("1", "Zelda", "snes"), entry("2", "Metroid", "nes")])
            .unwrap();
        assert_eq!(cache.count().unwrap(), 2);
        let page = cache.browse(Some("snes"), None, 10, 0).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].title, "Zelda");
        assert!(cache.get("1").unwrap().is_some());
    }

    #[test]
    fn caches_split_files_per_backend() {
        let dir = std::env::temp_dir().join(format!("marina-store-{}", std::process::id()));
        let caches = StoreCaches::new(&dir);
        caches
            .cache_for("romm")
            .unwrap()
            .upsert_entries(&[entry("1", "Zelda", "snes")])
            .unwrap();
        caches
            .cache_for("other")
            .unwrap()
            .upsert_entries(&[entry("9", "Sonic", "genesis")])
            .unwrap();
        assert_eq!(caches.cache_for("romm").unwrap().count().unwrap(), 1);
        assert_eq!(caches.cache_for("other").unwrap().count().unwrap(), 1);
        assert!(StoreCaches::cache_path_for(&dir, "romm").is_file());
        assert!(StoreCaches::cache_path_for(&dir, "other").is_file());
        std::fs::remove_dir_all(&dir).ok();
    }
}
