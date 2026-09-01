//! Application-wide runtime state.

use std::sync::Arc;

use marina_store_sqlite::SqliteLibrary;

use crate::{config::Config, storage};

/// Long-lived services and configuration shared by the application.
#[derive(Debug)]
pub(crate) struct AppState {
    pub(crate) config: Config,
    pub(crate) library: SqliteLibrary,
}

/// A shareable handle to the application's runtime state.
pub(crate) type AppStateHandle = Arc<AppState>;

impl AppState {
    /// Loads configuration and initializes the services required by the UI.
    pub(crate) async fn initialize()
    -> Result<AppStateHandle, Box<dyn std::error::Error + Send + Sync>> {
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
        let library = storage::connect(&config).await?;

        Ok(Arc::new(Self { config, library }))
    }
}
