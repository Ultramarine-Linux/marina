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
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use tracing::{error, info, warn};

use crate::ui::{notifications::NOTIFICATION, pages::home};
use crate::{GameState, LaunchArtifactData, MainWindow, app};

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

fn select_launch_artifact(item: &mut LibraryItem, index: usize) -> bool {
    if index >= item.files.len() {
        return false;
    }
    item.files.swap(0, index);
    true
}

fn queue_launch(
    state: app::AppStateHandle,
    window: slint::Weak<MainWindow>,
    game_id: String,
    selected_file: Option<usize>,
    played_store: home::PlayedStore,
    played_sources: home::PlayedSources,
) {
    tokio::spawn(async move {
        let launch_config = state.config.snapshot();
        let game_launcher = GameLauncher::new()
            .with_retroarch_config(launch_config.retroarch)
            .with_portmaster_config(launch_config.portmaster)
            .with_platform_configs(launch_config.platforms);
        let Ok(item_id) = LibraryItemId::parse(&game_id).ok_or(()) else {
            warn!(%game_id, "play requested with invalid library item id");
            publish_launch_status(&window, game_id, LaunchUiStatus::Idle);
            return;
        };
        match state.library.get(&item_id).await {
            Ok(Some(mut item)) => {
                if selected_file.is_none() && item.files.len() > 1 {
                    let options = item
                        .files
                        .iter()
                        .map(|file| {
                            (
                                file.name.clone(),
                                file.size_bytes
                                    .map(|bytes| {
                                        bytesize::ByteSize::b(bytes).display().iec().to_string()
                                    })
                                    .unwrap_or_else(|| "Unknown size".to_owned()),
                            )
                        })
                        .collect::<Vec<_>>();
                    let chooser_id = game_id.clone();
                    let _ = window.upgrade_in_event_loop(move |window| {
                        let game = window.global::<GameState>();
                        if game.get_selected_game().id.as_str() != chooser_id {
                            return;
                        }
                        game.set_launching_game_id(SharedString::default());
                        game.set_selected_launch_artifact(0);
                        game.set_launch_artifacts(ModelRc::from(std::rc::Rc::new(VecModel::from(
                            options
                                .into_iter()
                                .map(|(name, size)| LaunchArtifactData {
                                    name: SharedString::from(name),
                                    size: SharedString::from(size),
                                })
                                .collect::<Vec<_>>(),
                        ))));
                        game.set_launch_sheet_open(true);
                    });
                    return;
                }
                if let Some(index) = selected_file
                    && !select_launch_artifact(&mut item, index)
                {
                    warn!(%game_id, index, files = item.files.len(), "selected launch artifact is unavailable");
                    publish_launch_status(&window, game_id, LaunchUiStatus::Idle);
                    return;
                }

                let romm_session = romm_session_target(&state, &item);
                if let Some(target) = &romm_session {
                    download_romm_save_snapshots(target).await;
                }
                match game_launcher.launch_item(&item).await {
                    Ok(launched) => {
                        info!(game_id = %game_id, unit = %launched.unit_name, "game launch requested");
                        publish_launch_status(&window, game_id.clone(), LaunchUiStatus::Playing);
                        tokio::spawn(monitor_launched_game(
                            launched,
                            window.clone(),
                            game_id,
                            romm_session,
                        ));
                        if let Err(error) = state.library.record_play(&item.id) {
                            error!(%error, game_id = %item.id, "failed to persist play activity");
                        }
                        home::record_played(
                            &played_store,
                            &played_sources,
                            &window,
                            &item,
                            state.config.snapshot().romm_url.as_deref(),
                        );
                    }
                    Err(error) => {
                        error!(%error, game_id = %game_id, "failed to launch game");
                        publish_launch_status(&window, game_id, LaunchUiStatus::Idle);
                    }
                }
            }
            Ok(None) => {
                warn!(%game_id, "play requested for missing library item");
                publish_launch_status(&window, game_id, LaunchUiStatus::Idle);
            }
            Err(error) => {
                error!(%error, %game_id, "failed to resolve game for launch");
                publish_launch_status(&window, game_id, LaunchUiStatus::Idle);
            }
        }
    });
}

pub(crate) fn install(
    window: &MainWindow,
    library_state: &Arc<Mutex<Option<app::AppStateHandle>>>,
    played_store: &home::PlayedStore,
    played_sources: &home::PlayedSources,
) {
    let play_state = library_state.clone();
    let play_store = played_store.clone();
    let play_sources = played_sources.clone();
    let play_window = window.as_weak();
    window.global::<GameState>().on_play_requested(move |id| {
        let Some(window) = play_window.upgrade() else {
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
            publish_launch_status(&play_window, id.to_string(), LaunchUiStatus::Idle);
            return;
        };
        queue_launch(
            state,
            play_window.clone(),
            id.to_string(),
            None,
            play_store.clone(),
            play_sources.clone(),
        );
    });

    let artifact_state = library_state.clone();
    let artifact_store = played_store.clone();
    let artifact_sources = played_sources.clone();
    let artifact_window = window.as_weak();
    window
        .global::<GameState>()
        .on_launch_artifact_requested(move |id, index| {
            let Some(window) = artifact_window.upgrade() else {
                return;
            };
            let game = window.global::<GameState>();
            if game.get_launching_game_id() == id || game.get_playing_game_id() == id {
                return;
            }
            game.set_launch_sheet_open(false);
            game.set_launching_game_id(id.clone());
            drop(window);

            let state = artifact_state
                .lock()
                .expect("library state lock poisoned")
                .clone();
            let Some(state) = state else {
                warn!(game_id = %id, "artifact launch requested but library state is unavailable");
                publish_launch_status(&artifact_window, id.to_string(), LaunchUiStatus::Idle);
                return;
            };
            queue_launch(
                state,
                artifact_window.clone(),
                id.to_string(),
                Some(index.max(0) as usize),
                artifact_store.clone(),
                artifact_sources.clone(),
            );
        });
}

#[cfg(test)]
mod tests {
    use marina_core::{LibraryItem, LibraryItemFile};

    use super::select_launch_artifact;

    #[test]
    fn selected_artifact_becomes_the_transient_launch_target() {
        let mut item = LibraryItem::new_game("Example Game");
        item.files = vec![
            LibraryItemFile {
                name: "Example Game A.rom".into(),
                path: "/games/example-a.rom".into(),
                ..Default::default()
            },
            LibraryItemFile {
                name: "Example Game B.rom".into(),
                path: "/games/example-b.rom".into(),
                ..Default::default()
            },
        ];

        assert!(select_launch_artifact(&mut item, 1));
        assert_eq!(item.files[0].name, "Example Game B.rom");
    }
}
