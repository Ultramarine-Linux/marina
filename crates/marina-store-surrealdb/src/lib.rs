//! SurrealDB-backed storage for Marina's library interfaces.
//!
//! The backend accepts SurrealDB endpoint URIs, for example `mem://` for an
//! ephemeral test database or `surrealkv://./marina.db` for a local persistent
//! database.

use std::collections::HashMap;

use marina_library::{
    ItemKind, LibraryError, LibraryItem, LibraryItemId, LibraryRead, LibraryWrite, Platform,
    SearchQuery,
};
use serde::{Deserialize, Serialize};
use surrealdb::{
    Surreal,
    engine::any::{Any, IntoEndpoint},
    types::SurrealValue,
};
use thiserror::Error;

const ITEMS_TABLE: &str = "library";
const PLATFORMS_TABLE: &str = "platforms";

#[derive(Debug, Error)]
pub enum SurrealLibraryError {
    #[error("SurrealDB operation failed: {0}")]
    Database(#[from] surrealdb::Error),

    #[error("stored library item has an invalid id: {0}")]
    InvalidItemId(String),
}

#[derive(Debug, Serialize, Deserialize, SurrealValue)]
struct StoredItem {
    item_id: String,
    title: String,
    kind: String,
    provider_ids: HashMap<String, String>,
}

impl TryFrom<StoredItem> for LibraryItem {
    type Error = SurrealLibraryError;

    fn try_from(value: StoredItem) -> Result<Self, Self::Error> {
        let id = LibraryItemId::parse(&value.item_id)
            .ok_or_else(|| SurrealLibraryError::InvalidItemId(value.item_id.clone()))?;
        let kind = match value.kind.as_str() {
            "game" => ItemKind::Game,
            "app" => ItemKind::App,
            _ => ItemKind::Game,
        };

        Ok(Self {
            id,
            title: value.title,
            kind,
            provider_ids: value.provider_ids,
        })
    }
}

impl From<&LibraryItem> for StoredItem {
    fn from(value: &LibraryItem) -> Self {
        Self {
            item_id: value.id.to_string(),
            title: value.title.clone(),
            kind: match value.kind {
                ItemKind::Game => "game".to_owned(),
                ItemKind::App => "app".to_owned(),
            },
            provider_ids: value.provider_ids.clone(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, SurrealValue)]
struct StoredPlatform {
    slug: String,
    name: String,
}

/// A Marina library backed by an embedded or remote SurrealDB endpoint.
#[derive(Debug)]
pub struct SurrealLibrary {
    db: Surreal<Any>,
}

impl SurrealLibrary {
    /// Connects to a SurrealDB endpoint URI.
    ///
    /// Common local endpoints are `mem://` (or `memory://`) and
    /// `surrealkv://path/to/database`. Remote SurrealDB endpoints supported by
    /// `surrealdb::engine::any::connect` are accepted as well.
    pub async fn connect(uri: impl IntoEndpoint) -> Result<Self, SurrealLibraryError> {
        let db = surrealdb::engine::any::connect(uri).await?;
        db.use_ns("marina").use_db("library").await?;
        Ok(Self { db })
    }

    /// Adds or replaces a platform record. This is useful to seed the backend
    /// because `LibraryItem` currently does not carry platform metadata.
    pub async fn add_platform(&self, platform: Platform) -> Result<(), LibraryError> {
        let record = StoredPlatform {
            slug: platform.slug.clone(),
            name: platform.name,
        };
        self.db
            .upsert((PLATFORMS_TABLE, platform.slug))
            .content(record)
            .await
            .map(|_: Option<StoredPlatform>| ())
            .map_err(LibraryError::backend)
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
            .filter_map(|record| record.try_into().ok())
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
                    .is_none_or(|platform| item.provider_ids.keys().any(|key| key == platform));
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
            .select((ITEMS_TABLE, id.to_string()))
            .await
            .map_err(LibraryError::backend)?;
        record
            .map(TryInto::try_into)
            .transpose()
            .map_err(LibraryError::backend)
    }

    async fn platforms(&self) -> Result<Vec<Platform>, LibraryError> {
        let records: Vec<StoredPlatform> = self
            .db
            .select(PLATFORMS_TABLE)
            .await
            .map_err(LibraryError::backend)?;
        Ok(records
            .into_iter()
            .map(|platform| Platform::new(platform.slug, platform.name))
            .collect())
    }
}

impl LibraryWrite for SurrealLibrary {
    async fn add(&self, item: LibraryItem) -> Result<LibraryItem, LibraryError> {
        let id = item.id.to_string();
        let stored = StoredItem::from(&item);
        let record: Option<StoredItem> = self
            .db
            .upsert((ITEMS_TABLE, id))
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
            .update((ITEMS_TABLE, item.id.to_string()))
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
            .delete((ITEMS_TABLE, id.to_string()))
            .await
            .map(|_: Option<StoredItem>| ())
            .map_err(LibraryError::backend)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_uri_supports_library_crud_and_search() {
        let library = SurrealLibrary::connect("mem://").await.unwrap();
        let item = LibraryItem::game("Super Mario World");
        let id = item.id.clone();

        library.add(item).await.unwrap();
        assert_eq!(
            library.get(&id).await.unwrap().unwrap().title,
            "Super Mario World"
        );
        assert_eq!(
            library
                .search(SearchQuery::new().text("mario"))
                .await
                .unwrap()
                .len(),
            1
        );

        library.remove(&id).await.unwrap();
        assert!(library.get(&id).await.unwrap().is_none());
    }
}
