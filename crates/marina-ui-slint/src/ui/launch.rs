//! Game launch event handling.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use marina_core::{LibraryItem, LibraryItemId};
use marina_library::read::LibraryRead;
use marina_romm::{Client as RommClient, RommStore};
use marina_runtime::{GameLauncher, LaunchedGame, rom_path_for_item};
use slint::ComponentHandle;
use tracing::{error, info, warn};

use crate::ui::{notifications::NOTIFICATION, pages::home};
use crate::{GameState, MainWindow, app};

const GAME_STATUS_POLL_INTERVAL: Duration = Duration::from_millis(250);
const GAME_STARTUP_GRACE: Duration = Duration::from_secs(5);
const GAME_SAVES_ROOT: &str = "/var/games/saves";

#[derive(Clone, Copy)]
enum LaunchUiStatus {
    Idle,
    Playing,
}

#[derive(Clone)]
struct RommSessionTarget {
    client: RommClient,
    rom_id: i32,
    save_directory: PathBuf,
    rom_basename: String,
}

fn safe_path_component(value: &str) -> String {
    let value: String = value
        .chars()
        .map(|character| match character {
            '<' | '>' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            _ => character,
        })
        .collect();
    let value = value.trim().trim_matches('.');
    if value.is_empty() {
        "unknown".to_owned()
    } else {
        value.to_owned()
    }
}

fn romm_session_target(state: &app::AppState, item: &LibraryItem) -> Option<RommSessionTarget> {
    let raw_id = item.provider_ids.get("romm_id")?;
    let rom_id = match raw_id.parse() {
        Ok(id) => id,
        Err(error) => {
            warn!(%error, romm_id = raw_id, game_id = %item.id, "ignoring invalid RomM id");
            return None;
        }
    };
    let backend = state.store("romm")?;
    let store = backend.as_any().downcast_ref::<RommStore>()?;
    let rom_basename = rom_path_for_item(item)
        .and_then(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .filter(|stem| !stem.trim().is_empty())
        .unwrap_or_else(|| item.title.clone());
    Some(RommSessionTarget {
        client: store.client().clone(),
        rom_id,
        save_directory: Path::new(GAME_SAVES_ROOT).join(safe_path_component(&item.title)),
        rom_basename: safe_path_component(&rom_basename),
    })
}

fn report_romm_play_activity(target: RommSessionTarget) {
    tokio::spawn(async move {
        match target.client.record_play_activity(target.rom_id).await {
            Ok(_) => info!(rom_id = target.rom_id, "RomM play activity updated"),
            Err(error) => {
                warn!(%error, rom_id = target.rom_id, "failed to update RomM play activity")
            }
        }
    });
}

async fn download_romm_save_snapshots(target: &RommSessionTarget) {
    match marina_romm::download_save_directory(
        &target.client,
        target.rom_id,
        &target.save_directory,
        &target.rom_basename,
    )
    .await
    {
        Ok(report) => {
            info!(
                rom_id = target.rom_id,
                save_directory = %target.save_directory.display(),
                available = report.available,
                downloaded = report.downloaded,
                failed = report.failures.len(),
                "RomM save download completed"
            );
            if report.downloaded > 0 {
                NOTIFICATION.success(format!(
                    "Downloaded {} save snapshot{} from RomM",
                    report.downloaded,
                    if report.downloaded == 1 { "" } else { "s" }
                ));
            }
            if !report.failures.is_empty() {
                NOTIFICATION.error(format!(
                    "Failed to download {} RomM save{}",
                    report.failures.len(),
                    if report.failures.len() == 1 { "" } else { "s" }
                ));
            }
            for failure in report.failures {
                warn!(
                    rom_id = target.rom_id,
                    path = %failure.path.display(),
                    error = %failure.error,
                    "failed to download RomM save snapshot"
                );
            }
        }
        Err(error) => {
            warn!(
                %error,
                rom_id = target.rom_id,
                save_directory = %target.save_directory.display(),
                "failed to synchronize RomM saves before launch"
            );
            NOTIFICATION.error(format!("Could not download RomM saves: {error}"));
        }
    }
}

fn upload_romm_save_snapshots(target: RommSessionTarget) {
    tokio::spawn(async move {
        match marina_romm::upload_save_directory(
            &target.client,
            target.rom_id,
            &target.save_directory,
            &target.rom_basename,
        )
        .await
        {
            Ok(report) => {
                info!(
                    rom_id = target.rom_id,
                    save_directory = %target.save_directory.display(),
                    discovered = report.discovered,
                    uploaded = report.uploaded,
                    failed = report.failures.len(),
                    "RomM save snapshot upload completed"
                );
                if report.uploaded > 0 {
                    NOTIFICATION.success(format!(
                        "Uploaded {} save snapshot{} to RomM",
                        report.uploaded,
                        if report.uploaded == 1 { "" } else { "s" }
                    ));
                }
                if !report.failures.is_empty() {
                    NOTIFICATION.error(format!(
                        "Failed to upload {} RomM save{}",
                        report.failures.len(),
                        if report.failures.len() == 1 { "" } else { "s" }
                    ));
                }
                for failure in report.failures {
                    warn!(
                        rom_id = target.rom_id,
                        path = %failure.path.display(),
                        error = %failure.error,
                        "failed to upload RomM save snapshot"
                    );
                }
            }
            Err(error) => {
                warn!(
                    %error,
                    rom_id = target.rom_id,
                    save_directory = %target.save_directory.display(),
                    "failed to scan game saves for RomM upload"
                );
                NOTIFICATION.error(format!("Could not upload saves to RomM: {error}"));
            }
        }
    });
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
    mut romm_session: Option<RommSessionTarget>,
) {
    let startup_deadline = Instant::now() + GAME_STARTUP_GRACE;
    let mut observed_active = false;
    let mut error_reported = false;
    loop {
        tokio::time::sleep(GAME_STATUS_POLL_INTERVAL).await;
        match launched.is_active().await {
            Ok(true) => {
                if !observed_active && let Some(target) = romm_session.as_ref() {
                    report_romm_play_activity(target.clone());
                }
                observed_active = true;
                error_reported = false;
            }
            Ok(false) if observed_active || Instant::now() >= startup_deadline => {
                if observed_active && let Some(target) = romm_session.take() {
                    upload_romm_save_snapshots(target);
                }
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
                Ok(Some(item)) => {
                    let romm_session = romm_session_target(&state, &item);
                    if let Some(target) = &romm_session {
                        download_romm_save_snapshots(target).await;
                    }
                    match game_launcher.launch_item(&item).await {
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
                                romm_session,
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
                            publish_launch_status(
                                &played_window,
                                id.to_string(),
                                LaunchUiStatus::Idle,
                            );
                        }
                    }
                }
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
