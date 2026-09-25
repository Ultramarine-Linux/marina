use super::*;

pub(super) fn env_bool(name: &str) -> Option<bool> {
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
pub(super) fn apply_env(figment: Figment) -> Figment {
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
pub(super) fn config_sources() -> Vec<PathBuf> {
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
pub(super) fn config_sources_from(
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
pub(super) fn dropin_files(directory: &std::path::Path) -> Vec<PathBuf> {
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

pub(super) fn merge_config_files(
    figment: Figment,
    paths: impl IntoIterator<Item = PathBuf>,
) -> Figment {
    paths.into_iter().fold(figment, |figment, path| {
        tracing::info!(path = %path.display(), "loading config file");
        figment.merge(Toml::file(path))
    })
}

pub(super) fn xdg_config_dir() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
}

/// Selects the config file that users edit directly. It is intentionally
/// separate from the lower-priority, system-managed configuration sources.
pub(super) fn primary_config_path() -> Option<PathBuf> {
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
pub(super) fn config_write_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("MARINA_CONFIG").map(PathBuf::from) {
        return Some(path);
    }
    if PathBuf::from("marina.toml").is_file() {
        return Some(PathBuf::from("marina.toml"));
    }
    xdg_config_dir().map(|base| base.join("marina").join("config.toml"))
}
