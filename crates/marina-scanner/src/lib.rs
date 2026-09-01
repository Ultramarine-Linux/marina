use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use marina_core::{ItemKind, LibraryItem, LibraryItemFile, LibraryItemId};

#[derive(Debug)]
pub enum ScanError {
    Io { path: PathBuf, source: io::Error },
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "failed to scan {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for ScanError {}

/// Scans an ES-DE-style local library root.
///
/// The expected layout is `<root>/roms/<platform>/<game>/<files>`. A platform may
/// also contain a single ROM directly; that file becomes its own game entry.
pub fn scan(root: impl AsRef<Path>) -> Result<Vec<LibraryItem>, ScanError> {
    let root = root.as_ref();
    let platforms = read_dirs(&root.join("roms"))?;
    let mut items = Vec::new();

    for platform_dir in platforms {
        let platform_slug = platform_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();

        for entry in read_entries(&platform_dir)? {
            if entry.is_dir() {
                let files = rom_files(&entry)?;
                if files.is_empty() {
                    continue;
                }
                items.push(make_item(&platform_slug, &entry, &files));
            } else if is_rom_file(&entry) {
                items.push(make_item(
                    &platform_slug,
                    &entry,
                    std::slice::from_ref(&entry),
                ));
            }
        }
    }

    items.sort_by(|left, right| {
        left.platform_slug
            .cmp(&right.platform_slug)
            .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
    });
    Ok(items)
}

fn make_item(platform_slug: &str, entry: &Path, files: &[PathBuf]) -> LibraryItem {
    let title = entry
        .file_stem()
        .or_else(|| entry.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("Unknown game")
        .to_owned();
    let local_path = entry.to_string_lossy().into_owned();
    let mut item = LibraryItem {
        id: LibraryItemId::from_provider("local", "path", &local_path),
        title,
        kind: ItemKind::Game,
        platform_slug: Some(platform_slug.to_owned()),
        local_path: Some(local_path),
        ..LibraryItem::default()
    };
    item.files = files
        .iter()
        .filter_map(|path| {
            let metadata = fs::metadata(path).ok()?;
            Some(LibraryItemFile {
                provider_id: None,
                name: path.file_name()?.to_string_lossy().into_owned(),
                path: path.to_string_lossy().into_owned(),
                size_bytes: Some(metadata.len()),
            })
        })
        .collect();
    item
}

fn read_dirs(path: &Path) -> Result<Vec<PathBuf>, ScanError> {
    read_entries(path).map(|entries| entries.into_iter().filter(|path| path.is_dir()).collect())
}

fn read_entries(path: &Path) -> Result<Vec<PathBuf>, ScanError> {
    let entries = fs::read_dir(path).map_err(|source| ScanError::Io {
        path: path.to_owned(),
        source,
    })?;
    Ok(entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| !is_hidden(path) && !is_metadata_dir(path))
        .collect())
}

fn rom_files(path: &Path) -> Result<Vec<PathBuf>, ScanError> {
    Ok(read_entries(path)?
        .into_iter()
        .filter(|entry| entry.is_file() && is_rom_file(entry))
        .collect())
}

fn is_rom_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| !extension.eq_ignore_ascii_case("xml"))
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

fn is_metadata_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, ".media" | ".images" | ".cache"))
}

#[cfg(test)]
mod tests {
    use super::scan;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn scans_games_and_skips_media() {
        let root = std::env::temp_dir().join(format!(
            "marina-scanner-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let game = root.join("roms").join("snes").join("Super Mario World");
        fs::create_dir_all(game.join(".media")).unwrap();
        fs::write(game.join("Super Mario World.sfc"), b"rom").unwrap();
        fs::write(game.join(".media").join("box.png"), b"art").unwrap();

        let items = scan(&root).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Super Mario World");
        assert_eq!(items[0].files.len(), 1);
        assert_eq!(items[0].platform_slug.as_deref(), Some("snes"));

        fs::remove_dir_all(root).unwrap();
    }
}
