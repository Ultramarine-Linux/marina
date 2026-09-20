//! Application-wide runtime state.

use std::sync::Arc;

use marina_romm::RommStore;
use marina_store::StoreCaches;
use marina_store_sqlite::SqliteLibrary;

use crate::{config::Config, storage};

/// Long-lived services and configuration shared by the application.
#[derive(Debug)]
pub(crate) struct AppState {
    pub(crate) config: Config,
    pub(crate) library: SqliteLibrary,
    pub(crate) store_caches: StoreCaches,
    pub(crate) romm: Option<RommStore>,
}

/// A shareable handle to the application's runtime state.
pub(crate) type AppStateHandle = Arc<AppState>;

impl AppState {
    /// Loads configuration and initializes the services required by the UI.
    pub(crate) async fn initialize()
    -> Result<AppStateHandle, Box<dyn std::error::Error + Send + Sync>> {
        let started = std::time::Instant::now();
        tracing::info!("initializing application state");
        let config = Config::from_env();

        tracing::info!(
            uri = %config.storage_uri,
            "connecting to library store"
        );
        if let Some(root) = &config.library_root {
            tracing::info!(path = %root.display(), "configured local library root");
        } else {
            tracing::warn!("MARINA_LIBRARY_ROOT not set; local installation discovery is disabled");
        }
        tracing::info!("opening library store");
        let library = storage::connect(&config).await?;
        let store_caches = storage::connect_store_caches(&config);
        let romm = config
            .romm_url
            .clone()
            .map(|base_url| RommStore::new(base_url, config.romm_token.as_deref()));
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            "application state ready"
        );

        Ok(Arc::new(Self {
            config,
            library,
            store_caches,
            romm,
        }))
    }
}
