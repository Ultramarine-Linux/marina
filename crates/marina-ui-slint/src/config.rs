//! Runtime configuration: figment-merged TOML file + environment.
//!
//! Store backends live in their own config-file section so adding a backend
//! is a new `[store.<backend>]` table, not more top-level env vars:
//!
//! ```toml
//! [store.romm]
//! enable = true
//! url = "https://romm.example.com"
//! token = "rmm_..."
//! import_on_startup = false
//! ```
//!
//! Emulator runtimes follow the same pattern. `[retroarch]` holds the
//! frontend-wide settings and each `[platform."<slug>"]` table pins a
//! backend (and its settings) for one platform:
//!
//! ```toml
//! [retroarch]
//! binary = "retroarch"
//! cores_dir = "/var/games/retroarch/cores"
//!
//! [platform."gba"]
//! backend = "retroarch"
//! [platform."gba".retroarch]
//! core = "mgba_libretro.so"
//! ```
//!
//! Reads go through figment: the TOML file is the base layer and each
//! environment variable merges over it as a key-path tuple, so precedence
//! per field is: environment variable > config file > default. The file is
//! looked up at `$MARINA_CONFIG`, then `./marina.toml`, then
//! `$XDG_CONFIG_HOME/marina/config.toml` (`~/.config/marina/config.toml`).
//! When none exists, [`ensure_config_file`] writes [`default_config_template`]
//! (rendered from the config structs' defaults and `///` doc comments via
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
use marina_config_derive::ConfigTemplate;
use marina_portmaster::{DEFAULT_PORTS_DIR, DEFAULT_RELEASE, PortMasterConfig};
use marina_runtime::{
    PlatformRuntimeConfig, RetroArchConfig, portmaster::Config as RuntimePortMasterConfig,
};
use serde::Deserialize;
use tracing::warn;

/// Top-level Marina configuration file.
#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate)]
struct FileConfig {
    #[serde(default)]
    store: StoreSection,
    #[serde(default)]
    #[template(table)]
    library: LibrarySection,
    #[serde(default)]
    #[template(table)]
    clock: ClockSection,
    #[serde(default)]
    #[template(table)]
    retroarch: RetroArchConfig,
    #[serde(default)]
    #[template(table)]
    portmaster: RuntimePortMasterConfig,
    /// Per-platform backend and core selection, keyed by platform slug.
    #[serde(default)]
    #[template(example = "gba")]
    platform: HashMap<String, PlatformRuntimeConfig>,
}

/// Store backend namespace: each backend gets its own `[store.<backend>]`
/// table so adding one never needs new top-level keys.
#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate)]
struct StoreSection {
    #[serde(default)]
    #[template(table)]
    romm: RommConfig,
    #[serde(default)]
    #[template(table)]
    portmaster: PortMasterStoreConfig,
}

#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate)]
struct LibrarySection {
    /// Root directory containing the local library (`roms/<platform>/...`).
    #[serde(default)]
    #[template(env = "MARINA_LIBRARY_ROOT", example = "/var/games/library")]
    root: Option<PathBuf>,
    /// SQLite library database URI.
    #[serde(default)]
    #[template(env = "MARINA_STORAGE_URI", example = "sqlite://marina.db")]
    storage_uri: Option<String>,
    /// Directory holding per-backend store catalog caches.
    #[serde(default)]
    #[template(env = "MARINA_STORE_CACHE_DIR", example = "")]
    store_cache_dir: Option<PathBuf>,
    /// Scan the local library root for games at startup.
    #[serde(default)]
    #[template(env = "MARINA_SCAN_ON_STARTUP", example = "true")]
    scan_on_startup: Option<bool>,
}

/// Top-bar digital clock settings.
#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate)]
struct ClockSection {
    /// Use 12-hour time (`9:05 PM`) instead of 24-hour time (`21:05`).
    #[serde(default, alias = "12hr")]
    #[template(env = "MARINA_CLOCK_12HR", example = "false")]
    twelve_hour: Option<bool>,
}

/// A single store backend's file configuration.
#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate)]
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
    pub token: Option<String>,
    /// Import the RomM catalog into the store cache at startup.
    #[serde(default)]
    #[template(env = "MARINA_IMPORT_ROMM_ON_STARTUP", example = "false")]
    pub import_on_startup: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, ConfigTemplate)]
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

    pub retroarch: RetroArchConfig,
    pub portmaster: RuntimePortMasterConfig,
    pub platforms: HashMap<String, PlatformRuntimeConfig>,
}

impl Config {
    pub fn from_env() -> Self {
        if let Err(error) = ensure_config_file() {
            warn!(%error, "could not write default config file; continuing with defaults");
        }
        let mut figment = Figment::new();
        if let Some(path) = config_candidates().into_iter().find(|p| p.is_file()) {
            tracing::info!(path = %path.display(), "loading config file");
            figment = figment.merge(Toml::file(path));
        }
        // Env vars merge over the file as key-path tuples: `(key, value)`
        // with a dotted path like "store.romm.url" nests like the TOML.
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
        let romm = &file.store.romm;
        let portmaster = &file.store.portmaster;

        let romm_enabled = romm.enable.unwrap_or(false);
        let romm_url = if romm_enabled { romm.url.clone() } else { None };
        let romm_token = if romm_enabled {
            romm.token.clone()
        } else {
            None
        };
        let import_romm_on_startup = romm.import_on_startup.unwrap_or(false);
        let scan_on_startup = file.library.scan_on_startup.unwrap_or(true);
        let clock_twelve_hour = file.clock.twelve_hour.unwrap_or(false);
        if !scan_on_startup {
            tracing::info!(
                "local library scan disabled at startup by config or MARINA_SCAN_ON_STARTUP=false"
            );
        }
        if !romm_enabled {
            tracing::info!(
                "RomM backend disabled by default; enable it with [store.romm] enable = true"
            );
        } else if romm_url.is_none() {
            warn!("[store.romm] url not set — relative cover paths will not resolve");
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
                .storage_uri
                .clone()
                .unwrap_or(default_storage_uri),
            store_cache_dir: file
                .library
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
            library_root: file.library.root.clone().or(default_library_root),
            retroarch: file.retroarch.clone(),
            portmaster: file.portmaster.clone(),
            platforms: file.platform.clone(),
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

fn config_candidates() -> Vec<PathBuf> {
    if let Some(path) = env::var_os("MARINA_CONFIG").map(PathBuf::from) {
        return vec![path];
    }
    let mut candidates = vec![PathBuf::from("marina.toml")];
    let xdg = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")));
    if let Some(base) = xdg {
        candidates.push(base.join("marina").join("config.toml"));
    }
    candidates
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
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
        .map(|base| base.join("marina").join("config.toml"))
}

/// Renders the default configuration file. Values come from the config
/// structs' `Default` impls and documentation from their `///` doc
/// comments (see `ConfigTemplate`), so the template cannot drift from the
/// schema the way a hand-maintained example file can. Commented-out keys
/// show illustrative values; the app behaves exactly as if the file did
/// not exist until they are uncommented.
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
/// whitespace, and key order. Backing for future in-UI settings editing:
/// parse the config file into a [`toml_edit::Document`], upsert, write
/// back with `doc.to_string()`.
/// Not wired into the UI yet; the settings screen will call this.
#[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the tests that mutate process environment; Rust runs
    /// tests on threads sharing one environment.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn parses_store_romm_section() {
        let config: FileConfig = toml::from_str(
            r#"
[store.romm]
enable = true
url = "https://romm.example.com"
token = "secret"
import_on_startup = true

[library]
scan_on_startup = false
"#,
        )
        .unwrap();
        assert_eq!(
            config.store.romm.url.as_deref(),
            Some("https://romm.example.com")
        );
        assert_eq!(config.store.romm.token.as_deref(), Some("secret"));
        assert_eq!(config.store.romm.enable, Some(true));
        assert_eq!(config.library.scan_on_startup, Some(false));
    }

    #[test]
    fn parses_clock_section() {
        let config: FileConfig = toml::from_str(
            r#"
[clock]
twelve_hour = true
"#,
        )
        .unwrap();
        assert_eq!(config.clock.twelve_hour, Some(true));
        // The shorthand `12hr` key is accepted as an alias.
        let config: FileConfig = toml::from_str(
            r#"
[clock]
12hr = true
"#,
        )
        .unwrap();
        assert_eq!(config.clock.twelve_hour, Some(true));

        let figment = Figment::new().merge(Toml::string("[clock]\n12hr = true\n"));
        assert!(Config::from_figment(figment).clock_twelve_hour);
        assert!(!Config::from_figment(Figment::new()).clock_twelve_hour);
    }

    #[test]
    fn figment_merges_toml_base_with_key_path_overrides() {
        let figment = Figment::new()
            .merge(Toml::string(
                "[store.romm]\nenable = true\nurl = \"https://file.example.com\"\n",
            ))
            .merge(("store.romm.url", "https://override.example.com".to_string()));
        let config = Config::from_figment(figment);
        assert_eq!(
            config.romm_url.as_deref(),
            Some("https://override.example.com")
        );
    }

    #[test]
    fn toml_edit_upsert_preserves_comments() {
        let updated = upsert_toml_value(
            "# my romm server\n[store.romm]\nenable = false\n",
            &["store", "romm"],
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
        for table in ["store", "library", "clock", "retroarch", "platform"] {
            assert!(
                value.get(table).is_some(),
                "template is missing [{table}]:\n{template}"
            );
        }
        assert!(value["store"].get("romm").is_some());
        assert!(value["platform"].get("gba").is_some());
        // Non-optional leaves render their real defaults, active.
        assert_eq!(
            value["retroarch"].get("binary").and_then(|v| v.as_str()),
            Some("retroarch")
        );
        assert_eq!(
            value["retroarch"].get("cores_dir").and_then(|v| v.as_str()),
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
            ("store.romm.enable", "MARINA_ENABLE_ROMM", true),
            ("store.romm.url", "ROMM_URL", false),
            ("library.scan_on_startup", "MARINA_SCAN_ON_STARTUP", true),
            ("clock.twelve_hour", "MARINA_CLOCK_12HR", true),
            ("library.root", "MARINA_LIBRARY_ROOT", false),
            ("retroarch.binary", "MARINA_RETROARCH_BINARY", false),
            ("retroarch.cores_dir", "MARINA_RETROARCH_CORES_DIR", false),
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
[store.romm]
enable = true
url = "https://file.example.com"
token = "file-token"
import_on_startup = false

[library]
scan_on_startup = false

[retroarch]
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
[retroarch]
binary = "/usr/bin/retroarch"
cores_dir = "/var/games/retroarch/cores"
extra_args = ["-f"]

[platform."gba"]
backend = "retroarch"
[platform."gba".retroarch]
core = "mgba_libretro.so"

[platform."snes"]
backend = "native"
"#,
        )
        .unwrap();
        assert_eq!(config.retroarch.binary, PathBuf::from("/usr/bin/retroarch"));
        assert_eq!(
            config.retroarch.cores_dir,
            PathBuf::from("/var/games/retroarch/cores")
        );
        assert_eq!(config.retroarch.extra_args, vec!["-f"]);
        let gba = &config.platform["gba"];
        assert_eq!(
            gba.backend_kind("gba"),
            marina_runtime::PlatformBackendKind::RetroArch
        );
        assert_eq!(
            gba.retroarch.core.as_deref(),
            Some(std::path::Path::new("mgba_libretro.so"))
        );
        assert_eq!(
            config.platform["snes"].backend_kind("snes"),
            marina_runtime::PlatformBackendKind::Native
        );
    }

    #[test]
    fn platform_tables_accept_absolute_core_paths() {
        let figment = Figment::new().merge(Toml::string(
            "[platform.\"gba\"]\nbackend = \"retroarch\"\n[platform.\"gba\".retroarch]\ncore = \"/opt/cores/mgba_libretro.so\"\n",
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
            "[platform.\"snes\".retroarch]\ncore = \"snes9x_libretro.so\"\n",
        )
        .unwrap();
        let initial = Config::from_env();
        assert_eq!(
            initial.platforms["snes"].retroarch.core.as_deref(),
            Some(std::path::Path::new("snes9x_libretro.so"))
        );

        std::fs::write(
            &path,
            "[platform.\"snes\".retroarch]\ncore = \"bsnes_libretro.so\"\n",
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
                "[retroarch]\nbinary = \"/file/retroarch\"\ncores_dir = \"/file/cores\"\n",
            ))
            .merge(("retroarch.binary", "/env/retroarch".to_string()));
        let config = Config::from_figment(figment);
        assert_eq!(config.retroarch.binary, PathBuf::from("/env/retroarch"));
        assert_eq!(config.retroarch.cores_dir, PathBuf::from("/file/cores"));
    }
}
