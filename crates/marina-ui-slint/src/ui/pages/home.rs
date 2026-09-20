//! Home page data loading.

use std::sync::{Arc, Mutex};

use marina_core::LibraryAssetKind;
use slint::{ComponentHandle, Model, VecModel};

use super::library as shelf;
use crate::{GameCardData, HomeState, MainWindow, app, covers, game_cards, image as image_loader};

#[derive(Clone)]
pub(crate) struct PlayedEntry {
    id: String,
    title: String,
    platform: String,
    source: covers::CoverSource,
}

pub(crate) type PlayedStore = Arc<Mutex<Vec<PlayedEntry>>>;

pub(crate) fn new_played_store() -> PlayedStore {
    Arc::new(Mutex::new(Vec::new()))
}

const MAX_PLAYED: usize = 20;

pub(crate) fn record_played(
    store: &PlayedStore,
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
    }

    let source = entry.source.clone();
    let played_id = entry.id.clone();
    let _ = window.upgrade_in_event_loop(move |window| {
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

    spawn_cover_decode(window, source, played_id);
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
) {
    let items = state
        .library
        .recently_played_items(MAX_PLAYED)
        .unwrap_or_default();
    if items.is_empty() {
        return;
    }
    {
        let mut entries = store.lock().expect("played store poisoned");
        entries.clear();
        entries.extend(
            items
                .iter()
                .map(|item| played_entry(item, state.config.romm_url.as_deref())),
        );
    }
    let window = window.clone();
    let store = store.clone();
    let _ = window.upgrade_in_event_loop(move |window| {
        publish_played(&window, &store);
    });
}

fn publish_played(window: &MainWindow, store: &PlayedStore) {
    let cards = snapshot_cards(store);
    with_played_model(window, |model| model.set_vec(cards));
    let entries: Vec<(String, covers::CoverSource)> = store
        .lock()
        .expect("played store poisoned")
        .iter()
        .map(|entry| (entry.id.clone(), entry.source.clone()))
        .collect();
    for (id, source) in entries {
        spawn_cover_decode(&window.as_weak(), source, id);
    }
}

fn spawn_cover_decode(
    window: &slint::Weak<MainWindow>,
    source: covers::CoverSource,
    played_id: String,
) {
    let decode_window = window.clone();
    tokio::spawn(async move {
        let Some(decoded) = image_loader::load_scaled(
            &image_loader::ImageSource::from(&source),
            "shelf-cover",
            256,
        )
        .await
        else {
            return;
        };
        let _ = decode_window.upgrade_in_event_loop(move |window| {
            let (image, ratio) = image_loader::into_slint_image(decoded);
            with_played_model(&window, |model| {
                for index in 0..model.row_count() {
                    if model
                        .row_data(index)
                        .is_some_and(|row| row.id == played_id.as_str())
                    {
                        let mut row = model.row_data(index).expect("row read above");
                        row.cover = image.clone();
                        row.cover_ratio = ratio;
                        model.set_row_data(index, row);
                        break;
                    }
                }
            });
        });
    });
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
    let home_source_store = home_source_store.clone();
    let played_store = played_store.clone();

    window.global::<HomeState>().on_entered(move || {
        if let Some(window) = home_window.upgrade() {
            publish_played(&window, &played_store);
        }
        let Some(state) = home_state.lock().ok().and_then(|state| state.clone()) else {
            return;
        };
        let window = home_window.clone();
        let home_source_store = home_source_store.clone();
        tokio::spawn(async move {
            if let Ok((metadata, cover_sources)) =
                shelf::load_games(&state.library, state.config.romm_url.as_deref()).await
            {
                *home_source_store
                    .lock()
                    .expect("home cover source state poisoned") = cover_sources;
                let _ = window.upgrade_in_event_loop(move |window| {
                    if let Some(model) = window
                        .global::<HomeState>()
                        .get_games()
                        .as_any()
                        .downcast_ref::<VecModel<GameCardData>>()
                    {
                        model.set_vec(game_cards(metadata));
                    }
                    window.global::<HomeState>().set_loading(false);
                });
                if window.upgrade().is_some() {
                    let _ = window.upgrade_in_event_loop(|window| {
                        window.global::<HomeState>().invoke_cover_context_changed(0);
                    });
                }
            } else {
                let _ = window.upgrade_in_event_loop(|window| {
                    window.global::<HomeState>().set_loading(false);
                });
            }
        });
    });
}
