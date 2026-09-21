//! Store page event and data coordination (backend-agnostic).
//!
//! Catalog data flows: [`marina_store::StoreBackend`] -> per-backend
//! [`marina_store::StoreCache`] file (`<dir>/<backend>.db`) -> UI cards.
//! Browse pages retain only lightweight card fields. Full backend records are
//! scoped to detail/install futures and drop when those operations complete.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};
use tracing::{debug, error, info};

use super::library as shelf;
use crate::{
    GameCardData, HomeState, LibraryState, MainWindow, PlatformCardData, PreviewDetailsData,
    StoreArtifact, StoreState, ToastQueue, ToastVariant, game_cards, platform_asset_path,
};
use crate::{app, image};

#[derive(Debug)]
struct StorePlatformSession {
    generation: u64,
    slug: String,
    catalog_tasks: Vec<tokio::task::AbortHandle>,
    selection_tasks: Vec<tokio::task::AbortHandle>,
}

impl Drop for StorePlatformSession {
    fn drop(&mut self) {
        for task in self.catalog_tasks.drain(..) {
            task.abort();
        }
        for task in self.selection_tasks.drain(..) {
            task.abort();
        }
        debug!(
            generation = self.generation,
            platform = %self.slug,
            "store platform session dropped"
        );
    }
}

type ActiveStoreSession = Arc<Mutex<Option<StorePlatformSession>>>;
type CardParts = (String, String, String);

fn session_is_active(session: &ActiveStoreSession, generation: u64) -> bool {
    session
        .lock()
        .expect("Store session state poisoned")
        .as_ref()
        .is_some_and(|session| session.generation == generation)
}

fn register_catalog_task(
    active_session: &ActiveStoreSession,
    generation: u64,
    task: tokio::task::AbortHandle,
) {
    let mut active = active_session.lock().expect("Store session state poisoned");
    if let Some(session) = active
        .as_mut()
        .filter(|session| session.generation == generation)
    {
        session.catalog_tasks.push(task);
    } else {
        task.abort();
    }
}

fn begin_selection(active_session: &ActiveStoreSession, generation: u64) -> bool {
    let mut active = active_session.lock().expect("Store session state poisoned");
    let Some(session) = active
        .as_mut()
        .filter(|session| session.generation == generation)
    else {
        return false;
    };
    for task in session.selection_tasks.drain(..) {
        task.abort();
    }
    true
}

fn register_selection_task(
    active_session: &ActiveStoreSession,
    generation: u64,
    task: tokio::task::AbortHandle,
) {
    let mut active = active_session.lock().expect("Store session state poisoned");
    if let Some(session) = active
        .as_mut()
        .filter(|session| session.generation == generation)
    {
        session.selection_tasks.push(task);
    } else {
        task.abort();
    }
}

fn entry_card(entry: &marina_store::StoreEntry) -> CardParts {
    (
        entry.entry_id.clone(),
        entry.title.clone(),
        entry
            .platform_name
            .clone()
            .unwrap_or_else(|| entry.platform_slug.clone()),
    )
}

/// Builds UI cards. Must run on the event-loop thread: GameCardData holds
/// a Slint image and is not Send, so only plain tuples cross threads.
fn deduplicate_card_parts(parts: Vec<CardParts>) -> Vec<CardParts> {
    let mut seen = std::collections::HashSet::new();
    parts
        .into_iter()
        .filter(|(_, title, _)| seen.insert(title.trim().to_lowercase()))
        .collect()
}

fn cards_from(parts: Vec<CardParts>) -> Vec<GameCardData> {
    deduplicate_card_parts(parts)
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

fn publish_cards(
    window: &slint::Weak<MainWindow>,
    active_session: ActiveStoreSession,
    generation: u64,
    parts: Vec<CardParts>,
    select_first: bool,
) {
    let _ = window.upgrade_in_event_loop(move |window| {
        if !session_is_active(&active_session, generation) {
            return;
        }
        let cards = cards_from(parts);
        let first_id = select_first
            .then(|| cards.first().map(|game| game.id.clone()))
            .flatten();
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

pub(crate) fn install(
    window: &MainWindow,
    library_state: &Arc<Mutex<Option<app::AppStateHandle>>>,
) {
    window.global::<StoreState>().on_search(|query| {
        info!(query = %query, "Store search requested; RomM result loading is next");
    });

    let active_session: ActiveStoreSession = Arc::new(Mutex::new(None));
    let install_state = library_state.clone();
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
        let selected = selected.iter().collect::<Vec<_>>();
        if !selected.iter().any(|selected| *selected) {
            return;
        }
        let window = install_window.clone();
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
            let Some(rom) = entry
                .payload_json
                .as_deref()
                .and_then(|json| serde_json::from_str::<marina_romm::Rom>(json).ok())
            else {
                error!(entry_id = %entry_id, "store entry payload could not be decoded for install");
                return;
            };
            let file_ids = rom
                .files
                .files
                .iter()
                .zip(selected)
                .filter_map(|(file, selected)| selected.then_some(file.id.to_string()))
                .collect::<Vec<_>>();
            if file_ids.is_empty() {
                return;
            }
            let selected_count = file_ids.len();
            let status_window = window.clone();
            let _ = status_window.upgrade_in_event_loop(move |window| {
                window
                    .global::<StoreState>()
                    .set_install_status(SharedString::from(format!(
                        "Installing {selected_count} file(s)…"
                    )));
                window.global::<StoreState>().set_install_progress(0.0);
            });
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
                                // Refresh every stale consumer now: the
                                // library tab reloads platforms/counts, and
                                // the home shelf reloads recently-added, so
                                // the game is already there when switching
                                // tabs instead of needing a revisit.
                                window.global::<LibraryState>().invoke_entered();
                                window.global::<HomeState>().invoke_entered();
                            });
                        }
                        Err(error) => {
                            error!(%error, platform = %platform, "installed game saved but library refresh failed");
                            let _ = window.upgrade_in_event_loop(move |window| {
                                window
                                    .global::<StoreState>()
                                    .set_install_status(SharedString::from("Installed; library refresh failed"));
                                window.global::<StoreState>().set_install_progress(1.0);
                                // The save itself succeeded: still refresh both
                                // consumers so the game appears everywhere.
                                window.global::<LibraryState>().invoke_entered();
                                window.global::<HomeState>().invoke_entered();
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
    let store_query_session = active_session.clone();
    // Generation guard: quick navigation between platforms supersedes the
    // in-flight stream; stale pages must not touch the cache-backed list.
    let query_generation: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    let store_refresh_state = library_state.clone();
    let store_refresh_window = window.as_weak();
    window.global::<StoreState>().on_entered(move || {
        if store_refresh_window.upgrade().is_some_and(|window| {
            let store = window.global::<StoreState>();
            if store.get_platforms().row_count() > 0 {
                store.set_loading(false);
                true
            } else {
                false
            }
        }) {
            return;
        }
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
    let store_query_generation = query_generation.clone();
    window
        .global::<StoreState>()
        .on_platform_query(move |slug| {
            if let Some(window) = store_query_window.upgrade() {
                crate::ui::nav::drill_store(&window, slug.as_str());
            }
            let state = store_query_state
                .lock()
                .expect("library state lock poisoned")
                .clone();
            let Some(state) = state else { return };
            let Some(backend) = state.stores.get("romm").cloned() else {
                return;
            };
            let query_window = store_query_window.clone();
            let generation = store_query_generation.fetch_add(1, Ordering::Relaxed) + 1;
            {
                let mut active = store_query_session
                    .lock()
                    .expect("Store session state poisoned");
                *active = Some(StorePlatformSession {
                    generation,
                    slug: slug.to_string(),
                    catalog_tasks: Vec::new(),
                    selection_tasks: Vec::new(),
                });
            }
            let query_session = store_query_session.clone();
            let task_session = query_session.clone();
            let task = tokio::spawn(async move {
                // Cached-first: each backend owns its own `<backend>.db` file.
                // The cache may hold a partial platform (e.g. only the first
                // page from an older fetch), so a non-empty cache is shown
                // immediately but never ends the stream: below we compare
                // against the advertised total and top up what's missing.
                let mut cache_shown = false;
                let mut cached_len = 0_usize;
                if let Ok(cache) = state.store_caches.cache_for(backend.id()) {
                    if let Ok(entries) = cache.browse_cards(Some(&slug), None, usize::MAX, 0) {
                        cached_len = entries.len();
                        let cached_cards = entries.iter().map(entry_card).collect::<Vec<_>>();
                        if !cached_cards.is_empty() {
                            publish_cards(
                                &query_window,
                                query_session.clone(),
                                generation,
                                cached_cards,
                                true,
                            );
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
                        let active_session = query_session.clone();
                        let _ = query_window.upgrade_in_event_loop(move |window| {
                            if session_is_active(&active_session, generation) {
                                window.global::<StoreState>().set_loading(false);
                            }
                        });
                        return;
                    }
                    info!(platform = %slug, cached = cached_len, total, "store cache incomplete; streaming platform");
                }
                let loading_session = query_session.clone();
                let _ = query_window.upgrade_in_event_loop(move |window| {
                    if session_is_active(&loading_session, generation) {
                        window.global::<StoreState>().set_loading(true);
                    }
                });
                // Stream the platform page by page: every page is upserted
                // into the backend's cache file first, then the visible list
                // is republished from the accumulated rows.
                const PAGE_SIZE: usize = 100;
                let mut offset = 0_usize;
                let mut streamed: Vec<CardParts> = Vec::new();
                loop {
                    if !session_is_active(&query_session, generation) {
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
                            let error_session = query_session.clone();
                            let _ = query_window.upgrade_in_event_loop(move |window| {
                                if session_is_active(&error_session, generation) {
                                    window.global::<StoreState>().set_loading(false);
                                }
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
                    if !session_is_active(&query_session, generation) {
                        return;
                    }
                    streamed.extend(entries.iter().map(entry_card));
                    publish_cards(
                        &query_window,
                        query_session.clone(),
                        generation,
                        streamed.clone(),
                        !cache_shown,
                    );
                    cache_shown = true;
                    info!(platform = %slug, offset, rows = fetched, "store catalog page streamed");
                    if fetched < PAGE_SIZE {
                        break;
                    }
                    offset += fetched;
                }
                let complete_session = query_session.clone();
                let _ = query_window.upgrade_in_event_loop(move |window| {
                    if session_is_active(&complete_session, generation) {
                        window.global::<StoreState>().set_loading(false);
                    }
                });
            });
            register_catalog_task(&task_session, generation, task.abort_handle());
        });

    let unload_session = active_session.clone();
    let unload_generation = query_generation.clone();
    let unload_window = window.as_weak();
    window.global::<StoreState>().on_platform_exited(move || {
        // Taking the session ends the platform's ownership scope. Its Drop
        // runs here; queued work sees no active session and cannot republish.
        unload_generation.fetch_add(1, Ordering::Relaxed);
        let unloaded_session = unload_session
            .lock()
            .expect("Store session state poisoned")
            .take();
        drop(unloaded_session);

        let Some(window) = unload_window.upgrade() else {
            return;
        };
        let store = window.global::<StoreState>();
        store.set_games(ModelRc::from(std::rc::Rc::new(VecModel::from(Vec::<
            GameCardData,
        >::new(
        )))));
        store.set_details(PreviewDetailsData::default());
        store.set_tags(crate::string_model(Vec::new()));
        store.set_preview_image(Image::default());
        store.set_artifacts(ModelRc::from(std::rc::Rc::new(VecModel::from(Vec::<
            StoreArtifact,
        >::new(
        )))));
        store.set_selected_artifacts(std::rc::Rc::new(VecModel::from(Vec::<bool>::new())).into());
        store.set_selected_game_index(0);
        store.set_selected_artifact_index(0);
        store.set_game_list_scroll_y(0.0);
        store.set_loading(false);
        store.set_details_loading(false);
        crate::image::schedule_allocator_trim();
    });

    let detail_session = active_session.clone();
    let detail_state = library_state.clone();
    let detail_generation = query_generation.clone();
    let detail_window = window.as_weak();
    window
        .global::<StoreState>()
        .on_game_selected(move |id, _index| {
            let generation = detail_generation.load(Ordering::Relaxed);
            if !begin_selection(&detail_session, generation) {
                return;
            }
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
                let store = window.global::<StoreState>();
                let games = store.get_games();
                for index in 0..games.row_count() {
                    if let Some(mut game) = games.row_data(index)
                        && game.cover.size().width > 0
                    {
                        game.cover = Image::default();
                        game.cover_ratio = 1.0;
                        games.set_row_data(index, game);
                    }
                }
                store.set_details(PreviewDetailsData::default());
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
            let detail_window = detail_window.clone();
            let active_session = detail_session.clone();
            let task_session = active_session.clone();
            let generation_guard = detail_generation.clone();
            let task = tokio::spawn(async move {
                if let Ok(cache) = state.store_caches.cache_for(backend.id())
                    && let Ok(Some(entry)) = cache.get(id.as_str())
                    && let Some(json) = entry.payload_json
                    && let Ok(rom) = serde_json::from_str::<marina_romm::Rom>(&json)
                    && session_is_active(&active_session, generation)
                {
                    if let Some(task) = crate::populate_store_details(
                        &detail_window,
                        rom,
                        &base_url,
                        generation_guard.clone(),
                        generation,
                        false,
                    ) {
                        register_selection_task(&active_session, generation, task);
                    }
                }

                if !session_is_active(&active_session, generation) {
                    return;
                }
                match backend.get(&id).await {
                    Ok(Some(entry)) => {
                        if let Some(json) = entry.payload_json
                            && let Ok(rom) = serde_json::from_str::<marina_romm::Rom>(&json)
                            && session_is_active(&active_session, generation)
                        {
                            if let Some(task) = crate::populate_store_details(
                                &detail_window,
                                rom,
                                &base_url,
                                generation_guard,
                                generation,
                                true,
                            ) {
                                register_selection_task(&active_session, generation, task);
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        error!(%error, entry_id = %id, "store game detail hydration failed")
                    }
                }
            });
            register_selection_task(&task_session, generation, task.abort_handle());
        });
}

#[cfg(test)]
mod tests {
    use super::{CardParts, deduplicate_card_parts};

    #[test]
    fn duplicate_titles_are_grouped_case_insensitively() {
        let cards: Vec<CardParts> = vec![
            ("1".into(), "Final Fight 3".into(), "SNES".into()),
            ("2".into(), " final fight 3 ".into(), "SNES".into()),
            ("3".into(), "Final Fight 2".into(), "SNES".into()),
        ];

        let grouped = deduplicate_card_parts(cards);

        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].0, "1");
        assert_eq!(grouped[1].0, "3");
    }
}
