//! [`marina_store::StoreBackend`] implementation for RomM.

use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};

use async_trait::async_trait;
use marina_store::{StoreBackend, StoreEntry, StoreError, StorePlatform, StoreQuery};

use crate::{Auth, Client, PlatformQuery, Rom, RomQuery};

#[derive(Debug, Default)]
struct PlatformCache {
    by_slug: HashMap<String, i64>,
}

#[derive(Clone, Debug)]
pub struct RommStore {
    client: Client,
    platforms: std::sync::Arc<Mutex<PlatformCache>>,
}

impl RommStore {
    pub fn new(base_url: impl Into<String>, token: Option<&str>) -> Self {
        let client = Client::new(base_url);
        let client = match token.filter(|token| !token.trim().is_empty()) {
            Some(token) => client.with_auth(Auth::Bearer(token.to_owned())),
            None => client,
        };
        Self {
            client,
            platforms: std::sync::Arc::new(Mutex::new(PlatformCache::default())),
        }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn base_url(&self) -> &str {
        self.client.base_url()
    }

    async fn get_with_sibling_files(&self, id: i32) -> Result<Rom, StoreError> {
        let mut rom = self.client.get_rom(id).await.map_err(StoreError::backend)?;
        let sibling_ids = rom
            .siblings
            .sibling_roms
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|sibling| sibling.id)
            .filter(|sibling_id| *sibling_id != rom.id)
            .collect::<Vec<_>>();
        let mut file_ids = rom
            .files
            .files
            .iter()
            .map(|file| file.id)
            .collect::<HashSet<_>>();

        for sibling_id in sibling_ids {
            match self.client.get_rom(sibling_id).await {
                Ok(sibling) => {
                    rom.files.files.extend(
                        sibling
                            .files
                            .files
                            .into_iter()
                            .filter(|file| file_ids.insert(file.id)),
                    );
                }
                Err(error) => {
                    tracing::warn!(rom_id = id, sibling_id, %error, "failed to hydrate RomM sibling");
                }
            }
        }

        Ok(rom)
    }

    async fn platform_id_for_slug(&self, slug: &str) -> Result<Option<i64>, StoreError> {
        if let Some(id) = self
            .platforms
            .lock()
            .expect("romm platform cache poisoned")
            .by_slug
            .get(slug)
            .copied()
        {
            return Ok(Some(id));
        }
        let platforms = self
            .client
            .list_platforms(&PlatformQuery::default())
            .await
            .map_err(StoreError::backend)?;
        let id = platforms
            .into_iter()
            .find(|platform| platform.fs_slug == slug)
            .map(|platform| platform.id);
        if let Some(id) = id {
            self.platforms
                .lock()
                .expect("romm platform cache poisoned")
                .by_slug
                .insert(slug.to_owned(), id);
        }
        Ok(id)
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

    fn as_any(&self) -> &dyn std::any::Any {
        self
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
        let rom = self.get_with_sibling_files(id).await?;
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

    async fn install(
        &self,
        library: &(dyn marina_library::Library + Send + Sync),
        request: marina_store::InstallRequest,
    ) -> Result<marina_core::LibraryItem, StoreError> {
        let resolved = crate::install::resolve(
            &self.client,
            &request.entry,
            &request.file_ids,
            request.library_root,
        )
        .await
        .map_err(crate::install::map_error)?;
        crate::install::install(&self.client, library, resolved)
            .await
            .map_err(crate::install::map_error)
    }
}
