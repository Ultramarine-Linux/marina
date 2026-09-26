use super::*;

/// Top-level Marina configuration file.
#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate, ConfigSettings)]
#[serde(deny_unknown_fields)]
pub(super) struct FileConfig {
    #[serde(default)]
    #[template(table)]
    #[setting(section = "general", section_title = "General", section_order = "10")]
    pub(super) general: GeneralSection,

    #[serde(default)]
    #[template(table)]
    #[setting(section = "library", section_title = "Library", section_order = "30")]
    pub(super) library: LibrarySection,

    #[serde(default)]
    #[template(table)]
    #[setting(section = "runtime", section_title = "Runtime", section_order = "40")]
    pub(super) runtime: RuntimeSection,
}

#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate, ConfigSettings)]
#[serde(deny_unknown_fields)]
pub(super) struct GeneralSection {
    #[serde(default)]
    #[template(table)]
    #[setting(
        section = "general",
        panel = "time_date",
        panel_title = "Time & Date",
        panel_order = "10"
    )]
    pub(super) time_date: ClockSection,
}

#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate, ConfigSettings)]
#[serde(deny_unknown_fields)]
pub(super) struct LibrarySection {
    #[serde(default)]
    #[template(table)]
    #[setting(
        section = "library",
        panel = "local",
        panel_title = "Library",
        panel_order = "10"
    )]
    pub(super) local: LocalLibrarySection,
    #[serde(default)]
    #[template(table)]
    #[setting(
        section = "library",
        panel = "romm",
        panel_title = "RomM Integration",
        panel_order = "20"
    )]
    pub(super) romm: RommConfig,
    #[serde(default)]
    #[template(table)]
    #[setting(
        section = "library",
        panel = "portmaster",
        panel_title = "PortMaster",
        panel_order = "30"
    )]
    pub(super) portmaster: PortMasterStoreConfig,
}

#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate, ConfigSettings)]
#[serde(deny_unknown_fields)]
pub(super) struct LocalLibrarySection {
    /// Root directory containing the local library (`roms/<platform>/...`).
    #[serde(default)]
    #[template(env = "MARINA_LIBRARY_ROOT", example = "/var/games/library")]
    #[setting(control = "path")]
    pub(super) root: Option<PathBuf>,
    /// SQLite library database URI.
    #[serde(default)]
    #[template(env = "MARINA_STORAGE_URI", example = "sqlite://marina.db")]
    pub(super) storage_uri: Option<String>,
    /// Directory holding per-backend store catalog caches.
    #[serde(default)]
    #[template(env = "MARINA_STORE_CACHE_DIR", example = "")]
    #[setting(control = "path")]
    pub(super) store_cache_dir: Option<PathBuf>,
    /// Scan the local library root for games at startup.
    #[serde(default)]
    #[template(env = "MARINA_SCAN_ON_STARTUP", example = "true")]
    pub(super) scan_on_startup: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate, ConfigSettings)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeSection {
    #[serde(default)]
    #[template(table)]
    #[setting(
        section = "runtime",
        panel = "retroarch",
        panel_title = "RetroArch",
        panel_order = "10"
    )]
    pub(super) retroarch: RetroArchConfig,
    #[serde(default)]
    #[template(table)]
    #[setting(
        section = "runtime",
        panel = "portmaster",
        panel_title = "PortMaster",
        panel_order = "20"
    )]
    pub(super) portmaster: PortMasterRuntimeConfig,
}

#[derive(Clone, Debug, Deserialize, ConfigTemplate, ConfigSettings)]
#[serde(deny_unknown_fields)]
pub(super) struct RetroArchConfig {
    /// RetroArch frontend binary, resolved via `PATH` when relative.
    #[serde(default = "default_retroarch_binary")]
    #[template(env = "MARINA_RETROARCH_BINARY")]
    #[setting(control = "path")]
    pub(super) binary: PathBuf,
    /// Directories scanned for libretro cores (`*_libretro.so`), in priority order.
    #[serde(
        default = "default_cores_dirs",
        deserialize_with = "deserialize_path_list"
    )]
    #[setting(control = "list")]
    pub(super) cores_dir: Vec<PathBuf>,
    /// Extra frontend flags inserted before `-L <core> <rom>`.
    #[serde(default)]
    #[setting(title = "Extra arguments", control = "list")]
    pub(super) extra_args: Vec<String>,
    /// Per-platform backend and core selection, keyed by platform slug.
    #[serde(default)]
    #[template(example = "gba")]
    #[setting(title = "Platforms", control = "menu", order = "40")]
    pub(super) platforms: HashMap<String, PlatformRuntimeConfig>,
}

impl Default for RetroArchConfig {
    fn default() -> Self {
        let effective = EffectiveRetroArchConfig::default();
        Self {
            binary: effective.binary,
            cores_dir: effective.cores_dir,
            extra_args: effective.extra_args,
            platforms: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, ConfigTemplate, ConfigSettings)]
#[serde(deny_unknown_fields)]
pub(super) struct PortMasterRuntimeConfig {
    /// Host path containing `<port>.sh` launchers and the `PortMaster` tree.
    #[serde(default = "default_portmaster_ports_dir")]
    #[template(env = "MARINA_PORTMASTER_PORTS_DIR")]
    #[setting(control = "path")]
    pub(super) ports_dir: PathBuf,
}

impl Default for PortMasterRuntimeConfig {
    fn default() -> Self {
        Self {
            ports_dir: default_portmaster_ports_dir(),
        }
    }
}

fn default_retroarch_binary() -> PathBuf {
    PathBuf::from(marina_runtime::DEFAULT_RETROARCH_BINARY)
}

fn default_cores_dirs() -> Vec<PathBuf> {
    vec![
        PathBuf::from(marina_runtime::DEFAULT_CORES_DIR),
        PathBuf::from(marina_runtime::DEFAULT_SYSTEM_CORES_DIR),
    ]
}

fn deserialize_path_list<'de, D>(deserializer: D) -> Result<Vec<PathBuf>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum PathList {
        One(PathBuf),
        Many(Vec<PathBuf>),
    }

    Ok(match PathList::deserialize(deserializer)? {
        PathList::One(path) => vec![path],
        PathList::Many(paths) => paths,
    })
}

/// Top-bar digital clock settings.
#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate, ConfigSettings)]
#[serde(deny_unknown_fields)]
pub(super) struct ClockSection {
    /// Use 12-hour time (`9:05 PM`) instead of 24-hour time (`21:05`).
    #[serde(default, alias = "12hr")]
    #[template(env = "MARINA_CLOCK_12HR", example = "false")]
    pub(super) twelve_hour: Option<bool>,
}

/// A single store backend's file configuration.
#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate, ConfigSettings)]
#[serde(deny_unknown_fields)]
pub struct RommConfig {
    /// Whether the RomM store backend is enabled.
    #[serde(default)]
    #[template(env = "MARINA_ENABLE_ROMM", example = "false")]
    pub enable: Option<bool>,
    /// Base URL of the RomM server.
    #[serde(default)]
    #[template(env = "ROMM_URL", example = "https://romm.example.com")]
    pub url: Option<String>,
    /// API token for the RomM server.
    #[serde(default)]
    #[template(env = "ROMM_TOKEN", example = "")]
    #[setting(title = "API token", control = "secret", sensitive)]
    pub token: Option<String>,
    /// Import the RomM catalog into the store cache at startup.
    #[serde(default)]
    #[template(env = "MARINA_IMPORT_ROMM_ON_STARTUP", example = "false")]
    pub import_on_startup: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, ConfigTemplate, ConfigSettings)]
#[serde(deny_unknown_fields)]
pub(super) struct PortMasterStoreConfig {
    #[serde(default)]
    #[template(env = "MARINA_ENABLE_PORTMASTER", example = "false")]
    pub(super) enable: Option<bool>,
    /// Optional PortMaster-New release tag. When unset, Marina follows GitHub's latest release.
    #[serde(default)]
    #[template(example = "latest")]
    pub(super) release: Option<String>,

    /// PortMaster install directory. When unset, defaults to `/var/games/ports`.
    #[serde(default)]
    #[template(example = "/var/games/ports")]
    pub(super) ports_dir: Option<PathBuf>,
}

impl Default for PortMasterStoreConfig {
    fn default() -> Self {
        Self {
            enable: None,
            release: None,
            ports_dir: None,
        }
    }
}

fn default_portmaster_ports_dir() -> PathBuf {
    PathBuf::from(DEFAULT_PORTS_DIR)
}
