//! Game launch event handling.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use marina_core::LibraryItemId;
use marina_library::read::LibraryRead;
use marina_runtime::{GameLauncher, LaunchedGame};
use slint::ComponentHandle;
use tracing::{error, info, warn};

use crate::ui::pages::home;
use crate::{GameState, MainWindow, app};

const GAME_STATUS_POLL_INTERVAL: Duration = Duration::from_millis(250);
const GAME_STARTUP_GRACE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
enum LaunchUiStatus {
    Idle,
    Playing,
}

fn publish_launch_status(
    window: &slint::Weak<MainWindow>,
    game_id: String,
    status: LaunchUiStatus,
) {
    let _ = window.upgrade_in_event_loop(move |window| {
        let game = window.global::<GameState>();
        if game.get_launching_game_id().as_str() == game_id {
            game.set_launching_game_id("".into());
        }
        match status {
            LaunchUiStatus::Playing => game.set_playing_game_id(game_id.into()),
            LaunchUiStatus::Idle => {
                if game.get_playing_game_id().as_str() == game_id {
                    game.set_playing_game_id("".into());
                }
            }
        }
    });
}

async fn monitor_launched_game(
    launched: LaunchedGame,
    window: slint::Weak<MainWindow>,
    game_id: String,
) {
    let startup_deadline = Instant::now() + GAME_STARTUP_GRACE;
    let mut observed_active = false;
    let mut error_reported = false;
    loop {
        tokio::time::sleep(GAME_STATUS_POLL_INTERVAL).await;
        match launched.is_active().await {
            Ok(true) => {
                observed_active = true;
                error_reported = false;
            }
            Ok(false) if observed_active || Instant::now() >= startup_deadline => {
                publish_launch_status(&window, game_id, LaunchUiStatus::Idle);
                return;
            }
            Ok(false) => {}
            Err(error) if !error_reported => {
                warn!(%error, game_id, unit = %launched.unit_name, "failed to refresh launched game status");
                error_reported = true;
            }
            Err(_) => {}
        }
    }
}

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
    window.global::<GameState>().on_play_requested(move |id| {
        let Some(window) = played_window.upgrade() else {
            return;
        };
        let game = window.global::<GameState>();
        if game.get_launching_game_id() == id || game.get_playing_game_id() == id {
            return;
        }
        game.set_launching_game_id(id.clone());
        drop(window);

        let state = play_state
            .lock()
            .expect("library state lock poisoned")
            .clone();
        let Some(state) = state else {
            warn!(game_id = %id, "play requested but library state is unavailable");
            publish_launch_status(&played_window, id.to_string(), LaunchUiStatus::Idle);
            return;
        };
        let Ok(item_id) = LibraryItemId::parse(id.as_str()).ok_or(()) else {
            warn!(game_id = %id, "play requested with invalid library item id");
            publish_launch_status(&played_window, id.to_string(), LaunchUiStatus::Idle);
            return;
        };
        let played_store = played_store.clone();
        let played_sources = played_sources.clone();
        let played_window = played_window.clone();
        tokio::spawn(async move {
            // Take the latest shared snapshot at the launch boundary so
            // runtime changes made through Settings apply immediately.
            let launch_config = state.config.snapshot();
            let game_launcher = GameLauncher::new()
                .with_retroarch_config(launch_config.retroarch)
                .with_portmaster_config(launch_config.portmaster)
                .with_platform_configs(launch_config.platforms);
            match state.library.get(&item_id).await {
                Ok(Some(item)) => match game_launcher.launch_item(&item).await {
                    Ok(launched) => {
                        info!(
                            game_id = %id,
                            unit = %launched.unit_name,
                            "game launch requested"
                        );
                        publish_launch_status(
                            &played_window,
                            id.to_string(),
                            LaunchUiStatus::Playing,
                        );
                        tokio::spawn(monitor_launched_game(
                            launched,
                            played_window.clone(),
                            id.to_string(),
                        ));
                        if let Err(error) = state.library.record_play(&item.id) {
                            error!(%error, game_id = %id, "failed to persist play activity");
                        }
                        home::record_played(
                            &played_store,
                            &played_sources,
                            &played_window,
                            &item,
                            state.config.snapshot().romm_url.as_deref(),
                        );
                    }
                    Err(error) => {
                        error!(%error, game_id = %id, "failed to launch game");
                        publish_launch_status(&played_window, id.to_string(), LaunchUiStatus::Idle);
                    }
                },
                Ok(None) => {
                    warn!(game_id = %id, "play requested for missing library item");
                    publish_launch_status(&played_window, id.to_string(), LaunchUiStatus::Idle);
                }
                Err(error) => {
                    error!(%error, game_id = %id, "failed to resolve game for launch");
                    publish_launch_status(&played_window, id.to_string(), LaunchUiStatus::Idle);
                }
            }
        });
    });
}
