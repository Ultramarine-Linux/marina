//! Home page data loading.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use marina_core::LibraryAssetKind;
use slint::{ComponentHandle, Model, VecModel};
use tracing::debug;

use super::library as shelf;
use crate::{GameCardData, HomeState, MainWindow, ShellPage, ShellState, app, covers, game_cards};

#[derive(Clone)]
pub(crate) struct PlayedEntry {
    id: String,
    title: String,
    platform: String,
    source: covers::CoverSource,
}

pub(crate) type PlayedStore = Arc<Mutex<Vec<PlayedEntry>>>;

/// Cover sources for the played shelf, index-aligned with the played model.
/// Fed to the viewport loader; rebuilt on every played-store mutation.
pub(crate) type PlayedSources = Arc<Mutex<Vec<covers::CoverSource>>>;

pub(crate) fn new_played_store() -> PlayedStore {
    Arc::new(Mutex::new(Vec::new()))
}

const MAX_PLAYED: usize = 20;

#[derive(Debug)]
struct HomeSession {
    generation: u64,
    tasks: Vec<tokio::task::AbortHandle>,
}

impl Drop for HomeSession {
    fn drop(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
        debug!(generation = self.generation, "Home session dropped");
    }
}

type ActiveHomeSession = Arc<Mutex<Option<HomeSession>>>;

fn home_session_is_active(session: &ActiveHomeSession, generation: u64) -> bool {
    session
        .lock()
        .expect("Home session state poisoned")
        .as_ref()
        .is_some_and(|session| session.generation == generation)
}

fn register_home_task(
    active_session: &ActiveHomeSession,
    generation: u64,
    task: tokio::task::AbortHandle,
) {
    let mut active = active_session.lock().expect("Home session state poisoned");
    if let Some(session) = active
        .as_mut()
        .filter(|session| session.generation == generation)
    {
        session.tasks.push(task);
    } else {
        task.abort();
    }
}

pub(crate) fn record_played(
    store: &PlayedStore,
    played_sources: &PlayedSources,
    window: &slint::Weak<MainWindow>,
    item: &marina_core::LibraryItem,
    base_url: Option<&str>,
) {
    let entry = played_entry(item, base_url);

    {
        let mut entries = store.lock().expect("played store poisoned");
        entries.retain(|existing| existing.id != entry.id);
        entries.insert(0, entry.clone());
        entries.truncate(MAX_PLAYED);
        *played_sources.lock().expect("played sources poisoned") =
            entries.iter().map(|entry| entry.source.clone()).collect();
    }

    let _ = window.upgrade_in_event_loop(move |window| {
        if window.global::<ShellState>().get_page() != ShellPage::Home {
            return;
        }
        with_played_model(&window, |model| {
            if let Some(index) = (0..model.row_count()).find(|&index| {
                model
                    .row_data(index)
                    .is_some_and(|row| row.id == entry.id.as_str())
            }) {
                model.remove(index);
            }
            model.insert(
                0,
                GameCardData {
                    id: entry.id.clone().into(),
                    title: entry.title.clone().into(),
                    platform: entry.platform.clone().into(),
                    cover: slint::Image::default(),
                    cover_ratio: 1.0,
                },
            );
            while model.row_count() > MAX_PLAYED {
                model.remove(model.row_count() - 1);
            }
        });
    });
    // Covers load through the viewport loader (played shelf included), which
    // picks the new row up on the next viewport report or context refresh.
}

fn played_entry(item: &marina_core::LibraryItem, base_url: Option<&str>) -> PlayedEntry {
    let local_cover = item
        .assets
        .iter()
        .find(|asset| matches!(asset.kind, LibraryAssetKind::CoverSmall))
        .or_else(|| {
            item.assets
                .iter()
                .find(|asset| matches!(asset.kind, LibraryAssetKind::CoverLarge))
        })
        .and_then(|asset| asset.local_path.clone());
    PlayedEntry {
        id: item.id.to_string(),
        title: item.title.clone(),
        platform: item
            .platform_slug
            .clone()
            .unwrap_or_else(|| "Unknown".into()),
        source: covers::source_for(item.cover.as_deref(), local_cover.as_deref(), base_url),
    }
}

/// Fills the in-memory played store from persisted activity so the
/// recently-played shelf survives restarts, then publishes it if the window
/// is already up.
pub(crate) async fn hydrate_played(
    state: app::AppStateHandle,
    window: slint::Weak<MainWindow>,
    store: PlayedStore,
    played_sources: PlayedSources,
) {
    let items = state
        .library
        .recently_played_items(MAX_PLAYED)
        .unwrap_or_default();
    if items.is_empty() {
        return;
    }
    {
        let config = state.config.snapshot();
        let mut entries = store.lock().expect("played store poisoned");
        entries.clear();
        entries.extend(
            items
                .iter()
                .map(|item| played_entry(item, config.romm_url.as_deref())),
        );
        *played_sources.lock().expect("played sources poisoned") =
            entries.iter().map(|entry| entry.source.clone()).collect();
    }
    let window = window.clone();
    let store = store.clone();
    let _ = window.upgrade_in_event_loop(move |window| {
        publish_played(&window, &store);
    });
}

fn publish_played(window: &MainWindow, store: &PlayedStore) {
    if window.global::<ShellState>().get_page() != ShellPage::Home {
        return;
    }
    let cards = snapshot_cards(store);
    with_played_model(window, |model| {
        if !same_cards(model, &cards) {
            model.set_vec(cards);
        }
    });
    // Covers resolve through the viewport loader like every other shelf;
    // this refresh replays the last viewport per shelf.
    window.global::<HomeState>().invoke_cover_context_changed(0);
}

fn with_played_model(window: &MainWindow, update: impl FnOnce(&VecModel<GameCardData>)) {
    if let Some(model) = window
        .global::<HomeState>()
        .get_played_games()
        .as_any()
        .downcast_ref::<VecModel<GameCardData>>()
    {
        update(model);
    }
}

fn same_cards(model: &VecModel<GameCardData>, cards: &[GameCardData]) -> bool {
    model.row_count() == cards.len()
        && cards.iter().enumerate().all(|(index, card)| {
            model.row_data(index).is_some_and(|current| {
                current.id == card.id
                    && current.title == card.title
                    && current.platform == card.platform
            })
        })
}

fn snapshot_cards(store: &PlayedStore) -> Vec<GameCardData> {
    store
        .lock()
        .expect("played store poisoned")
        .iter()
        .map(|entry| GameCardData {
            id: entry.id.clone().into(),
            title: entry.title.clone().into(),
            platform: entry.platform.clone().into(),
            cover: slint::Image::default(),
            cover_ratio: 1.0,
        })
        .collect()
}

pub(crate) fn install(
    window: &MainWindow,
    library_state: &Arc<Mutex<Option<app::AppStateHandle>>>,
    home_source_store: &Arc<Mutex<Vec<covers::CoverSource>>>,
    played_store: &PlayedStore,
) {
    let home_state = library_state.clone();
    let home_window = window.as_weak();
    let entered_home_sources = home_source_store.clone();
    let played_store = played_store.clone();
    let home_generation = Arc::new(AtomicU64::new(0));
    let active_session: ActiveHomeSession = Arc::new(Mutex::new(None));
    let entered_generation = home_generation.clone();
    let entered_session = active_session.clone();

    window.global::<HomeState>().on_entered(move || {
        let generation = entered_generation.fetch_add(1, Ordering::Relaxed) + 1;
        *entered_session.lock().expect("Home session state poisoned") = Some(HomeSession {
            generation,
            tasks: Vec::new(),
        });
        if let Some(window) = home_window.upgrade() {
            publish_played(&window, &played_store);
        }
        let Some(state) = home_state.lock().ok().and_then(|state| state.clone()) else {
            return;
        };
        let window = home_window.clone();
        let home_source_store = entered_home_sources.clone();
        let request_session = entered_session.clone();
        let task_session = request_session.clone();
        let task = tokio::spawn(async move {
            let config = state.config.snapshot();
            if let Ok((metadata, cover_sources)) =
                shelf::load_games(&state.library, config.romm_url.as_deref()).await
            {
                let _ = window.upgrade_in_event_loop(move |window| {
                    // The query may finish after a tab change. Do not republish
                    // Home cards or restart cover loading while Home is hidden.
                    if !home_session_is_active(&request_session, generation)
                        || window.global::<ShellState>().get_page() != ShellPage::Home
                    {
                        return;
                    }
                    *home_source_store
                        .lock()
                        .expect("home cover source state poisoned") = cover_sources;
                    if let Some(model) = window
                        .global::<HomeState>()
                        .get_games()
                        .as_any()
                        .downcast_ref::<VecModel<GameCardData>>()
                    {
                        let cards = game_cards(metadata);
                        if !same_cards(model, &cards) {
                            model.set_vec(cards);
                        }
                    }
                    window.global::<HomeState>().set_loading(false);
                    window.global::<HomeState>().invoke_cover_context_changed(0);
                });
            } else {
                let _ = window.upgrade_in_event_loop(|window| {
                    window.global::<HomeState>().set_loading(false);
                });
            }
        });
        register_home_task(&task_session, generation, task.abort_handle());
    });

    let exited_generation = home_generation;
    let exited_session = active_session;
    let exited_window = window.as_weak();
    window.global::<HomeState>().on_exited(move || {
        exited_generation.fetch_add(1, Ordering::Relaxed);
        let unloaded_session = exited_session
            .lock()
            .expect("Home session state poisoned")
            .take();
        drop(unloaded_session);
        if let Some(window) = exited_window.upgrade() {
            window.global::<HomeState>().set_loading(false);
        }
        crate::image::schedule_allocator_trim();
    });
}
