//! PortMaster release-backed store.
//!
//! The catalog and installer are sourced from a pinned PortMaster-New release.
//! HarbourMaster remains the installer of record; Marina only bootstraps its
//! PortMaster payload and registers the resulting launcher in the library.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
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
use tokio::{process::Command, sync::Mutex};

pub const DEFAULT_RELEASE: &str = "2026-09-21_0303";
pub const DEFAULT_PORTS_DIR: &str = "/var/games/ports";
pub const DEFAULT_HARBOURMASTER: &str = "/var/games/ports/PortMaster/harbourmaster";
const RELEASE_BASE: &str = "https://github.com/PortsMaster/PortMaster-New/releases/download";

fn default_release() -> String {
    DEFAULT_RELEASE.to_owned()
}
fn default_ports_dir() -> PathBuf {
    PathBuf::from(DEFAULT_PORTS_DIR)
}
fn default_harbourmaster() -> PathBuf {
    PathBuf::from(DEFAULT_HARBOURMASTER)
}

#[derive(Clone, Debug)]
pub struct PortMasterConfig {
    pub release: String,
    pub ports_dir: PathBuf,
    pub binary: PathBuf,
}

impl Default for PortMasterConfig {
    fn default() -> Self {
        Self {
            release: default_release(),
            ports_dir: default_ports_dir(),
            binary: default_harbourmaster(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("PortMaster catalog request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("PortMaster catalog has an unsupported JSON shape")]
    CatalogShape,

    #[error("PortMaster command failed: {status}: {stderr}")]
    Command { status: String, stderr: String },
    #[error("PortMaster command could not start: {0}")]
    Io(#[from] std::io::Error),
    #[error("PortMaster package installed but no launcher was found in {0}")]
    MissingLauncher(String),
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

    async fn ensure_harbourmaster(&self) -> Result<(), StoreError> {
        if tokio::fs::try_exists(&self.config.binary)
            .await
            .unwrap_or(false)
        {
            #[cfg(unix)]
            tokio::fs::set_permissions(&self.config.binary, std::fs::Permissions::from_mode(0o755))
                .await
                .map_err(|e| StoreError::backend(Error::Io(e)))?;
            return Ok(());
        }
        tokio::fs::create_dir_all(&self.config.ports_dir)
            .await
            .map_err(|e| StoreError::backend(Error::Io(e)))?;
        let archive = self.config.ports_dir.join("PortMaster.zip");
        let bytes = self
            .client
            .get(self.asset_url("PortMaster.zip"))
            .send()
            .await
            .map_err(|e| StoreError::backend(Error::Http(e)))?
            .error_for_status()
            .map_err(|e| StoreError::backend(Error::Http(e)))?
            .bytes()
            .await
            .map_err(|e| StoreError::backend(Error::Http(e)))?;
        tokio::fs::write(&archive, &bytes)
            .await
            .map_err(|e| StoreError::backend(Error::Io(e)))?;
        let output = Command::new("python3")
            .args(["-m", "zipfile", "-e"])
            .arg(&archive)
            .arg(&self.config.ports_dir)
            .output()
            .await
            .map_err(|e| StoreError::backend(Error::Io(e)))?;
        if !output.status.success() {
            return Err(StoreError::backend(Error::Command {
                status: output.status.to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            }));
        }
        #[cfg(unix)]
        tokio::fs::set_permissions(&self.config.binary, std::fs::Permissions::from_mode(0o755))
            .await
            .map_err(|e| StoreError::backend(Error::Io(e)))?;
        Ok(())
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
        self.ensure_harbourmaster().await?;
        let package_id = request.entry.entry_id.clone();
        let install_target = request
            .entry
            .payload_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<Value>(json).ok())
            .and_then(|port| {
                port.pointer("/source/url")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| package_id.clone());
        let output = Command::new("python3")
            .arg(&self.config.binary)
            .current_dir(&self.config.ports_dir)
            .env("HM_TOOLS_DIR", &self.config.ports_dir)
            .env("HM_PORTS_DIR", &self.config.ports_dir)
            .env("HM_SCRIPTS_DIR", &self.config.ports_dir)
            .args(["--no-check", "install", &install_target])
            .output()
            .await
            .map_err(|e| StoreError::backend(Error::Io(e)))?;
        if !output.status.success() {
            return Err(StoreError::backend(Error::Command {
                status: output.status.to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            }));
        }
        let launcher = find_launcher(&self.config.ports_dir, &package_id)
            .await
            .map_err(StoreError::backend)?;
        let mut item = LibraryItem::new_game(request.entry.title.clone());
        item.platform_slug = Some("portmaster".into());
        item.local_path = Some(launcher.to_string_lossy().into_owned());
        item.provider_ids
            .insert("portmaster.package".into(), package_id.clone());
        item.provider_ids
            .insert("portmaster.release".into(), self.config.release.clone());
        if let Some(image_url) = request
            .entry
            .payload_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<Value>(json).ok())
            .and_then(|port| {
                port.get("marina_image_url")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
        {
            let image_path = self
                .config
                .ports_dir
                .join(".marina-media")
                .join(format!("{}.png", package_id.trim_end_matches(".zip")));
            if let Some(parent) = image_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| StoreError::backend(Error::Io(e)))?;
            }
            if let Ok(response) = self.client.get(&image_url).send().await
                && let Ok(response) = response.error_for_status()
                && let Ok(bytes) = response.bytes().await
            {
                tokio::fs::write(&image_path, bytes)
                    .await
                    .map_err(|e| StoreError::backend(Error::Io(e)))?;
                item.assets.push(marina_core::LibraryAsset {
                    kind: marina_core::LibraryAssetKind::CoverLarge,
                    source: Some(image_url),
                    local_path: Some(image_path.to_string_lossy().into_owned()),
                });
            }
        }
        library.add(item).await.map_err(StoreError::backend)
    }
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
            let image_url = port
                .pointer("/attr/image/screenshot")
                .and_then(Value::as_str)
                .filter(|image| !image.is_empty())
                .map(|image| {
                                    format!(
                                        "https://github.com/PortsMaster/PortMaster-Info/raw/main/images/{}.screenshot.{}",
                                        package_key.trim_end_matches(".zip"),
                                        image.rsplit('.').next().unwrap_or("png")
                                    )
                                });
            let mut payload = port.clone();
            if let Some(image_url) = image_url {
                payload["marina_image_url"] = Value::String(image_url);
            }
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

async fn find_launcher(root: &Path, package: &str) -> Result<PathBuf, Error> {
    let stem = package
        .strip_suffix(".zip")
        .unwrap_or(package)
        .to_ascii_lowercase();
    let mut dirs = vec![root.to_owned()];
    while let Some(dir) = dirs.pop() {
        let mut entries = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) == Some("sh")
                && path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.to_ascii_lowercase() == stem)
            {
                return Ok(path);
            }
        }
    }
    Err(Error::MissingLauncher(root.display().to_string()))
}
