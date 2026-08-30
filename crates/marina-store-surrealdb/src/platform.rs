use marina_library::{LibraryError, Platform, PlatformRead, PlatformWrite};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

use crate::{PLATFORMS_TABLE, SurrealLibrary};

#[derive(Debug, Serialize, Deserialize, SurrealValue)]
pub(crate) struct StoredPlatform {
    pub(crate) slug: String,
    pub(crate) name: String,
}

impl PlatformRead for SurrealLibrary {
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

impl PlatformWrite for SurrealLibrary {
    async fn add_platform(&self, platform: Platform) -> Result<Platform, LibraryError> {
        let record = StoredPlatform {
            slug: platform.slug.clone(),
            name: platform.name.clone(),
        };
        self.db
            .upsert((PLATFORMS_TABLE, platform.slug.clone()))
            .content(record)
            .await
            .map(|_: Option<StoredPlatform>| platform)
            .map_err(LibraryError::backend)
    }

    async fn update_platform(&self, platform: Platform) -> Result<Platform, LibraryError> {
        self.add_platform(platform).await
    }

    async fn remove_platform(&self, slug: &str) -> Result<(), LibraryError> {
        self.db
            .delete((PLATFORMS_TABLE, slug))
            .await
            .map(|_: Option<StoredPlatform>| ())
            .map_err(LibraryError::backend)
    }
}
