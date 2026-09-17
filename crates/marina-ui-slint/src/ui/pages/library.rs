//! Shelf view-model and layout sizing.

use std::{
    collections::BTreeMap,
    rc::Rc,
    sync::{Arc, Mutex},
};

use marina_core::LibraryItemId;
use marina_library::{
    error::LibraryError,
    query::SearchQuery,
    read::{LibraryRead, PlatformRead},
};
use marina_store_sqlite::SqliteLibrary;
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};
use tracing::error;

use crate::covers::{self, CoverSource};
use crate::{
    GameCardData, MainWindow, PlatformCardData, PlatformCardMetadata, app, game_cards,
    platform_asset_path, preview_details,
};

pub(crate) fn install(
    window: &MainWindow,
    library_state: &Arc<Mutex<Option<app::AppStateHandle>>>,
    source_store: &Arc<Mutex<Vec<CoverSource>>>,
    loader: &Rc<std::cell::RefCell<covers::CoverLoader>>,
) {
    let detail_state = library_state.clone();
    let detail_window = window.as_weak();
    let selected_loader = loader.clone();
    window.on_game_selected(move |id, index| {
        // Load the selected row's cover on demand. The detail request below
        // supplies the rest of the metadata.
        let cover_height = detail_window
            .upgrade()
            .map(|window| window.get_shelf_cover_height())
            .unwrap_or(200.0);
        selected_loader
            .borrow_mut()
            .update(index as f32 * (cover_height + 16.0), 240.0);
        let state = detail_state
            .lock()
            .expect("library state lock poisoned")
            .clone();
        let Some(state) = state else {
            return;
        };
        let Ok(item_id) = LibraryItemId::parse(id.as_str()).ok_or(()) else {
            return;
        };
        let window = detail_window.clone();
        tokio::spawn(async move {
            match state.library.get(&item_id).await {
                Ok(Some(item)) => {
                    let details = preview_details(item);
                    let _ = window.upgrade_in_event_loop(move |window| {
                        window.set_game_details(details);
                        window.set_game_details_loading(false);
                    });
                }
                Ok(None) | Err(_) => {
                    let _ = window.upgrade_in_event_loop(|window| {
                        window.set_game_details_loading(false);
                    });
                }
            }
        });
    });

    let open_state = library_state.clone();
    let open_window = window.as_weak();
    window.on_game_opened(move |id| {
        let Some(window) = open_window.upgrade() else {
            return;
        };
        let games = window.get_games();
        let Some(game) = (0..games.row_count())
            .filter_map(|index| games.row_data(index))
            .find(|game| game.id == id)
        else {
            return;
        };
        window.set_selected_game(game);
        window.set_game_page_visible(true);
        window.set_game_details_loading(true);

        let state = open_state
            .lock()
            .expect("library state lock poisoned")
            .clone();
        let Some(state) = state else {
            return;
        };
        let Ok(item_id) = LibraryItemId::parse(id.as_str()).ok_or(()) else {
            return;
        };
        let detail_window = window.as_weak();
        tokio::spawn(async move {
            match state.library.get(&item_id).await {
                Ok(Some(item)) => {
                    let detail_cover = item
                        .assets
                        .iter()
                        .filter_map(|asset| {
                            let priority = match asset.kind {
                                marina_core::LibraryAssetKind::CoverLarge => 0,
                                marina_core::LibraryAssetKind::CoverSmall => 1,
                                _ => return None,
                            };
                            asset.local_path.clone().map(|path| (priority, path))
                        })
                        .min_by_key(|(priority, _)| *priority)
                        .map(|(_, path)| path);
                    let details = preview_details(item);
                    let _ = detail_window.upgrade_in_event_loop(move |window| {
                        if let Some(path) = detail_cover.and_then(|path| {
                            slint::Image::load_from_path(std::path::Path::new(&path)).ok()
                        }) {
                            window.set_selected_game(GameCardData {
                                cover: path,
                                ..window.get_selected_game()
                            });
                        }
                        window.set_game_details(details);
                        window.set_game_details_loading(false);
                    });
                }
                Ok(None) | Err(_) => {
                    let _ = detail_window.upgrade_in_event_loop(|window| {
                        window.set_game_details_loading(false);
                    });
                }
            }
        });
    });

    let query_state = library_state.clone();
    let query_sources = source_store.clone();
    let query_window = window.as_weak();
    window.on_platform_query(move |platform_slug| {
        let state = query_state
            .lock()
            .expect("library state lock poisoned")
            .clone();
        let Some(state) = state else {
            return;
        };
        let base_url = state.config.romm_url.clone();
        let sources = query_sources.clone();
        let window = query_window.clone();
        tokio::spawn(async move {
            let loaded =
                load_platform_games(&state.library, base_url.as_deref(), platform_slug.as_str())
                    .await;
            let (metadata, cover_sources) = match loaded {
                Ok(loaded) => loaded,
                Err(error) => {
                    error!(%error, platform = %platform_slug, "platform games loading failed");
                    let _ = window.upgrade_in_event_loop(|window| {
                        window.set_library_loading(false);
                    });
                    return;
                }
            };
            *sources.lock().expect("cover source state poisoned") = cover_sources;
            let _ = window.upgrade_in_event_loop(move |window| {
                let model = window.get_platform_games();
                let model = model
                    .as_any()
                    .downcast_ref::<VecModel<GameCardData>>()
                    .expect("platform game model should be a VecModel");
                let games = game_cards(metadata);
                model.set_vec(games);
                window.set_library_loading(false);
                if window.get_active_tab() == 1 {
                    // Replacing the platform model invalidates lazy-cover
                    // residency; immediately request the visible cards.
                    window.invoke_cover_context_changed(1);
                }
            });
        });
    });

    let library_refresh_state = library_state.clone();
    let library_refresh_window = window.as_weak();
    window.on_library_entered(move || {
        let Some(state) = library_refresh_state
            .lock()
            .ok()
            .and_then(|state| state.clone())
        else {
            return;
        };
        let window = library_refresh_window.clone();
        tokio::spawn(async move {
            let metadata = match load_games(&state.library, state.config.romm_url.as_deref()).await
            {
                Ok((metadata, _)) => metadata,
                Err(error) => {
                    error!(%error, "library game metadata refresh failed");
                    return;
                }
            };
            let platforms = match state.library.platforms().await {
                Ok(platforms) => platforms,
                Err(error) => {
                    error!(%error, "library platform refresh failed");
                    return;
                }
            };
            let mut platform_names = platforms
                .into_iter()
                .map(|platform| (platform.slug, platform.name))
                .collect::<BTreeMap<_, _>>();
            for game in metadata {
                if game.platform.is_empty() {
                    continue;
                }
                // A newly added game may precede an explicit platform record.
                // Its stable platform slug is still sufficient to browse it.
                platform_names
                    .entry(game.platform.clone())
                    .or_insert(game.platform);
            }

            let mut platform_counts = BTreeMap::<String, usize>::new();
            for slug in platform_names.keys() {
                let count = match state.library.count(SearchQuery::new().platform(slug)).await {
                    Ok(count) => count,
                    Err(error) => {
                        error!(%error, platform = %slug, "library platform count refresh failed");
                        return;
                    }
                };
                platform_counts.insert(slug.clone(), count);
            }

            let icon_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("ui/assets/platforms/systematic");
            let cards = platform_names
                .into_iter()
                .map(|(slug, name)| {
                    let game_count = platform_counts.get(&slug).copied().unwrap_or_default();
                    PlatformCardMetadata {
                        icon_path: platform_asset_path(&icon_root, &slug),
                        slug,
                        name,
                        game_count: format!(
                            "{} {}",
                            game_count,
                            if game_count == 1 { "game" } else { "games" }
                        ),
                    }
                })
                .collect::<Vec<_>>();
            let _ = window.upgrade_in_event_loop(move |window| {
                let cards = cards
                    .into_iter()
                    .map(|platform| PlatformCardData {
                        slug: SharedString::from(platform.slug),
                        name: SharedString::from(platform.name),
                        game_count: SharedString::from(platform.game_count),
                        icon: platform
                            .icon_path
                            .and_then(|path| {
                                Image::load_from_path(std::path::Path::new(&path)).ok()
                            })
                            .unwrap_or_default(),
                    })
                    .collect::<Vec<_>>();
                window.set_platforms(ModelRc::from(std::rc::Rc::new(VecModel::from(cards))));
            });
        });
    });
}

/// Sendable metadata that can cross from the Tokio task to the UI event loop.
/// The Slint image is created only after crossing onto the UI thread.
#[derive(Clone, Debug)]
pub struct GameMetadata {
    pub id: String,
    pub title: String,
    pub platform: String,
}

pub async fn load_games(
    library: &SqliteLibrary,
    romm_base_url: Option<&str>,
) -> Result<(Vec<GameMetadata>, Vec<CoverSource>), LibraryError> {
    let started = std::time::Instant::now();
    tracing::info!("loading home game metadata");
    let result = covers::load_games_metadata(library, romm_base_url).await;
    tracing::info!(
        elapsed_ms = started.elapsed().as_millis() as u64,
        success = result.is_ok(),
        "home game metadata request finished"
    );
    result
}

/// Loads one alphabetized page of games for a platform.
///
/// Pagination is deliberately applied in the backend query so the UI never
/// needs to hold the complete platform library in memory.
pub async fn load_platform_games(
    library: &SqliteLibrary,
    romm_base_url: Option<&str>,
    platform_slug: &str,
) -> Result<(Vec<GameMetadata>, Vec<CoverSource>), LibraryError> {
    let mut items = library
        .search_cards(SearchQuery::new().platform(platform_slug).limit(usize::MAX))
        .await?;
    items.sort_by(|left, right| left.title.to_lowercase().cmp(&right.title.to_lowercase()));

    Ok(items
        .into_iter()
        .map(|item| {
            let source = covers::source_for(
                item.cover.as_deref(),
                item.cover_small_local_path
                    .as_deref()
                    .or(item.cover_large_local_path.as_deref()),
                romm_base_url,
            );
            let metadata = GameMetadata {
                id: item.id.to_string(),
                title: item.title,
                platform: item.platform_name.unwrap_or_else(|| "Unknown".into()),
            };
            (metadata, source)
        })
        .unzip())
}
