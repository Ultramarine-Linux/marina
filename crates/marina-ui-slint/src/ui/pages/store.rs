//! Store page event and data coordination (backend-agnostic).
//!
//! Catalog data flows: [`marina_store::StoreBackend`] -> per-backend
//! [`marina_store::StoreCache`] file (`<dir>/<backend>.db`) -> UI cards.
//! The in-memory `Vec<Rom>` is hydrated from cached payload JSON so the
//! page works offline; network refreshes upsert into the backend's own file.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};
use tracing::{error, info};

use super::library as shelf;
use crate::{
    GameCardData, LibraryState, MainWindow, PlatformCardData, PreviewDetailsData, StoreArtifact,
    StoreState, ToastQueue, ToastVariant, game_cards, platform_asset_path,
};
use crate::{app, image};

fn decode_roms(entries: &[marina_store::StoreEntry]) -> Vec<marina_romm::Rom> {
    entries
        .iter()
        .filter_map(|entry| {
            entry
                .payload_json
                .as_deref()
                .and_then(|json| serde_json::from_str::<marina_romm::Rom>(json).ok())
        })
        .collect()
}

fn rom_card(rom: &marina_romm::Rom) -> (String, String, String) {
    (
        rom.id.to_string(),
        rom.name
            .clone()
            .unwrap_or_else(|| rom.files.fs_name.clone()),
        rom.platform
            .platform_display_name
            .clone()
            .unwrap_or(rom.platform.platform_fs_slug.clone()),
    )
}

/// Builds UI cards. Must run on the event-loop thread: GameCardData holds
/// a Slint image and is not Send, so only plain tuples cross threads.
fn cards_from(parts: Vec<(String, String, String)>) -> Vec<GameCardData> {
    parts
        .into_iter()
        .map(|(id, title, platform)| GameCardData {
            id: SharedString::from(id),
            title: SharedString::from(title),
            platform: SharedString::from(platform),
            cover: Image::default(),
            cover_ratio: 1.0,
        })
        .collect()
}

fn show_roms(
    window: &slint::Weak<MainWindow>,
    remote_store: &Arc<Mutex<Vec<marina_romm::Rom>>>,
    roms: Vec<marina_romm::Rom>,
) {
    *remote_store.lock().expect("remote Store state poisoned") = roms.clone();
    let cards = roms.iter().map(rom_card).collect::<Vec<_>>();
    let _ = window.upgrade_in_event_loop(move |window| {
        let cards = cards_from(cards);
        let first_id = cards.first().map(|game| game.id.clone());
        window
            .global::<StoreState>()
            .set_games(ModelRc::from(std::rc::Rc::new(VecModel::from(cards))));
        window.global::<StoreState>().set_loading(false);
        if let Some(first_id) = first_id {
            window.global::<StoreState>().set_selected_game_index(0);
            window.global::<StoreState>().set_details_loading(true);
            window
                .global::<StoreState>()
                .invoke_game_selected(first_id, 0);
        }
    });
}

/// Republishes the model from the accumulated in-memory rows without
/// touching selection or details. Used as streamed pages land in the cache.
fn republish_roms(
    window: &slint::Weak<MainWindow>,
    remote_store: &Arc<Mutex<Vec<marina_romm::Rom>>>,
) {
    let cards = remote_store
        .lock()
        .expect("remote Store state poisoned")
        .iter()
        .map(rom_card)
        .collect::<Vec<_>>();
    let _ = window.upgrade_in_event_loop(move |window| {
        let cards = cards_from(cards);
        window
            .global::<StoreState>()
            .set_games(ModelRc::from(std::rc::Rc::new(VecModel::from(cards))));
        window.global::<StoreState>().set_loading(false);
    });
}

pub(crate) fn install(
    window: &MainWindow,
    library_state: &Arc<Mutex<Option<app::AppStateHandle>>>,
) {
    window.global::<StoreState>().on_search(|query| {
        info!(query = %query, "Store search requested; RomM result loading is next");
    });

    let store_roms: Arc<Mutex<Vec<marina_romm::Rom>>> = Arc::new(Mutex::new(Vec::new()));
    let install_state = library_state.clone();
    let install_roms = store_roms.clone();
    let install_window = window.as_weak();
    window
            .global::<StoreState>()
            .on_install_requested(move |id, selected| {
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
        let file_ids = rom
            .files
            .files
            .iter()
            .zip(selected.iter())
            .filter_map(|(file, selected)| selected.then_some(file.id.to_string()))
            .collect::<Vec<_>>();
        if file_ids.is_empty() {
            return;
        }
        let window = install_window.clone();
        let selected_count = file_ids.len();
        let _ = window.upgrade_in_event_loop(move |window| {
            window
                .global::<StoreState>()
                .set_install_status(SharedString::from(format!(
                    "Installing {selected_count} file(s)…"
                )));
            window.global::<StoreState>().set_install_progress(0.0);
        });
        let Some(backend) = state.stores.get("romm").cloned() else {
            return;
        };
        let entry_id = id.to_string();
        tokio::spawn(async move {
            let entry = match backend.get(&entry_id).await {
                Ok(Some(entry)) => entry,
                Ok(None) => {
                    error!(entry_id = %entry_id, "store entry vanished before install");
                    return;
                }
                Err(error) => {
                    error!(%error, "store entry hydration for install failed");
                    return;
                }
            };
            let result = backend
                .install(
                    &state.library,
                    marina_store::InstallRequest {
                        entry,
                        file_ids,
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
                                    .global::<LibraryState>()
                                    .get_games()
                                    .as_any()
                                    .downcast_ref::<VecModel<GameCardData>>()
                                {
                                    model.set_vec(cards);
                                }
                                window.global::<LibraryState>().set_loading(false);
                                window.global::<StoreState>().set_loading(false);
                                window.global::<StoreState>().set_details_loading(false);
                                window
                                    .global::<StoreState>()
                                    .set_install_status(SharedString::from("Installed"));
                                window.global::<StoreState>().set_install_progress(1.0);
                                window.global::<LibraryState>().invoke_entered();
                            });
                        }
                        Err(error) => {
                            error!(%error, platform = %platform, "installed game saved but library refresh failed");
                            let _ = window.upgrade_in_event_loop(move |window| {
                                window
                                    .global::<StoreState>()
                                    .set_install_status(SharedString::from("Installed; library refresh failed"));
                                window.global::<StoreState>().set_install_progress(1.0);
                            });
                        }
                    }
                }
                Err(error) => {
                    error!(%error, "Store installation failed");
                    let _ = window.upgrade_in_event_loop(move |window| {
                        window
                            .global::<StoreState>()
                            .set_install_status(SharedString::from(format!(
                                "Install failed: {error}"
                            )));
                        window.global::<StoreState>().set_install_progress(0.0);
                    });
                }
            }
        });
    });
    let store_query_state = library_state.clone();
    let store_query_window = window.as_weak();
    let store_query_roms = store_roms.clone();
    // Generation guard: quick navigation between platforms supersedes the
    // in-flight stream; stale pages must not touch the cache-backed list.
    let query_generation: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    let store_refresh_state = library_state.clone();
    let store_refresh_window = window.as_weak();
    window.global::<StoreState>().on_entered(move || {
        let state = store_refresh_state
            .lock()
            .ok()
            .and_then(|state| state.clone());
        let Some(state) = state else { return };
        let Some(backend) = state.stores.get("romm").cloned() else {
            return;
        };
        let window = store_refresh_window.clone();
        let _ = window.upgrade_in_event_loop(|window| {
            window.global::<StoreState>().set_loading(true);
        });
        tokio::spawn(async move {
            match backend.list_platforms().await {
                Ok(platforms) => {
                    let icon_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("ui/assets/platforms/systematic");
                    // Publish the list immediately; icons resolve
                    // concurrently per row instead of blocking the list on
                    // a sequential await chain.
                    let icon_jobs = platforms
                        .iter()
                        .filter_map(|platform| {
                            platform_asset_path(&icon_root, &platform.slug)
                                .map(|path| (platform.slug.clone(), path))
                        })
                        .collect::<Vec<_>>();
                    let count = platforms.len();
                    let _ = window.upgrade_in_event_loop(move |window| {
                        let cards = platforms
                            .into_iter()
                            .map(|platform| PlatformCardData {
                                slug: SharedString::from(platform.slug),
                                name: SharedString::from(platform.name),
                                game_count: SharedString::from(
                                    platform
                                        .game_count
                                        .map(|count| count.to_string())
                                        .unwrap_or_default(),
                                ),
                                icon: Image::default(),
                            })
                            .collect::<Vec<_>>();
                        window
                            .global::<StoreState>()
                            .set_platforms(ModelRc::from(std::rc::Rc::new(VecModel::from(cards))));
                        window.global::<StoreState>().set_loading(false);
                        window.global::<ToastQueue>().invoke_show(
                            SharedString::from(format!("Loaded {count} store platforms")),
                            ToastVariant::Success,
                        );
                    });
                    for (slug, path) in icon_jobs {
                        let icon_window = window.clone();
                        tokio::spawn(async move {
                            let Some(decoded) =
                                image::load_path_scaled(path, "platform-icon", 256).await
                            else {
                                return;
                            };
                            let _ = icon_window.upgrade_in_event_loop(move |window| {
                                let platforms = window.global::<StoreState>().get_platforms();
                                if let Some(index) = (0..platforms.row_count()).find(|&index| {
                                    platforms
                                        .row_data(index)
                                        .is_some_and(|platform| platform.slug.as_str() == slug)
                                }) {
                                    if let Some(mut platform) = platforms.row_data(index) {
                                        platform.icon = image::into_slint_image(decoded).0;
                                        platforms.set_row_data(index, platform);
                                    }
                                }
                            });
                        });
                    }
                }
                Err(error) => {
                    error!(%error, "store platform refresh failed");
                    let message = SharedString::from(format!("Store refresh failed: {error}"));
                    let _ = window.upgrade_in_event_loop(move |window| {
                        window.global::<StoreState>().set_loading(false);
                        window
                            .global::<ToastQueue>()
                            .invoke_show(message, ToastVariant::Error);
                    });
                }
            }
        });
    });
    window
        .global::<StoreState>()
        .on_platform_query(move |slug| {
            let state = store_query_state
                .lock()
                .expect("library state lock poisoned")
                .clone();
            let Some(state) = state else { return };
            let Some(backend) = state.stores.get("romm").cloned() else {
                return;
            };
            let query_window = store_query_window.clone();
            let remote_store = store_query_roms.clone();
            let generation = query_generation.fetch_add(1, Ordering::Relaxed) + 1;
            let query_gen = query_generation.clone();
            tokio::spawn(async move {
                // Cached-first: each backend owns its own `<backend>.db` file.
                // The cache may hold a partial platform (e.g. only the first
                // page from an older fetch), so a non-empty cache is shown
                // immediately but never ends the stream: below we compare
                // against the advertised total and top up what's missing.
                let mut cache_shown = false;
                let mut cached_len = 0_usize;
                if let Ok(cache) = state.store_caches.cache_for(backend.id()) {
                    if let Ok(entries) = cache.browse(Some(&slug), None, usize::MAX, 0) {
                        let cached = decode_roms(&entries);
                        cached_len = cached.len();
                        if !cached.is_empty() {
                            show_roms(&query_window, &remote_store, cached);
                            cache_shown = true;
                        }
                    }
                }
                // Advertised platform size decides whether the cache is
                // already complete. Unknown totals always stream.
                let total = backend
                    .list_platforms()
                    .await
                    .ok()
                    .and_then(|platforms| {
                        platforms
                            .into_iter()
                            .find(|platform| platform.slug == slug.as_str())
                    })
                    .and_then(|platform| platform.game_count);
                if let Some(total) = total {
                    let total = total as usize;
                    if cached_len >= total {
                        let _ = query_window.upgrade_in_event_loop(|window| {
                            window.global::<StoreState>().set_loading(false);
                        });
                        return;
                    }
                    info!(platform = %slug, cached = cached_len, total, "store cache incomplete; streaming platform");
                }
                let _ = query_window.upgrade_in_event_loop(|window| {
                    window.global::<StoreState>().set_loading(true);
                });
                // Stream the platform page by page: every page is upserted
                // into the backend's cache file first, then the visible list
                // is republished from the accumulated rows.
                const PAGE_SIZE: usize = 100;
                let mut offset = 0_usize;
                let mut streamed: Vec<marina_romm::Rom> = Vec::new();
                loop {
                    if query_gen.load(Ordering::Relaxed) != generation {
                        return;
                    }
                    let entries = match backend
                        .browse(marina_store::StoreQuery {
                            platform_slug: Some(slug.to_string()),
                            limit: PAGE_SIZE,
                            offset,
                            ..Default::default()
                        })
                        .await
                    {
                        Ok(entries) => entries,
                        Err(error) => {
                            error!(%error, platform = %slug, "Store game loading failed");
                            let _ = query_window.upgrade_in_event_loop(|window| {
                                window.global::<StoreState>().set_loading(false);
                            });
                            return;
                        }
                    };
                    if entries.is_empty() {
                        break;
                    }
                    if let Ok(cache) = state.store_caches.cache_for(backend.id()) {
                        if let Err(error) = cache.upsert_entries(&entries) {
                            error!(%error, "store catalog cache write failed");
                        }
                    }
                    let fetched = entries.len();
                    if query_gen.load(Ordering::Relaxed) != generation {
                        return;
                    }
                    streamed.extend(decode_roms(&entries));
                    // Replace (not extend): the stream restarts at offset 0,
                    // so the store mirrors exactly what's been streamed.
                    *remote_store.lock().expect("remote Store state poisoned") =
                        streamed.clone();
                    if !cache_shown {
                        // First paint also selects the first game.
                        show_roms(
                            &query_window,
                            &remote_store,
                            streamed.clone(),
                        );
                        cache_shown = true;
                    } else {
                        republish_roms(&query_window, &remote_store);
                    }
                    info!(platform = %slug, offset, rows = fetched, "store catalog page streamed");
                    if fetched < PAGE_SIZE {
                        break;
                    }
                    offset += fetched;
                }
                let _ = query_window.upgrade_in_event_loop(|window| {
                    window.global::<StoreState>().set_loading(false);
                });
            });
        });

    let detail_store = store_roms.clone();
    let detail_state = library_state.clone();
    let detail_window = window.as_weak();
    window
        .global::<StoreState>()
        .on_game_selected(move |id, _index| {
            let cached_rom = detail_store
                .lock()
                .expect("remote Store state poisoned")
                .iter()
                .find(|rom| rom.id.to_string() == id.as_str())
                .cloned();
            let Some(state) = detail_state
                .lock()
                .expect("library state lock poisoned")
                .clone()
            else {
                return;
            };
            let Some(backend) = state.stores.get("romm").cloned() else {
                return;
            };
            // Cover URLs need the backend's native base URL, which the trait
            // doesn't cover: downcast back to RomM for that one value.
            // Detail hydration itself (`get`) stays on the trait.
            let Some(romm) = backend.as_any().downcast_ref::<marina_romm::RommStore>() else {
                return;
            };
            let base_url = romm.base_url().to_owned();
            // Unload the pane first: stale text, tags, artifacts, and cover
            // from the previous entry must not linger while the new one
            // hydrates. An empty pane beats a stale one.
            if let Some(window) = detail_window.upgrade() {
                window
                    .global::<StoreState>()
                    .set_details(PreviewDetailsData::default());
                window
                    .global::<StoreState>()
                    .set_tags(crate::string_model(Vec::new()));
                window
                    .global::<StoreState>()
                    .set_artifacts(ModelRc::from(std::rc::Rc::new(VecModel::from(Vec::<
                        StoreArtifact,
                    >::new(
                    )))));
                window
                    .global::<StoreState>()
                    .set_preview_image(Image::default());
                window.global::<StoreState>().set_details_loading(true);
            }
            if let Some(rom) = cached_rom {
                crate::populate_store_details(&detail_window, rom, &base_url);
            }
            let detail_roms = detail_store.clone();
            let detail_window = detail_window.clone();
            tokio::spawn(async move {
                match backend.get(&id).await {
                    Ok(Some(entry)) => {
                        if let Some(json) = entry.payload_json {
                            if let Ok(rom) = serde_json::from_str::<marina_romm::Rom>(&json) {
                                *detail_roms.lock().expect("remote Store state poisoned") =
                                    vec![rom.clone()];
                                crate::populate_store_details(&detail_window, rom, &base_url);
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        error!(%error, entry_id = %id, "store game detail hydration failed")
                    }
                }
            });
        });
}
