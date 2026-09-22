//! Shelf view-model and layout sizing.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use marina_core::LibraryItemId;
use marina_library::{
    error::LibraryError,
    query::SearchQuery,
    read::{LibraryRead, PlatformRead},
};
use marina_store_sqlite::SqliteLibrary;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use tracing::{debug, error, warn};

use crate::covers::{self, CoverSource};
use crate::image;
use crate::{
    GameCardData, GameState, HomeState, LibraryState, MainWindow, PlatformCardData,
    PlatformCardMetadata, ShellPage, ShellState, app, game_cards, platform_asset_path,
    preview_details,
};

pub(crate) fn install(
    window: &MainWindow,
    library_state: &Arc<Mutex<Option<app::AppStateHandle>>>,
) {
    let detail_state = library_state.clone();
    let detail_window = window.as_weak();

    window
        .global::<LibraryState>()
        .on_game_selected(move |id, index| {
                    debug!(game_id = %id, index, "library preview selection requested");
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
            // Clear the pane first: the previous game's details must not
            // linger while the new fetch is in flight.
            let _ = window.upgrade_in_event_loop(|window| {
                let games = window.global::<LibraryState>().get_games();
                for index in 0..games.row_count() {
                    if let Some(mut game) = games.row_data(index)
                        && game.cover.size().width > 0
                    {
                        game.cover = slint::Image::default();
                        game.cover_ratio = 1.0;
                        games.set_row_data(index, game);
                    }
                }
                window
                    .global::<GameState>()
                    .set_details(crate::empty_preview_details());
                window
                    .global::<GameState>()
                    .set_tags(crate::string_model(Vec::new()));
                window.global::<GameState>().set_details_loading(true);
            });
            tokio::spawn(async move {
                match state.library.get(&item_id).await {
                    Ok(Some(item)) => {
                        let cover_path = item
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
                        debug!(game_id = %id, ?cover_path, assets = item.assets.len(), "library preview item loaded");
                        let decoded = if let Some(path) = cover_path {
                            let source = covers::source_for(None, Some(&path), None);
                            match image::load_scaled(
                                &image::ImageSource::from(&source),
                                "library-preview",
                                image::PREVIEW_MAX_DIMENSION,
                            )
                            .await
                            {
                                Some(decoded) => Some(decoded),
                                None => {
                                    warn!(game_id = %id, %path, "library preview cover produced no bytes");
                                    None
                                }
                            }
                        } else {
                            warn!(game_id = %id, "library preview item has no local cover asset");
                            None
                        };
                        let tags = item.tags.clone();
                        let details = preview_details(item);
                        let _ = window.upgrade_in_event_loop(move |window| {
                            let games = window.global::<LibraryState>().get_games();
                            let selected_index = window.global::<LibraryState>().get_selected_game_index().max(0) as usize;
                            if selected_index != index as usize
                                || games.row_data(selected_index).is_none_or(|game| game.id != id)
                            {
                                debug!(game_id = %id, index, selected_index, "discarding stale library preview");
                                return;
                            }
                            if let Some(decoded) = decoded
                                && let Some(mut game) = games.row_data(selected_index)
                            {
                                let (image, ratio) = image::into_slint_image(decoded);
                                game.cover = image;
                                game.cover_ratio = ratio;
                                games.set_row_data(selected_index, game);
                                debug!(game_id = %id, index, "library preview cover applied");
                            }
                            window.global::<GameState>().set_details(details);
                            window.global::<GameState>().set_tags(crate::string_model(tags));
                            window.global::<GameState>().set_details_loading(false);
                        });
                    }
                    Ok(None) => {
                        warn!(game_id = %id, "library preview item missing");
                        let _ = window.upgrade_in_event_loop(|window| {
                            window.global::<GameState>().set_details_loading(false);
                        });
                    }
                    Err(error) => {
                        error!(%error, game_id = %id, "library preview lookup failed");
                        let _ = window.upgrade_in_event_loop(|window| {
                            window.global::<GameState>().set_details_loading(false);
                        });
                    }
                }
            });
        });

    let open_window = window.as_weak();
    window
        .global::<LibraryState>()
        .on_game_opened(move |id, index| {
            let Some(window) = open_window.upgrade() else {
                return;
            };
            let games = window.global::<LibraryState>().get_games();
            let Some(game) = games.row_data(index.max(0) as usize) else {
                return;
            };
            if game.id != id {
                return;
            }
            window.global::<GameState>().set_selected_game(game);
            window.global::<GameState>().set_details_loading(true);
            window
                .global::<GameState>()
                .set_details(crate::empty_preview_details());
            window
                .global::<GameState>()
                .set_tags(crate::string_model(Vec::new()));
            crate::ui::nav::publish(&window);

            // Defer the route change until the list click callback has unwound.
            let route_window = window.as_weak();
            let _ = route_window.upgrade_in_event_loop(|window| {
                window
                    .global::<ShellState>()
                    .set_page(ShellPage::GameDetails);
                // Publish again now that the route matches: the title crumb
                // only appends on the details route.
                crate::ui::nav::publish(&window);
            });
        });

    let open_state = library_state.clone();
    let open_window = window.as_weak();
    window.global::<HomeState>().on_game_opened(move |id| {
        let Some(window) = open_window.upgrade() else {
            return;
        };
        // Both home shelves forward here, but they are backed by different
        // models (recently-added vs recently-played): the pressed card may
        // live in either one.
        let home = window.global::<HomeState>();
        let find_game = |model: ModelRc<GameCardData>| {
            (0..model.row_count())
                .filter_map(move |index| model.row_data(index))
                .find(|game| game.id == id)
        };
        let Some(game) = find_game(home.get_games()).or_else(|| find_game(home.get_played_games()))
        else {
            warn!(game_id = %id, "home game-opened for a card in neither home model");
            return;
        };
        window.global::<GameState>().set_selected_game(game);
        window.global::<GameState>().set_details_loading(true);
        window
            .global::<GameState>()
            .set_details(crate::empty_preview_details());
        window
            .global::<GameState>()
            .set_tags(crate::string_model(Vec::new()));
        crate::ui::nav::publish(&window);

        // Changing routes synchronously from the card's click callback deletes
        // the callback's parent item while Slint is still dispatching the
        // pointer event (Slint issue #6426). Defer the destructive tree change
        // until the callback has unwound.
        let route_window = window.as_weak();
        let _ = route_window.upgrade_in_event_loop(|window| {
            crate::ui::nav::leave_home(&window);
            window
                .global::<ShellState>()
                .set_page(ShellPage::GameDetails);
            // Publish again now that the route matches: the title crumb
            // only appends on the details route.
            crate::ui::nav::publish(&window);
        });

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
                    let detail_image = if let Some(path) = detail_cover {
                        let source = covers::source_for(None, Some(&path), None);
                        match image::load_scaled(
                            &image::ImageSource::from(&source),
                            "library-detail",
                            image::PREVIEW_MAX_DIMENSION,
                        )
                        .await
                        {
                            Some(decoded) => Some(decoded),
                            None => None,
                        }
                    } else {
                        None
                    };
                    let tags = item.tags.clone();
                    let details = preview_details(item);
                    let _ = detail_window.upgrade_in_event_loop(move |window| {
                        let game_state = window.global::<GameState>();
                        if game_state.get_selected_game().id != id
                            || window.global::<ShellState>().get_page() != ShellPage::GameDetails
                        {
                            debug!(game_id = %id, "discarding stale game details");
                            return;
                        }
                        if let Some(decoded) = detail_image {
                            let (image, _) = image::into_slint_image(decoded);
                            window
                                .global::<GameState>()
                                .set_selected_game(GameCardData {
                                    cover: image,
                                    ..window.global::<GameState>().get_selected_game()
                                });
                        }
                        window.global::<GameState>().set_details(details);
                        window
                            .global::<GameState>()
                            .set_tags(crate::string_model(tags));
                        window.global::<GameState>().set_details_loading(false);
                    });
                }
                Ok(None) | Err(_) => {
                    let _ = detail_window.upgrade_in_event_loop(|window| {
                        window.global::<GameState>().set_details_loading(false);
                    });
                }
            }
        });
    });

    let query_state = library_state.clone();

    let query_window = window.as_weak();
    window
        .global::<LibraryState>()
        .on_platform_query(move |platform_slug| {
            if let Some(window) = query_window.upgrade() {
                crate::ui::nav::drill_library(&window, platform_slug.as_str());
            }
            let state = query_state
                .lock()
                .expect("library state lock poisoned")
                .clone();
            let Some(state) = state else {
                return;
            };
            let base_url = state.config.romm_url.clone();
            let window = query_window.clone();
            tokio::spawn(async move {
                let loaded = load_platform_games(
                    &state.library,
                    base_url.as_deref(),
                    platform_slug.as_str(),
                )
                .await;
                let (metadata, _) = match loaded {
                    Ok(loaded) => loaded,
                    Err(error) => {
                        error!(%error, platform = %platform_slug, "platform games loading failed");
                        let _ = window.upgrade_in_event_loop(|window| {
                            window.global::<LibraryState>().set_loading(false);
                        });
                        return;
                    }
                };
                let _ = window.upgrade_in_event_loop(move |window| {
                    let model = window.global::<LibraryState>().get_games();
                    let model = model
                        .as_any()
                        .downcast_ref::<VecModel<GameCardData>>()
                        .expect("platform game model should be a VecModel");
                    let games = game_cards(metadata);
                    let first_id = games.first().map(|game| game.id.clone());
                    model.set_vec(games);
                    window.global::<LibraryState>().set_selected_game_index(0);
                    window.global::<LibraryState>().set_loading(false);
                    if let Some(first_id) = first_id {
                        window.global::<GameState>().set_details_loading(true);
                        window
                            .global::<LibraryState>()
                            .invoke_game_selected(first_id, 0);
                    }
                });
            });
        });

    let library_refresh_state = library_state.clone();
    let library_refresh_window = window.as_weak();
    window.global::<LibraryState>().on_entered(move || {
        let loading_window = library_refresh_window.clone();
        let _ = loading_window.upgrade_in_event_loop(|window| {
            let library = window.global::<LibraryState>();
            if library.get_platforms().row_count() == 0 {
                library.set_loading(true);
            }
        });
        let Some(state) = library_refresh_state
            .lock()
            .ok()
            .and_then(|state| state.clone())
        else {
            let _ = loading_window.upgrade_in_event_loop(|window| {
                window.global::<LibraryState>().set_loading(false);
            });
            return;
        };
        let window = library_refresh_window.clone();
        tokio::spawn(async move {
            if let Err(error) = refresh_platform_cards(&state.library, &window).await {
                error!(%error, "cached library platform refresh failed");
            }

            let discovered_apps = tokio::task::spawn_blocking(marina_apps::discover)
                .await
                .unwrap_or_default();
            if let Err(error) = marina_apps::sync(&state.library, discovered_apps).await {
                error!(%error, "library Apps rescan failed");
            }

            if let Err(error) = refresh_platform_cards(&state.library, &window).await {
                error!(%error, "reconciled library platform refresh failed");
            }
        });
    });
}

pub(crate) async fn refresh_platform_cards(
    library: &SqliteLibrary,
    window: &slint::Weak<MainWindow>,
) -> Result<(), String> {
    let platforms = match library.platforms().await {
        Ok(platforms) => platforms,
        Err(error) => {
            let _ = window.upgrade_in_event_loop(|window| {
                window.global::<LibraryState>().set_loading(false);
            });
            return Err(error.to_string());
        }
    };
    let platform_names = platforms
        .into_iter()
        .map(|platform| (platform.slug, platform.name))
        .collect::<BTreeMap<_, _>>();

    let initial_platforms = platform_names
        .iter()
        .map(|(slug, name)| (slug.clone(), name.clone()))
        .collect::<Vec<_>>();
    let _ = window.upgrade_in_event_loop(move |window| {
        let initial_cards = initial_platforms
            .into_iter()
            .map(|(slug, name)| PlatformCardData {
                slug: SharedString::from(slug),
                name: SharedString::from(name),
                game_count: SharedString::from("Loading…"),
                icon: Default::default(),
            })
            .collect::<Vec<_>>();
        let library = window.global::<LibraryState>();
        if library.get_platforms().row_count() == 0 {
            library.set_platforms(ModelRc::from(std::rc::Rc::new(VecModel::from(
                initial_cards,
            ))));
        }
        library.set_loading(false);
    });

    let mut platform_counts = BTreeMap::<String, usize>::new();
    for slug in platform_names.keys() {
        let count = library
            .count(SearchQuery::new().platform(slug))
            .await
            .map_err(|error| format!("{slug}: {error}"))?;
        platform_counts.insert(slug.clone(), count);
    }

    let cards = platform_names
        .into_iter()
        .map(|(slug, name)| {
            let game_count = platform_counts.get(&slug).copied().unwrap_or_default();
            PlatformCardMetadata {
                icon_path: None,
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
        let icon_slugs = cards
            .iter()
            .map(|platform| platform.slug.clone())
            .collect::<Vec<_>>();
        let cards = cards
            .into_iter()
            .map(|platform| PlatformCardData {
                slug: SharedString::from(platform.slug),
                name: SharedString::from(platform.name),
                game_count: SharedString::from(platform.game_count),
                icon: Default::default(),
            })
            .collect::<Vec<_>>();
        let model = std::rc::Rc::new(VecModel::from(cards));
        window
            .global::<LibraryState>()
            .set_platforms(ModelRc::from(model.clone()));
        window.global::<LibraryState>().set_loading(false);

        for (index, slug) in icon_slugs.into_iter().enumerate() {
            let icon_window = window.as_weak();
            tokio::spawn(async move {
                let Some(path) = platform_asset_path(
                    &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("ui/assets/platforms/systematic"),
                    &slug,
                ) else {
                    return;
                };
                let Some(decoded) = image::load_path_scaled(path, "platform-icon", 256).await
                else {
                    return;
                };
                let _ = icon_window.upgrade_in_event_loop(move |window| {
                    let model = window.global::<LibraryState>().get_platforms();
                    if let Some(model) = model.as_any().downcast_ref::<VecModel<PlatformCardData>>()
                        && let Some(mut platform) = model.row_data(index)
                        && platform.slug == slug
                    {
                        platform.icon = image::into_slint_image(decoded).0;
                        model.set_row_data(index, platform);
                    }
                });
            });
        }
    });

    Ok(())
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
