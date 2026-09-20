//! Game launch event handling.

use std::sync::{Arc, Mutex};

use marina_core::LibraryItemId;
use marina_library::read::LibraryRead;
use marina_runtime::GameLauncher;
use slint::ComponentHandle;
use tracing::{error, info, warn};

use crate::ui::pages::home;
use crate::{GameState, MainWindow, app};

pub(crate) fn install(
    window: &MainWindow,
    library_state: &Arc<Mutex<Option<app::AppStateHandle>>>,
    played_store: &home::PlayedStore,
    played_sources: &home::PlayedSources,
) {
    let play_state = library_state.clone();
    let played_store = played_store.clone();
    let played_sources = played_sources.clone();
    let played_window = window.as_weak();
    let game_launcher = GameLauncher::new();
    window.global::<GameState>().on_play_requested(move |id| {
        let state = play_state
            .lock()
            .expect("library state lock poisoned")
            .clone();
        let Some(state) = state else {
            warn!(game_id = %id, "play requested but library state is unavailable");
            return;
        };
        let Ok(item_id) = LibraryItemId::parse(id.as_str()).ok_or(()) else {
            warn!(game_id = %id, "play requested with invalid library item id");
            return;
        };
        let game_launcher = game_launcher.clone();
        let played_store = played_store.clone();
        let played_sources = played_sources.clone();
        let played_window = played_window.clone();
        tokio::spawn(async move {
            match state.library.get(&item_id).await {
                Ok(Some(item)) => match game_launcher.launch_item(&item).await {
                    Ok(launched) => {
                        info!(
                            game_id = %id,
                            unit = %launched.unit_name,
                            "game launch requested"
                        );
                        if let Err(error) = state.library.record_play(&item.id) {
                            error!(%error, game_id = %id, "failed to persist play activity");
                        }
                        home::record_played(
                            &played_store,
                            &played_sources,
                            &played_window,
                            &item,
                            state.config.romm_url.as_deref(),
                        );
                    }
                    Err(error) => error!(%error, game_id = %id, "failed to launch game"),
                },
                Ok(None) => {
                    warn!(game_id = %id, "play requested for missing library item")
                }
                Err(error) => {
                    error!(%error, game_id = %id, "failed to resolve game for launch")
                }
            }
        });
    });
}
