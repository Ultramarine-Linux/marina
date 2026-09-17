//! Store page event and data coordination.

use std::sync::{Arc, Mutex};

use marina_romm::{PlatformQuery, RomQuery};
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};
use tracing::{error, info};

use super::library as shelf;
use crate::{
    GameCardData, MainWindow, PlatformCardData, PlatformCardMetadata, ToastQueue, ToastVariant,
    game_cards, platform_asset_path,
};
use crate::{app, romm_auth};

pub(crate) fn install(
    window: &MainWindow,
    library_state: &Arc<Mutex<Option<app::AppStateHandle>>>,
) {
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
                    let refreshed = shelf::load_platform_games(&state.library, None, &platform).await;
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
            let query = RomQuery {
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
            crate::populate_store_details(&detail_window, rom, &base_url);
        }
        let token = state.config.romm_token.clone();
        let detail_roms = detail_store.clone();
        let detail_window = detail_window.clone();
        tokio::spawn(async move {
            let client = romm_auth::client(base_url.clone(), token.as_deref());
            match client.get_rom(id).await {
                Ok(rom) => {
                    *detail_roms.lock().expect("remote Store state poisoned") = vec![rom.clone()];
                    crate::populate_store_details(&detail_window, rom, &base_url);
                }
                Err(error) => error!(%error, rom_id = id, "RomM game detail hydration failed"),
            }
        });
    });
}
