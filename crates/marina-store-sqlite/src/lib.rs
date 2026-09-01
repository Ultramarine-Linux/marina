use chrono::{DateTime, FixedOffset};
use marina_core::{
    ItemKind, LibraryAsset, LibraryCard, LibraryItem, LibraryItemFile, LibraryItemId, Platform,
};
use marina_library::{
    error::LibraryError,
    query::SearchQuery,
    read::{LibraryRead, PlatformRead},
    write::{LibraryWrite, PlatformWrite},
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
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
    url: Option<String>,
    path: Option<String>,
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
                    url: v.url.clone(),
                    path: v.path.clone(),
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
                    url: v.url,
                    path: v.path,
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
impl SqliteLibrary {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        let c = Connection::open(path)?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; CREATE TABLE IF NOT EXISTS platforms(slug TEXT PRIMARY KEY,name TEXT NOT NULL); CREATE TABLE IF NOT EXISTS library_items(id TEXT PRIMARY KEY,title TEXT NOT NULL,platform_slug TEXT,local_path TEXT,json TEXT NOT NULL, UNIQUE(platform_slug,local_path)); CREATE TABLE IF NOT EXISTS library_item_files(library_item_id TEXT NOT NULL,provider_id TEXT,local_path TEXT NOT NULL,name TEXT NOT NULL,size_bytes INTEGER,PRIMARY KEY(library_item_id,local_path),UNIQUE(provider_id)); CREATE TABLE IF NOT EXISTS remote_rom_cache(provider TEXT NOT NULL,rom_id TEXT NOT NULL,title TEXT NOT NULL,platform_slug TEXT NOT NULL,json TEXT NOT NULL,PRIMARY KEY(provider,rom_id)); CREATE INDEX IF NOT EXISTS idx_remote_rom_title ON remote_rom_cache(provider,title); CREATE INDEX IF NOT EXISTS idx_remote_rom_platform ON remote_rom_cache(provider,platform_slug); CREATE INDEX IF NOT EXISTS idx_items_title ON library_items(title); CREATE INDEX IF NOT EXISTS idx_items_platform ON library_items(platform_slug); CREATE INDEX IF NOT EXISTS idx_items_path ON library_items(local_path); CREATE INDEX IF NOT EXISTS idx_item_files_provider ON library_item_files(provider_id);")?;
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
        let j = serde_json::to_string(&Stored::from(&x)).map_err(err)?;
        let c = self.conn.lock().unwrap();
        c.execute("INSERT OR REPLACE INTO library_items(id,title,platform_slug,local_path,json) VALUES(?,?,?,?,?)",params![x.id.to_string(),x.title,x.platform_slug,x.local_path,j]).map_err(err)?;
        c.execute(
            "DELETE FROM library_item_files WHERE library_item_id=?",
            params![x.id.to_string()],
        )
        .map_err(err)?;
        for file in &x.files {
            c.execute("INSERT OR REPLACE INTO library_item_files(library_item_id,provider_id,local_path,name,size_bytes) VALUES(?,?,?,?,?)", params![x.id.to_string(), file.provider_id, file.path, file.name, file.size_bytes.map(|size| size as i64)]).map_err(err)?;
        }
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
        let mut s=c.prepare("SELECT json FROM library_items WHERE (?1 IS NULL OR lower(title) LIKE '%'||lower(?1)||'%' OR lower(json) LIKE '%'||lower(?1)||'%') AND (?2 IS NULL OR platform_slug=?2) ORDER BY title LIMIT ?3 OFFSET ?4").map_err(err)?;
        let lim = q.limit.map(|x| x as i64).unwrap_or(-1);
        s.query_map(
            params![q.text, q.platform, lim, q.offset as i64],
            Self::item,
        )
        .map_err(err)?
        .collect::<Result<_, _>>()
        .map_err(err)
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
    async fn list(&self, l: u32) -> Result<Vec<LibraryItem>, LibraryError> {
        self.search(SearchQuery::new().limit(l as usize)).await
    }
    async fn list_cards(&self, l: u32) -> Result<Vec<LibraryCard>, LibraryError> {
        self.search_cards(SearchQuery::new().limit(l as usize))
            .await
    }
    async fn search_cards(&self, q: SearchQuery) -> Result<Vec<LibraryCard>, LibraryError> {
        self.search(q)
            .await?
            .into_iter()
            .map(|x| {
                Ok(LibraryCard {
                    id: x.id,
                    title: x.title,
                    kind: x.kind,
                    platform_name: x.platform_slug,
                    regions: x.regions,
                    cover: x.cover,
                })
            })
            .collect()
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
        self.conn
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM library_items WHERE id=?",
                params![id.to_string()],
            )
            .map(|_| ())
            .map_err(err)
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
    #[tokio::test]
    async fn crud_search() {
        let db = SqliteLibrary::in_memory().unwrap();
        let mut x = LibraryItem::new_game("Zelda");
        x.platform_slug = Some("snes".into());
        db.add(x.clone()).await.unwrap();
        assert_eq!(db.get(&x.id).await.unwrap().unwrap().title, "Zelda");
        assert_eq!(
            db.search(SearchQuery::new().text("zel"))
                .await
                .unwrap()
                .len(),
            1
        );
        db.remove(&x.id).await.unwrap();
        assert!(db.get(&x.id).await.unwrap().is_none());
    }
}
