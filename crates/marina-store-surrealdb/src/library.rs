use std::collections::HashMap;

use chrono::{DateTime, FixedOffset};
use marina_core::{LibraryAsset, LibraryItemFile};
use marina_library::{
    ItemKind, LibraryCard, LibraryError, LibraryItem, LibraryItemId, LibraryRead, LibraryWrite,
    SearchQuery,
};
use serde::{Deserialize, Serialize};
use surrealdb::types::{RecordId, RecordIdKey, SurrealValue, Uuid as SurrealUuid};

use crate::{ITEMS_TABLE, PLATFORMS_TABLE, SurrealLibrary, SurrealLibraryError};

fn parse_datetime(value: Option<String>) -> Option<DateTime<FixedOffset>> {
    value.and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
}

fn item_record_id(id: &LibraryItemId) -> RecordId {
    RecordId::new(ITEMS_TABLE, SurrealUuid::from(id.as_uuid()))
}

#[derive(Debug, Serialize, Deserialize, SurrealValue)]
pub(crate) struct StoredItem {
    pub(crate) item_id: SurrealUuid,
    pub(crate) title: String,
    pub(crate) kind: String,
    pub(crate) platform_slug: Option<String>,
    pub(crate) platform: RecordId,
    pub(crate) provider_ids: HashMap<String, String>,
    pub(crate) summary: Option<String>,
    pub(crate) alternative_names: Option<Vec<String>>,
    pub(crate) tags: Option<Vec<String>>,
    pub(crate) languages: Option<Vec<String>>,
    pub(crate) regions: Option<Vec<String>>,
    pub(crate) cover: Option<String>,
    pub(crate) created_at: Option<String>,
    pub(crate) released_at: Option<String>,
    pub(crate) updated_at: Option<String>,
    pub(crate) files: Option<Vec<StoredFile>>,
    pub(crate) assets: Option<Vec<StoredAsset>>,
}

#[derive(Debug, Serialize, Deserialize, SurrealValue)]
pub(crate) struct StoredLibraryCard {
    pub(crate) item_id: SurrealUuid,
    pub(crate) title: String,
    pub(crate) kind: String,
    pub(crate) platform_slug: Option<String>,
    pub(crate) platform_name: Option<String>,
    pub(crate) regions: Option<Vec<String>>,
    pub(crate) cover: Option<String>,
}

impl From<StoredLibraryCard> for LibraryCard {
    fn from(value: StoredLibraryCard) -> Self {
        Self {
            id: LibraryItemId::from_uuid(value.item_id.into_inner()),
            title: value.title,
            kind: match value.kind.as_str() {
                "app" => ItemKind::App,
                _ => ItemKind::Game,
            },
            platform_name: value
                .platform_name
                .filter(|name| !name.trim().is_empty())
                .or(value.platform_slug),
            regions: value.regions.unwrap_or_default(),
            cover: value.cover,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, SurrealValue)]
pub(crate) struct StoredFile {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) size_bytes: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, SurrealValue)]
pub(crate) struct StoredAsset {
    pub(crate) url: Option<String>,
    pub(crate) path: Option<String>,
}

impl TryFrom<StoredItem> for LibraryItem {
    type Error = SurrealLibraryError;

    fn try_from(value: StoredItem) -> Result<Self, Self::Error> {
        let id = LibraryItemId::from_uuid(value.item_id.into_inner());
        let kind = match value.kind.as_str() {
            "game" => ItemKind::Game,
            "app" => ItemKind::App,
            _ => ItemKind::Game,
        };

        Ok(Self {
            id,
            title: value.title,
            kind,
            platform_slug: match value.platform.key {
                RecordIdKey::String(slug) => Some(slug),
                _ => return Err(SurrealLibraryError::InvalidPlatformLink),
            },
            provider_ids: value.provider_ids,
            summary: value.summary,
            alternative_names: value.alternative_names.unwrap_or_default(),
            tags: value.tags.unwrap_or_default(),
            languages: value.languages.unwrap_or_default(),
            regions: value.regions.unwrap_or_default(),
            cover: value.cover,
            created_at: parse_datetime(value.created_at),
            released_at: parse_datetime(value.released_at),
            updated_at: parse_datetime(value.updated_at),
            files: value
                .files
                .unwrap_or_default()
                .into_iter()
                .map(|file| LibraryItemFile {
                    name: file.name,
                    path: file.path,
                    size_bytes: file.size_bytes,
                })
                .collect(),
            assets: value
                .assets
                .unwrap_or_default()
                .into_iter()
                .map(|asset| LibraryAsset {
                    url: asset.url,
                    path: asset.path,
                })
                .collect(),
        })
    }
}

impl From<&LibraryItem> for StoredItem {
    fn from(value: &LibraryItem) -> Self {
        Self {
            item_id: SurrealUuid::from(value.id.as_uuid()),
            title: value.title.clone(),
            kind: match value.kind {
                ItemKind::Game => "game".to_owned(),
                ItemKind::App => "app".to_owned(),
            },
            platform_slug: value.platform_slug.clone(),
            platform: RecordId::new(
                PLATFORMS_TABLE,
                value.platform_slug.clone().unwrap_or_default(),
            ),
            provider_ids: value.provider_ids.clone(),
            summary: value.summary.clone(),
            alternative_names: Some(value.alternative_names.clone()),
            tags: Some(value.tags.clone()),
            languages: Some(value.languages.clone()),
            regions: Some(value.regions.clone()),
            cover: value.cover.clone(),
            created_at: value.created_at.map(|date| date.to_rfc3339()),
            released_at: value.released_at.map(|date| date.to_rfc3339()),
            updated_at: value.updated_at.map(|date| date.to_rfc3339()),
            files: Some(
                value
                    .files
                    .iter()
                    .map(|file| StoredFile {
                        name: file.name.clone(),
                        path: file.path.clone(),
                        size_bytes: file.size_bytes,
                    })
                    .collect(),
            ),
            assets: Some(
                value
                    .assets
                    .iter()
                    .map(|asset| StoredAsset {
                        url: asset.url.clone(),
                        path: asset.path.clone(),
                    })
                    .collect(),
            ),
        }
    }
}

impl LibraryRead for SurrealLibrary {
    async fn search(&self, query: SearchQuery) -> Result<Vec<LibraryItem>, LibraryError> {
        let records: Vec<StoredItem> = self
            .db
            .select(ITEMS_TABLE)
            .await
            .map_err(LibraryError::backend)?;
        let needle = query.text.map(|text| text.to_lowercase());
        let platform = query.platform;

        let mut items = records
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<LibraryItem>, SurrealLibraryError>>()
            .map_err(LibraryError::backend)?
            .into_iter()
            .filter(|item: &LibraryItem| {
                let text_matches = needle.as_ref().is_none_or(|needle| {
                    item.title.to_lowercase().contains(needle)
                        || item
                            .provider_ids
                            .values()
                            .any(|value| value.to_lowercase().contains(needle))
                });
                let platform_matches = platform
                    .as_ref()
                    .is_none_or(|platform| item.platform_slug.as_deref() == Some(platform));
                text_matches && platform_matches
            })
            .skip(query.offset);

        let items: Vec<_> = match query.limit {
            Some(limit) => items.by_ref().take(limit).collect(),
            None => items.collect(),
        };
        Ok(items)
    }

    async fn get(&self, id: &LibraryItemId) -> Result<Option<LibraryItem>, LibraryError> {
        let record: Option<StoredItem> = self
            .db
            .select(item_record_id(id))
            .await
            .map_err(LibraryError::backend)?;
        record
            .map(TryInto::try_into)
            .transpose()
            .map_err(LibraryError::backend)
    }

    async fn list(&self, limit: u32) -> Result<Vec<LibraryItem>, LibraryError> {
        let mut response = self
            .db
            .query(format!("SELECT * FROM {ITEMS_TABLE} LIMIT $limit"))
            .bind(("limit", limit))
            .await
            .map_err(LibraryError::backend)?;
        let items: Vec<StoredItem> = response.take(0).map_err(LibraryError::backend)?;

        items
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, SurrealLibraryError>>()
            .map_err(LibraryError::backend)
    }

    async fn list_cards(&self, limit: u32) -> Result<Vec<LibraryCard>, LibraryError> {
        let mut response = self
            .db
            .query(format!(
                "SELECT item_id, title, kind, platform_slug, platform.name AS platform_name, regions, cover FROM {ITEMS_TABLE} LIMIT $limit"
            ))
            .bind(("limit", limit))
            .await
            .map_err(LibraryError::backend)?;
        let cards: Vec<StoredLibraryCard> = response.take(0).map_err(LibraryError::backend)?;

        Ok(cards.into_iter().map(Into::into).collect())
    }

    async fn search_cards(&self, query: SearchQuery) -> Result<Vec<LibraryCard>, LibraryError> {
        let mut filters = Vec::new();
        if query.text.is_some() {
            filters.push("string::lowercase(title) CONTAINS $text");
        }
        if query.platform.is_some() {
            filters.push("platform_slug = $platform");
        }

        let where_clause = if filters.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", filters.join(" AND "))
        };
        let limit = query
            .limit
            .map(|limit| u32::try_from(limit).unwrap_or(u32::MAX))
            .unwrap_or(100);
        let offset = u32::try_from(query.offset).unwrap_or(u32::MAX);

        let mut request = self
            .db
            .query(format!(
                "SELECT item_id, title, kind, platform_slug, platform.name AS platform_name, regions, cover FROM {ITEMS_TABLE}{where_clause} ORDER BY title LIMIT $limit START $offset"
            ))
            .bind(("limit", limit))
            .bind(("offset", offset));

        if let Some(text) = query.text {
            request = request.bind(("text", text.to_lowercase()));
        }
        if let Some(platform) = query.platform {
            request = request.bind(("platform", platform));
        }

        let mut response = request.await.map_err(LibraryError::backend)?;
        let cards: Vec<StoredLibraryCard> = response.take(0).map_err(LibraryError::backend)?;

        Ok(cards.into_iter().map(Into::into).collect())
    }
}

impl LibraryWrite for SurrealLibrary {
    async fn add(&self, item: LibraryItem) -> Result<LibraryItem, LibraryError> {
        let record_id = item_record_id(&item.id);
        let stored = StoredItem::from(&item);
        let record: Option<StoredItem> = self
            .db
            .upsert(record_id)
            .content(stored)
            .await
            .map_err(LibraryError::backend)?;
        record
            .map(TryInto::try_into)
            .transpose()
            .map_err(LibraryError::backend)
            .map(|result| result.unwrap_or(item))
    }

    async fn update(&self, item: LibraryItem) -> Result<LibraryItem, LibraryError> {
        let stored = StoredItem::from(&item);
        let record: Option<StoredItem> = self
            .db
            .update(item_record_id(&item.id))
            .content(stored)
            .await
            .map_err(LibraryError::backend)?;
        record
            .map(TryInto::try_into)
            .transpose()
            .map_err(LibraryError::backend)
            .map(|result| result.unwrap_or(item))
    }

    async fn remove(&self, id: &LibraryItemId) -> Result<(), LibraryError> {
        self.db
            .delete(item_record_id(id))
            .await
            .map(|_: Option<StoredItem>| ())
            .map_err(LibraryError::backend)
    }
}
