use marina_library::{LibraryError, Platform, PlatformWrite};
use surrealdb::{
    Surreal,
    engine::{any::Any, any::IntoEndpoint},
    opt::auth::Root,
};
use thiserror::Error;

pub(crate) const ITEMS_TABLE: &str = "library";
pub(crate) const PLATFORMS_TABLE: &str = "platform";

#[derive(Debug, Error)]
pub enum SurrealLibraryError {
    #[error("SurrealDB operation failed: {0}")]
    Database(#[from] surrealdb::Error),

    #[error("SurrealDB schema synchronization failed: {0}")]
    Schema(#[from] surrealkit::anyhow::Error),

    #[error("stored library item has an invalid id: {0}")]
    InvalidItemId(String),

    #[error("stored library item has a non-string platform link")]
    InvalidPlatformLink,
}

/// A Marina library backed by an embedded or remote SurrealDB endpoint.
#[derive(Debug)]
pub struct SurrealLibrary {
    pub(crate) db: Surreal<Any>,
}

impl SurrealLibrary {
    /// Connects to an endpoint and selects Marina's namespace and database.
    pub async fn connect(uri: impl IntoEndpoint) -> Result<Self, SurrealLibraryError> {
        let db = surrealdb::engine::any::connect(uri).await?;
        select_database(&db).await?;
        Ok(Self { db })
    }

    /// Connects and authenticates as a SurrealDB root user before selecting
    /// Marina's namespace and database.
    pub async fn connect_with_root(
        uri: impl IntoEndpoint,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Result<Self, SurrealLibraryError> {
        let db = surrealdb::engine::any::connect(uri).await?;
        db.signin(Root {
            username: username.into(),
            password: password.into(),
        })
        .await?;
        select_database(&db).await?;
        Ok(Self { db })
    }

    /// Adds or replaces a batch of platform records.
    pub async fn add_platforms(
        &self,
        platforms: impl IntoIterator<Item = Platform>,
    ) -> Result<Vec<Platform>, LibraryError> {
        let mut imported = Vec::new();
        for platform in platforms {
            imported.push(PlatformWrite::add_platform(self, platform).await?);
        }
        Ok(imported)
    }
}

async fn select_database(db: &Surreal<Any>) -> Result<(), SurrealLibraryError> {
    db.use_ns("marina").use_db("library").await?;
    tracing::info!("selected database, running schema sync");
    surrealkit::Sync::embedded(crate::embedded_schema::SCHEMA)
        .prune(false)
        .run(db)
        .await?;
    Ok(())
}
