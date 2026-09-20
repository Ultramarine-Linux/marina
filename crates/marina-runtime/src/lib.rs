//! Runtime services for launching and supervising games.

use std::{path::PathBuf, process::Stdio};

use marina_core::{LibraryItem, Platform};
use thiserror::Error;
use tokio::process::Command;
use tracing::{debug, info, warn};
use ulid::Ulid;

const SYSTEMD_RUN: &str = "systemd-run";
const APP_SLICE: &str = "graphical-apps.slice";

/// A game launch request resolved from a library item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchRequest {
    pub item_id: String,
    pub application_id: String,
    pub title: String,
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub working_directory: Option<PathBuf>,
}

impl LaunchRequest {
    pub fn from_item(item: &LibraryItem) -> Option<Self> {
        let command = item
            .provider_ids
            .get("xdg.exec")
            .and_then(|value| serde_json::from_str::<Vec<String>>(value).ok())
            .filter(|command| !command.is_empty());
        let (executable, arguments) = if let Some(mut command) = command {
            let executable = PathBuf::from(command.remove(0));
            (executable, command)
        } else if let Some(file) = item.files.first() {
            // Installed/scanned games point local_path at the containing
            // directory; the executable is the recorded file entry.
            (PathBuf::from(&file.path), Vec::new())
        } else {
            (PathBuf::from(item.local_path.as_deref()?), Vec::new())
        };
        Some(Self {
            item_id: item.id.to_string(),
            application_id: item
                .provider_ids
                .get("xdg.desktop")
                .cloned()
                .unwrap_or_else(|| item.id.to_string()),
            title: item.title.clone(),
            executable,
            arguments,
            working_directory: item.provider_ids.get("xdg.path").map(PathBuf::from),
        })
    }
}

/// The transient systemd unit created for a launched game.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchedGame {
    pub unit_name: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum RuntimeBackend {
    #[default]
    Native,
    Runtime(String),
    RetroArch(String),
}

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("game has no local launch path")]
    MissingLocalPath,
    #[error("failed to invoke systemd-run: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("systemd-run failed with status {status}: {stderr}")]
    Systemd { status: String, stderr: String },
    #[error("runtime backend is not implemented: {0}")]
    UnsupportedBackend(String),
}

/// Launches games as transient per-user systemd services in `graphical-apps.slice`.
#[derive(Clone, Debug, Default)]
pub struct GameLauncher {
    backend: RuntimeBackend,
}

impl RuntimeBackend {
    pub fn for_platform(platform: &Platform) -> Self {
        let backend = Self::for_platform_slug(&platform.slug);
        debug!(platform = %platform.slug, ?backend, "selected runtime backend for platform");
        backend
    }

    pub fn for_platform_slug(slug: &str) -> Self {
        let backend = match slug {
            // XDG desktop entries are already executable application commands.
            "apps" => Self::Native,
            // Platform-specific emulator mappings will be added here.
            _ => Self::Native,
        };
        debug!(platform = %slug, ?backend, "resolved runtime backend slug");
        backend
    }
}

impl GameLauncher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_backend(backend: RuntimeBackend) -> Self {
        Self { backend }
    }

    pub fn backend(&self) -> &RuntimeBackend {
        &self.backend
    }

    pub async fn launch_item(&self, item: &LibraryItem) -> Result<LaunchedGame, LaunchError> {
        debug!(
            game_id = %item.id,
            title = %item.title,
            platform = ?item.platform_slug,
            configured_backend = ?self.backend,
            "resolving launch backend"
        );
        let request = LaunchRequest::from_item(item).ok_or_else(|| {
            warn!(game_id = %item.id, "launch request has no executable path");
            LaunchError::MissingLocalPath
        })?;
        let launcher = if self.backend == RuntimeBackend::Native {
            Self::with_backend(
                item.platform_slug
                    .as_deref()
                    .map(RuntimeBackend::for_platform_slug)
                    .unwrap_or(RuntimeBackend::Native),
            )
        } else {
            self.clone()
        };
        debug!(game_id = %item.id, backend = ?launcher.backend, "dispatching launch request");
        launcher.launch(request).await
    }

    pub async fn launch(&self, request: LaunchRequest) -> Result<LaunchedGame, LaunchError> {
        debug!(
            application_id = %request.application_id,
            executable = %request.executable.display(),
            arguments = ?request.arguments,
            working_directory = ?request.working_directory,
            backend = ?self.backend,
            "launch backend invoked"
        );
        match &self.backend {
            RuntimeBackend::Native => self.launch_native(request).await,
            RuntimeBackend::Runtime(runtime) => {
                warn!(runtime = %runtime, "runtime backend is not implemented");
                Err(LaunchError::UnsupportedBackend(format!(
                    "Runtime({runtime})"
                )))
            }
            RuntimeBackend::RetroArch(core) => {
                warn!(core = %core, "RetroArch backend is not implemented");
                Err(LaunchError::UnsupportedBackend(format!(
                    "RetroArch({core})"
                )))
            }
        }
    }

    async fn launch_native(&self, request: LaunchRequest) -> Result<LaunchedGame, LaunchError> {
        let unit_name = unit_name(&request.application_id);
        info!(unit = %unit_name, slice = APP_SLICE, "starting native transient launch service");
        let mut command = Command::new(SYSTEMD_RUN);
        command.args(systemd_run_args(&unit_name));
        if let Some(working_directory) = &request.working_directory {
            command.args([
                "--working-directory",
                working_directory.to_string_lossy().as_ref(),
            ]);
        }
        let output = command
            .arg(&request.executable)
            .args(&request.arguments)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(LaunchError::Spawn)?;

        if !output.status.success() {
            warn!(unit = %unit_name, status = ?output.status, "native transient launch service failed");
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
    fn native_is_the_default_backend() {
        assert_eq!(RuntimeBackend::default(), RuntimeBackend::Native);
        assert_eq!(GameLauncher::new().backend(), &RuntimeBackend::Native);
    }

    #[test]
    fn platform_backend_defaults_are_explicit() {
        assert_eq!(
            RuntimeBackend::for_platform(&Platform::new("apps", "Apps")),
            RuntimeBackend::Native
        );
        assert_eq!(
            RuntimeBackend::for_platform_slug("snes"),
            RuntimeBackend::Native
        );
    }

    #[test]
    fn backend_configuration_is_preserved() {
        let launcher = GameLauncher::with_backend(RuntimeBackend::RetroArch("snes9x".into()));
        assert_eq!(
            launcher.backend(),
            &RuntimeBackend::RetroArch("snes9x".into())
        );
    }

    #[test]
    fn request_uses_item_local_path() {
        let mut item = LibraryItem::new_game("Example");
        item.local_path = Some("/games/example.sh".into());
        let request = LaunchRequest::from_item(&item).expect("local path");
        assert_eq!(request.executable, PathBuf::from("/games/example.sh"));
        assert_eq!(request.application_id, item.id.to_string());
        assert!(request.arguments.is_empty());
        assert!(request.working_directory.is_none());
    }

    #[test]
    fn request_prefers_file_entries_over_the_game_directory() {
        let mut item = LibraryItem::new_game("Example");
        item.local_path = Some("/games/example".into());
        item.files.push(marina_core::LibraryItemFile {
            provider_id: Some("romm:file:7".into()),
            name: "example.zip".into(),
            path: "/games/example/example.zip".into(),
            size_bytes: Some(42),
        });
        let request = LaunchRequest::from_item(&item).expect("file entry");
        assert_eq!(
            request.executable,
            PathBuf::from("/games/example/example.zip")
        );
    }

    #[test]
    fn missing_local_path_is_not_launchable() {
        let item = LibraryItem::new_game("Example");
        assert!(LaunchRequest::from_item(&item).is_none());
    }
}
