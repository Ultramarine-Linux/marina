//! [`marina_store::StoreBackend`] implementation for RomM.

use async_trait::async_trait;
use marina_store::{StoreBackend, StoreEntry, StoreError, StorePlatform, StoreQuery};

use crate::{Auth, Client, PlatformQuery, RomQuery};

#[derive(Clone, Debug)]
pub struct RommStore {
    client: Client,
}

impl RommStore {
    pub fn new(base_url: impl Into<String>, token: Option<&str>) -> Self {
        let client = Client::new(base_url);
        let client = match token.filter(|token| !token.trim().is_empty()) {
            Some(token) => client.with_auth(Auth::Bearer(token.to_owned())),
            None => client,
        };
        Self { client }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn base_url(&self) -> &str {
        self.client.base_url()
    }

    async fn platform_id_for_slug(&self, slug: &str) -> Result<Option<i64>, StoreError> {
        let platforms = self
            .client
            .list_platforms(&PlatformQuery::default())
            .await
            .map_err(StoreError::backend)?;
        Ok(platforms
            .into_iter()
            .find(|platform| platform.fs_slug == slug)
            .map(|platform| platform.id))
    }
}

#[async_trait]
impl StoreBackend for RommStore {
    fn id(&self) -> &str {
        "romm"
    }

    fn display_name(&self) -> &str {
        "RomM"
    }

    async fn list_platforms(&self) -> Result<Vec<StorePlatform>, StoreError> {
        let platforms = self
            .client
            .list_platforms(&PlatformQuery::default())
            .await
            .map_err(StoreError::backend)?;
        Ok(platforms
            .into_iter()
            .map(|platform| StorePlatform {
                slug: platform.fs_slug,
                name: platform.display_name,
                backend_id: Some(platform.id.to_string()),
                game_count: u64::try_from(platform.rom_count).ok(),
            })
            .collect())
    }

    async fn browse(&self, query: StoreQuery) -> Result<Vec<StoreEntry>, StoreError> {
        let platform_ids = match &query.platform_slug {
            Some(slug) => match self.platform_id_for_slug(slug).await? {
                Some(id) => vec![id],
                None => return Ok(Vec::new()),
            },
            None => Vec::new(),
        };
        let rom_query = RomQuery {
            platform_ids,
            search_term: query.search.clone(),
            limit: Some(query.limit.min(i64::MAX as usize) as i64),
            offset: Some(query.offset.min(i64::MAX as usize) as i64),
            with_files: Some(true),
            ..Default::default()
        };
        let page = self
            .client
            .list_roms(&rom_query)
            .await
            .map_err(StoreError::backend)?;
        Ok(page
            .items
            .into_iter()
            .map(|rom| {
                let title = rom
                    .name
                    .clone()
                    .unwrap_or_else(|| rom.files.fs_name.clone());
                StoreEntry {
                    backend_id: "romm".into(),
                    entry_id: rom.id.to_string(),
                    title,
                    platform_slug: rom.platform.platform_fs_slug.clone(),
                    platform_name: Some(
                        rom.platform
                            .platform_display_name
                            .clone()
                            .unwrap_or(rom.platform.platform_fs_slug.clone()),
                    ),
                    payload_json: serde_json::to_string(&rom).ok(),
                }
            })
            .collect())
    }

    async fn get(&self, entry_id: &str) -> Result<Option<StoreEntry>, StoreError> {
        let Ok(id) = entry_id.parse::<i32>() else {
            return Ok(None);
        };
        let rom = self.client.get_rom(id).await.map_err(StoreError::backend)?;
        let title = rom
            .name
            .clone()
            .unwrap_or_else(|| rom.files.fs_name.clone());
        Ok(Some(StoreEntry {
            backend_id: "romm".into(),
            entry_id: rom.id.to_string(),
            title,
            platform_slug: rom.platform.platform_fs_slug.clone(),
            platform_name: Some(
                rom.platform
                    .platform_display_name
                    .clone()
                    .unwrap_or(rom.platform.platform_fs_slug.clone()),
            ),
            payload_json: serde_json::to_string(&rom).ok(),
        }))
    }
}
