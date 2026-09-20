//! Runtime configuration: figment-merged TOML file + environment.
//!
//! Store backends live in their own config-file section so adding a backend
//! is a new `[store.<backend>]` table, not more top-level env vars:
//!
//! ```toml
//! [store.romm]
//! enable = true
//! url = "https://romm.example.com"
//! token = "rmm_..."
//! import_on_startup = false
//! ```
//!
//! Reads go through figment: the TOML file is the base layer and each
//! environment variable merges over it as a key-path tuple, so precedence
//! per field is: environment variable > config file > default. The file is
//! looked up at `$MARINA_CONFIG`, then `./marina.toml`, then
//! `$XDG_CONFIG_HOME/marina/config.toml` (`~/.config/marina/config.toml`).
//! A missing file is fine and just yields defaults.
//!
//! Writes go through [`toml_edit`] (see [`upsert_toml_value`]) so in-UI
//! settings editing preserves comments and formatting. Figment is
//! read-only and cannot write back.

use std::env;
use std::path::PathBuf;

use figment::{
    Figment,
    providers::{Format, Toml},
};
use serde::Deserialize;
use tracing::warn;

#[derive(Clone, Debug, Default, Deserialize)]
struct FileConfig {
    #[serde(default)]
    store: StoreSection,
    #[serde(default)]
    library: LibrarySection,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct StoreSection {
    #[serde(default)]
    romm: RommConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct LibrarySection {
    #[serde(default)]
    root: Option<PathBuf>,
    #[serde(default)]
    storage_uri: Option<String>,
    #[serde(default)]
    store_cache_dir: Option<PathBuf>,
    #[serde(default)]
    scan_on_startup: Option<bool>,
}

/// A single store backend's file configuration.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RommConfig {
    #[serde(default)]
    pub enable: Option<bool>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub import_on_startup: Option<bool>,
}

#[derive(Debug)]
pub struct Config {
    pub storage_uri: String,
    pub store_cache_dir: std::path::PathBuf,

    pub romm_url: Option<String>,
    pub romm_token: Option<String>,
    pub import_romm_on_startup: bool,
    pub scan_on_startup: bool,
    pub library_root: Option<std::path::PathBuf>,
}

impl Config {
    pub fn from_env() -> Self {
        let mut figment = Figment::new();
        if let Some(path) = config_candidates().into_iter().find(|p| p.is_file()) {
            tracing::info!(path = %path.display(), "loading config file");
            figment = figment.merge(Toml::file(path));
        }
        // Env vars merge over the file as key-path tuples: `(key, value)`
        // with a dotted path like "store.romm.url" nests like the TOML.
        if let Some(value) = env_bool("MARINA_ENABLE_ROMM") {
            figment = figment.merge(("store.romm.enable", value));
        }
        if let Ok(value) = env::var("ROMM_URL") {
            figment = figment.merge(("store.romm.url", value));
        }
        if let Ok(value) = env::var("ROMM_TOKEN") {
            figment = figment.merge(("store.romm.token", value));
        }
        if let Some(value) = env_bool("MARINA_IMPORT_ROMM_ON_STARTUP") {
            figment = figment.merge(("store.romm.import_on_startup", value));
        }
        if let Some(value) = env_bool("MARINA_SCAN_ON_STARTUP") {
            figment = figment.merge(("library.scan_on_startup", value));
        }
        if let Ok(value) = env::var("MARINA_STORAGE_URI") {
            figment = figment.merge(("library.storage_uri", value));
        }
        if let Some(value) = env::var_os("MARINA_STORE_CACHE_DIR") {
            figment = figment.merge((
                "library.store_cache_dir",
                PathBuf::from(value).to_string_lossy().into_owned(),
            ));
        }
        if let Some(value) = env::var_os("MARINA_LIBRARY_ROOT") {
            figment = figment.merge((
                "library.root",
                PathBuf::from(value).to_string_lossy().into_owned(),
            ));
        }
        Self::from_figment(figment)
    }

    fn from_figment(figment: Figment) -> Self {
        let file: FileConfig = match figment.extract() {
            Ok(file) => file,
            Err(error) => {
                warn!(%error, "ignoring invalid config; using defaults");
                FileConfig::default()
            }
        };
        let romm = &file.store.romm;

        let romm_enabled = romm.enable.unwrap_or(false);
        let romm_url = if romm_enabled { romm.url.clone() } else { None };
        let romm_token = if romm_enabled {
            romm.token.clone()
        } else {
            None
        };
        let import_romm_on_startup = romm.import_on_startup.unwrap_or(false);
        let scan_on_startup = file.library.scan_on_startup.unwrap_or(true);
        if !scan_on_startup {
            tracing::info!(
                "local library scan disabled at startup by config or MARINA_SCAN_ON_STARTUP=false"
            );
        }
        if !romm_enabled {
            tracing::info!(
                "RomM backend disabled by default; enable it with [store.romm] enable = true"
            );
        } else if romm_url.is_none() {
            warn!("[store.romm] url not set — relative cover paths will not resolve");
        }

        let default_library_root = Some(std::path::PathBuf::from("/var/games/library"));
        let default_storage_uri = state_dir()
            .map(|path| {
                format!(
                    "sqlite://{}",
                    path.join("marina").join("library.db").display()
                )
            })
            .unwrap_or_else(|| "sqlite://marina.db".to_owned());

        Self {
            storage_uri: file
                .library
                .storage_uri
                .clone()
                .unwrap_or(default_storage_uri),
            store_cache_dir: file
                .library
                .store_cache_dir
                .clone()
                .or_else(|| {
                    env::var("MARINA_STORE_CACHE_URI").ok().and_then(|uri| {
                        let path = uri.strip_prefix("sqlite://").unwrap_or(&uri);
                        std::path::Path::new(path)
                            .parent()
                            .map(|parent| parent.to_path_buf())
                    })
                })
                .unwrap_or_else(|| {
                    state_dir()
                        .map(|path| path.join("marina").join("store-cache"))
                        .unwrap_or_else(|| std::path::PathBuf::from("store-cache"))
                }),

            romm_url,
            romm_token,
            import_romm_on_startup,
            scan_on_startup,
            library_root: file.library.root.clone().or(default_library_root),
        }
    }
}

fn env_bool(name: &str) -> Option<bool> {
    env::var(name).ok().map(|value| {
        !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    })
}

fn config_candidates() -> Vec<PathBuf> {
    if let Some(path) = env::var_os("MARINA_CONFIG").map(PathBuf::from) {
        return vec![path];
    }
    let mut candidates = vec![PathBuf::from("marina.toml")];
    let xdg = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")));
    if let Some(base) = xdg {
        candidates.push(base.join("marina").join("config.toml"));
    }
    candidates
}

/// Updates a single scalar in a TOML document while preserving comments,
/// whitespace, and key order. Backing for future in-UI settings editing:
/// parse the config file into a [`toml_edit::Document`], upsert, write
/// back with `doc.to_string()`.
/// Not wired into the UI yet; the settings screen will call this.
#[allow(dead_code)]
pub fn upsert_toml_value(
    document: &str,
    table_path: &[&str],
    key: &str,
    value: toml_edit::Item,
) -> Result<String, toml_edit::TomlError> {
    let mut doc: toml_edit::Document = document.parse()?;
    let mut table = doc.as_table_mut();
    for segment in table_path {
        if !table.contains_key(*segment) {
            table.insert(*segment, toml_edit::Item::Table(toml_edit::Table::new()));
        }
        let entry = &mut table[*segment];
        if !entry.is_table() {
            *entry = toml_edit::Item::Table(toml_edit::Table::new());
        }
        table = entry.as_table_mut().expect("just ensured table");
    }
    table.insert(key, value);
    Ok(doc.to_string())
}

fn state_dir() -> Option<std::path::PathBuf> {
    env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".local").join("state")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_store_romm_section() {
        let config: FileConfig = toml::from_str(
            r#"
[store.romm]
enable = true
url = "https://romm.example.com"
token = "secret"
import_on_startup = true

[library]
scan_on_startup = false
"#,
        )
        .unwrap();
        assert_eq!(
            config.store.romm.url.as_deref(),
            Some("https://romm.example.com")
        );
        assert_eq!(config.store.romm.token.as_deref(), Some("secret"));
        assert_eq!(config.store.romm.enable, Some(true));
        assert_eq!(config.library.scan_on_startup, Some(false));
    }

    #[test]
    fn figment_merges_toml_base_with_key_path_overrides() {
        let figment = Figment::new()
            .merge(Toml::string(
                "[store.romm]\nenable = true\nurl = \"https://file.example.com\"\n",
            ))
            .merge(("store.romm.url", "https://override.example.com".to_string()));
        let config = Config::from_figment(figment);
        assert_eq!(
            config.romm_url.as_deref(),
            Some("https://override.example.com")
        );
    }

    #[test]
    fn toml_edit_upsert_preserves_comments() {
        let updated = upsert_toml_value(
            "# my romm server\n[store.romm]\nenable = false\n",
            &["store", "romm"],
            "url",
            toml_edit::value("https://romm.example.com"),
        )
        .unwrap();
        assert!(updated.contains("# my romm server"));
        assert!(updated.contains("enable = false"));
        assert!(updated.contains(r#"url = "https://romm.example.com""#));
    }
}
