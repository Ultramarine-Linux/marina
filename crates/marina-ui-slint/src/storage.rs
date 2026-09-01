//! Local library storage connection.

use marina_store_sqlite::SqliteLibrary;
use tracing::instrument;

use crate::config::Config;

/// Opens the configured local SQLite library.
#[instrument(skip_all, fields(uri = %config.storage_uri))]
pub async fn connect(
    config: &Config,
) -> Result<SqliteLibrary, Box<dyn std::error::Error + Send + Sync>> {
    let path = config
        .storage_uri
        .strip_prefix("sqlite://")
        .unwrap_or(config.storage_uri.as_str());
    Ok(SqliteLibrary::open(path)?)
}
