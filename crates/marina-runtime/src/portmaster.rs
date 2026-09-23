//! PortMaster service launching.
//!
//! PortMaster launchers are intentionally not executed directly by Marina. The
//! image-provided `portmaster@.service` template creates the user/mount
//! namespace and supplies the compatibility layout expected by PortMaster.

use std::path::{Path, PathBuf};

use marina_config_derive::ConfigTemplate;
use marina_core::LibraryItem;
use serde::Deserialize;
use tokio::process::Command;
use tracing::info;

use crate::{LaunchError, LaunchRequest, LaunchedGame, start_user_service};

/// Name of the systemd user template installed by the appliance image.
pub const SERVICE_TEMPLATE: &str = "portmaster@";
/// PortMaster's conventional launcher directory.
pub const DEFAULT_PORTS_DIR: &str = "/var/games/ports";
/// Persistent writable upper layers for PortMaster ports.
pub const DEFAULT_SAVES_DIR: &str = "/var/games/saves/ports";

/// PortMaster-specific paths and service configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, ConfigTemplate)]
pub struct Config {
    /// Host path containing `<port>.sh` launchers and the `PortMaster` tree.
    #[serde(default = "default_ports_dir")]
    #[template(env = "MARINA_PORTMASTER_PORTS_DIR")]
    pub ports_dir: PathBuf,
}

fn default_ports_dir() -> PathBuf {
    PathBuf::from(DEFAULT_PORTS_DIR)
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ports_dir: default_ports_dir(),
        }
    }
}

/// Resolve and launch a PortMaster item through the installed user service.
pub(crate) async fn launch(
    request: &LaunchRequest,
    config: &Config,
) -> Result<LaunchedGame, LaunchError> {
    let instance = instance_name(&request.executable, &config.ports_dir)?;

    let escaped = Command::new("systemd-escape")
        .args(["--template=portmaster@.service", &instance])
        .output()
        .await
        .map_err(LaunchError::Spawn)?;
    if !escaped.status.success() {
        return Err(LaunchError::Systemd {
            status: escaped.status.to_string(),
            stderr: String::from_utf8_lossy(&escaped.stderr).trim().to_owned(),
        });
    }
    let unit_name = String::from_utf8_lossy(&escaped.stdout).trim().to_owned();
    info!(unit = %unit_name, port = %instance, "starting contained PortMaster service");
    start_user_service(&unit_name, &request.title).await
}

/// Return the systemd template instance for a PortMaster launcher.
///
/// PortMaster's service contract is `<ports-dir>/<name>.sh`; arguments and
/// desktop-entry commands are deliberately ignored because launchers source
/// their own control files and derive `GAMEDIR` from `%i`.
pub fn instance_name(executable: &Path, ports_dir: &Path) -> Result<String, LaunchError> {
    let file_name = executable
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(LaunchError::MissingLocalPath)?;
    let expected_dir = executable.parent().unwrap_or_else(|| Path::new("."));
    if expected_dir != ports_dir || !file_name.ends_with(".sh") {
        return Err(LaunchError::InvalidPortLauncher {
            path: executable.display().to_string(),
        });
    }
    let instance = file_name
        .strip_suffix(".sh")
        .filter(|name| !name.is_empty())
        .ok_or_else(|| LaunchError::InvalidPortLauncher {
            path: executable.display().to_string(),
        })?;
    if !instance
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b' '))
    {
        return Err(LaunchError::InvalidPortLauncher {
            path: executable.display().to_string(),
        });
    }
    Ok(instance.to_owned())
}

/// Build the request used by PortMaster when an item points at a launcher.
pub fn request_for_item(item: &LibraryItem) -> Option<LaunchRequest> {
    let request = LaunchRequest::from_item(item)?;
    if request.executable.extension().and_then(|ext| ext.to_str()) != Some("sh") {
        return None;
    }
    Some(request)
}
