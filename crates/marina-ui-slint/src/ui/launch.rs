//! Game launch event handling.

use std::sync::{Arc, Mutex};

use marina_core::LibraryItemId;
use marina_library::read::LibraryRead;
use marina_runtime::GameLauncher;
use slint::ComponentHandle;
use tracing::{error, info, warn};

use crate::{GameState, MainWindow, app};

pub(crate) fn install(
    window: &MainWindow,
    library_state: &Arc<Mutex<Option<app::AppStateHandle>>>,
) {
    let play_state = library_state.clone();
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
        tokio::spawn(async move {
            match state.library.get(&item_id).await {
                Ok(Some(item)) => match game_launcher.launch_item(&item).await {
                    Ok(launched) => info!(
                        game_id = %id,
                        unit = %launched.unit_name,
                        "game launch requested"
                    ),
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
