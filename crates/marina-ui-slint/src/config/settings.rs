use super::*;

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
