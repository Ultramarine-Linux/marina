//! Backend-agnostic store abstraction plus per-backend SQLite caches.
//!
//! The main library database owns local items only. Remote store catalogs
//! (RomM today, other backends tomorrow) live behind [`StoreBackend`] and
//! are cached in *separate* SQLite files (one per backend) owned by
//! [`StoreCache`] / [`StoreCaches`].

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error(transparent)]
    Backend(#[from] Box<dyn std::error::Error + Send + Sync>),
    #[error("store backend '{0}' is not configured")]
    NotConfigured(String),
    #[error("entry not found")]
    NotFound,
}

impl StoreError {
    pub fn backend<E: std::error::Error + Send + Sync + 'static>(error: E) -> Self {
        Self::Backend(Box::new(error))
    }
}

/// A platform exposed by a store backend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorePlatform {
    pub slug: String,
    pub name: String,
    /// Backend-native platform id (e.g. RomM platform id), if any.
    pub backend_id: Option<String>,
    pub game_count: Option<u64>,
}

/// A lightweight catalog entry. Full backend payloads stay behind the
/// backend; `payload_json` round-trips the raw record for the cache.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreEntry {
    pub backend_id: String,
    pub entry_id: String,
    pub title: String,
    pub platform_slug: String,
    pub platform_name: Option<String>,
    pub payload_json: Option<String>,
}

/// Backend query for browsing a catalog.
#[derive(Clone, Debug, Default)]
pub struct StoreQuery {
    pub platform_slug: Option<String>,
    pub search: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

impl StoreQuery {
    pub fn all() -> Self {
        Self {
            limit: usize::MAX,
            ..Default::default()
        }
    }
}

/// A pluggable remote store (RomM, etc.).
#[async_trait]
pub trait StoreBackend: Send + Sync + std::fmt::Debug {
    /// Stable backend identifier, also used as the cache namespace.
    fn id(&self) -> &str;
    fn display_name(&self) -> &str;
    async fn list_platforms(&self) -> Result<Vec<StorePlatform>, StoreError>;
    async fn browse(&self, query: StoreQuery) -> Result<Vec<StoreEntry>, StoreError>;
    async fn get(&self, entry_id: &str) -> Result<Option<StoreEntry>, StoreError>;
}

mod cache;
pub use cache::{StoreCache, StoreCaches};
