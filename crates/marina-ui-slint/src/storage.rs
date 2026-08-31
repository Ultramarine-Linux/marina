//! Library storage connection.

use marina_store_surrealdb::{SurrealLibrary, SurrealLibraryError};
use tracing::instrument;

use crate::config::Config;

/// Connects to the configured SurrealDB endpoint, authenticating as root for
/// remote (`ws://`/`wss://`) endpoints.
#[instrument(skip_all, fields(uri = %config.storage_uri))]
pub async fn connect(config: &Config) -> Result<SurrealLibrary, SurrealLibraryError> {
    if config.storage_uri.starts_with("ws://") || config.storage_uri.starts_with("wss://") {
        SurrealLibrary::connect_with_root(
            config.storage_uri.as_str(),
            config.storage_username.clone(),
            config.storage_password.clone(),
        )
        .await
    } else {
        SurrealLibrary::connect(config.storage_uri.as_str()).await
    }
}
