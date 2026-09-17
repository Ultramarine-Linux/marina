//! Home page data loading.

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Model, VecModel};

use super::library as shelf;
use crate::{GameCardData, HomeState, MainWindow, app, covers, game_cards};

pub(crate) fn install(
    window: &MainWindow,
    library_state: &Arc<Mutex<Option<app::AppStateHandle>>>,
    home_source_store: &Arc<Mutex<Vec<covers::CoverSource>>>,
) {
    let home_state = library_state.clone();
    let home_window = window.as_weak();
    let home_source_store = home_source_store.clone();

    window.global::<HomeState>().on_entered(move || {
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
