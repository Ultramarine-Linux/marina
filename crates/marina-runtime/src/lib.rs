//! Runtime services for launching and supervising games.

use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use marina_config_derive::{ConfigSettings, ConfigTemplate};
use marina_core::LibraryItem;
use serde::Deserialize;
use thiserror::Error;
use tracing::{debug, info, warn};
use ulid::Ulid;

pub mod portmaster;

const APP_SLICE: &str = "graphical-apps.slice";

/// Default RetroArch frontend binary, resolved via `PATH`.
pub const DEFAULT_RETROARCH_BINARY: &str = "retroarch";
/// Default directory scanned for libretro cores (`*_libretro.so`).
pub const DEFAULT_CORES_DIR: &str = "/var/games/retroarch/cores";
/// Default system directory scanned for libretro cores.
pub const DEFAULT_SYSTEM_CORES_DIR: &str = "/usr/lib64/libretro/";

fn default_retroarch_binary() -> PathBuf {
    PathBuf::from(DEFAULT_RETROARCH_BINARY)
}

fn default_cores_dirs() -> Vec<PathBuf> {
    vec![
        PathBuf::from(DEFAULT_CORES_DIR),
        PathBuf::from(DEFAULT_SYSTEM_CORES_DIR),
    ]
}

/// Frontend-level RetroArch configuration.
#[derive(Clone, Debug, Deserialize, ConfigTemplate, ConfigSettings)]
pub struct RetroArchConfig {
    /// RetroArch frontend binary, resolved via `PATH` when relative.
    #[serde(default = "default_retroarch_binary")]
    #[template(env = "MARINA_RETROARCH_BINARY")]
    pub binary: PathBuf,
    /// Directories scanned for libretro cores (`*_libretro.so`), in priority order.
    #[serde(default = "default_cores_dirs")]
    pub cores_dir: Vec<PathBuf>,
    /// Extra frontend flags inserted before `-L <core> <rom>`,
    /// e.g. `["-f", "--verbose"]`.
    #[serde(default)]
    pub extra_args: Vec<String>,
}

impl Default for RetroArchConfig {
    fn default() -> Self {
        Self {
            binary: default_retroarch_binary(),
            cores_dir: default_cores_dirs(),
            extra_args: Vec::new(),
        }
    }
}

/// Per-platform RetroArch settings.
#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate, ConfigSettings)]
pub struct RetroArchPlatformConfig {
    /// Core for this platform: either a bare file name resolved against the
    /// `[retroarch] cores_dir` directories or a full path to the core.
    #[serde(default)]
    #[template(example = "mgba_libretro.so")]
    pub core: Option<PathBuf>,
}

/// The backend selected for a single platform.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PlatformBackendKind {
    #[default]
    Native,
    RetroArch,
    Portmaster,
}

/// Per-platform runtime configuration. The template renders one commented
/// `[platform."<slug>"]` example; copy and adapt it per platform.
#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate, ConfigSettings)]
pub struct PlatformRuntimeConfig {
    /// Backend for this platform: `"native"` or `"retroarch"`. When unset,
    /// the `"apps"` platform stays native and every other platform infers
    /// RetroArch. Set `"native"` explicitly to opt a platform back out.
    #[serde(default)]
    #[template(example = "retroarch")]
    pub backend: Option<String>,
    /// Alias for `backend`, accepted so `platform = "retroarch"` keeps
    /// working. `backend` wins when both are set.
    #[serde(default)]
    #[template(skip)]
    pub platform: Option<String>,
    #[serde(default)]
    #[template(table)]
    #[setting(panel = "retroarch-core")]
    pub retroarch: RetroArchPlatformConfig,
}

impl PlatformRuntimeConfig {
    /// Resolves the backend for `platform_slug`. An explicit `backend` (or
    /// its `platform` alias) always wins. Otherwise a configured core
    /// implies RetroArch, the `"apps"` platform stays native, and every
    /// other platform infers RetroArch (which then requires a core).
    pub fn backend_kind(&self, platform_slug: &str) -> PlatformBackendKind {
        if let Some(raw) = self.backend.as_deref().or(self.platform.as_deref()) {
            match raw.trim().to_ascii_lowercase().as_str() {
                "retroarch" => return PlatformBackendKind::RetroArch,
                "portmaster" => return PlatformBackendKind::Portmaster,
                "native" | "" => return PlatformBackendKind::Native,
                other => {
                    warn!(backend = %other, "unknown platform backend; falling back to native");
                    return PlatformBackendKind::Native;
                }
            }
        }
        if platform_slug == "portmaster" {
            PlatformBackendKind::Portmaster
        } else if self.retroarch.core.is_some() || platform_slug != "apps" {
            PlatformBackendKind::RetroArch
        } else {
            PlatformBackendKind::Native
        }
    }

    /// Convenience constructor for a platform pinned to a RetroArch core.
    pub fn retroarch(core: impl Into<PathBuf>) -> Self {
        Self {
            backend: Some("retroarch".to_owned()),
            platform: None,
            retroarch: RetroArchPlatformConfig {
                core: Some(core.into()),
            },
        }
    }
}

/// A single libretro core discovered in the cores directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetroArchCore {
    /// File name, e.g. `"mgba_libretro.so"`.
    pub name: String,
    pub path: PathBuf,
}

/// Scans `cores_dirs` for libretro cores (`*_libretro.so`/`.dylib`/`.dll`),
/// sorted by file name. Used to validate configured cores and to produce
/// helpful "available cores" errors.
pub fn discover_cores(cores_dirs: &[PathBuf]) -> io::Result<Vec<RetroArchCore>> {
    let mut cores = Vec::new();
    for cores_dir in cores_dirs {
        let entries = match std::fs::read_dir(cores_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                warn!(dir = %cores_dir.display(), "libretro core directory does not exist");
                continue;
            }
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !is_libretro_core(&name) {
                continue;
            }
            debug!(core = %name, path = %path.display(), "discovered libretro core");
            cores.push(RetroArchCore { name, path });
        }
        info!(dir = %cores_dir.display(), "libretro core directory scan completed");
    }
    cores.sort_by(|left, right| left.name.cmp(&right.name));
    info!(count = cores.len(), "libretro core scan completed");
    Ok(cores)
}

fn is_libretro_core(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    lower.contains("_libretro.")
        && matches!(
            Path::new(&lower).extension().and_then(|ext| ext.to_str()),
            Some("so" | "dylib" | "dll")
        )
}

/// Resolves a configured core value to a concrete path. Absolute paths (or
/// values containing a path separator, e.g. `"subdir/core.so"`) are used
/// as-is; bare file names are joined onto the first configured core
/// directory containing the file, or the first configured directory when the
/// file is missing.
pub fn resolve_core_path(core: &Path, cores_dirs: &[PathBuf]) -> PathBuf {
    if core.is_absolute() || core.components().count() > 1 {
        return core.to_owned();
    }
    cores_dirs
        .iter()
        .map(|cores_dir| cores_dir.join(core))
        .find(|path| path.is_file())
        .or_else(|| cores_dirs.first().map(|cores_dir| cores_dir.join(core)))
        .unwrap_or_else(|| core.to_owned())
}

/// The ROM content to hand to RetroArch. Unlike [`LaunchRequest::from_item`],
/// desktop-entry commands are ignored: the first recorded file entry wins,
/// falling back to `local_path`.
pub fn rom_path_for_item(item: &LibraryItem) -> Option<PathBuf> {
    if let Some(file) = item.files.first() {
        return Some(PathBuf::from(&file.path));
    }
    item.local_path.as_deref().map(PathBuf::from)
}

/// Builds the RetroArch argument vector: extra args first, then
/// `-L <core> <rom>`.
pub fn retroarch_argv(extra_args: &[String], core: &str, rom: &Path) -> Vec<String> {
    let mut args = Vec::with_capacity(extra_args.len() + 3);
    args.extend(extra_args.iter().cloned());
    args.push("-L".to_owned());
    args.push(core.to_owned());
    args.push(rom.to_string_lossy().into_owned());
    args
}

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

/// A Marina-created transient game service that systemd still reports as active.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunningGame {
    pub unit_name: String,
    pub item_id: String,
    pub application_id: String,
    pub title: String,
    pub platform_slug: Option<String>,
    pub backend: RuntimeBackend,
}

#[derive(Debug, Default)]
struct LaunchRegistryState {
    launching: bool,
    running: Option<RunningGame>,
}

#[derive(Debug, Default)]
struct LaunchRegistry {
    state: Mutex<LaunchRegistryState>,
}

static TRANSIENT_LAUNCH_REGISTRY: OnceLock<Arc<LaunchRegistry>> = OnceLock::new();

fn transient_launch_registry() -> Arc<LaunchRegistry> {
    TRANSIENT_LAUNCH_REGISTRY
        .get_or_init(|| Arc::new(LaunchRegistry::default()))
        .clone()
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum RuntimeBackend {
    #[default]
    Native,
    Runtime(String),
    RetroArch(String),
    Portmaster,
}

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("game has no local launch path")]
    MissingLocalPath,
    #[error(
        "no RetroArch core configured for platform '{platform}'; set [platform.\"{platform}\"].retroarch.core to a core file name or path"
    )]
    MissingCore { platform: String },
    #[error("RetroArch core not found: {core} ({hint})")]
    CoreNotFound { core: String, hint: String },
    #[error("failed to invoke a system utility: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("system utility failed with status {status}: {stderr}")]
    Systemd { status: String, stderr: String },
    #[error("failed to launch through the systemd user manager: {0}")]
    SystemdDbus(#[from] marina_systemd::Error),
    #[error("a Marina transient game launch is already in progress")]
    LaunchInProgress,
    #[error("a Marina game is already running: {unit_name} ({title})")]
    GameAlreadyRunning { unit_name: String, title: String },
    #[error("runtime backend is not implemented: {0}")]
    UnsupportedBackend(String),
    #[error("invalid PortMaster launcher path: {path}")]
    InvalidPortLauncher { path: String },
    #[error("failed to prepare PortMaster save directory {path}: {source}")]
    PortmasterSetup {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// Launches games as transient per-user systemd services in `graphical-apps.slice`.
#[derive(Clone, Debug)]
pub struct GameLauncher {
    backend: RuntimeBackend,
    retroarch: RetroArchConfig,
    portmaster: portmaster::Config,
    platforms: HashMap<String, PlatformRuntimeConfig>,
    registry: Arc<LaunchRegistry>,
}

impl Default for GameLauncher {
    fn default() -> Self {
        Self {
            backend: RuntimeBackend::default(),
            retroarch: RetroArchConfig::default(),
            portmaster: portmaster::Config::default(),
            platforms: HashMap::new(),
            registry: transient_launch_registry(),
        }
    }
}

struct LaunchPermit {
    registry: Arc<LaunchRegistry>,
    committed: bool,
}

impl LaunchRegistry {
    async fn reserve(self: &Arc<Self>) -> Result<LaunchPermit, LaunchError> {
        let running = {
            let mut state = self.state.lock().expect("launch registry lock poisoned");
            if state.launching {
                return Err(LaunchError::LaunchInProgress);
            }
            state.launching = true;
            state.running.clone()
        };

        if let Some(running) = running {
            match marina_systemd::user_unit_is_active(&running.unit_name).await {
                Ok(true) => {
                    self.release_reservation();
                    return Err(LaunchError::GameAlreadyRunning {
                        unit_name: running.unit_name,
                        title: running.title,
                    });
                }
                Ok(false) => {
                    let mut state = self.state.lock().expect("launch registry lock poisoned");
                    if state
                        .running
                        .as_ref()
                        .is_some_and(|current| current.unit_name == running.unit_name)
                    {
                        state.running = None;
                    }
                }
                Err(error) => {
                    self.release_reservation();
                    return Err(error.into());
                }
            }
        }

        Ok(LaunchPermit {
            registry: self.clone(),
            committed: false,
        })
    }

    async fn running_games(&self) -> Result<Vec<RunningGame>, LaunchError> {
        let running = self
            .state
            .lock()
            .expect("launch registry lock poisoned")
            .running
            .clone();
        let Some(running) = running else {
            return Ok(Vec::new());
        };

        if marina_systemd::user_unit_is_active(&running.unit_name).await? {
            return Ok(vec![running]);
        }

        let mut state = self.state.lock().expect("launch registry lock poisoned");
        if state
            .running
            .as_ref()
            .is_some_and(|current| current.unit_name == running.unit_name)
        {
            state.running = None;
        }
        Ok(Vec::new())
    }

    fn commit(&self, running: RunningGame) {
        let mut state = self.state.lock().expect("launch registry lock poisoned");
        state.running = Some(running);
        state.launching = false;
    }

    fn release_reservation(&self) {
        self.state
            .lock()
            .expect("launch registry lock poisoned")
            .launching = false;
    }
}

impl LaunchPermit {
    fn commit(mut self, running: RunningGame) {
        self.registry.commit(running);
        self.committed = true;
    }
}

impl Drop for LaunchPermit {
    fn drop(&mut self) {
        if !self.committed {
            self.registry.release_reservation();
        }
    }
}

impl RuntimeBackend {
    /// Resolves the backend for a platform using per-platform configuration.
    ///
    /// The `"apps"` platform is native by default; every other platform
    /// infers RetroArch. A platform pinned to RetroArch without a configured
    /// core fails with [`LaunchError::MissingCore`] rather than silently
    /// falling back to a native launch.
    pub fn for_platform_config(
        slug: &str,
        platforms: &HashMap<String, PlatformRuntimeConfig>,
        retroarch: &RetroArchConfig,
    ) -> Result<Self, LaunchError> {
        let default;
        let config = match platforms.get(slug) {
            Some(config) => config,
            None => {
                default = PlatformRuntimeConfig::default();
                &default
            }
        };
        let backend = match config.backend_kind(slug) {
            PlatformBackendKind::Native => Self::Native,
            PlatformBackendKind::Portmaster => Self::Portmaster,
            PlatformBackendKind::RetroArch => {
                let core = platforms
                    .get(slug)
                    .and_then(|config| config.retroarch.core.as_deref())
                    .ok_or_else(|| LaunchError::MissingCore {
                        platform: slug.to_owned(),
                    })?;
                Self::RetroArch(
                    resolve_core_path(core, &retroarch.cores_dir)
                        .to_string_lossy()
                        .into_owned(),
                )
            }
        };
        debug!(platform = %slug, ?backend, "resolved runtime backend from platform config");
        Ok(backend)
    }
}

impl GameLauncher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_backend(backend: RuntimeBackend) -> Self {
        Self {
            backend,
            ..Self::default()
        }
    }

    pub fn with_retroarch_config(mut self, config: RetroArchConfig) -> Self {
        self.retroarch = config;
        self
    }

    pub fn with_portmaster_config(mut self, config: portmaster::Config) -> Self {
        self.portmaster = config;
        self
    }

    pub fn portmaster_config(&self) -> &portmaster::Config {
        &self.portmaster
    }

    pub fn with_platform_configs(
        mut self,
        platforms: HashMap<String, PlatformRuntimeConfig>,
    ) -> Self {
        self.platforms = platforms;
        self
    }

    pub fn backend(&self) -> &RuntimeBackend {
        &self.backend
    }

    pub fn retroarch_config(&self) -> &RetroArchConfig {
        &self.retroarch
    }

    pub fn platform_configs(&self) -> &HashMap<String, PlatformRuntimeConfig> {
        &self.platforms
    }

    /// Return Marina-created transient game services that systemd still reports
    /// as active. This is the runtime snapshot used to prevent duplicate
    /// launches and will later drive input-profile selection.
    pub async fn running_games(&self) -> Result<Vec<RunningGame>, LaunchError> {
        self.registry.running_games().await
    }

    pub async fn launch_item(&self, item: &LibraryItem) -> Result<LaunchedGame, LaunchError> {
        debug!(
            game_id = %item.id,
            title = %item.title,
            platform = ?item.platform_slug,
            configured_backend = ?self.backend,
            "resolving launch backend"
        );
        let backend = if self.backend == RuntimeBackend::Native {
            match item.platform_slug.as_deref() {
                Some(slug) => {
                    RuntimeBackend::for_platform_config(slug, &self.platforms, &self.retroarch)?
                }
                None => RuntimeBackend::Native,
            }
        } else {
            self.backend.clone()
        };
        debug!(game_id = %item.id, backend = ?backend, "dispatching launch request");
        let tracked = matches!(
            &backend,
            RuntimeBackend::Native | RuntimeBackend::RetroArch(_)
        );
        let permit = if tracked {
            Some(self.registry.reserve().await?)
        } else {
            None
        };
        let running = tracked.then(|| RunningGame {
            unit_name: String::new(),
            item_id: item.id.to_string(),
            application_id: item
                .provider_ids
                .get("xdg.desktop")
                .cloned()
                .unwrap_or_else(|| item.id.to_string()),
            title: item.title.clone(),
            platform_slug: item.platform_slug.clone(),
            backend: backend.clone(),
        });
        let result = match backend {
            RuntimeBackend::Native => {
                let request = LaunchRequest::from_item(item).ok_or_else(|| {
                    warn!(game_id = %item.id, "launch request has no executable path");
                    LaunchError::MissingLocalPath
                })?;
                self.launch_native(request).await
            }
            RuntimeBackend::Portmaster => {
                let request =
                    portmaster::request_for_item(item).ok_or(LaunchError::MissingLocalPath)?;
                portmaster::launch(&request, &self.portmaster).await
            }
            RuntimeBackend::RetroArch(core) => {
                let rom = rom_path_for_item(item).ok_or_else(|| {
                    warn!(game_id = %item.id, "retroarch launch has no ROM path");
                    LaunchError::MissingLocalPath
                })?;
                let core = resolve_core_path(Path::new(&core), &self.retroarch.cores_dir)
                    .to_string_lossy()
                    .into_owned();
                self.launch_retroarch(item, &core, &rom).await
            }
            RuntimeBackend::Runtime(runtime) => {
                warn!(runtime = %runtime, "runtime backend is not implemented");
                Err(LaunchError::UnsupportedBackend(format!(
                    "Runtime({runtime})"
                )))
            }
        };
        self.finish_tracked_launch(result, permit, running)
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
        let tracked = matches!(
            &self.backend,
            RuntimeBackend::Native | RuntimeBackend::RetroArch(_)
        );
        let permit = if tracked {
            Some(self.registry.reserve().await?)
        } else {
            None
        };
        let running = tracked.then(|| RunningGame {
            unit_name: String::new(),
            item_id: request.item_id.clone(),
            application_id: request.application_id.clone(),
            title: request.title.clone(),
            platform_slug: None,
            backend: self.backend.clone(),
        });
        let result = match &self.backend {
            RuntimeBackend::Native => self.launch_native(request).await,
            RuntimeBackend::Runtime(runtime) => {
                warn!(runtime = %runtime, "runtime backend is not implemented");
                Err(LaunchError::UnsupportedBackend(format!(
                    "Runtime({runtime})"
                )))
            }
            RuntimeBackend::Portmaster => portmaster::launch(&request, &self.portmaster).await,
            RuntimeBackend::RetroArch(core) => {
                let core = resolve_core_path(Path::new(core), &self.retroarch.cores_dir)
                    .to_string_lossy()
                    .into_owned();
                // An explicit RetroArch backend treats the request executable
                // as ROM content.
                let item = retroarch_item_for_request(&request);
                self.launch_retroarch(&item, &core, &request.executable)
                    .await
            }
        };
        self.finish_tracked_launch(result, permit, running)
    }

    fn finish_tracked_launch(
        &self,
        result: Result<LaunchedGame, LaunchError>,
        permit: Option<LaunchPermit>,
        running: Option<RunningGame>,
    ) -> Result<LaunchedGame, LaunchError> {
        match result {
            Ok(launched) => {
                if let (Some(permit), Some(mut running)) = (permit, running) {
                    running.unit_name.clone_from(&launched.unit_name);
                    permit.commit(running);
                }
                Ok(launched)
            }
            Err(error) => Err(error),
        }
    }

    async fn launch_native(&self, request: LaunchRequest) -> Result<LaunchedGame, LaunchError> {
        let unit_name = unit_name(&request.application_id);
        info!(unit = %unit_name, slice = APP_SLICE, "starting native transient launch service");
        let args: Vec<String> = request.arguments.iter().cloned().collect();
        run_transient(
            &unit_name,
            &request.executable,
            &args,
            request.working_directory.as_deref(),
            &request.title,
        )
        .await
    }

    async fn launch_retroarch(
        &self,
        item: &LibraryItem,
        core: &str,
        rom: &Path,
    ) -> Result<LaunchedGame, LaunchError> {
        let core_path = Path::new(core);
        if !core_path.is_file() {
            let available = discover_cores(&self.retroarch.cores_dir)
                .unwrap_or_default()
                .into_iter()
                .map(|core| core.name)
                .collect::<Vec<_>>();
            let hint = if available.is_empty() {
                format!(
                    "no cores found in {}",
                    self.retroarch
                        .cores_dir
                        .iter()
                        .map(|dir| dir.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else {
                format!("available: {}", available.join(", "))
            };
            warn!(core = %core, rom = %rom.display(), hint = %hint, "retroarch core file is missing");
            return Err(LaunchError::CoreNotFound {
                core: core.to_owned(),
                hint,
            });
        }
        let application_id = item
            .provider_ids
            .get("xdg.desktop")
            .cloned()
            .unwrap_or_else(|| item.id.to_string());
        let unit_name = unit_name(&application_id);
        info!(unit = %unit_name, slice = APP_SLICE, core = %core, rom = %rom.display(), "starting retroarch transient launch service");
        let args = retroarch_argv(&self.retroarch.extra_args, core, rom);
        run_transient(
            &unit_name,
            &self.retroarch.binary,
            &args,
            rom.parent(),
            &item.title,
        )
        .await
    }
}

fn retroarch_item_for_request(request: &LaunchRequest) -> LibraryItem {
    let mut item = LibraryItem::new_game(request.title.clone());
    item.provider_ids
        .insert("xdg.desktop".to_owned(), request.application_id.clone());
    item
}

pub(crate) async fn start_user_service(
    unit_name: &str,
    title: &str,
) -> Result<LaunchedGame, LaunchError> {
    let job = marina_systemd::start_user_unit(unit_name).await?;
    tracing::info!(unit = %unit_name, title = %title, job = %job, "queued user service launch");
    Ok(LaunchedGame {
        unit_name: unit_name.to_owned(),
    })
}

async fn run_transient(
    unit_name: &str,
    executable: &Path,
    arguments: &[String],
    working_directory: Option<&Path>,
    title: &str,
) -> Result<LaunchedGame, LaunchError> {
    let service = marina_systemd::TransientService {
        unit_name: unit_name.to_owned(),
        description: title.to_owned(),
        executable: executable.to_string_lossy().into_owned(),
        arguments: arguments.to_vec(),
        working_directory: working_directory.map(|path| path.to_string_lossy().into_owned()),
        slice: APP_SLICE.to_owned(),
    };
    let job = marina_systemd::start_transient_user_service(&service).await?;

    tracing::info!(unit = %unit_name, title = %title, path = %executable.display(), job = %job, "queued game launch");
    Ok(LaunchedGame {
        unit_name: unit_name.to_owned(),
    })
}

fn unit_name(application_id: &str) -> String {
    format!("app-{application_id}-{}.service", ulid())
}

fn ulid() -> String {
    Ulid::new().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn write_file(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, b"fake").unwrap();
        path
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "marina-runtime-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

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

    fn running_game(unit_name: &str) -> RunningGame {
        RunningGame {
            unit_name: unit_name.to_owned(),
            item_id: "item-1".to_owned(),
            application_id: "example.desktop".to_owned(),
            title: "Example".to_owned(),
            platform_slug: Some("apps".to_owned()),
            backend: RuntimeBackend::Native,
        }
    }

    #[test]
    fn launch_permit_releases_an_unsuccessful_reservation() {
        let registry = Arc::new(LaunchRegistry {
            state: Mutex::new(LaunchRegistryState {
                launching: true,
                running: None,
            }),
        });
        let permit = LaunchPermit {
            registry: registry.clone(),
            committed: false,
        };

        drop(permit);

        assert!(
            !registry
                .state
                .lock()
                .expect("launch registry lock poisoned")
                .launching
        );
    }

    #[test]
    fn launch_permit_commits_the_running_game() {
        let registry = Arc::new(LaunchRegistry {
            state: Mutex::new(LaunchRegistryState {
                launching: true,
                running: None,
            }),
        });
        let permit = LaunchPermit {
            registry: registry.clone(),
            committed: false,
        };

        permit.commit(running_game("app-example.service"));

        let state = registry
            .state
            .lock()
            .expect("launch registry lock poisoned");
        assert!(!state.launching);
        assert_eq!(
            state.running.as_ref().map(|game| game.unit_name.as_str()),
            Some("app-example.service")
        );
    }

    #[test]
    fn launchers_share_the_transient_registry() {
        let first = GameLauncher::new();
        let second = GameLauncher::new();

        assert!(Arc::ptr_eq(&first.registry, &second.registry));
    }

    #[test]
    fn native_is_the_default_backend() {
        assert_eq!(RuntimeBackend::default(), RuntimeBackend::Native);
        assert_eq!(GameLauncher::new().backend(), &RuntimeBackend::Native);
    }

    #[test]
    fn platform_backend_defaults_are_explicit() {
        let platforms = HashMap::new();
        let retroarch = RetroArchConfig::default();
        // XDG desktop entries are already executable application commands.
        assert_eq!(
            RuntimeBackend::for_platform_config("apps", &platforms, &retroarch).unwrap(),
            RuntimeBackend::Native
        );
        // Every other platform infers RetroArch, which needs a configured core.
        assert!(matches!(
            RuntimeBackend::for_platform_config("snes", &platforms, &retroarch),
            Err(LaunchError::MissingCore { .. })
        ));
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

    #[test]
    fn retroarch_config_has_sane_defaults() {
        let config = RetroArchConfig::default();
        assert_eq!(config.binary, PathBuf::from("retroarch"));
        assert_eq!(
            config.cores_dir,
            vec![
                PathBuf::from("/var/games/retroarch/cores"),
                PathBuf::from("/usr/lib64/libretro/"),
            ]
        );
        assert!(config.extra_args.is_empty());
    }

    #[test]
    fn apps_platform_defaults_to_native() {
        let config = PlatformRuntimeConfig::default();
        assert_eq!(config.backend_kind("apps"), PlatformBackendKind::Native);
    }

    #[test]
    fn portmaster_platform_defaults_to_portmaster() {
        let config = PlatformRuntimeConfig::default();
        assert_eq!(
            config.backend_kind("portmaster"),
            PlatformBackendKind::Portmaster
        );
    }

    #[test]
    fn unconfigured_platforms_infer_retroarch() {
        let config = PlatformRuntimeConfig::default();
        assert_eq!(config.backend_kind("gba"), PlatformBackendKind::RetroArch);
        assert_eq!(config.backend_kind("snes"), PlatformBackendKind::RetroArch);
    }

    #[test]
    fn configured_core_implies_retroarch_without_backend_key() {
        let config = PlatformRuntimeConfig::retroarch("mgba_libretro.so");
        // `retroarch()` sets the backend key too; drop it to simulate a
        // core-only `[platform."gba".retroarch]` table.
        let config = PlatformRuntimeConfig {
            backend: None,
            ..config
        };
        assert_eq!(config.backend_kind("gba"), PlatformBackendKind::RetroArch);
    }

    #[test]
    fn explicit_native_wins_over_inference() {
        let config = PlatformRuntimeConfig {
            backend: Some("native".into()),
            platform: None,
            retroarch: RetroArchPlatformConfig {
                core: Some("mgba_libretro.so".into()),
            },
        };
        assert_eq!(config.backend_kind("gba"), PlatformBackendKind::Native);
    }

    #[test]
    fn platform_backend_alias_is_accepted() {
        let config = PlatformRuntimeConfig {
            backend: None,
            platform: Some("retroarch".into()),
            retroarch: RetroArchPlatformConfig { core: None },
        };
        assert_eq!(config.backend_kind("gba"), PlatformBackendKind::RetroArch);
    }

    #[test]
    fn unknown_platform_backend_falls_back_to_native() {
        let config = PlatformRuntimeConfig {
            backend: Some("dolphin".into()),
            platform: None,
            retroarch: RetroArchPlatformConfig { core: None },
        };
        assert_eq!(config.backend_kind("gba"), PlatformBackendKind::Native);
    }

    #[test]
    fn platform_config_selects_retroarch_with_resolved_core() {
        let mut platforms = HashMap::new();
        platforms.insert(
            "gba".to_owned(),
            PlatformRuntimeConfig::retroarch("mgba_libretro.so"),
        );
        let retroarch = RetroArchConfig::default();
        assert_eq!(
            RuntimeBackend::for_platform_config("gba", &platforms, &retroarch).unwrap(),
            RuntimeBackend::RetroArch("/var/games/retroarch/cores/mgba_libretro.so".into())
        );
        // Unconfigured platforms infer RetroArch, so without a core they
        // fail instead of silently launching natively.
        assert!(matches!(
            RuntimeBackend::for_platform_config("snes", &platforms, &retroarch),
            Err(LaunchError::MissingCore { .. })
        ));
    }

    #[test]
    fn platform_config_absolute_core_is_used_verbatim() {
        let mut platforms = HashMap::new();
        platforms.insert(
            "gba".to_owned(),
            PlatformRuntimeConfig::retroarch("/opt/cores/mgba_libretro.so"),
        );
        let retroarch = RetroArchConfig::default();
        assert_eq!(
            RuntimeBackend::for_platform_config("gba", &platforms, &retroarch).unwrap(),
            RuntimeBackend::RetroArch("/opt/cores/mgba_libretro.so".into())
        );
    }

    #[test]
    fn retroarch_without_core_is_missing_not_native() {
        let mut platforms = HashMap::new();
        platforms.insert(
            "gba".to_owned(),
            PlatformRuntimeConfig {
                backend: Some("retroarch".into()),
                platform: None,
                retroarch: RetroArchPlatformConfig { core: None },
            },
        );
        let error =
            RuntimeBackend::for_platform_config("gba", &platforms, &RetroArchConfig::default())
                .expect_err("missing core must fail");
        assert!(matches!(error, LaunchError::MissingCore { .. }));
    }

    #[test]
    fn core_names_resolve_against_cores_dir() {
        let cores_dir = Path::new("/var/games/retroarch/cores");
        assert_eq!(
            resolve_core_path(Path::new("mgba_libretro.so"), &[cores_dir.to_path_buf()],),
            cores_dir.join("mgba_libretro.so")
        );
        assert_eq!(
            resolve_core_path(
                Path::new("/opt/cores/snes9x_libretro.so"),
                &[cores_dir.to_path_buf()],
            ),
            PathBuf::from("/opt/cores/snes9x_libretro.so")
        );
        assert_eq!(
            resolve_core_path(
                Path::new("custom/snes9x_libretro.so"),
                &[cores_dir.to_path_buf()],
            ),
            PathBuf::from("custom/snes9x_libretro.so")
        );
    }

    #[test]
    fn core_paths_and_discovery_support_multiple_directories() {
        let first = temp_dir("cores-first");
        let second = temp_dir("cores-second");
        write_file(&second, "snes9x_libretro.so");

        assert_eq!(
            resolve_core_path(
                Path::new("snes9x_libretro.so"),
                &[first.clone(), second.clone()],
            ),
            second.join("snes9x_libretro.so")
        );
        assert_eq!(
            discover_cores(&[first.clone(), second.clone()])
                .unwrap()
                .into_iter()
                .map(|core| core.name)
                .collect::<Vec<_>>(),
            vec!["snes9x_libretro.so"]
        );

        std::fs::remove_dir_all(&first).unwrap();
        std::fs::remove_dir_all(&second).unwrap();
    }

    #[test]
    fn core_discovery_lists_libretro_shared_objects() {
        let dir = temp_dir("cores");
        write_file(&dir, "mgba_libretro.so");
        write_file(&dir, "snes9x_libretro.so");
        write_file(&dir, "retroarch.cfg");
        write_file(&dir, "README.txt");

        let cores = discover_cores(&[dir.clone()]).unwrap();
        assert_eq!(
            cores
                .iter()
                .map(|core| core.name.clone())
                .collect::<Vec<_>>(),
            vec!["mgba_libretro.so", "snes9x_libretro.so"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rom_path_prefers_file_entries_and_ignores_desktop_commands() {
        let mut item = LibraryItem::new_game("Pokemon");
        item.local_path = Some("/games/gba/Pokemon".into());
        item.files.push(marina_core::LibraryItemFile {
            provider_id: None,
            name: "pokemon.gba".into(),
            path: "/games/gba/Pokemon/pokemon.gba".into(),
            size_bytes: Some(42),
        });
        item.provider_ids.insert(
            "xdg.exec".into(),
            serde_json::to_string(&vec!["/usr/bin/false"]).unwrap(),
        );
        assert_eq!(
            rom_path_for_item(&item),
            Some(PathBuf::from("/games/gba/Pokemon/pokemon.gba"))
        );
    }

    #[test]
    fn retroarch_argv_orders_flags_core_and_content() {
        let argv = retroarch_argv(
            &["-f".to_owned()],
            "/cores/mgba_libretro.so",
            Path::new("/games/pokemon.gba"),
        );
        assert_eq!(
            argv,
            vec!["-f", "-L", "/cores/mgba_libretro.so", "/games/pokemon.gba"]
        );
    }

    #[test]
    fn launcher_carries_runtime_configuration() {
        let mut platforms = HashMap::new();
        platforms.insert(
            "gba".to_owned(),
            PlatformRuntimeConfig::retroarch("mgba_libretro.so"),
        );
        let launcher = GameLauncher::new()
            .with_retroarch_config(RetroArchConfig {
                binary: PathBuf::from("/usr/bin/retroarch"),
                ..RetroArchConfig::default()
            })
            .with_platform_configs(platforms);
        assert_eq!(
            launcher.retroarch_config().binary,
            PathBuf::from("/usr/bin/retroarch")
        );
        assert!(launcher.platform_configs().contains_key("gba"));
    }

    #[tokio::test]
    async fn launch_item_without_core_reports_missing_core() {
        let mut item = LibraryItem::new_game("Pokemon");
        item.platform_slug = Some("gba".into());
        item.local_path = Some("/games/pokemon.gba".into());
        let mut platforms = HashMap::new();
        platforms.insert(
            "gba".to_owned(),
            PlatformRuntimeConfig {
                backend: Some("retroarch".into()),
                platform: None,
                retroarch: RetroArchPlatformConfig { core: None },
            },
        );
        let launcher = GameLauncher::new().with_platform_configs(platforms);
        let error = launcher
            .launch_item(&item)
            .await
            .expect_err("missing core must fail before spawning");
        assert!(matches!(error, LaunchError::MissingCore { .. }));
    }

    #[tokio::test]
    async fn launch_item_with_missing_core_file_lists_available_cores() {
        let cores_dir = temp_dir("missing-core-file");
        write_file(&cores_dir, "mgba_libretro.so");
        let mut item = LibraryItem::new_game("Pokemon");
        item.platform_slug = Some("gba".into());
        item.local_path = Some("/games/pokemon.gba".into());
        let mut platforms = HashMap::new();
        platforms.insert(
            "gba".to_owned(),
            PlatformRuntimeConfig::retroarch("snes9x_libretro.so"),
        );
        let launcher = GameLauncher::new()
            .with_retroarch_config(RetroArchConfig {
                cores_dir: vec![cores_dir.clone()],
                ..RetroArchConfig::default()
            })
            .with_platform_configs(platforms);
        let error = launcher
            .launch_item(&item)
            .await
            .expect_err("missing core file must fail before spawning");
        match error {
            LaunchError::CoreNotFound { core, hint } => {
                assert!(core.ends_with("snes9x_libretro.so"), "core: {core}");
                assert!(
                    hint.contains("mgba_libretro.so"),
                    "hint should list discovered cores: {hint}"
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
        std::fs::remove_dir_all(&cores_dir).unwrap();
    }

    #[tokio::test]
    async fn launch_item_on_unconfigured_platform_infers_retroarch() {
        let mut item = LibraryItem::new_game("Pokemon");
        item.platform_slug = Some("gba".into());
        item.local_path = Some("/games/pokemon.gba".into());
        let launcher = GameLauncher::new();
        let error = launcher
            .launch_item(&item)
            .await
            .expect_err("unconfigured platform must not launch natively");
        assert!(
            matches!(error, LaunchError::MissingCore { .. }),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn retroarch_template_section_renders_doc_comments() {
        let mut out = String::new();
        RetroArchConfig::default().__marina_config_section(&mut out, "retroarch", true);
        assert!(out.contains("[retroarch]"), "missing header:\n{out}");
        assert!(
            out.contains("binary = \"retroarch\""),
            "missing default value:\n{out}"
        );
        assert!(
            out.contains("Directories scanned for libretro cores"),
            "missing doc comment:\n{out}"
        );
        assert!(
            !out.contains("MARINA_RETROARCH_CORES_DIR"),
            "vector fields should not render a scalar env override:\n{out}"
        );
    }
}
