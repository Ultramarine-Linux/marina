//! Runtime services for launching and supervising games.

use std::{path::PathBuf, process::Stdio};

use marina_core::LibraryItem;
use thiserror::Error;
use tokio::process::Command;
use ulid::Ulid;

const SYSTEMD_RUN: &str = "systemd-run";
const APP_SLICE: &str = "app.slice";

/// A game launch request resolved from a library item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchRequest {
    pub item_id: String,
    pub title: String,
    pub executable: PathBuf,
}

impl LaunchRequest {
    pub fn from_item(item: &LibraryItem) -> Option<Self> {
        Some(Self {
            item_id: item.id.to_string(),
            title: item.title.clone(),
            executable: PathBuf::from(item.local_path.as_deref()?),
        })
    }
}

/// The transient systemd unit created for a launched game.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchedGame {
    pub unit_name: String,
}

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("game has no local launch path")]
    MissingLocalPath,
    #[error("failed to invoke systemd-run: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("systemd-run failed with status {status}: {stderr}")]
    Systemd { status: String, stderr: String },
}

/// Launches games as transient per-user systemd services in `app.slice`.
#[derive(Clone, Debug, Default)]
pub struct GameLauncher;

impl GameLauncher {
    pub fn new() -> Self {
        Self
    }

    pub async fn launch_item(&self, item: &LibraryItem) -> Result<LaunchedGame, LaunchError> {
        let request = LaunchRequest::from_item(item).ok_or(LaunchError::MissingLocalPath)?;
        self.launch(request).await
    }

    pub async fn launch(&self, request: LaunchRequest) -> Result<LaunchedGame, LaunchError> {
        let unit_name = unit_name(&request.item_id);
        let output = Command::new(SYSTEMD_RUN)
            .args(systemd_run_args(&unit_name))
            .arg(&request.executable)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(LaunchError::Spawn)?;

        if !output.status.success() {
            return Err(LaunchError::Systemd {
                status: output
                    .status
                    .code()
                    .map_or_else(|| "signal".to_owned(), |code| code.to_string()),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }

        tracing::info!(unit = %unit_name, title = %request.title, path = %request.executable.display(), "launched game");
        Ok(LaunchedGame { unit_name })
    }
}

fn unit_name(application_id: &str) -> String {
    format!("app-{application_id}-{}.service", ulid())
}

fn ulid() -> String {
    Ulid::new().to_string()
}

fn systemd_run_args(unit_name: &str) -> [&str; 8] {
    [
        "--user",
        "--no-block",
        "--collect",
        "--quiet",
        "--unit",
        unit_name,
        "--slice",
        APP_SLICE,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_names_follow_desktop_app_convention() {
        let first = unit_name("game-id");
        assert!(first.starts_with("app-game-id-"));
        assert!(first.ends_with(".service"));
        assert_eq!(first.len(), "app-game-id-".len() + 26 + ".service".len());
    }

    #[test]
    fn ulids_use_the_canonical_alphabet() {
        let value = ulid();
        assert_eq!(value.len(), 26);
        assert!(
            value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'A'..=b'Z').contains(&byte))
        );
    }

    #[test]
    fn request_uses_item_local_path() {
        let mut item = LibraryItem::new_game("Example");
        item.local_path = Some("/games/example.sh".into());
        let request = LaunchRequest::from_item(&item).expect("local path");
        assert_eq!(request.executable, PathBuf::from("/games/example.sh"));
    }

    #[test]
    fn missing_local_path_is_not_launchable() {
        let item = LibraryItem::new_game("Example");
        assert!(LaunchRequest::from_item(&item).is_none());
    }
}
