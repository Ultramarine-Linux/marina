//! Runtime configuration: figment-merged TOML file + environment.
//!
//! Store backends live in the library namespace:
//!
//! ```toml
//! [library.romm]
//! enable = true
//! url = "https://romm.example.com"
//! token = "rmm_..."
//! import_on_startup = false
//! ```
//!
//! Emulator runtimes live under `[runtime.retroarch]`; each
//! `[runtime.retroarch.platforms."<slug>"]` table pins a backend and its
//! settings for one platform:
//!
//! ```toml
//! [runtime.retroarch]
//! binary = "retroarch"
//! cores_dir = "/var/games/retroarch/cores"
//!
//! [runtime.retroarch.platforms."gba"]
//! backend = "retroarch"
//! [runtime.retroarch.platforms."gba".retroarch]
//! core = "mgba_libretro.so"
//! ```
//!
//! Reads go through figment. TOML sources are merged in this order, with
//! later sources overriding earlier fields:
//!
//! 1. `/usr/share/marina/config.toml.d/*.toml`
//! 2. `/usr/share/marina/config.toml`
//! 3. `/etc/marina/config.toml.d/*.toml`
//! 4. `/etc/marina/config.toml`
//! 5. `$XDG_CONFIG_HOME/marina/config.toml.d/*.toml`
//! 6. The primary config file: `$MARINA_CONFIG`, `./marina.toml`, or
//!    `$XDG_CONFIG_HOME/marina/config.toml` (`~/.config/marina/config.toml`)
//! 7. Environment variables
//!
//! Drop-ins are loaded lexicographically by filename. When no primary config
//! exists, [`ensure_config_file`] writes [`default_config_template`] (rendered
//! from the config structs' defaults and `///` doc comments via
//! `ConfigTemplate`) to `$MARINA_CONFIG` or the XDG location on startup.
//!
//! Writes go through [`toml_edit`] (see [`upsert_toml_value`]) so in-UI
//! settings editing preserves comments and formatting. Figment is
//! read-only and cannot write back.

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use figment::{
    Figment,
    providers::{Format, Toml},
};
use marina_config_derive::{ConfigSettings, ConfigTemplate};
use marina_portmaster::{DEFAULT_PORTS_DIR, DEFAULT_RELEASE, PortMasterConfig};
use marina_runtime::{
    PlatformRuntimeConfig, RetroArchConfig as EffectiveRetroArchConfig,
    portmaster::Config as RuntimePortMasterConfig,
};
use serde::Deserialize;
use tracing::warn;

mod settings;
mod sources;
#[cfg(test)]
mod tests;
pub use settings::*;
use sources::*;

mod schema;
use schema::*;

#[derive(Clone, Debug)]
pub struct Config {
    pub storage_uri: String,
    pub store_cache_dir: std::path::PathBuf,

    pub romm_url: Option<String>,
    pub romm_token: Option<String>,
    pub import_romm_on_startup: bool,
    pub portmaster_store: Option<PortMasterConfig>,
    pub scan_on_startup: bool,
    pub library_root: Option<std::path::PathBuf>,
    pub clock_twelve_hour: bool,

    pub retroarch: EffectiveRetroArchConfig,
    pub portmaster: RuntimePortMasterConfig,
    pub platforms: HashMap<String, PlatformRuntimeConfig>,
}

/// A process-wide, reloadable configuration snapshot.
///
/// Readers clone a snapshot before starting work, so no lock is held across
/// filesystem, network, or UI operations. Reloads replace the snapshot only
/// after the edited TOML has been validated and atomically written.
#[derive(Clone, Debug)]
pub(crate) struct ConfigHandle(Arc<RwLock<Config>>);

impl ConfigHandle {
    pub(crate) fn snapshot(&self) -> Config {
        self.0
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn replace(&self, config: Config) {
        *self
            .0
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = config;
    }
}

static SHARED_CONFIG: OnceLock<ConfigHandle> = OnceLock::new();

/// Returns the process-wide configuration, loading it on first use.
pub(crate) fn shared() -> ConfigHandle {
    SHARED_CONFIG
        .get_or_init(|| ConfigHandle(Arc::new(RwLock::new(Config::from_env()))))
        .clone()
}

/// Reloads every configuration source and publishes the new snapshot to all
/// future readers.
pub(crate) fn reload() {
    shared().replace(Config::from_env());
}

impl Config {
    pub fn from_env() -> Self {
        if let Err(error) = ensure_config_file() {
            warn!(%error, "could not write default config file; continuing with defaults");
        }
        let figment = merge_config_files(Figment::new(), config_sources());
        // Env vars merge over the file as key-path tuples: `(key, value)`
        // with a dotted path like "library.romm.url" nests like the TOML.
        // The bindings are generated from `#[template(env = "..")]`,
        // clap-derive style, so they cannot disagree with the template.
        Self::from_figment(apply_env(figment))
    }

    fn from_figment(figment: Figment) -> Self {
        let file: FileConfig = match figment.extract() {
            Ok(file) => file,
            Err(error) => {
                warn!(%error, "ignoring invalid config; using defaults");
                FileConfig::default()
            }
        };
        let romm = &file.library.romm;
        let portmaster = &file.library.portmaster;

        let romm_enabled = romm.enable.unwrap_or(false);
        let romm_url = if romm_enabled { romm.url.clone() } else { None };
        let romm_token = if romm_enabled {
            romm.token.clone()
        } else {
            None
        };
        let import_romm_on_startup = romm.import_on_startup.unwrap_or(false);
        let scan_on_startup = file.library.local.scan_on_startup.unwrap_or(true);
        let clock_twelve_hour = file.general.time_date.twelve_hour.unwrap_or(false);
        if !scan_on_startup {
            tracing::info!(
                "local library scan disabled at startup by config or MARINA_SCAN_ON_STARTUP=false"
            );
        }
        if !romm_enabled {
            tracing::info!(
                "RomM backend disabled by default; enable it with [library.romm] enable = true"
            );
        } else if romm_url.is_none() {
            warn!("[library.romm] url not set — relative cover paths will not resolve");
        }

        let default_library_root = Some(std::path::PathBuf::from("/var/games/library"));
        let default_storage_uri = dirs::state_dir()
            .map(|path| {
                format!(
                    "sqlite://{}",
                    path.join("marina").join("library.db").display()
                )
            })
            .unwrap_or_else(|| "sqlite://marina.db".to_owned());

        let portmaster_config = portmaster
            .enable
            .unwrap_or(false)
            .then(|| PortMasterConfig {
                release: portmaster.release.clone(),
                ports_dir: portmaster.ports_dir.clone(),
            });

        Self {
            storage_uri: file
                .library
                .local
                .storage_uri
                .clone()
                .unwrap_or(default_storage_uri),
            store_cache_dir: file
                .library
                .local
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
                    dirs::state_dir()
                        .map(|path| path.join("marina").join("store-cache"))
                        .unwrap_or_else(|| std::path::PathBuf::from("store-cache"))
                }),

            romm_url,
            romm_token,
            import_romm_on_startup,
            portmaster_store: portmaster_config,
            scan_on_startup,
            clock_twelve_hour,
            library_root: file.library.local.root.clone().or(default_library_root),
            retroarch: EffectiveRetroArchConfig {
                binary: file.runtime.retroarch.binary.clone(),
                cores_dir: file.runtime.retroarch.cores_dir.clone(),
                extra_args: file.runtime.retroarch.extra_args.clone(),
            },
            portmaster: RuntimePortMasterConfig {
                ports_dir: file.runtime.portmaster.ports_dir.clone(),
            },
            platforms: file.runtime.retroarch.platforms.clone(),
        }
    }
}
