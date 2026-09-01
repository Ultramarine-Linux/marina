//! Runtime configuration sourced from the environment (and `.env`).

use std::env;

use tracing::warn;

#[derive(Debug)]
pub struct Config {
    pub storage_uri: String,

    pub romm_url: Option<String>,
    pub romm_token: Option<String>,
    pub import_romm_on_startup: bool,
    pub library_root: Option<std::path::PathBuf>,
}

impl Config {
    pub fn from_env() -> Self {
        let romm_url = env::var("ROMM_URL").ok();
        let romm_token = env::var("ROMM_TOKEN").ok();
        let import_romm_on_startup = env::var("MARINA_IMPORT_ROMM_ON_STARTUP")
            .map(|value| {
                !matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
            .unwrap_or(true);
        if romm_url.is_none() {
            warn!("ROMM_URL not set — relative cover paths will not resolve");
        }

        Self {
            storage_uri: env::var("MARINA_STORAGE_URI")
                .unwrap_or_else(|_| "sqlite://marina.db".to_owned()),

            romm_url,
            romm_token,
            import_romm_on_startup,
            library_root: env::var_os("MARINA_LIBRARY_ROOT").map(std::path::PathBuf::from),
        }
    }
}
