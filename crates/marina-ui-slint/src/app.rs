//! Application-wide runtime state.

use std::sync::Arc;

use marina_store_surrealdb::{SurrealLibrary, SurrealLibraryError};

use crate::{config::Config, storage};

/// Long-lived services and configuration shared by the application.
#[derive(Debug)]
pub(crate) struct AppState {
    pub(crate) config: Config,
    pub(crate) library: SurrealLibrary,
}

/// A shareable handle to the application's runtime state.
pub(crate) type AppStateHandle = Arc<AppState>;

impl AppState {
    /// Loads configuration and initializes the services required by the UI.
    pub(crate) async fn initialize() -> Result<AppStateHandle, SurrealLibraryError> {
        let config = Config::from_env();

        tracing::info!(
            uri = %config.storage_uri,
            "connecting to library store"
        );
        let library = storage::connect(&config).await?;

        Ok(Arc::new(Self { config, library }))
    }
}
