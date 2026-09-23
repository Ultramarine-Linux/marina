//! PortMaster release-backed store.
//!
//! The catalog and installer are sourced from a pinned PortMaster-New release.
//! HarbourMaster remains the installer of record; Marina only bootstraps its
//! PortMaster payload and registers the resulting launcher in the library.

use std::{
    io::Cursor,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use marina_core::LibraryItem;
use marina_library::Library;
use marina_store::{
    InstallMode, InstallRequest, StoreBackend, StoreEntry, StoreError, StorePlatform, StoreQuery,
};
use serde_json::Value;
use tokio::sync::Mutex;

mod installer;
use zip::ZipArchive;

pub const DEFAULT_RELEASE: &str = "2026-09-21_0303";
pub const DEFAULT_PORTS_DIR: &str = "/var/games/ports";

const RELEASE_BASE: &str = "https://github.com/PortsMaster/PortMaster-New/releases/download";

fn default_release() -> String {
    DEFAULT_RELEASE.to_owned()
}
fn default_ports_dir() -> PathBuf {
    PathBuf::from(DEFAULT_PORTS_DIR)
}

#[derive(Clone, Debug)]
pub struct PortMasterConfig {
    pub release: String,
    pub ports_dir: PathBuf,
}

impl Default for PortMasterConfig {
    fn default() -> Self {
        Self {
            release: default_release(),
            ports_dir: default_ports_dir(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("PortMaster catalog request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("PortMaster catalog has an unsupported JSON shape")]
    CatalogShape,
    #[error("PortMaster image archive is missing {0}")]
    ImageMissing(String),
    #[error("PortMaster image archive contains an unsafe path")]
    UnsafeArchivePath,
    #[error("PortMaster image archive error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("PortMaster file operation failed: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug)]
pub struct PortMasterStore {
    config: PortMasterConfig,
    client: reqwest::Client,
    catalog: Arc<Mutex<Option<Vec<StoreEntry>>>>,
}

impl PortMasterStore {
    pub fn new(config: PortMasterConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
            catalog: Arc::new(Mutex::new(None)),
        }
    }

    pub fn config(&self) -> &PortMasterConfig {
        &self.config
    }

    fn asset_url(&self, asset: &str) -> String {
        format!("{}/{}/{}", RELEASE_BASE, self.config.release, asset)
    }

    async fn catalog_entries(&self) -> Result<Vec<StoreEntry>, StoreError> {
        if let Some(entries) = self.catalog.lock().await.clone() {
            return Ok(entries);
        }
        let value = self
            .client
            .get(self.asset_url("ports.json"))
            .send()
            .await
            .map_err(|e| StoreError::backend(Error::Http(e)))?
            .error_for_status()
            .map_err(|e| StoreError::backend(Error::Http(e)))?
            .json::<Value>()
            .await
            .map_err(|e| StoreError::backend(Error::Http(e)))?;
        let entries = parse_catalog(&value).map_err(StoreError::backend)?;
        *self.catalog.lock().await = Some(entries.clone());
        Ok(entries)
    }
}

#[async_trait]
impl StoreBackend for PortMasterStore {
    fn id(&self) -> &str {
        "portmaster"
    }
    fn display_name(&self) -> &str {
        "PortMaster"
    }
    fn install_mode(&self) -> InstallMode {
        InstallMode::SingleArtifact
    }
    async fn preview_image(
        &self,
        entry: &StoreEntry,
    ) -> Result<Option<marina_store::StoreImage>, StoreError> {
        let Some(image_name) = entry
            .payload_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<Value>(json).ok())
            .and_then(|port| {
                port.pointer("/attr/image/screenshot")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .filter(|name| !name.is_empty())
        else {
            return Ok(None);
        };
        let package = entry.entry_id.trim_end_matches(".zip");
        let extension = image_name.rsplit('.').next().unwrap_or("png");
        let member = format!("{package}.screenshot.{extension}");
        let media_dir = self.config.ports_dir.join(".marina-media");
        tokio::fs::create_dir_all(&media_dir)
            .await
            .map_err(|e| StoreError::backend(Error::Io(e)))?;
        let archive_path = media_dir.join(format!("images-{}.zip", self.config.release));
        if !tokio::fs::try_exists(&archive_path).await.unwrap_or(false) {
            let bytes = self
                .client
                .get(self.asset_url("images.zip"))
                .send()
                .await
                .map_err(|e| StoreError::backend(Error::Http(e)))?
                .error_for_status()
                .map_err(|e| StoreError::backend(Error::Http(e)))?
                .bytes()
                .await
                .map_err(|e| StoreError::backend(Error::Http(e)))?;
            tokio::fs::write(&archive_path, bytes)
                .await
                .map_err(|e| StoreError::backend(Error::Io(e)))?;
        }
        let image_path = media_dir.join(&member);
        if !tokio::fs::try_exists(&image_path).await.unwrap_or(false) {
            let archive = archive_path.clone();
            let output = image_path.clone();
            tokio::task::spawn_blocking(move || extract_image(&archive, &member, &output))
                .await
                .map_err(|e| StoreError::backend(Error::Io(std::io::Error::other(e))))?
                .map_err(StoreError::backend)?;
        }
        Ok(Some(marina_store::StoreImage::new(image_path)))
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn list_platforms(&self) -> Result<Vec<StorePlatform>, StoreError> {
        let entries = self.catalog_entries().await?;
        Ok(vec![StorePlatform {
            slug: "portmaster".into(),
            name: "PortMaster".into(),
            backend_id: Some(self.id().into()),
            game_count: Some(entries.len() as u64),
        }])
    }

    async fn browse(&self, query: StoreQuery) -> Result<Vec<StoreEntry>, StoreError> {
        let mut entries =
            self.catalog_entries()
                .await?
                .into_iter()
                .filter(|_entry| {
                    query
                        .platform_slug
                        .as_deref()
                        .is_none_or(|slug| slug == "portmaster")
                })
                .filter(|entry| {
                    query.search.as_deref().is_none_or(|term| {
                        entry.title.to_lowercase().contains(&term.to_lowercase())
                    })
                })
                .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.title.to_lowercase());
        Ok(entries
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect())
    }

    async fn get(&self, entry_id: &str) -> Result<Option<StoreEntry>, StoreError> {
        Ok(self
            .catalog_entries()
            .await?
            .into_iter()
            .find(|entry| entry.entry_id == entry_id))
    }

    async fn install(
        &self,
        library: &(dyn Library + Send + Sync),
        request: InstallRequest,
    ) -> Result<LibraryItem, StoreError> {
        installer::ensure_payload(
            &self.client,
            self.asset_url("PortMaster.zip"),
            self.config.ports_dir.clone(),
        )
        .await
        .map_err(StoreError::backend)?;
        let package_id = request.entry.entry_id.clone();
        let payload = request
            .entry
            .payload_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<Value>(json).ok());
        let install_target = payload
            .as_ref()
            .and_then(|port| port.pointer("/source/url").and_then(Value::as_str))
            .ok_or_else(|| StoreError::backend(installer::Error::MissingLauncher))?;
        let expected_md5 = payload
            .as_ref()
            .and_then(|port| port.pointer("/source/md5").and_then(Value::as_str));
        let items = payload
            .as_ref()
            .and_then(|port| port.get("items").and_then(Value::as_array))
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let launcher = installer::install_port(
            &self.client,
            install_target.to_owned(),
            expected_md5,
            package_id.clone(),
            items,
            self.config.ports_dir.clone(),
        )
        .await
        .map_err(StoreError::backend)?;
        let mut item = LibraryItem::new_game(request.entry.title.clone());
        item.platform_slug = Some("portmaster".into());
        item.local_path = Some(launcher.to_string_lossy().into_owned());
        item.provider_ids
            .insert("portmaster.package".into(), package_id.clone());
        item.provider_ids
            .insert("portmaster.release".into(), self.config.release.clone());
        if let Some(image) = self.preview_image(&request.entry).await? {
            item.assets.push(marina_core::LibraryAsset {
                kind: marina_core::LibraryAssetKind::CoverLarge,
                source: None,
                local_path: Some(image.path.to_string_lossy().into_owned()),
            });
        }
        library.add(item).await.map_err(StoreError::backend)
    }
}

fn extract_image(archive_path: &Path, member: &str, output: &Path) -> Result<(), Error> {
    if Path::new(member).components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    }) {
        return Err(Error::UnsafeArchivePath);
    }
    let bytes = std::fs::read(archive_path)?;
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let mut image = archive.by_name(member).map_err(|error| match error {
        zip::result::ZipError::FileNotFound => Error::ImageMissing(member.to_owned()),
        other => Error::Zip(other),
    })?;
    let mut output_file = std::fs::File::create(output)?;
    std::io::copy(&mut image, &mut output_file)?;
    Ok(())
}

fn parse_catalog(value: &Value) -> Result<Vec<StoreEntry>, Error> {
    let ports = value
        .get("ports")
        .and_then(Value::as_object)
        .ok_or(Error::CatalogShape)?;
    ports
        .iter()
        .map(|(package_key, port)| {
            let package = port
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| port.get("filename").and_then(Value::as_str))
                .unwrap_or(package_key);
            let title = port
                .pointer("/attr/title")
                .and_then(Value::as_str)
                .or_else(|| port.get("title").and_then(Value::as_str))
                .unwrap_or(package)
                .to_owned();
            let payload = port.clone();
            Ok(StoreEntry {
                backend_id: "portmaster".into(),
                entry_id: package.to_owned(),
                title,
                platform_slug: "portmaster".into(),
                platform_name: Some("PortMaster".into()),
                payload_json: Some(payload.to_string()),
            })
        })
        .collect()
}
