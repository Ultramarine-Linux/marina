//! Runtime configuration sourced from the environment (and `.env`).

use std::env;

use tracing::warn;

#[derive(Debug)]
pub struct Config {
    pub storage_uri: String,

    pub romm_url: Option<String>,
    pub romm_token: Option<String>,
    pub import_romm_on_startup: bool,
    pub scan_on_startup: bool,
    pub library_root: Option<std::path::PathBuf>,
}

impl Config {
    pub fn from_env() -> Self {
        let romm_enabled = env::var("MARINA_ENABLE_ROMM")
            .map(|value| {
                !matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
            .unwrap_or(false);
        let romm_url = romm_enabled.then(|| env::var("ROMM_URL").ok()).flatten();
        let romm_token = romm_enabled.then(|| env::var("ROMM_TOKEN").ok()).flatten();
        let import_romm_on_startup = env::var("MARINA_IMPORT_ROMM_ON_STARTUP")
            .map(|value| {
                !matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
            .unwrap_or(false);
        let scan_on_startup = env::var("MARINA_SCAN_ON_STARTUP")
            .map(|value| {
                !matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
            .unwrap_or(true);
        if !scan_on_startup {
            tracing::info!(
                "local library scan disabled at startup by MARINA_SCAN_ON_STARTUP=false"
            );
        }
        if !romm_enabled {
            tracing::info!(
                "RomM backend disabled by default; set MARINA_ENABLE_ROMM=true to enable"
            );
        } else if romm_url.is_none() {
            warn!("ROMM_URL not set — relative cover paths will not resolve");
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
            storage_uri: env::var("MARINA_STORAGE_URI").unwrap_or(default_storage_uri),

            romm_url,
            romm_token,
            import_romm_on_startup,
            scan_on_startup,
            library_root: env::var_os("MARINA_LIBRARY_ROOT")
                .map(std::path::PathBuf::from)
                .or(default_library_root),
        }
    }
}

fn state_dir() -> Option<std::path::PathBuf> {
    env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".local").join("state")))
}
