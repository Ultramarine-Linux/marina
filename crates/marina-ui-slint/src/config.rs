//! Runtime configuration: figment-merged TOML file + environment.
//!
//! Store backends live in the library namespace:
//!
//! ```toml
//! [library.romm]
//! enable = true
//! url = "https://romm.example.com"
//! token = "rmm_..."
//! import_on_startup = false
//! ```
//!
//! Emulator runtimes live under `[runtime.retroarch]`; each
//! `[runtime.retroarch.platforms."<slug>"]` table pins a backend and its
//! settings for one platform:
//!
//! ```toml
//! [runtime.retroarch]
//! binary = "retroarch"
//! cores_dir = "/var/games/retroarch/cores"
//!
//! [runtime.retroarch.platforms."gba"]
//! backend = "retroarch"
//! [runtime.retroarch.platforms."gba".retroarch]
//! core = "mgba_libretro.so"
//! ```
//!
//! Reads go through figment. TOML sources are merged in this order, with
//! later sources overriding earlier fields:
//!
//! 1. `/usr/share/marina/config.toml.d/*.toml`
//! 2. `/usr/share/marina/config.toml`
//! 3. `/etc/marina/config.toml.d/*.toml`
//! 4. `/etc/marina/config.toml`
//! 5. `$XDG_CONFIG_HOME/marina/config.toml.d/*.toml`
//! 6. The primary config file: `$MARINA_CONFIG`, `./marina.toml`, or
//!    `$XDG_CONFIG_HOME/marina/config.toml` (`~/.config/marina/config.toml`)
//! 7. Environment variables
//!
//! Drop-ins are loaded lexicographically by filename. When no primary config
//! exists, [`ensure_config_file`] writes [`default_config_template`] (rendered
//! from the config structs' defaults and `///` doc comments via
//! `ConfigTemplate`) to `$MARINA_CONFIG` or the XDG location on startup.
//!
//! Writes go through [`toml_edit`] (see [`upsert_toml_value`]) so in-UI
//! settings editing preserves comments and formatting. Figment is
//! read-only and cannot write back.

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;

use figment::{
    Figment,
    providers::{Format, Toml},
};
use marina_config_derive::{ConfigSettings, ConfigTemplate};
use marina_portmaster::{DEFAULT_PORTS_DIR, DEFAULT_RELEASE, PortMasterConfig};
use marina_runtime::{
    PlatformRuntimeConfig, RetroArchConfig as EffectiveRetroArchConfig,
    portmaster::Config as RuntimePortMasterConfig,
};
use serde::Deserialize;
use tracing::warn;

/// Top-level Marina configuration file.
#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate, ConfigSettings)]
struct FileConfig {
    #[serde(default)]
    #[template(table)]
    #[setting(section = "general", section_title = "General", section_order = "10")]
    general: GeneralSection,

    #[serde(default)]
    #[template(table)]
    #[setting(section = "library", section_title = "Library", section_order = "30")]
    library: LibrarySection,

    #[serde(default)]
    #[template(table)]
    #[setting(section = "runtime", section_title = "Runtime", section_order = "40")]
    runtime: RuntimeSection,
}

#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate, ConfigSettings)]
struct GeneralSection {
    #[serde(default)]
    #[template(table)]
    #[setting(
        section = "general",
        panel = "time_date",
        panel_title = "Time & Date",
        panel_order = "10"
    )]
    time_date: ClockSection,
}

#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate, ConfigSettings)]
struct LibrarySection {
    #[serde(default)]
    #[template(table)]
    #[setting(
        section = "library",
        panel = "local",
        panel_title = "Library",
        panel_order = "10"
    )]
    local: LocalLibrarySection,
    #[serde(default)]
    #[template(table)]
    #[setting(
        section = "library",
        panel = "romm",
        panel_title = "RomM Integration",
        panel_order = "20"
    )]
    romm: RommConfig,
    #[serde(default)]
    #[template(table)]
    portmaster: PortMasterStoreConfig,
}

#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate, ConfigSettings)]
struct LocalLibrarySection {
    /// Root directory containing the local library (`roms/<platform>/...`).
    #[serde(default)]
    #[template(env = "MARINA_LIBRARY_ROOT", example = "/var/games/library")]
    #[setting(control = "path")]
    root: Option<PathBuf>,
    /// SQLite library database URI.
    #[serde(default)]
    #[template(env = "MARINA_STORAGE_URI", example = "sqlite://marina.db")]
    storage_uri: Option<String>,
    /// Directory holding per-backend store catalog caches.
    #[serde(default)]
    #[template(env = "MARINA_STORE_CACHE_DIR", example = "")]
    #[setting(control = "path")]
    store_cache_dir: Option<PathBuf>,
    /// Scan the local library root for games at startup.
    #[serde(default)]
    #[template(env = "MARINA_SCAN_ON_STARTUP", example = "true")]
    scan_on_startup: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate, ConfigSettings)]
struct RuntimeSection {
    #[serde(default)]
    #[template(table)]
    #[setting(
        section = "runtime",
        panel = "retroarch",
        panel_title = "RetroArch",
        panel_order = "10"
    )]
    retroarch: RetroArchConfig,
    #[serde(default)]
    #[template(table)]
    portmaster: PortMasterRuntimeConfig,
}

#[derive(Clone, Debug, Deserialize, ConfigTemplate, ConfigSettings)]
struct RetroArchConfig {
    /// RetroArch frontend binary, resolved via `PATH` when relative.
    #[serde(default = "default_retroarch_binary")]
    #[template(env = "MARINA_RETROARCH_BINARY")]
    #[setting(control = "path")]
    binary: PathBuf,
    /// Directory scanned for libretro cores (`*_libretro.so`).
    #[serde(default = "default_cores_dir")]
    #[template(env = "MARINA_RETROARCH_CORES_DIR")]
    #[setting(control = "path")]
    cores_dir: PathBuf,
    /// Extra frontend flags inserted before `-L <core> <rom>`.
    #[serde(default)]
    #[setting(title = "Extra arguments", control = "list")]
    extra_args: Vec<String>,
    /// Per-platform backend and core selection, keyed by platform slug.
    #[serde(default)]
    #[template(example = "gba")]
    #[setting(title = "Platforms", control = "menu", order = "40")]
    platforms: HashMap<String, PlatformRuntimeConfig>,
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
struct PortMasterRuntimeConfig {
    /// Host path containing `<port>.sh` launchers and the `PortMaster` tree.
    #[serde(default = "default_portmaster_ports_dir")]
    #[template(env = "MARINA_PORTMASTER_PORTS_DIR")]
    #[setting(control = "path")]
    ports_dir: PathBuf,
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

fn default_cores_dir() -> PathBuf {
    PathBuf::from(marina_runtime::DEFAULT_CORES_DIR)
}

/// Top-bar digital clock settings.
#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate, ConfigSettings)]
struct ClockSection {
    /// Use 12-hour time (`9:05 PM`) instead of 24-hour time (`21:05`).
    #[serde(default, alias = "12hr")]
    #[template(env = "MARINA_CLOCK_12HR", example = "false")]
    twelve_hour: Option<bool>,
}

/// A single store backend's file configuration.
#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate, ConfigSettings)]
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

#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate, ConfigSettings)]
struct PortMasterStoreConfig {
    #[serde(default)]
    #[template(env = "MARINA_ENABLE_PORTMASTER", example = "false")]
    enable: Option<bool>,
    #[serde(default = "default_portmaster_release")]
    release: String,

    #[serde(default = "default_portmaster_ports_dir")]
    ports_dir: PathBuf,
}

fn default_portmaster_release() -> String {
    DEFAULT_RELEASE.to_owned()
}

fn default_portmaster_ports_dir() -> PathBuf {
    PathBuf::from(DEFAULT_PORTS_DIR)
}

#[derive(Debug)]
pub struct Config {
    pub storage_uri: String,
    pub store_cache_dir: std::path::PathBuf,

    pub romm_url: Option<String>,
    pub romm_token: Option<String>,
    pub import_romm_on_startup: bool,
    pub portmaster_store: Option<PortMasterConfig>,
    pub scan_on_startup: bool,
    pub library_root: Option<std::path::PathBuf>,
    pub clock_twelve_hour: bool,

    pub retroarch: EffectiveRetroArchConfig,
    pub portmaster: RuntimePortMasterConfig,
    pub platforms: HashMap<String, PlatformRuntimeConfig>,
}

impl Config {
    pub fn from_env() -> Self {
        if let Err(error) = ensure_config_file() {
            warn!(%error, "could not write default config file; continuing with defaults");
        }
        let figment = merge_config_files(Figment::new(), config_sources());
        // Env vars merge over the file as key-path tuples: `(key, value)`
        // with a dotted path like "library.romm.url" nests like the TOML.
        // The bindings are generated from `#[template(env = "..")]`,
        // clap-derive style, so they cannot disagree with the template.
        Self::from_figment(apply_env(figment))
    }

    fn from_figment(figment: Figment) -> Self {
        let file: FileConfig = match figment.extract() {
            Ok(file) => file,
            Err(error) => {
                warn!(%error, "ignoring invalid config; using defaults");
                FileConfig::default()
            }
        };
        let romm = &file.library.romm;
        let portmaster = &file.library.portmaster;

        let romm_enabled = romm.enable.unwrap_or(false);
        let romm_url = if romm_enabled { romm.url.clone() } else { None };
        let romm_token = if romm_enabled {
            romm.token.clone()
        } else {
            None
        };
        let import_romm_on_startup = romm.import_on_startup.unwrap_or(false);
        let scan_on_startup = file.library.local.scan_on_startup.unwrap_or(true);
        let clock_twelve_hour = file.general.time_date.twelve_hour.unwrap_or(false);
        if !scan_on_startup {
            tracing::info!(
                "local library scan disabled at startup by config or MARINA_SCAN_ON_STARTUP=false"
            );
        }
        if !romm_enabled {
            tracing::info!(
                "RomM backend disabled by default; enable it with [library.romm] enable = true"
            );
        } else if romm_url.is_none() {
            warn!("[library.romm] url not set — relative cover paths will not resolve");
        }

        let default_library_root = Some(std::path::PathBuf::from("/var/games/library"));
        let default_storage_uri = dirs::state_dir()
            .map(|path| {
                format!(
                    "sqlite://{}",
                    path.join("marina").join("library.db").display()
                )
            })
            .unwrap_or_else(|| "sqlite://marina.db".to_owned());

        let portmaster_config = portmaster
            .enable
            .unwrap_or(false)
            .then(|| PortMasterConfig {
                release: portmaster.release.clone(),
                ports_dir: portmaster.ports_dir.clone(),
            });

        Self {
            storage_uri: file
                .library
                .local
                .storage_uri
                .clone()
                .unwrap_or(default_storage_uri),
            store_cache_dir: file
                .library
                .local
                .store_cache_dir
                .clone()
                .or_else(|| {
                    env::var("MARINA_STORE_CACHE_URI").ok().and_then(|uri| {
                        let path = uri.strip_prefix("sqlite://").unwrap_or(&uri);
                        std::path::Path::new(path)
                            .parent()
                            .map(|parent| parent.to_path_buf())
                    })
                })
                .unwrap_or_else(|| {
                    dirs::state_dir()
                        .map(|path| path.join("marina").join("store-cache"))
                        .unwrap_or_else(|| std::path::PathBuf::from("store-cache"))
                }),

            romm_url,
            romm_token,
            import_romm_on_startup,
            portmaster_store: portmaster_config,
            scan_on_startup,
            clock_twelve_hour,
            library_root: file.library.local.root.clone().or(default_library_root),
            retroarch: EffectiveRetroArchConfig {
                binary: file.runtime.retroarch.binary.clone(),
                cores_dir: file.runtime.retroarch.cores_dir.clone(),
                extra_args: file.runtime.retroarch.extra_args.clone(),
            },
            portmaster: RuntimePortMasterConfig {
                ports_dir: file.runtime.portmaster.ports_dir.clone(),
            },
            platforms: file.runtime.retroarch.platforms.clone(),
        }
    }
}

fn env_bool(name: &str) -> Option<bool> {
    env::var(name).ok().map(|value| {
        !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    })
}

/// Merges every `#[template(env = "..")]` binding over the file layer:
/// environment variable > config file > default. Bool bindings use the
/// lenient [`env_bool`] parsing; the rest merge as strings.
fn apply_env(figment: Figment) -> Figment {
    let mut figment = figment;
    for (path, var, is_bool) in FileConfig::default().__marina_env_bindings("") {
        if is_bool {
            if let Some(value) = env_bool(var) {
                figment = figment.merge((path, value));
            }
        } else if let Ok(value) = env::var(var) {
            figment = figment.merge((path, value));
        }
    }
    figment
}

/// Returns all TOML files that form the file configuration layer, ordered
/// from lowest to highest priority.
fn config_sources() -> Vec<PathBuf> {
    let xdg = xdg_config_dir();
    config_sources_from(
        std::path::Path::new("/usr/share/marina/config.toml.d"),
        std::path::Path::new("/usr/share/marina/config.toml"),
        std::path::Path::new("/etc/marina/config.toml.d"),
        std::path::Path::new("/etc/marina/config.toml"),
        xdg.as_deref()
            .map(|base| base.join("marina").join("config.toml.d"))
            .as_deref(),
        primary_config_path().as_deref(),
    )
}

/// Builds the source list independently of the fixed system locations so its
/// ordering can be covered by unit tests.
fn config_sources_from(
    shared_dropins: &std::path::Path,
    shared_config: &std::path::Path,
    etc_dropins: &std::path::Path,
    etc_config: &std::path::Path,
    user_dropins: Option<&std::path::Path>,
    primary_config: Option<&std::path::Path>,
) -> Vec<PathBuf> {
    let mut sources = dropin_files(shared_dropins);
    if shared_config.is_file() {
        sources.push(shared_config.to_path_buf());
    }
    sources.extend(dropin_files(etc_dropins));
    if etc_config.is_file() {
        sources.push(etc_config.to_path_buf());
    }
    if let Some(directory) = user_dropins {
        sources.extend(dropin_files(directory));
    }
    if let Some(path) = primary_config.filter(|path| path.is_file()) {
        sources.push(path.to_path_buf());
    }
    sources
}

/// Lists immediate `*.toml` drop-ins in deterministic filename order.
fn dropin_files(directory: &std::path::Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };

    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    files.sort();
    files
}

fn merge_config_files(figment: Figment, paths: impl IntoIterator<Item = PathBuf>) -> Figment {
    paths.into_iter().fold(figment, |figment, path| {
        tracing::info!(path = %path.display(), "loading config file");
        figment.merge(Toml::file(path))
    })
}

fn xdg_config_dir() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
}

/// Selects the config file that users edit directly. It is intentionally
/// separate from the lower-priority, system-managed configuration sources.
fn primary_config_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("MARINA_CONFIG").map(PathBuf::from) {
        return Some(path);
    }
    let local = PathBuf::from("marina.toml");
    if local.is_file() {
        return Some(local);
    }
    xdg_config_dir().map(|base| base.join("marina").join("config.toml"))
}

/// Where a missing config file gets created: an explicit `$MARINA_CONFIG`
/// path is honored verbatim, otherwise the XDG config home
/// (`~/.config/marina/config.toml`). An existing `./marina.toml` is never
/// shadowed — it stays the highest-priority lookup after `$MARINA_CONFIG`.
fn config_write_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("MARINA_CONFIG").map(PathBuf::from) {
        return Some(path);
    }
    if PathBuf::from("marina.toml").is_file() {
        return Some(PathBuf::from("marina.toml"));
    }
    xdg_config_dir().map(|base| base.join("marina").join("config.toml"))
}

/// Renders the default configuration file. Values come from the config
/// structs' `Default` impls and documentation from their `///` doc
/// comments (see `ConfigTemplate`), so the template cannot drift from the
/// schema the way a hand-maintained example file can. Commented-out keys
/// show illustrative values; the app behaves exactly as if the file did
/// not exist until they are uncommented.
/// Returns the generated, framework-neutral settings schema.
///
/// The UI adapter owns presentation and persistence; this method only exposes
/// the metadata emitted by `ConfigSettings`.
pub fn settings_schema() -> Vec<(
    String,
    String,
    String,
    &'static str,
    Option<&'static str>,
    bool,
    String,
    String,
    i32,
    String,
    String,
    i32,
    i32,
)> {
    FileConfig::default().__marina_settings("")
}

pub fn default_config_template() -> String {
    let mut out = String::from(CONFIG_PREAMBLE);
    FileConfig::default().__marina_config_transparent(&mut out, "", true);
    // End the file with exactly one newline.
    while out.ends_with("\n\n") {
        out.pop();
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

const CONFIG_PREAMBLE: &str = "\
# Marina configuration. This file was generated on first run; every field
# can still be overridden by its environment variable (see `Env:` notes).
# Uncomment a key and set it to take effect.
";

/// Creates the config file from [`default_config_template`] when none
/// exists. Existing files are never touched. Failures are returned so the
/// caller can log and continue with defaults — startup must never fail
/// just because the template could not be written.
pub fn ensure_config_file() -> std::io::Result<PathBuf> {
    let Some(path) = config_write_path() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no writable config location (set MARINA_CONFIG or HOME)",
        ));
    };
    if path.is_file() {
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(&path, default_config_template())?;
    tracing::info!(path = %path.display(), "wrote default config file");
    Ok(path)
}

/// Updates a single scalar in a TOML document while preserving comments,
/// whitespace, and key order. Parses the config file into a
/// [`toml_edit::Document`], upserts, then renders it with `doc.to_string()`.
pub fn upsert_toml_value(
    document: &str,
    table_path: &[&str],
    key: &str,
    value: toml_edit::Item,
) -> Result<String, toml_edit::TomlError> {
    let mut doc: toml_edit::Document = document.parse()?;
    let mut table = doc.as_table_mut();
    for segment in table_path {
        if !table.contains_key(*segment) {
            table.insert(*segment, toml_edit::Item::Table(toml_edit::Table::new()));
        }
        let entry = &mut table[*segment];
        if !entry.is_table() {
            *entry = toml_edit::Item::Table(toml_edit::Table::new());
        }
        table = entry.as_table_mut().expect("just ensured table");
    }
    table.insert(key, value);
    Ok(doc.to_string())
}

/// A scalar setting value read from or written to the user configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScalarSettingValue {
    Bool(bool),
    String(String),
}

/// Returns the active config file, creating the generated template first when
/// needed. This is intended for background tasks; it performs filesystem I/O.
pub fn active_config_path() -> Result<PathBuf, String> {
    ensure_config_file().map_err(|error| error.to_string())
}

/// Reads values that are explicitly present in the active TOML file. Missing
/// optional scalar values use an empty string or `false`, matching the
/// generated template's raw, editable configuration state.
pub fn read_scalar_settings() -> Result<HashMap<String, ScalarSettingValue>, String> {
    let path = active_config_path()?;
    let document = std::fs::read_to_string(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let value: toml::Value = toml::from_str(&document)
        .map_err(|error| format!("could not parse {}: {error}", path.display()))?;
    let mut values = HashMap::new();
    for (setting_path, _, _, control, _, _, _, _, _, _, _, _, _) in settings_schema() {
        let value = setting_path
            .split('.')
            .try_fold(&value, |value, key| value.get(key));
        let scalar = match (control, value) {
            ("toggle", Some(toml::Value::Boolean(value))) => ScalarSettingValue::Bool(*value),
            ("toggle", _) => ScalarSettingValue::Bool(false),
            (_, Some(toml::Value::String(value))) => ScalarSettingValue::String(value.clone()),
            (_, _) => ScalarSettingValue::String(String::new()),
        };
        values.insert(setting_path, scalar);
    }
    Ok(values)
}

/// Writes an editable bool, string, or path setting to the active TOML file.
/// The edited document is deserialized as [`FileConfig`] before an atomic
/// replacement, so invalid UI input never corrupts the user's configuration.
pub fn write_scalar_setting(path: &str, value: ScalarSettingValue) -> Result<(), String> {
    let config_path = active_config_path()?;
    let document = std::fs::read_to_string(&config_path)
        .map_err(|error| format!("could not read {}: {error}", config_path.display()))?;
    let mut segments = path.split('.').collect::<Vec<_>>();
    let key = segments
        .pop()
        .filter(|key| !key.is_empty())
        .ok_or_else(|| "setting path cannot be empty".to_owned())?;
    if segments.iter().any(|segment| segment.is_empty()) {
        return Err(format!("invalid setting path `{path}`"));
    }
    let item = match value {
        ScalarSettingValue::Bool(value) => toml_edit::value(value),
        ScalarSettingValue::String(value) => toml_edit::value(value),
    };
    let updated = upsert_toml_value(&document, &segments, key, item)
        .map_err(|error| format!("could not update TOML: {error}"))?;
    toml::from_str::<FileConfig>(&updated)
        .map_err(|error| format!("updated configuration is invalid: {error}"))?;

    let temporary = config_path.with_extension(format!("toml.{}.tmp", std::process::id()));
    std::fs::write(&temporary, updated)
        .map_err(|error| format!("could not write {}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, &config_path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        format!(
            "could not replace {} with updated configuration: {error}",
            config_path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the tests that mutate process environment; Rust runs
    /// tests on threads sharing one environment.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn parses_library_sections() {
        let config: FileConfig = toml::from_str(
            r#"
[library.romm]
enable = true
url = "https://romm.example.com"
token = "secret"
import_on_startup = true

[library.local]
scan_on_startup = false
"#,
        )
        .unwrap();
        assert_eq!(
            config.library.romm.url.as_deref(),
            Some("https://romm.example.com")
        );
        assert_eq!(config.library.romm.token.as_deref(), Some("secret"));
        assert_eq!(config.library.romm.enable, Some(true));
        assert_eq!(config.library.local.scan_on_startup, Some(false));
    }

    #[test]
    fn parses_clock_section() {
        let config: FileConfig = toml::from_str(
            r#"
[general.time_date]
twelve_hour = true
"#,
        )
        .unwrap();
        assert_eq!(config.general.time_date.twelve_hour, Some(true));
        // The shorthand `12hr` key is accepted as an alias.
        let config: FileConfig = toml::from_str(
            r#"
[general.time_date]
12hr = true
"#,
        )
        .unwrap();
        assert_eq!(config.general.time_date.twelve_hour, Some(true));

        let figment = Figment::new().merge(Toml::string("[general.time_date]\n12hr = true\n"));
        assert!(Config::from_figment(figment).clock_twelve_hour);
        assert!(!Config::from_figment(Figment::new()).clock_twelve_hour);
    }

    #[test]
    fn figment_merges_toml_base_with_key_path_overrides() {
        let figment = Figment::new()
            .merge(Toml::string(
                "[library.romm]\nenable = true\nurl = \"https://file.example.com\"\n",
            ))
            .merge((
                "library.romm.url",
                "https://override.example.com".to_string(),
            ));
        let config = Config::from_figment(figment);
        assert_eq!(
            config.romm_url.as_deref(),
            Some("https://override.example.com")
        );
    }

    #[test]
    fn config_file_layers_are_ordered_and_primary_wins() {
        let root = std::env::temp_dir().join(format!(
            "marina-config-layers-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let shared_dropins = root.join("usr/share/marina/config.toml.d");
        let shared_config = root.join("usr/share/marina/config.toml");
        let etc_dropins = root.join("etc/marina/config.toml.d");
        let etc_config = root.join("etc/marina/config.toml");
        let user_dropins = root.join("config/marina/config.toml.d");
        let primary = root.join("config/marina/config.toml");

        for directory in [&shared_dropins, &etc_dropins, &user_dropins] {
            std::fs::create_dir_all(directory).unwrap();
        }
        std::fs::create_dir_all(primary.parent().unwrap()).unwrap();
        std::fs::write(
            shared_dropins.join("20-later.toml"),
            "[runtime.retroarch]\nbinary = \"/shared-20\"\n",
        )
        .unwrap();
        std::fs::write(
            shared_dropins.join("10-earlier.toml"),
            "[runtime.retroarch]\nbinary = \"/shared-10\"\ncores_dir = \"/shared/cores\"\n",
        )
        .unwrap();
        std::fs::write(shared_dropins.join("README"), "not TOML").unwrap();
        std::fs::write(
            &shared_config,
            "[runtime.retroarch]\nbinary = \"/shared-main\"\n",
        )
        .unwrap();
        std::fs::write(
            etc_dropins.join("10-admin.toml"),
            "[runtime.retroarch]\nbinary = \"/etc-dropin\"\n",
        )
        .unwrap();
        std::fs::write(&etc_config, "[runtime.retroarch]\nbinary = \"/etc-main\"\n").unwrap();
        std::fs::write(
            user_dropins.join("10-user.toml"),
            "[runtime.retroarch]\nbinary = \"/user-dropin\"\n",
        )
        .unwrap();
        std::fs::write(&primary, "[runtime.retroarch]\nbinary = \"/primary\"\n").unwrap();

        let sources = config_sources_from(
            &shared_dropins,
            &shared_config,
            &etc_dropins,
            &etc_config,
            Some(&user_dropins),
            Some(&primary),
        );
        assert_eq!(
            sources,
            vec![
                shared_dropins.join("10-earlier.toml"),
                shared_dropins.join("20-later.toml"),
                shared_config.clone(),
                etc_dropins.join("10-admin.toml"),
                etc_config.clone(),
                user_dropins.join("10-user.toml"),
                primary.clone(),
            ]
        );

        let config = Config::from_figment(merge_config_files(Figment::new(), sources));
        assert_eq!(config.retroarch.binary, PathBuf::from("/primary"));
        assert_eq!(config.retroarch.cores_dir, PathBuf::from("/shared/cores"));

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn toml_edit_upsert_preserves_comments() {
        let updated = upsert_toml_value(
            "# my romm server\n[library.romm]\nenable = false\n",
            &["library", "romm"],
            "url",
            toml_edit::value("https://romm.example.com"),
        )
        .unwrap();
        assert!(updated.contains("# my romm server"));
        assert!(updated.contains("enable = false"));
        assert!(updated.contains(r#"url = "https://romm.example.com""#));
    }

    #[test]
    fn generated_template_parses_and_covers_every_key() {
        let template = default_config_template();
        let value: toml::Value = toml::from_str(&template).expect("template must parse as TOML");
        for table in ["general", "library", "runtime"] {
            assert!(
                value.get(table).is_some(),
                "template is missing [{table}]:\n{template}"
            );
        }
        assert!(value["general"].get("time_date").is_some());
        assert!(value["library"].get("local").is_some());
        assert!(value["library"].get("romm").is_some());
        assert!(value["runtime"]["retroarch"].get("platforms").is_some());
        // Non-optional leaves render their real defaults, active.
        assert_eq!(
            value["runtime"]["retroarch"]
                .get("binary")
                .and_then(|v| v.as_str()),
            Some("retroarch")
        );
        assert_eq!(
            value["runtime"]["retroarch"]
                .get("cores_dir")
                .and_then(|v| v.as_str()),
            Some("/var/games/retroarch/cores")
        );
        // Optional leaves render as commented placeholders with docs + env.
        for key in [
            "enable =",
            "url =",
            "token =",
            "import_on_startup =",
            "root =",
            "storage_uri =",
            "store_cache_dir =",
            "scan_on_startup =",
            "twelve_hour =",
            "backend =",
            "core =",
        ] {
            assert!(
                template.contains(key),
                "template is missing key `{key}`:\n{template}"
            );
        }
        for marker in [
            "MARINA_ENABLE_ROMM",
            "ROMM_URL",
            "ROMM_TOKEN",
            "MARINA_IMPORT_ROMM_ON_STARTUP",
            "MARINA_LIBRARY_ROOT",
            "MARINA_STORAGE_URI",
            "MARINA_STORE_CACHE_DIR",
            "MARINA_SCAN_ON_STARTUP",
            "MARINA_CLOCK_12HR",
            "MARINA_RETROARCH_BINARY",
            "MARINA_RETROARCH_CORES_DIR",
        ] {
            assert!(
                template.contains(marker),
                "template is missing env `{marker}`:\n{template}"
            );
        }
        // Doc comments made it into the file; the skipped alias did not.
        assert!(template.contains("libretro cores"));
        assert!(
            !template.contains("platform = "),
            "deprecated `platform` alias must stay out of the template"
        );
    }

    #[test]
    fn ensure_writes_template_once_and_never_overwrites() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "marina-config-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("nested").join("config.toml");
        let previous = env::var_os("MARINA_CONFIG");
        // SAFETY: this is the only test that touches MARINA_CONFIG, and no
        // other test reads it, so no other thread can observe the mutation.
        unsafe {
            env::set_var("MARINA_CONFIG", &path);
        }

        let written = ensure_config_file().expect("ensure must create a missing file");
        assert_eq!(written, path);
        assert_eq!(
            std::fs::read_to_string(&path).expect("written file must be readable"),
            default_config_template()
        );
        // A second call is a no-op so user edits survive restarts.
        std::fs::write(&path, "# user edits\n").unwrap();
        let kept = ensure_config_file().expect("ensure must keep an existing file");
        assert_eq!(kept, path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# user edits\n");

        std::fs::remove_dir_all(&dir).ok();
        // SAFETY: same as above; restores the pre-test environment.
        unsafe {
            match previous {
                Some(value) => env::set_var("MARINA_CONFIG", value),
                None => env::remove_var("MARINA_CONFIG"),
            }
        }
    }

    #[test]
    fn env_vars_merge_over_file_through_generated_bindings() {
        let _guard = ENV_LOCK.lock().unwrap();
        // The generated bindings must address the documented key paths.
        let bindings = FileConfig::default().__marina_env_bindings("");
        for (path, var, is_bool) in [
            ("library.romm.enable", "MARINA_ENABLE_ROMM", true),
            ("library.romm.url", "ROMM_URL", false),
            (
                "library.local.scan_on_startup",
                "MARINA_SCAN_ON_STARTUP",
                true,
            ),
            ("general.time_date.twelve_hour", "MARINA_CLOCK_12HR", true),
            ("library.local.root", "MARINA_LIBRARY_ROOT", false),
            ("runtime.retroarch.binary", "MARINA_RETROARCH_BINARY", false),
            (
                "runtime.retroarch.cores_dir",
                "MARINA_RETROARCH_CORES_DIR",
                false,
            ),
        ] {
            let expected = (path.to_owned(), var, is_bool);
            assert!(
                bindings.contains(&expected),
                "missing env binding {expected:?} in {bindings:?}"
            );
        }

        const TOUCHED: &[&str] = &[
            "MARINA_CONFIG",
            "MARINA_ENABLE_ROMM",
            "ROMM_URL",
            "ROMM_TOKEN",
            "MARINA_IMPORT_ROMM_ON_STARTUP",
            "MARINA_SCAN_ON_STARTUP",
            "MARINA_CLOCK_12HR",
            "MARINA_STORAGE_URI",
            "MARINA_STORE_CACHE_DIR",
            "MARINA_LIBRARY_ROOT",
            "MARINA_RETROARCH_BINARY",
            "MARINA_RETROARCH_CORES_DIR",
        ];
        let stashed: Vec<(String, Option<std::ffi::OsString>)> = TOUCHED
            .iter()
            .map(|var| ((*var).to_owned(), env::var_os(var)))
            .collect();
        // SAFETY: this is the only test that touches these variables, and
        // no other test reads them, so no other thread can observe this.
        unsafe {
            for var in TOUCHED {
                env::remove_var(var);
            }
        }
        let dir = std::env::temp_dir().join(format!(
            "marina-config-env-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("config.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &path,
            r#"
[library.romm]
enable = true
url = "https://file.example.com"
token = "file-token"
import_on_startup = false

[library.local]
scan_on_startup = false

[runtime.retroarch]
binary = "/file/retroarch"
cores_dir = "/file/cores"
"#,
        )
        .unwrap();
        unsafe {
            env::set_var("MARINA_CONFIG", &path);
            env::set_var("ROMM_URL", "https://env.example.com");
            env::set_var("MARINA_IMPORT_ROMM_ON_STARTUP", "true");
            env::set_var("MARINA_SCAN_ON_STARTUP", "yes");
            env::set_var("MARINA_LIBRARY_ROOT", "/env/library");
            env::set_var("MARINA_RETROARCH_BINARY", "/env/retroarch");
        }

        let config = Config::from_env();
        assert_eq!(config.romm_url.as_deref(), Some("https://env.example.com"));
        assert!(config.import_romm_on_startup);
        assert!(config.scan_on_startup);
        assert_eq!(config.library_root, Some(PathBuf::from("/env/library")));
        assert_eq!(config.retroarch.binary, PathBuf::from("/env/retroarch"));
        // Nothing set in the environment: the file value survives.
        assert_eq!(config.retroarch.cores_dir, PathBuf::from("/file/cores"));
        assert_eq!(config.romm_token.as_deref(), Some("file-token"));

        std::fs::remove_dir_all(&dir).ok();
        unsafe {
            for (var, value) in stashed {
                match value {
                    Some(value) => env::set_var(var, value),
                    None => env::remove_var(var),
                }
            }
        }
    }

    #[test]
    fn default_database_path_uses_xdg_state_dir() {
        let _guard = ENV_LOCK.lock().unwrap();
        let state = std::env::temp_dir().join(format!(
            "marina-state-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let previous_state = env::var_os("XDG_STATE_HOME");
        let previous_cache_uri = env::var_os("MARINA_STORE_CACHE_URI");
        // SAFETY: only this test touches these variables, guarded by ENV_LOCK.
        unsafe {
            env::set_var("XDG_STATE_HOME", &state);
            env::remove_var("MARINA_STORE_CACHE_URI");
        }

        let config = Config::from_figment(Figment::new());
        assert_eq!(
            config.storage_uri,
            format!("sqlite://{}/marina/library.db", state.display())
        );
        assert_eq!(
            config.store_cache_dir,
            state.join("marina").join("store-cache")
        );

        unsafe {
            match previous_state {
                Some(value) => env::set_var("XDG_STATE_HOME", value),
                None => env::remove_var("XDG_STATE_HOME"),
            }
            match previous_cache_uri {
                Some(value) => env::set_var("MARINA_STORE_CACHE_URI", value),
                None => env::remove_var("MARINA_STORE_CACHE_URI"),
            }
        }
    }

    #[test]
    fn parses_retroarch_and_platform_sections() {
        let config: FileConfig = toml::from_str(
            r#"
[runtime.retroarch]
binary = "/usr/bin/retroarch"
cores_dir = "/var/games/retroarch/cores"
extra_args = ["-f"]

[runtime.retroarch.platforms."gba"]
backend = "retroarch"
[runtime.retroarch.platforms."gba".retroarch]
core = "mgba_libretro.so"

[runtime.retroarch.platforms."snes"]
backend = "native"
"#,
        )
        .unwrap();
        assert_eq!(
            config.runtime.retroarch.binary,
            PathBuf::from("/usr/bin/retroarch")
        );
        assert_eq!(
            config.runtime.retroarch.cores_dir,
            PathBuf::from("/var/games/retroarch/cores")
        );
        assert_eq!(config.runtime.retroarch.extra_args, vec!["-f"]);
        let gba = &config.runtime.retroarch.platforms["gba"];
        assert_eq!(
            gba.backend_kind("gba"),
            marina_runtime::PlatformBackendKind::RetroArch
        );
        assert_eq!(
            gba.retroarch.core.as_deref(),
            Some(std::path::Path::new("mgba_libretro.so"))
        );
        assert_eq!(
            config.runtime.retroarch.platforms["snes"].backend_kind("snes"),
            marina_runtime::PlatformBackendKind::Native
        );
    }

    #[test]
    fn platform_tables_accept_absolute_core_paths() {
        let figment = Figment::new().merge(Toml::string(
            "[runtime.retroarch.platforms.\"gba\"]\nbackend = \"retroarch\"\n[runtime.retroarch.platforms.\"gba\".retroarch]\ncore = \"/opt/cores/mgba_libretro.so\"\n",
        ));
        let config = Config::from_figment(figment);
        assert_eq!(
            config.platforms["gba"].retroarch.core.as_deref(),
            Some(std::path::Path::new("/opt/cores/mgba_libretro.so"))
        );
    }

    #[test]
    fn runtime_config_reload_observes_core_file_edits() {
        let _guard = ENV_LOCK.lock().unwrap();
        const TOUCHED: &[&str] = &[
            "MARINA_CONFIG",
            "MARINA_RETROARCH_BINARY",
            "MARINA_RETROARCH_CORES_DIR",
        ];
        let stashed = TOUCHED
            .iter()
            .map(|var| ((*var).to_owned(), env::var_os(var)))
            .collect::<Vec<_>>();
        let dir = std::env::temp_dir().join(format!(
            "marina-config-reload-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("config.toml");
        std::fs::create_dir_all(&dir).unwrap();
        unsafe {
            for var in TOUCHED {
                env::remove_var(var);
            }
            env::set_var("MARINA_CONFIG", &path);
        }

        std::fs::write(
            &path,
            "[runtime.retroarch.platforms.\"snes\".retroarch]\ncore = \"snes9x_libretro.so\"\n",
        )
        .unwrap();
        let initial = Config::from_env();
        assert_eq!(
            initial.platforms["snes"].retroarch.core.as_deref(),
            Some(std::path::Path::new("snes9x_libretro.so"))
        );

        std::fs::write(
            &path,
            "[runtime.retroarch.platforms.\"snes\".retroarch]\ncore = \"bsnes_libretro.so\"\n",
        )
        .unwrap();
        let reloaded = Config::from_env();
        assert_eq!(
            reloaded.platforms["snes"].retroarch.core.as_deref(),
            Some(std::path::Path::new("bsnes_libretro.so"))
        );

        std::fs::remove_dir_all(&dir).ok();
        unsafe {
            for (var, value) in stashed {
                match value {
                    Some(value) => env::set_var(var, value),
                    None => env::remove_var(var),
                }
            }
        }
    }

    #[test]
    fn retroarch_env_overrides_merge_over_file() {
        let figment = Figment::new()
            .merge(Toml::string(
                "[runtime.retroarch]\nbinary = \"/file/retroarch\"\ncores_dir = \"/file/cores\"\n",
            ))
            .merge(("runtime.retroarch.binary", "/env/retroarch".to_string()));
        let config = Config::from_figment(figment);
        assert_eq!(config.retroarch.binary, PathBuf::from("/env/retroarch"));
        assert_eq!(config.retroarch.cores_dir, PathBuf::from("/file/cores"));
    }
}
