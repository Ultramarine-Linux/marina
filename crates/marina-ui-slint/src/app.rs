//! Application-wide runtime state.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
};

use marina_library::{
    query::SearchQuery,
    read::{LibraryRead, PlatformRead},
    write::{LibraryWrite, PlatformWrite},
};
use marina_romm::RommStore;
use marina_scanner::scan;
use marina_store::{StoreBackend, StoreCaches};
use marina_store_sqlite::SqliteLibrary;

use crate::{config::Config, storage};

/// Long-lived services and configuration shared by the application.
#[derive(Debug)]
pub(crate) struct AppState {
    pub(crate) config: Config,
    pub(crate) library: SqliteLibrary,
    pub(crate) store_caches: StoreCaches,
    /// Pluggable store backends by stable id ("romm", …). Access through
    /// the [`StoreBackend`] trait; downcast via `as_any` only for
    /// backend-specific operations the trait doesn't cover.
    pub(crate) stores: HashMap<String, Arc<dyn StoreBackend>>,
}

/// A shareable handle to the application's runtime state.
pub(crate) type AppStateHandle = Arc<AppState>;

pub(crate) async fn reconcile_local_games(state: &AppStateHandle) {
    if !state.config.scan_on_startup {
        return;
    }

    let Some(root) = state.config.library_root.clone() else {
        return;
    };

    match tokio::fs::try_exists(&root).await {
        Ok(false) => {
            tracing::info!(path = %root.display(), "local library root does not exist yet; skipping scan");
        }
        Ok(true) => {
            let scan_root = root.clone();
            match tokio::task::spawn_blocking(move || scan(&scan_root)).await {
                Ok(Ok(items)) => {
                    tracing::info!(count = items.len(), "local game scan completed");
                    // The scanner is filesystem presence only: it must never
                    // overwrite enriched records (e.g. from a store
                    // install). If an entry for a scanned path already
                    // exists, the game is there — leave it exactly as is.
                    // The scanner only adds missing entries and removes
                    // ones whose files vanished from disk.
                    let scanned_paths: HashSet<String> = items
                        .iter()
                        .filter_map(|item| item.local_path.clone())
                        .collect();
                    let known_platforms: HashSet<String> = state
                        .library
                        .platforms()
                        .await
                        .unwrap_or_default()
                        .into_iter()
                        .map(|platform| platform.slug)
                        .collect();
                    for item in items {
                        let platform_slug = item.platform_slug.clone();
                        if let Some(slug) = platform_slug.as_deref() {
                            if !known_platforms.contains(slug) {
                                let _ = state
                                    .library
                                    .add_platform(marina_core::Platform::new(slug, slug))
                                    .await;
                            }
                        }
                        let existing = state
                            .library
                            .search(
                                SearchQuery::new()
                                    .platform(platform_slug.as_deref().unwrap_or_default())
                                    .limit(usize::MAX),
                            )
                            .await
                            .ok()
                            .and_then(|items| {
                                items
                                    .into_iter()
                                    .find(|candidate| candidate.local_path == item.local_path)
                            });
                        if existing.is_some() {
                            continue;
                        }
                        if let Err(error) = state.library.add(item).await {
                            tracing::error!(%error, "failed to store scanned local game");
                        }
                    }
                    // Prune entries in the scanner's domain whose files no
                    // longer exist physically. Anything outside
                    // `<root>/roms` (e.g. XDG apps) is left alone.
                    let roms_root = root.join("roms");
                    match state
                        .library
                        .search(SearchQuery::new().limit(usize::MAX))
                        .await
                    {
                        Ok(stored) => {
                            for item in stored {
                                let Some(path) = item.local_path.as_deref() else {
                                    continue;
                                };
                                if !Path::new(path).starts_with(&roms_root) {
                                    continue;
                                }
                                if !scanned_paths.contains(path) {
                                    tracing::info!(
                                        path,
                                        "local game files vanished; removing entry"
                                    );
                                    if let Err(error) = state.library.remove(&item.id).await {
                                        tracing::error!(%error, path, "failed to remove vanished game");
                                    }
                                }
                            }
                        }
                        Err(error) => tracing::error!(%error, "failed to list library for prune"),
                    }
                }
                Ok(Err(error)) => tracing::error!(%error, "local game scan failed"),
                Err(error) => tracing::error!(%error, "local game scan task failed"),
            }
        }
        Err(error) => {
            tracing::error!(%error, path = %root.display(), "could not inspect local library root")
        }
    }
}

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
        let mut stores: HashMap<String, Arc<dyn StoreBackend>> = HashMap::new();
        if let Some(base_url) = config.romm_url.clone() {
            let backend = RommStore::new(base_url, config.romm_token.as_deref());
            stores.insert(backend.id().to_owned(), Arc::new(backend));
        }
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            "application state ready"
        );

        Ok(Arc::new(Self {
            config,
            library,
            store_caches,
            stores,
        }))
    }
}
