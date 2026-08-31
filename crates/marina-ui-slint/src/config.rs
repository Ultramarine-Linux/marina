//! Runtime configuration sourced from the environment (and `.env`).

use std::env;

use tracing::warn;

#[derive(Debug)]
pub struct Config {
    pub storage_uri: String,
    pub storage_username: String,
    pub storage_password: String,
    pub romm_url: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        let romm_url = env::var("ROMM_URL").ok();
        if romm_url.is_none() {
            warn!("ROMM_URL not set — relative cover paths will not resolve");
        }

        Self {
            storage_uri: env::var("MARINA_STORAGE_URI").unwrap_or_else(|_| "mem://".to_owned()),
            storage_username: env::var("MARINA_STORAGE_USERNAME")
                .unwrap_or_else(|_| "root".to_owned()),
            storage_password: env::var("MARINA_STORAGE_PASSWORD")
                .unwrap_or_else(|_| "root".to_owned()),
            romm_url,
        }
    }
}
