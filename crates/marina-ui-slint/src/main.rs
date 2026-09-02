use std::{
    cell::Cell,
    collections::BTreeMap,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

use marina_core::LibraryItemId;
use marina_library::{
    query::SearchQuery,
    read::{LibraryRead, PlatformRead},
    write::{LibraryWrite, PlatformWrite},
};
use marina_romm::{PlatformQuery, RomQuery};
use marina_scanner::scan;
use serde::Serialize;
use slint::{Image, Model, ModelRc, SharedString, VecModel};
use tracing::{debug, error, info};
use tracing_subscriber::EnvFilter;

slint::include_modules!();

mod app;
mod cache;
mod config;
mod covers;
mod romm_auth;
mod shelf;
mod storage;

const TOAST_DURATION: Duration = Duration::from_secs(4);
const TOAST_DISMISS_ANIMATION: Duration = Duration::from_millis(250);

fn configure_toasts(window: &MainWindow) {
    let items = Rc::new(VecModel::from(Vec::<ToastItem>::new()));
    let toast_queue = window.global::<ToastQueue>();
    toast_queue.set_items(ModelRc::from(items.clone()));

    let dismiss_items = items.clone();
    toast_queue.on_dismiss(move |id| {
        dismiss_toast(dismiss_items.clone(), id);
    });

    let next_id = Rc::new(Cell::new(0_i32));
    toast_queue.on_show(move |text, variant| {
        let id = next_id.get();
        next_id.set(id.saturating_add(1));
        items.push(ToastItem {
            id,
            text,
            variant,
            dismissed: false,
        });

        let auto_dismiss_items = items.clone();
        slint::Timer::single_shot(TOAST_DURATION, move || {
            dismiss_toast(auto_dismiss_items, id);
        });
    });
}

fn dismiss_toast(items: Rc<VecModel<ToastItem>>, id: i32) {
    let Some(index) = (0..items.row_count())
        .find(|&index| items.row_data(index).is_some_and(|item| item.id == id))
    else {
        return;
    };
    let Some(mut item) = items.row_data(index) else {
        return;
    };
    if item.dismissed {
        return;
    }

    item.dismissed = true;
    items.set_row_data(index, item);

    slint::Timer::single_shot(TOAST_DISMISS_ANIMATION, move || {
        if let Some(index) = (0..items.row_count())
            .find(|&index| items.row_data(index).is_some_and(|item| item.id == id))
        {
            items.remove(index);
        }
    });
}

#[tokio::main]
async fn main() -> Result<(), slint::PlatformError> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    dotenvy::dotenv().ok();

    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--export-ui-fixture") {
        let path = args.get(2).map(String::as_str).unwrap_or("ui-fixture.json");
        if let Err(error) = export_ui_fixture(path).await {
            eprintln!("failed to export UI fixture: {error}");
        }
        return Ok(());
    }

    let (username, display_name, initials) = profile_identity();
    let window = MainWindow::new()?;
    configure_toasts(&window);
    window.set_profile_name(SharedString::from(display_name));
    window.set_profile_username(SharedString::from(username.clone()));
    window.set_profile_initials(SharedString::from(initials));
    window.set_profile_image(Image::default());

    let profile_window = window.as_weak();
    tokio::spawn(async move {
        let icon_path = format!("/var/lib/AccountsService/icons/{username}");
        let Some(bytes) = tokio::fs::read(icon_path).await.ok() else {
            return;
        };
        let _ = profile_window.upgrade_in_event_loop(move |window| {
            if let Some((image, _)) = covers::decode(&bytes) {
                window.set_profile_image(image);
            }
        });
    });
    window.set_games(ModelRc::from(std::rc::Rc::new(VecModel::from(Vec::new()))));
    window.set_platform_games(ModelRc::from(std::rc::Rc::new(VecModel::from(Vec::new()))));
    window.set_selected_game(empty_game_card());
    window.set_game_details(empty_preview_details());
    window.set_shelf_height(shelf::shelf_height());
    window.set_loading(true);
    window.set_library_loading(false);

    let library_state: Arc<Mutex<Option<app::AppStateHandle>>> = Arc::new(Mutex::new(None));

    // Keep the loader alive for the lifetime of the window. Metadata and
    // artwork are populated after the shell is already visible.
    let (loader, source_store) = covers::spawn_loader(&window, reqwest::Client::new(), Vec::new());
    let home_source_store = Arc::new(Mutex::new(Vec::new()));
    let loader_for_scroll = loader.clone();
    window.on_viewport_changed(move |scroll_x, viewport_width| {
        loader_for_scroll
            .borrow_mut()
            .update(scroll_x, viewport_width);
    });

    let loader_for_context = loader.clone();
    let context_sources = source_store.clone();
    let context_home_sources = home_source_store.clone();
    window.on_cover_context_changed(move |active_tab| {
        debug!(active_tab, "cover context changed");
        if active_tab == 1 {
            let mut loader = loader_for_context.borrow_mut();
            loader.reset();
            loader.update(0.0, 1_280.0);
            debug!("cover context reset complete; refreshing platform residency");
            return;
        }
        let home_sources = context_home_sources
            .lock()
            .expect("home cover source state poisoned")
            .clone();
        let source_count = home_sources.len();
        *context_sources.lock().expect("cover source state poisoned") = home_sources;
        debug!(source_count, "cover context switched to home sources");
        let mut loader = loader_for_context.borrow_mut();
        loader.reset();
        loader.update(0.0, 1_280.0);
        debug!("cover context reset complete; refreshing home residency");
    });

    let weak_window = window.as_weak();
    let detail_state = library_state.clone();
    let detail_window = window.as_weak();
    let selected_loader = loader.clone();
    window.on_game_selected(move |id, index| {
        // Load the selected row's cover on demand. The detail request below
        // supplies the rest of the metadata.
        selected_loader
            .borrow_mut()
            .update(index as f32 * 216.0, 240.0);
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

    let play_state = library_state.clone();
    window.on_game_play_requested(move |id| {
        let state = play_state
            .lock()
            .expect("library state lock poisoned")
            .clone();
        let Some(state) = state else {
            tracing::warn!(game_id = %id, "play requested but library state is unavailable");
            return;
        };
        let Ok(item_id) = LibraryItemId::parse(id.as_str()).ok_or(()) else {
            tracing::warn!(game_id = %id, "play requested with invalid library item id");
            return;
        };
        tokio::spawn(async move {
            match state.library.get(&item_id).await {
                Ok(Some(item)) => {
                    tracing::info!(game_id = %id, local_path = ?item.local_path, "play requested; launcher stub");
                }
                Ok(None) => tracing::warn!(game_id = %id, "play requested for missing library item"),
                Err(error) => tracing::error!(%error, game_id = %id, "failed to resolve game path for launcher stub"),
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
            let loaded = shelf::load_platform_games(
                &state.library,
                base_url.as_deref(),
                platform_slug.as_str(),
            )
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

    let home_state = library_state.clone();
    let home_window = window.as_weak();
    let home_sources = home_source_store.clone();
    let home_loader_sources = source_store.clone();

    window.on_home_entered(move || {
        let Some(state) = home_state.lock().ok().and_then(|state| state.clone()) else {
            return;
        };
        let window = home_window.clone();
        let home_sources = home_sources.clone();
        let home_loader_sources = home_loader_sources.clone();
        tokio::spawn(async move {
            if let Ok((metadata, cover_sources)) =
                shelf::load_games(&state.library, state.config.romm_url.as_deref()).await
            {
                *home_sources
                    .lock()
                    .expect("home cover source state poisoned") = cover_sources.clone();
                *home_loader_sources
                    .lock()
                    .expect("cover source state poisoned") = cover_sources;
                let _ = window.upgrade_in_event_loop(move |window| {
                    if let Some(model) = window
                        .get_games()
                        .as_any()
                        .downcast_ref::<VecModel<GameCardData>>()
                    {
                        model.set_vec(game_cards(metadata));
                        if window.get_active_tab() == 0 {
                            // Replacing the model clears its images. Reset the loader so
                            // rows already marked resident are requested for the new model.
                            window.invoke_cover_context_changed(0);
                        }
                    }
                    window.set_loading(false);
                });
            } else {
                let _ = window.upgrade_in_event_loop(|window| {
                    window.set_loading(false);
                });
            }
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
            let metadata =
                match shelf::load_games(&state.library, state.config.romm_url.as_deref()).await {
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

    let state_store = library_state.clone();
    tokio::spawn(async move {
        let state = match app::AppState::initialize().await {
            Ok(state) => state,
            Err(error) => {
                error!(%error, "application initialization failed");
                return;
            }
        };

        if let Some(root) = state.config.library_root.as_ref() {
            match tokio::fs::try_exists(root).await {
                Ok(false) => {
                    info!(path = %root.display(), "local library root does not exist yet; skipping scan");
                }
                Ok(true) => match scan(root) {
                    Ok(items) => {
                        info!(count = items.len(), "local game scan completed");
                        for item in items {
                            let platform_slug = item.platform_slug.clone();
                            if let Some(slug) = platform_slug.as_deref() {
                                let _ = state
                                    .library
                                    .add_platform(marina_core::Platform::new(slug, slug))
                                    .await;
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
                            let result = if let Some(mut existing) = existing {
                                // Scanner data describes filesystem presence only. Preserve
                                // provider metadata/assets from an enriched installed record.
                                existing.files = item.files;
                                state.library.update(existing).await
                            } else {
                                state.library.add(item).await
                            };
                            if let Err(error) = result {
                                error!(%error, "failed to store scanned local game");
                            }
                        }
                    }
                    Err(error) => error!(%error, "local game scan failed"),
                },
                Err(error) => {
                    error!(%error, path = %root.display(), "could not inspect local library root")
                }
            }
        }

        info!("loading game metadata");
        let loaded = shelf::load_games(&state.library, state.config.romm_url.as_deref()).await;
        let (metadata, cover_sources) = match loaded {
            Ok(loaded) => loaded,
            Err(error) => {
                error!(%error, "game metadata loading failed");
                return;
            }
        };
        info!(count = metadata.len(), "library loaded");
        *source_store.lock().expect("cover source state poisoned") = cover_sources.clone();
        *home_source_store
            .lock()
            .expect("home cover source state poisoned") = cover_sources;
        *state_store.lock().expect("library state lock poisoned") = Some(state.clone());

        if state.config.import_romm_on_startup {
            if let Some(base_url) = state.config.romm_url.clone() {
                let sync_state = state.clone();
                tokio::spawn(async move {
                    let client =
                        romm_auth::client(base_url, sync_state.config.romm_token.as_deref());
                    match client.list_platforms(&PlatformQuery::default()).await {
                        Ok(platforms) => {
                            for platform in platforms {
                                let mut offset = 0_i64;
                                loop {
                                    let query = RomQuery {
                                        platform_ids: vec![platform.id],
                                        limit: Some(100),
                                        offset: Some(offset),
                                        with_files: Some(true),
                                        ..Default::default()
                                    };
                                    let page = match client.list_roms(&query).await {
                                        Ok(page) => page,
                                        Err(error) => {
                                            error!(%error, platform = %platform.fs_slug, "RomM catalog sync failed");
                                            break;
                                        }
                                    };
                                    let rows = page
                                        .items
                                        .iter()
                                        .filter_map(|rom| {
                                            Some((
                                                rom.id.to_string(),
                                                rom.name
                                                    .clone()
                                                    .unwrap_or_else(|| rom.files.fs_name.clone()),
                                                rom.platform.platform_fs_slug.clone(),
                                                serde_json::to_string(rom).ok()?,
                                            ))
                                        })
                                        .collect::<Vec<_>>();
                                    if let Err(error) =
                                        sync_state.library.upsert_remote_json("romm", &rows)
                                    {
                                        error!(%error, "RomM catalog cache write failed");
                                        break;
                                    }
                                    let count = page.items.len() as i64;
                                    info!(platform = %platform.fs_slug, offset, rows = count, total = ?page.total, "RomM catalog page cached");
                                    if count == 0
                                        || page.total.is_some_and(|total| offset + count >= total)
                                    {
                                        break;
                                    }
                                    offset += count;
                                }
                            }
                        }
                        Err(error) => error!(%error, "RomM catalog platform sync failed"),
                    }
                });
            }
        } else {
            info!("RomM startup catalog import disabled by MARINA_IMPORT_ROMM_ON_STARTUP");
        }

        let platform_metadata = match state.library.platforms().await {
            Ok(platforms) => platforms,
            Err(error) => {
                error!(%error, "platform metadata loading failed");
                Vec::new()
            }
        };
        info!(
            count = platform_metadata.len(),
            "local platform records loaded"
        );
        let icon_root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/assets/platforms/systematic");
        let mut platform_cards = Vec::with_capacity(platform_metadata.len());
        for platform in platform_metadata {
            let game_count = metadata
                .iter()
                .filter(|game| game.platform.eq_ignore_ascii_case(&platform.name))
                .count();
            let icon_path = platform_asset_path(&icon_root, &platform.slug);
            platform_cards.push(PlatformCardMetadata {
                slug: platform.slug,
                name: platform.name,
                game_count: format!(
                    "{} {}",
                    game_count,
                    if game_count == 1 { "game" } else { "games" }
                ),
                icon_path,
            });
        }

        let _ = weak_window.upgrade_in_event_loop(move |window| {
            let platforms: Vec<PlatformCardData> = platform_cards
                .into_iter()
                .map(|platform| PlatformCardData {
                    slug: SharedString::from(platform.slug),
                    name: SharedString::from(platform.name),
                    icon: platform
                        .icon_path
                        .and_then(|path| Image::load_from_path(std::path::Path::new(&path)).ok())
                        .unwrap_or_default(),
                    game_count: SharedString::from(platform.game_count),
                })
                .collect();
            window.set_platforms(ModelRc::from(std::rc::Rc::new(VecModel::from(platforms))));
            let games = game_cards(metadata);
            window.set_games(ModelRc::from(std::rc::Rc::new(VecModel::from(games))));
            window.set_loading(false);
            // The Shelf's init callback requests the initial visible range.
        });

        if let Some(base_url) = state.config.romm_url.clone() {
            let store_window = weak_window.clone();
            tokio::spawn(async move {
                let result = romm_auth::client(base_url, state.config.romm_token.as_deref())
                    .list_platforms(&PlatformQuery::default())
                    .await;
                match result {
                    Ok(remote_platforms) => {
                        let icon_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                            .join("ui/assets/platforms/systematic");
                        let cards = remote_platforms
                            .into_iter()
                            .map(|platform| PlatformCardMetadata {
                                icon_path: platform_asset_path(&icon_root, &platform.fs_slug),
                                slug: platform.fs_slug,
                                name: platform.display_name,
                                game_count: format!("{} games", platform.rom_count),
                            })
                            .collect::<Vec<_>>();
                        let _ = store_window.upgrade_in_event_loop(move |window| {
                            let cards = cards
                                .into_iter()
                                .map(|platform| PlatformCardData {
                                    slug: SharedString::from(platform.slug),
                                    name: SharedString::from(platform.name),
                                    icon: platform
                                        .icon_path
                                        .and_then(|path| {
                                            Image::load_from_path(std::path::Path::new(&path)).ok()
                                        })
                                        .unwrap_or_default(),
                                    game_count: SharedString::from(platform.game_count),
                                })
                                .collect::<Vec<_>>();
                            window.set_store_platforms(ModelRc::from(std::rc::Rc::new(
                                VecModel::from(cards),
                            )));
                            window.set_store_loading(false);
                        });
                    }
                    Err(error) => {
                        error!(%error, "RomM Store platform loading failed");
                        let _ = store_window
                            .upgrade_in_event_loop(|window| window.set_store_loading(false));
                    }
                }
            });
        } else {
            let _ = weak_window.upgrade_in_event_loop(|window| window.set_store_loading(false));
        }
    });

    window.on_store_search(|query| {
        info!(query = %query, "Store search requested; RomM result loading is next");
    });

    let store_roms: Arc<Mutex<Vec<marina_romm::Rom>>> = Arc::new(Mutex::new(Vec::new()));
    let install_state = library_state.clone();
    let install_roms = store_roms.clone();
    let install_window = window.as_weak();
    window.on_store_install_requested(move |id, selected| {
        let Ok(id) = id.parse::<i32>() else { return };
        let Some(state) = install_state
            .lock()
            .expect("library state lock poisoned")
            .clone()
        else {
            return;
        };
        let Some(root) = state.config.library_root.clone() else {
            error!("cannot install Store game without MARINA_LIBRARY_ROOT");
            return;
        };
        let Some(rom) = install_roms
            .lock()
            .expect("remote Store state poisoned")
            .iter()
            .find(|rom| rom.id == id)
            .cloned()
        else {
            return;
        };
        let files = rom
            .files
            .files
            .iter()
            .zip(selected.iter())
            .filter_map(|(file, selected)| selected.then_some(file.clone()))
            .collect::<Vec<_>>();
        if files.is_empty() {
            return;
        }
        let window = install_window.clone();
        let selected_count = files.len();
        let _ = window.upgrade_in_event_loop(move |window| {
            window.set_store_install_status(SharedString::from(format!(
                "Installing {selected_count} file(s)…"
            )));
            window.set_store_install_progress(0.0);
        });
        tokio::spawn(async move {
            let Some(base_url) = state.config.romm_url.clone() else {
                return;
            };
            let client = romm_auth::client(base_url, state.config.romm_token.as_deref());
            let result = marina_install::install(
                &client,
                &state.library,
                marina_install::InstallRequest {
                    rom,
                    files,
                    library_root: root,
                },
            )
            .await;
            match result {
                Ok(item) => {
                    let platform = item.platform_slug.clone().unwrap_or_default();
                    let refreshed =
                        shelf::load_platform_games(&state.library, None, &platform).await;
                    match refreshed {
                        Ok((metadata, _)) => {
                            let _ = window.upgrade_in_event_loop(move |window| {
                            let cards = game_cards(metadata);
                            if let Some(model) = window
                                .get_platform_games()
                                .as_any()
                                .downcast_ref::<VecModel<GameCardData>>()
                            {
                                model.set_vec(cards);
                            }
                            window.set_library_loading(false);
                            window.set_store_loading(false);
                            window.set_store_details_loading(false);
                            window.set_store_install_status(SharedString::from("Installed"));
                            window.set_store_install_progress(1.0);
                            // A newly installed game may introduce a platform,
                            // so refresh the Library catalog without requiring
                            // the user to leave and re-enter that tab.
                            window.invoke_library_entered();
                            });
                        }
                        Err(error) => {
                            error!(%error, platform = %platform, "installed game saved but library refresh failed");
                            let _ = window.upgrade_in_event_loop(move |window| {
                                window.set_store_install_status(SharedString::from("Installed; library refresh failed"));
                                window.set_store_install_progress(1.0);
                            });
                        }
                    }
                }
                Err(error) => {
                    error!(%error, "Store installation failed");
                    let _ = window.upgrade_in_event_loop(move |window| {
                        window.set_store_install_status(SharedString::from(format!(
                            "Install failed: {error}"
                        )));
                        window.set_store_install_progress(0.0);
                    });
                }
            }
        });
    });
    let store_query_state = library_state.clone();
    let store_query_window = window.as_weak();
    let store_query_roms = store_roms.clone();
    let store_refresh_state = library_state.clone();
    let store_refresh_window = window.as_weak();
    window.on_store_entered(move || {
        let state = store_refresh_state
            .lock()
            .ok()
            .and_then(|state| state.clone());
        let Some(state) = state else { return };
        let Some(base_url) = state.config.romm_url.clone() else {
            return;
        };
        let window = store_refresh_window.clone();
        let _ = window.upgrade_in_event_loop(|window| {
            window.set_store_loading(true);
        });
        tokio::spawn(async move {
            match romm_auth::client(base_url, state.config.romm_token.as_deref())
                .list_platforms(&PlatformQuery::default())
                .await
            {
                Ok(platforms) => {
                    let icon_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("ui/assets/platforms/systematic");
                    let cards = platforms
                        .into_iter()
                        .map(|platform| PlatformCardMetadata {
                            icon_path: platform_asset_path(&icon_root, &platform.fs_slug),
                            slug: platform.fs_slug,
                            name: platform.display_name,
                            game_count: platform.rom_count.to_string(),
                        })
                        .collect::<Vec<_>>();
                    let count = cards.len();
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
                        window.set_store_platforms(ModelRc::from(std::rc::Rc::new(
                            VecModel::from(cards),
                        )));
                        window.set_store_loading(false);
                        window.global::<ToastQueue>().invoke_show(
                            SharedString::from(format!("Loaded {count} RomM platforms")),
                            ToastVariant::Success,
                        );
                    });
                }
                Err(error) => {
                    error!(%error, "RomM platform refresh failed");
                    let message = SharedString::from(format!("RomM refresh failed: {error}"));
                    let _ = window.upgrade_in_event_loop(move |window| {
                        window.set_store_loading(false);
                        window
                            .global::<ToastQueue>()
                            .invoke_show(message, ToastVariant::Error);
                    });
                }
            }
        });
    });
    window.on_store_platform_query(move |slug| {
        let state = store_query_state
            .lock()
            .expect("library state lock poisoned")
            .clone();
        let Some(state) = state else { return };
        let Some(base_url) = state.config.romm_url.clone() else {
            return;
        };
        let query_window = store_query_window.clone();
        let remote_store = store_query_roms.clone();
        tokio::spawn(async move {
            if let Ok(cached) =
                state
                    .library
                    .remote_json_page("romm", Some(&slug), None, usize::MAX, 0)
            {
                let cached = cached
                    .into_iter()
                    .filter_map(|json| serde_json::from_str::<marina_romm::Rom>(&json).ok())
                    .collect::<Vec<_>>();
                if !cached.is_empty() {
                    *remote_store.lock().expect("remote Store state poisoned") = cached.clone();
                    let cards = cached
                        .into_iter()
                        .map(|rom| {
                            (
                                rom.id.to_string(),
                                rom.name.unwrap_or_else(|| rom.files.fs_name.clone()),
                                rom.platform
                                    .platform_display_name
                                    .unwrap_or(rom.platform.platform_fs_slug),
                            )
                        })
                        .collect::<Vec<_>>();
                    let _ = query_window.upgrade_in_event_loop(move |window| {
                        let cards = cards
                            .into_iter()
                            .map(|(id, title, platform)| GameCardData {
                                id: SharedString::from(id),
                                title: SharedString::from(title),
                                platform: SharedString::from(platform),
                                cover: Image::default(),
                                cover_ratio: 1.0,
                            })
                            .collect::<Vec<_>>();
                        let first_id = cards.first().map(|game| game.id.clone());
                        window.set_store_games(ModelRc::from(std::rc::Rc::new(VecModel::from(
                            cards,
                        ))));
                        window.set_store_loading(false);
                        if let Some(first_id) = first_id {
                            window.set_store_selected_game_index(0);
                            window.set_store_details_loading(true);
                            window.invoke_store_game_selected(first_id, 0);
                        }
                    });
                    return;
                }
            }
            let client = romm_auth::client(base_url, state.config.romm_token.as_deref());
            let platforms = match client.list_platforms(&PlatformQuery::default()).await {
                Ok(platforms) => platforms,
                Err(error) => {
                    error!(%error, "Store platform lookup failed");
                    return;
                }
            };
            let Some(platform) = platforms
                .into_iter()
                .find(|platform| platform.fs_slug == slug.as_str())
            else {
                return;
            };
            let query = marina_romm::RomQuery {
                platform_ids: vec![platform.id],
                limit: Some(100),
                offset: Some(0),
                ..Default::default()
            };
            let page = match client.list_roms(&query).await {
                Ok(page) => page,
                Err(error) => {
                    error!(%error, "Store game loading failed");
                    return;
                }
            };
            let remote_items = page.items;

            let cache_rows = remote_items
                .iter()
                .filter_map(|rom| {
                    Some((
                        rom.id.to_string(),
                        rom.name
                            .clone()
                            .unwrap_or_else(|| rom.files.fs_name.clone()),
                        rom.platform.platform_fs_slug.clone(),
                        serde_json::to_string(rom).ok()?,
                    ))
                })
                .collect::<Vec<_>>();
            if let Err(error) = state.library.upsert_remote_json("romm", &cache_rows) {
                error!(%error, "RomM remote catalog cache write failed");
            }
            *remote_store.lock().expect("remote Store state poisoned") = remote_items.clone();
            let cards = remote_items
                .into_iter()
                .map(|rom| {
                    (
                        rom.id.to_string(),
                        rom.name.unwrap_or_else(|| rom.files.fs_name.clone()),
                        platform.display_name.clone(),
                    )
                })
                .collect::<Vec<_>>();
            let _ = query_window.upgrade_in_event_loop(move |window| {
                let cards = cards
                    .into_iter()
                    .map(|(id, title, platform)| GameCardData {
                        id: SharedString::from(id),
                        title: SharedString::from(title),
                        platform: SharedString::from(platform),
                        cover: Image::default(),
                        cover_ratio: 1.0,
                    })
                    .collect::<Vec<_>>();
                let first_id = cards.first().map(|game| game.id.clone());
                window.set_store_games(ModelRc::from(std::rc::Rc::new(VecModel::from(cards))));
                window.set_store_loading(false);
                if let Some(first_id) = first_id {
                    window.set_store_selected_game_index(0);
                    window.set_store_details_loading(true);
                    window.invoke_store_game_selected(first_id, 0);
                }
            });
        });
    });

    let detail_store = store_roms.clone();
    let detail_state = library_state.clone();
    let detail_window = window.as_weak();
    window.on_store_game_selected(move |id, _index| {
        let Ok(id) = id.parse::<i32>() else { return };
        if let Some(window) = detail_window.upgrade() {
            window.set_store_preview_image(Image::default());
        }
        let cached_rom = detail_store
            .lock()
            .expect("remote Store state poisoned")
            .iter()
            .find(|rom| rom.id == id)
            .cloned();
        let Some(state) = detail_state
            .lock()
            .expect("library state lock poisoned")
            .clone()
        else {
            return;
        };
        let Some(base_url) = state.config.romm_url.clone() else {
            return;
        };
        if let Some(rom) = cached_rom {
            populate_store_details(&detail_window, rom, &base_url);
        }
        let token = state.config.romm_token.clone();
        let detail_roms = detail_store.clone();
        let detail_window = detail_window.clone();
        tokio::spawn(async move {
            let client = romm_auth::client(base_url.clone(), token.as_deref());
            match client.get_rom(id).await {
                Ok(rom) => {
                    *detail_roms.lock().expect("remote Store state poisoned") = vec![rom.clone()];
                    populate_store_details(&detail_window, rom, &base_url);
                }
                Err(error) => error!(%error, rom_id = id, "RomM game detail hydration failed"),
            }
        });
    });

    window.run()
}

fn populate_store_details(window: &slint::Weak<MainWindow>, rom: marina_romm::Rom, base_url: &str) {
    let rom_id = rom.id.to_string();
    let cover_source = covers::source_for(rom.cover_path().as_deref(), None, Some(base_url));
    let screenshot = rom
        .assets
        .merged_screenshots
        .first()
        .cloned()
        .or_else(|| {
            rom.assets
                .user_screenshots
                .first()
                .map(|screenshot| screenshot.download_path.clone())
        })
        .or_else(|| {
            rom.assets
                .all_user_screenshots
                .first()
                .map(|screenshot| screenshot.download_path.clone())
        });
    let screenshot_source = covers::source_for(screenshot.as_deref(), None, Some(base_url));
    let item: marina_core::LibraryItem = rom.clone().into();
    let rom_prefix = rom.files.full_path.trim_end_matches('/').to_owned();
    let artifact_count = rom.files.files.len();
    let mut artifact_tree = ArtifactTree::default();
    for (file_index, file) in rom.files.files.iter().enumerate() {
        artifact_tree.insert(
            &display_artifact_path(file, &rom_prefix),
            file_index,
            file.file_size_bytes,
        );
    }
    let mut artifacts = Vec::new();
    artifact_tree.flatten(0, &mut artifacts);
    let details = PreviewDetailsData {
        title: SharedString::from(item.title),
        summary: SharedString::from(item.summary.unwrap_or_default()),
        released_at: SharedString::default(),
        languages: SharedString::from(item.languages.join(", ")),
        regions: SharedString::from(item.regions.join(", ")),
        tags: SharedString::from(item.tags.join(", ")),
    };
    let selected_rom_id = rom_id.clone();
    let _ = window.upgrade_in_event_loop(move |window| {
        let games = window.get_store_games();
        let selected_index = window.get_store_selected_game_index().max(0) as usize;
        if games
            .row_data(selected_index)
            .is_none_or(|game| game.id.as_str() != selected_rom_id)
        {
            return;
        }
        window.set_store_details(details);
        window.set_store_preview_image(Image::default());
        window.set_store_artifacts(ModelRc::from(std::rc::Rc::new(VecModel::from(artifacts))));
        window.set_store_selected_artifacts(
            std::rc::Rc::new(VecModel::from(vec![false; artifact_count])).into(),
        );
        window.set_store_details_loading(false);
    });

    let preview_window = window.clone();
    let preview_rom_id = rom_id.clone();
    tokio::spawn(async move {
        let Some(bytes) = covers::load_bytes(&reqwest::Client::new(), &screenshot_source).await
        else {
            return;
        };
        let _ = preview_window.upgrade_in_event_loop(move |window| {
            let games = window.get_store_games();
            let selected_index = window.get_store_selected_game_index().max(0) as usize;
            if games
                .row_data(selected_index)
                .is_none_or(|game| game.id.as_str() != preview_rom_id)
            {
                return;
            }
            let Some((image, _)) = covers::decode(&bytes) else {
                return;
            };
            window.set_store_preview_image(image);
        });
    });

    let cover_window = window.clone();
    tokio::spawn(async move {
        let Some(bytes) = covers::load_bytes(&reqwest::Client::new(), &cover_source).await else {
            return;
        };
        let _ = cover_window.upgrade_in_event_loop(move |window| {
            let Some((image, ratio)) = covers::decode(&bytes) else {
                return;
            };
            let games = window.get_store_games();
            let Some(index) = (0..games.row_count()).find(|&index| {
                games
                    .row_data(index)
                    .is_some_and(|game| game.id.as_str() == rom_id)
            }) else {
                return;
            };
            if let Some(mut game) = games.row_data(index) {
                game.cover = image;
                game.cover_ratio = ratio;
                games.set_row_data(index, game);
            }
        });
    });
}

#[derive(Default)]
struct ArtifactTree {
    directories: BTreeMap<String, Self>,
    files: Vec<(String, usize, i64)>,
}

impl ArtifactTree {
    fn insert(&mut self, path: &str, file_index: usize, file_size_bytes: i64) {
        let parts = path
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let Some((file_name, directories)) = parts.split_last() else {
            return;
        };

        let mut node = self;
        for directory in directories {
            node = node.directories.entry((*directory).to_owned()).or_default();
        }
        node.files
            .push(((*file_name).to_owned(), file_index, file_size_bytes));
    }

    fn flatten(&self, depth: i32, rows: &mut Vec<StoreArtifact>) {
        for (directory, child) in &self.directories {
            rows.push(StoreArtifact {
                path: SharedString::from(directory),
                size: SharedString::default(),
                depth,
                is_directory: true,
                file_index: -1,
            });
            child.flatten(depth + 1, rows);
        }

        let mut files = self.files.clone();
        files.sort_by(|left, right| left.0.cmp(&right.0));
        for (file_name, file_index, file_size_bytes) in files {
            rows.push(StoreArtifact {
                path: SharedString::from(file_name),
                size: SharedString::from(format!("{file_size_bytes} bytes")),
                depth,
                is_directory: false,
                file_index: file_index as i32,
            });
        }
    }
}

fn display_artifact_path(file: &marina_romm::RomFile, rom_prefix: &str) -> String {
    let source = if file.full_path.is_empty() {
        &file.file_path
    } else {
        &file.full_path
    };
    let stripped = source
        .strip_prefix(rom_prefix)
        .unwrap_or(source)
        .trim_start_matches('/');
    if stripped.is_empty() {
        file.file_name.clone()
    } else {
        stripped.to_owned()
    }
}

struct PlatformCardMetadata {
    slug: String,
    name: String,
    game_count: String,
    icon_path: Option<String>,
}

#[derive(Serialize)]
struct UiFixture {
    platforms: Vec<UiFixturePlatform>,
    games: Vec<UiFixtureGame>,
}

#[derive(Serialize)]
struct UiFixturePlatform {
    slug: String,
    name: String,
}

#[derive(Serialize)]
struct UiFixtureGame {
    id: String,
    title: String,
    platform: String,
    cover: Option<String>,
    regions: Vec<String>,
}

async fn export_ui_fixture(path: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = app::AppState::initialize().await?;
    let platforms = state
        .library
        .platforms()
        .await?
        .into_iter()
        .map(|platform| UiFixturePlatform {
            slug: platform.slug,
            name: platform.name,
        })
        .collect();
    let games = state
        .library
        .list_cards(u32::MAX)
        .await?
        .into_iter()
        .map(|game| UiFixtureGame {
            id: game.id.to_string(),
            title: game.title,
            platform: game.platform_name.unwrap_or_else(|| "Unknown".into()),
            cover: game.cover,
            regions: game.regions,
        })
        .collect();
    let fixture = UiFixture { platforms, games };
    let json = serde_json::to_string_pretty(&fixture)?;
    std::fs::write(path, json)?;
    println!("wrote UI fixture to {path}");
    Ok(())
}

fn profile_identity() -> (String, String, String) {
    let username = std::env::var("USER").unwrap_or_else(|_| "user".into());
    let gecos = std::fs::read_to_string("/etc/passwd")
        .ok()
        .and_then(|passwd| {
            passwd.lines().find_map(|line| {
                let fields: Vec<_> = line.split(':').collect();
                if fields.first().copied() != Some(username.as_str()) {
                    return None;
                }
                fields
                    .get(4)
                    .and_then(|gecos| gecos.split(',').next())
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
            })
        });
    let display_name = gecos.unwrap_or_else(|| username.clone());
    let initials = profile_initials(&display_name);
    (username, display_name, initials)
}

fn profile_initials(name: &str) -> String {
    let initials: String = name
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect();
    if initials.is_empty() {
        name.chars()
            .next()
            .unwrap_or('U')
            .to_uppercase()
            .to_string()
    } else {
        initials.to_uppercase()
    }
}

fn empty_game_card() -> GameCardData {
    GameCardData {
        id: SharedString::default(),
        title: SharedString::default(),
        platform: SharedString::default(),
        cover: Image::default(),
        cover_ratio: 1.0,
    }
}

fn game_cards(metadata: Vec<shelf::GameMetadata>) -> Vec<GameCardData> {
    metadata
        .into_iter()
        .map(|item| GameCardData {
            id: SharedString::from(item.id),
            title: SharedString::from(item.title),
            platform: SharedString::from(item.platform),
            cover: Image::default(),
            cover_ratio: 1.0,
        })
        .collect()
}

fn empty_preview_details() -> PreviewDetailsData {
    PreviewDetailsData {
        title: SharedString::default(),
        summary: SharedString::default(),
        released_at: SharedString::default(),
        languages: SharedString::default(),
        regions: SharedString::default(),
        tags: SharedString::default(),
    }
}

fn preview_details(item: marina_core::LibraryItem) -> PreviewDetailsData {
    PreviewDetailsData {
        title: SharedString::from(item.title),
        summary: SharedString::from(item.summary.unwrap_or_default()),
        released_at: SharedString::from(
            item.released_at
                .map(|date| date.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
        ),
        languages: SharedString::from(item.languages.join(", ")),
        regions: SharedString::from(item.regions.join(", ")),
        tags: SharedString::from(item.tags.join(", ")),
    }
}

fn platform_asset_path(root: &std::path::Path, slug: &str) -> Option<String> {
    let exact_name = match slug {
        "ndsi" => "nintendo-dsi",
        "win" => "pc-50x-family",
        _ => slug,
    };
    let exact_path = root.join(format!("{exact_name}.svg"));
    if exact_path.is_file() {
        return Some(exact_path.to_string_lossy().into_owned());
    }

    if let Some(prefix) = slug.split('-').next() {
        let prefix_path = root.join(format!("{prefix}.svg"));
        if prefix_path.is_file() {
            return Some(prefix_path.to_string_lossy().into_owned());
        }
    }

    let default_path = root.join("default.svg");
    default_path
        .is_file()
        .then(|| default_path.to_string_lossy().into_owned())
}
