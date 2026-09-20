//! Local library storage connection.

use marina_store_sqlite::SqliteLibrary;
use tracing::instrument;

use crate::config::Config;

/// Opens the configured local SQLite library.
#[instrument(skip_all, fields(uri = %config.storage_uri))]
pub async fn connect(
    config: &Config,
) -> Result<SqliteLibrary, Box<dyn std::error::Error + Send + Sync>> {
    let started = std::time::Instant::now();
    tracing::info!(uri = %config.storage_uri, "opening SQLite connection");
    let path = config
        .storage_uri
        .strip_prefix("sqlite://")
        .unwrap_or(config.storage_uri.as_str());
    let library = SqliteLibrary::open(path)?;
    tracing::info!(
        elapsed_ms = started.elapsed().as_millis() as u64,
        "SQLite connection ready"
    );
    Ok(library)
}

/// Opens the per-backend store-catalog cache directory (separate from the library).
/// Each backend gets its own `<backend_id>.db` file inside the directory.
pub fn connect_store_caches(config: &Config) -> marina_store::StoreCaches {
    marina_store::StoreCaches::new(config.store_cache_dir.clone())
}
