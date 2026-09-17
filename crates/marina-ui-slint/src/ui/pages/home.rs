//! Home page data loading.

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Model, VecModel};

use super::library as shelf;
use crate::{GameCardData, MainWindow, app, covers, game_cards};

pub(crate) fn install(
    window: &MainWindow,
    library_state: &Arc<Mutex<Option<app::AppStateHandle>>>,
    source_store: &Arc<Mutex<Vec<covers::CoverSource>>>,
    home_source_store: &Arc<Mutex<Vec<covers::CoverSource>>>,
) {
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
}
