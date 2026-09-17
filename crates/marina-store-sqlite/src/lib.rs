use chrono::{DateTime, FixedOffset, Utc};
use marina_core::{
    ItemKind, LibraryAsset, LibraryAssetKind, LibraryCard, LibraryItem, LibraryItemFile,
    LibraryItemId, Platform,
};
use marina_library::{
    error::LibraryError,
    query::{SearchQuery, SearchSort},
    read::{LibraryRead, PlatformCount, PlatformRead},
    write::{LibraryWrite, PlatformWrite},
};
use rusqlite::{Connection, OptionalExtension, ToSql, params, params_from_iter};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Mutex};
use tracing::debug;

#[derive(Debug)]
pub struct SqliteLibrary {
    conn: Mutex<Connection>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteCatalogRow {
    pub provider: String,
    pub rom_id: String,
    pub title: String,
    pub platform_slug: String,
}
#[derive(Debug, Serialize, Deserialize)]
struct Stored {
    id: String,
    title: String,
    kind: String,
    platform_slug: Option<String>,
    local_path: Option<String>,
    provider_ids: std::collections::HashMap<String, String>,
    summary: Option<String>,
    alternative_names: Vec<String>,
    tags: Vec<String>,
    languages: Vec<String>,
    regions: Vec<String>,
    cover: Option<String>,
    created_at: Option<String>,
    released_at: Option<String>,
    updated_at: Option<String>,
    files: Vec<LibraryItemFileDto>,
    assets: Vec<LibraryAssetDto>,
}
#[derive(Debug, Serialize, Deserialize)]
struct LibraryItemFileDto {
    provider_id: Option<String>,
    name: String,
    path: String,
    size_bytes: Option<u64>,
}
#[derive(Debug, Serialize, Deserialize)]
struct LibraryAssetDto {
    kind: LibraryAssetKind,
    source: Option<String>,
    local_path: Option<String>,
}
impl From<&LibraryItem> for Stored {
    fn from(x: &LibraryItem) -> Self {
        Self {
            id: x.id.to_string(),
            title: x.title.clone(),
            kind: match x.kind {
                ItemKind::Game => "game",
                ItemKind::App => "app",
            }
            .into(),
            platform_slug: x.platform_slug.clone(),
            local_path: x.local_path.clone(),
            provider_ids: x.provider_ids.clone(),
            summary: x.summary.clone(),
            alternative_names: x.alternative_names.clone(),
            tags: x.tags.clone(),
            languages: x.languages.clone(),
            regions: x.regions.clone(),
            cover: x.cover.clone(),
            created_at: x.created_at.map(|v| v.to_rfc3339()),
            released_at: x.released_at.map(|v| v.to_rfc3339()),
            updated_at: x.updated_at.map(|v| v.to_rfc3339()),
            files: x
                .files
                .iter()
                .map(|v| LibraryItemFileDto {
                    provider_id: v.provider_id.clone(),
                    name: v.name.clone(),
                    path: v.path.clone(),
                    size_bytes: v.size_bytes,
                })
                .collect(),
            assets: x
                .assets
                .iter()
                .map(|v| LibraryAssetDto {
                    kind: v.kind.clone(),
                    source: v.source.clone(),
                    local_path: v.local_path.clone(),
                })
                .collect(),
        }
    }
}
impl TryFrom<Stored> for LibraryItem {
    type Error = String;
    fn try_from(x: Stored) -> Result<Self, String> {
        Ok(Self {
            id: LibraryItemId::parse(&x.id).ok_or("invalid id")?,
            title: x.title,
            kind: if x.kind == "app" {
                ItemKind::App
            } else {
                ItemKind::Game
            },
            platform_slug: x.platform_slug,
            local_path: x.local_path,
            provider_ids: x.provider_ids,
            summary: x.summary,
            alternative_names: x.alternative_names,
            tags: x.tags,
            languages: x.languages,
            regions: x.regions,
            cover: x.cover,
            created_at: parse(x.created_at),
            released_at: parse(x.released_at),
            updated_at: parse(x.updated_at),
            files: x
                .files
                .into_iter()
                .map(|v| LibraryItemFile {
                    provider_id: v.provider_id,
                    name: v.name,
                    path: v.path,
                    size_bytes: v.size_bytes,
                })
                .collect(),
            assets: x
                .assets
                .into_iter()
                .map(|v| LibraryAsset {
                    kind: v.kind,
                    source: v.source,
                    local_path: v.local_path,
                })
                .collect(),
        })
    }
}
fn parse(v: Option<String>) -> Option<DateTime<FixedOffset>> {
    v.and_then(|x| DateTime::parse_from_rfc3339(&x).ok())
}
fn err<E: std::error::Error + Send + Sync + 'static>(e: E) -> LibraryError {
    LibraryError::backend(e)
}

fn card(row: &rusqlite::Row<'_>) -> Result<LibraryCard, rusqlite::Error> {
    let id: String = row.get(0)?;
    let regions_json: String = row.get(4)?;
    let regions = serde_json::from_str(&regions_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(LibraryCard {
        id: LibraryItemId::parse(&id).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::other("invalid id")),
            )
        })?,
        title: row.get(1)?,
        kind: if row.get::<_, String>(2)? == "app" {
            ItemKind::App
        } else {
            ItemKind::Game
        },
        platform_name: row.get(3)?,
        regions,
        cover: row.get(5)?,
        cover_small_local_path: row.get(6)?,
        cover_large_local_path: row.get(7)?,
    })
}

impl SqliteLibrary {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
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
        let c = Connection::open(path)?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; CREATE TABLE IF NOT EXISTS platforms(slug TEXT PRIMARY KEY,name TEXT NOT NULL); CREATE TABLE IF NOT EXISTS library_items(id TEXT PRIMARY KEY,title TEXT NOT NULL,kind TEXT NOT NULL DEFAULT 'game',platform_slug TEXT,local_path TEXT,json TEXT NOT NULL,last_updated INTEGER NOT NULL DEFAULT 0,regions_json TEXT NOT NULL DEFAULT '[]',cover TEXT,cover_small_local_path TEXT,cover_large_local_path TEXT, UNIQUE(platform_slug,local_path)); CREATE TABLE IF NOT EXISTS library_item_files(library_item_id TEXT NOT NULL,provider_id TEXT,local_path TEXT NOT NULL,name TEXT NOT NULL,size_bytes INTEGER,PRIMARY KEY(library_item_id,local_path),UNIQUE(provider_id)); CREATE TABLE IF NOT EXISTS remote_rom_cache(provider TEXT NOT NULL,rom_id TEXT NOT NULL,title TEXT NOT NULL,platform_slug TEXT NOT NULL,json TEXT NOT NULL,PRIMARY KEY(provider,rom_id)); CREATE INDEX IF NOT EXISTS idx_remote_rom_title ON remote_rom_cache(provider,title); CREATE INDEX IF NOT EXISTS idx_remote_rom_platform ON remote_rom_cache(provider,platform_slug); CREATE INDEX IF NOT EXISTS idx_items_title ON library_items(title); CREATE INDEX IF NOT EXISTS idx_items_platform ON library_items(platform_slug); CREATE INDEX IF NOT EXISTS idx_items_path ON library_items(local_path); CREATE INDEX IF NOT EXISTS idx_item_files_provider ON library_item_files(provider_id);")?;
        let has_last_updated = c
            .prepare("PRAGMA table_info(library_items)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == "last_updated");
        if !has_last_updated {
            c.execute(
                "ALTER TABLE library_items ADD COLUMN last_updated INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        c.execute(
            "UPDATE library_items SET last_updated = CAST(strftime('%s', 'now') AS INTEGER) * 1000 WHERE last_updated = 0",
            [],
        )?;
        let mut needs_card_backfill = false;
        for (name, definition) in [
            ("kind", "TEXT NOT NULL DEFAULT 'game'"),
            ("regions_json", "TEXT NOT NULL DEFAULT '[]'"),
            ("cover", "TEXT"),
            ("cover_small_local_path", "TEXT"),
            ("cover_large_local_path", "TEXT"),
        ] {
            let exists = c
                .prepare("PRAGMA table_info(library_items)")?
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?
                .iter()
                .any(|column| column == name);
            if !exists {
                c.execute(
                    &format!("ALTER TABLE library_items ADD COLUMN {name} {definition}"),
                    [],
                )?;
                needs_card_backfill = true;
            }
        }
        if needs_card_backfill {
            c.execute(
                "UPDATE library_items SET kind=COALESCE(json_extract(json, '$.kind'), 'game'),
             regions_json=COALESCE(json_extract(json, '$.regions'), '[]'),
             cover=json_extract(json, '$.cover'),
             cover_small_local_path=(SELECT json_extract(value, '$.local_path') FROM json_each(json, '$.assets') WHERE json_extract(value, '$.kind')='cover_small' LIMIT 1),
             cover_large_local_path=(SELECT json_extract(value, '$.local_path') FROM json_each(json, '$.assets') WHERE json_extract(value, '$.kind')='cover_large' LIMIT 1)
             WHERE json_extract(json, '$.regions') IS NOT NULL",
                [],
            )?;
        }
        c.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_items_last_updated ON library_items(last_updated DESC);",
        )?;
        Ok(Self {
            conn: Mutex::new(c),
        })
    }
    pub fn in_memory() -> Result<Self, rusqlite::Error> {
        Self::open(":memory:")
    }

    /// Store lightweight provider-owned catalog rows without entering the local library.
    pub fn upsert_remote_json(
        &self,
        provider: &str,
        rows: &[(String, String, String, String)],
    ) -> Result<(), rusqlite::Error> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        {
            let mut statement = tx.prepare(
                "INSERT OR REPLACE INTO remote_rom_cache(provider,rom_id,title,platform_slug,json) VALUES(?,?,?,?,?)",
            )?;
            for (rom_id, title, platform_slug, json) in rows {
                statement.execute(params![provider, rom_id, title, platform_slug, json])?;
            }
        }
        tx.commit()?;
        debug!(
            provider,
            rows = rows.len(),
            "remote catalog rows committed to SQLite"
        );
        Ok(())
    }

    /// Read a provider catalog page locally. This is deliberately not a LibraryRead method.
    pub fn remote_json_page(
        &self,
        provider: &str,
        platform_slug: Option<&str>,
        search: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<String>, rusqlite::Error> {
        let c = self.conn.lock().unwrap();
        let mut statement = c.prepare(
            "SELECT json FROM remote_rom_cache WHERE provider=?1 AND (?2 IS NULL OR platform_slug=?2) AND (?3 IS NULL OR lower(title) LIKE '%'||lower(?3)||'%') ORDER BY title, rom_id LIMIT ?4 OFFSET ?5",
        )?;
        statement
            .query_map(
                params![provider, platform_slug, search, limit as i64, offset as i64],
                |row| row.get(0),
            )?
            .collect()
    }

    pub fn remote_catalog_page(
        &self,
        provider: &str,
        platform_slug: Option<&str>,
        search: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<RemoteCatalogRow>, rusqlite::Error> {
        let c = self.conn.lock().unwrap();
        let mut statement = c.prepare(
            "SELECT provider,rom_id,title,platform_slug FROM remote_rom_cache WHERE provider=?1 AND (?2 IS NULL OR platform_slug=?2) AND (?3 IS NULL OR lower(title) LIKE '%'||lower(?3)||'%') ORDER BY title, rom_id LIMIT ?4 OFFSET ?5",
        )?;
        statement
            .query_map(
                params![provider, platform_slug, search, limit as i64, offset as i64],
                |row| {
                    Ok(RemoteCatalogRow {
                        provider: row.get(0)?,
                        rom_id: row.get(1)?,
                        title: row.get(2)?,
                        platform_slug: row.get(3)?,
                    })
                },
            )?
            .collect()
    }

    pub fn remote_json(
        &self,
        provider: &str,
        rom_id: &str,
    ) -> Result<Option<String>, rusqlite::Error> {
        let c = self.conn.lock().unwrap();
        let mut statement =
            c.prepare("SELECT json FROM remote_rom_cache WHERE provider=?1 AND rom_id=?2")?;
        let mut rows = statement.query(params![provider, rom_id])?;
        rows.next()?.map(|row| row.get(0)).transpose()
    }

    pub fn remote_json_count(&self, provider: &str) -> Result<u64, rusqlite::Error> {
        let c = self.conn.lock().unwrap();
        c.query_row(
            "SELECT COUNT(*) FROM remote_rom_cache WHERE provider=?",
            params![provider],
            |row| row.get::<_, u64>(0),
        )
    }
    fn item(row: &rusqlite::Row) -> Result<LibraryItem, rusqlite::Error> {
        let s: String = row.get(0)?;
        serde_json::from_str::<Stored>(&s)
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })
            .and_then(|x| {
                x.try_into().map_err(|e: String| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::other(e)),
                    )
                })
            })
    }
    fn save(&self, x: LibraryItem) -> Result<LibraryItem, LibraryError> {
        let item_id = x.id.to_string();
        let j = serde_json::to_string(&Stored::from(&x)).map_err(err)?;
        let regions_json = serde_json::to_string(&x.regions).map_err(err)?;
        let (cover_small, cover_large) = (
            x.assets
                .iter()
                .find(|asset| matches!(asset.kind, LibraryAssetKind::CoverSmall))
                .and_then(|asset| asset.local_path.as_deref()),
            x.assets
                .iter()
                .find(|asset| matches!(asset.kind, LibraryAssetKind::CoverLarge))
                .and_then(|asset| asset.local_path.as_deref()),
        );
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction().map_err(err)?;
        // `last_updated` is the item's added-to-library timestamp. Preserve it on replacement;
        // the scalar subquery avoids a separate read round-trip.
        tx.execute(
            "INSERT INTO library_items(id,title,kind,platform_slug,local_path,json,last_updated,regions_json,cover,cover_small_local_path,cover_large_local_path)
             VALUES(?,?,?,?,?,?,COALESCE((SELECT last_updated FROM library_items WHERE id=?),?),?,?,?,?)
             ON CONFLICT(id) DO UPDATE SET title=excluded.title, kind=excluded.kind, platform_slug=excluded.platform_slug,
             local_path=excluded.local_path, json=excluded.json, regions_json=excluded.regions_json,
             cover=excluded.cover, cover_small_local_path=excluded.cover_small_local_path,
             cover_large_local_path=excluded.cover_large_local_path",
            params![
                item_id,
                x.title,
                match x.kind {
                    ItemKind::Game => "game",
                    ItemKind::App => "app",
                },
                x.platform_slug,
                x.local_path,
                j,
                item_id,
                Utc::now().timestamp_millis(),
                regions_json,
                x.cover,
                cover_small,
                cover_large,
            ],
        )
        .map_err(err)?;

        if x.files.is_empty() {
            tx.execute(
                "DELETE FROM library_item_files WHERE library_item_id=?",
                params![item_id],
            )
            .map_err(err)?;
        } else {
            let mut paths: Vec<&dyn ToSql> = vec![&item_id];
            paths.extend(x.files.iter().map(|file| &file.path as &dyn ToSql));
            let placeholders = std::iter::repeat_n("?", x.files.len())
                .collect::<Vec<_>>()
                .join(",");
            tx.execute(
                &format!("DELETE FROM library_item_files WHERE library_item_id=? AND local_path NOT IN ({placeholders})"),
                params_from_iter(paths),
            )
            .map_err(err)?;
            for file in &x.files {
                tx.execute(
                    "INSERT INTO library_item_files(library_item_id,provider_id,local_path,name,size_bytes) VALUES(?,?,?,?,?)
                     ON CONFLICT(library_item_id,local_path) DO UPDATE SET provider_id=excluded.provider_id,
                     name=excluded.name,size_bytes=excluded.size_bytes
                     WHERE provider_id IS NOT excluded.provider_id OR name IS NOT excluded.name OR size_bytes IS NOT excluded.size_bytes",
                    params![item_id, file.provider_id, file.path, file.name, file.size_bytes.map(|size| size as i64)],
                )
                .map_err(err)?;
            }
        }
        tx.commit().map_err(err)?;
        Ok(x)
    }
}
impl PlatformRead for SqliteLibrary {
    async fn platforms(&self) -> Result<Vec<Platform>, LibraryError> {
        let c = self.conn.lock().unwrap();
        let mut s = c
            .prepare("SELECT slug,name FROM platforms ORDER BY name")
            .map_err(err)?;
        s.query_map([], |r| {
            Ok(Platform::new(
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
            ))
        })
        .map_err(err)?
        .collect::<Result<_, _>>()
        .map_err(err)
    }
}
impl LibraryRead for SqliteLibrary {
    async fn search(&self, q: SearchQuery) -> Result<Vec<LibraryItem>, LibraryError> {
        let c = self.conn.lock().unwrap();
        let order_by = match q.sort {
            SearchSort::Title => "title",
            SearchSort::LastUpdated => "last_updated DESC, title",
        };
        let sql = format!(
            "SELECT json FROM library_items WHERE (?1 IS NULL OR lower(title) LIKE '%'||lower(?1)||'%' OR lower(json) LIKE '%'||lower(?1)||'%') AND (?2 IS NULL OR platform_slug=?2) ORDER BY {order_by} LIMIT ?3 OFFSET ?4"
        );
        let mut s = c.prepare(&sql).map_err(err)?;
        let lim = q.limit.map(|x| x as i64).unwrap_or(-1);
        s.query_map(
            params![q.text, q.platform, lim, q.offset as i64],
            Self::item,
        )
        .map_err(err)?
        .collect::<Result<_, _>>()
        .map_err(err)
    }
    async fn count(&self, q: SearchQuery) -> Result<usize, LibraryError> {
        let c = self.conn.lock().unwrap();
        let count = c
            .query_row(
                "SELECT COUNT(*) FROM library_items WHERE (?1 IS NULL OR lower(title) LIKE '%'||lower(?1)||'%' OR lower(json) LIKE '%'||lower(?1)||'%') AND (?2 IS NULL OR platform_slug=?2)",
                params![q.text, q.platform],
                |row| row.get::<_, i64>(0),
            )
            .map_err(err)?;
        Ok(count as usize)
    }

    async fn get(&self, id: &LibraryItemId) -> Result<Option<LibraryItem>, LibraryError> {
        let c = self.conn.lock().unwrap();
        let mut s = c
            .prepare("SELECT json FROM library_items WHERE id=?")
            .map_err(err)?;
        let mut r = s.query(params![id.to_string()]).map_err(err)?;
        r.next()
            .map_err(err)?
            .map(Self::item)
            .transpose()
            .map_err(err)
    }
    async fn find_by_local_paths(
        &self,
        paths: &[String],
    ) -> Result<Vec<LibraryItem>, LibraryError> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let c = self.conn.lock().unwrap();
        let mut items = Vec::new();
        for batch in paths.chunks(900) {
            let placeholders = std::iter::repeat_n("?", batch.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql =
                format!("SELECT json FROM library_items WHERE local_path IN ({placeholders})");
            let mut statement = c.prepare(&sql).map_err(err)?;
            let batch_items = statement
                .query_map(rusqlite::params_from_iter(batch), Self::item)
                .map_err(err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(err)?;
            items.extend(batch_items);
        }
        Ok(items)
    }
    async fn list(&self, l: u32) -> Result<Vec<LibraryItem>, LibraryError> {
        self.search(SearchQuery::new().limit(l as usize)).await
    }
    async fn list_cards(&self, l: u32) -> Result<Vec<LibraryCard>, LibraryError> {
        self.search_cards(SearchQuery::new().limit(l as usize))
            .await
    }
    async fn search_cards(&self, q: SearchQuery) -> Result<Vec<LibraryCard>, LibraryError> {
        let c = self.conn.lock().unwrap();
        let order_by = match q.sort {
            SearchSort::Title => "title",
            SearchSort::LastUpdated => "last_updated DESC, title",
        };
        let sql = format!(
            "SELECT id,title,kind,platform_slug,regions_json,cover,cover_small_local_path,cover_large_local_path
             FROM library_items
             WHERE (?1 IS NULL OR lower(title) LIKE '%'||lower(?1)||'%')
             AND (?2 IS NULL OR platform_slug=?2)
             ORDER BY {order_by} LIMIT ?3 OFFSET ?4"
        );
        let mut statement = c.prepare(&sql).map_err(err)?;
        let limit = q.limit.map(|limit| limit as i64).unwrap_or(-1);
        statement
            .query_map(params![q.text, q.platform, limit, q.offset as i64], card)
            .map_err(err)?
            .collect::<Result<_, _>>()
            .map_err(err)
    }

    async fn get_by_local_path(
        &self,
        local_path: &str,
    ) -> Result<Option<LibraryItem>, LibraryError> {
        let c = self.conn.lock().unwrap();
        c.query_row(
            "SELECT json FROM library_items WHERE local_path=?",
            params![local_path],
            Self::item,
        )
        .optional()
        .map_err(err)
    }

    async fn platform_counts(&self) -> Result<Vec<PlatformCount>, LibraryError> {
        let c = self.conn.lock().unwrap();
        let mut statement = c
            .prepare(
                "SELECT library_items.platform_slug, COALESCE(platforms.name, library_items.platform_slug), COUNT(*)
                 FROM library_items
                 LEFT JOIN platforms ON platforms.slug=library_items.platform_slug
                 WHERE library_items.platform_slug IS NOT NULL
                 GROUP BY library_items.platform_slug, platforms.name
                 ORDER BY COALESCE(platforms.name, library_items.platform_slug)",
            )
            .map_err(err)?;
        statement
            .query_map([], |row| {
                Ok(PlatformCount {
                    platform: Platform::new(row.get::<_, String>(0)?, row.get::<_, String>(1)?),
                    count: row.get::<_, i64>(2)? as usize,
                })
            })
            .map_err(err)?
            .collect::<Result<_, _>>()
            .map_err(err)
    }
}
impl LibraryWrite for SqliteLibrary {
    async fn add(&self, x: LibraryItem) -> Result<LibraryItem, LibraryError> {
        self.save(x)
    }
    async fn update(&self, x: LibraryItem) -> Result<LibraryItem, LibraryError> {
        self.save(x)
    }
    async fn remove(&self, id: &LibraryItemId) -> Result<(), LibraryError> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction().map_err(err)?;
        tx.execute(
            "DELETE FROM library_item_files WHERE library_item_id=?",
            params![id.to_string()],
        )
        .map_err(err)?;
        tx.execute(
            "DELETE FROM library_items WHERE id=?",
            params![id.to_string()],
        )
        .map_err(err)?;
        tx.commit().map_err(err)
    }
}
impl PlatformWrite for SqliteLibrary {
    async fn add_platform(&self, p: Platform) -> Result<Platform, LibraryError> {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO platforms VALUES(?,?)",
                params![p.slug, p.name],
            )
            .map_err(err)?;
        Ok(p)
    }
    async fn update_platform(&self, p: Platform) -> Result<Platform, LibraryError> {
        self.add_platform(p).await
    }
    async fn remove_platform(&self, s: &str) -> Result<(), LibraryError> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM platforms WHERE slug=?", params![s])
            .map(|_| ())
            .map_err(err)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use marina_library::{read::LibraryRead, write::LibraryWrite};

    #[test]
    fn open_creates_missing_parent_directories() {
        let root = std::env::temp_dir().join(format!(
            "marina-sqlite-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let database_path = root.join("nested/data/library.sqlite");

        let database = SqliteLibrary::open(&database_path).unwrap();

        assert!(database_path.is_file());
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn save_replaces_item_and_files_atomically() {
        let db = SqliteLibrary::in_memory().unwrap();

        let mut x = LibraryItem::new_game("Zelda");
        x.platform_slug = Some("snes".into());
        db.add(x.clone()).await.unwrap();
        let last_updated = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT last_updated FROM library_items WHERE id=?",
                params![x.id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert!(last_updated > 0);
        let mut refreshed = x.clone();
        refreshed.title = "The Legend of Zelda".into();
        db.update(refreshed).await.unwrap();
        let preserved_last_updated = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT last_updated FROM library_items WHERE id=?",
                params![x.id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(preserved_last_updated, last_updated);
        assert_eq!(
            db.get(&x.id).await.unwrap().unwrap().title,
            "The Legend of Zelda"
        );
        assert_eq!(
            db.search(SearchQuery::new().text("zel"))
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            db.count(SearchQuery::new().platform("snes")).await.unwrap(),
            1
        );
        let mut y = LibraryItem::new_game("Metroid");
        y.platform_slug = Some("nes".into());
        db.add(y.clone()).await.unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "UPDATE library_items SET last_updated=1 WHERE id=?",
                params![x.id.to_string()],
            )
            .unwrap();
            conn.execute(
                "UPDATE library_items SET last_updated=2 WHERE id=?",
                params![y.id.to_string()],
            )
            .unwrap();
        }
        let recently_added = db
            .search(SearchQuery::new().sort(SearchSort::LastUpdated))
            .await
            .unwrap();
        assert_eq!(recently_added[0].title, "Metroid");
        db.remove(&x.id).await.unwrap();
        assert!(db.get(&x.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn cards_use_card_columns_and_indexed_path_lookup() {
        let db = SqliteLibrary::in_memory().unwrap();
        let mut item = LibraryItem::new_game("Card title");
        item.platform_slug = Some("snes".into());
        item.local_path = Some("/games/card.rom".into());
        item.summary = Some("secret searchable summary".into());
        item.regions = vec!["US".into()];
        item.cover = Some("cover.jpg".into());
        item.assets.push(LibraryAsset {
            kind: LibraryAssetKind::CoverSmall,
            source: None,
            local_path: Some("small.jpg".into()),
        });
        db.add(item.clone()).await.unwrap();

        assert_eq!(
            db.search_cards(SearchQuery::new().text("secret"))
                .await
                .unwrap()
                .len(),
            0
        );
        let card = db
            .search_cards(SearchQuery::new().text("card"))
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(card.regions, vec!["US"]);
        assert_eq!(card.cover.as_deref(), Some("cover.jpg"));
        assert_eq!(card.cover_small_local_path.as_deref(), Some("small.jpg"));
        assert_eq!(
            db.get_by_local_path("/games/card.rom")
                .await
                .unwrap()
                .unwrap()
                .id,
            item.id
        );

        let counts = db.platform_counts().await.unwrap();
        assert_eq!(counts[0].platform.slug, "snes");
        assert_eq!(counts[0].count, 1);
    }
}
