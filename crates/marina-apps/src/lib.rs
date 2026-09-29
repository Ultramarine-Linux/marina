//! Discovery of applications installed through the XDG desktop entry system.

use freedesktop_desktop_entry::{Iter, default_paths, get_languages_from_env};
use marina_core::{ItemKind, LibraryAsset, LibraryAssetKind, LibraryItem, LibraryItemId};
use marina_library::{
    error::LibraryError,
    query::SearchQuery,
    read::LibraryRead,
    write::{LibraryWrite, PlatformWrite},
};

/// The stable platform identifier used for discovered desktop applications.
pub const PLATFORM_SLUG: &str = "apps";
pub const PLATFORM_NAME: &str = "Apps";

/// Loads visible XDG application desktop entries as Marina library items.
///
/// `freedesktop-desktop-entry` supplies the standard XDG application paths in
/// priority order. The first entry for an application ID wins, so a user's
/// desktop entry overrides a system entry and `Hidden=true` correctly masks it.
pub fn discover() -> Vec<LibraryItem> {
    let locales = get_languages_from_env();
    let mut items = Vec::new();
    let mut seen = std::collections::BTreeSet::new();

    for entry in Iter::new(default_paths()).entries(Some(&locales)) {
        if !seen.insert(entry.id().to_owned()) || entry.hidden() || entry.no_display() {
            continue;
        }
        if entry.type_() != Some("Application") {
            continue;
        }

        let Some(name) = entry.full_name(&locales).map(|name| name.into_owned()) else {
            continue;
        };
        let Ok(mut command) = entry.parse_exec() else {
            continue;
        };
        let Some(raw_executable) = command.first().cloned() else {
            continue;
        };
        if raw_executable.starts_with('%') {
            continue;
        }
        command[0] = expand_home(&raw_executable);

        let mut item = LibraryItem::new_game(name);
        item.id = LibraryItemId::from_provider("xdg", "desktop", entry.id());
        item.kind = ItemKind::App;
        item.platform_slug = Some(PLATFORM_SLUG.to_owned());
        // Desktop entries may share an executable while representing distinct
        // applications or profiles. Use the unique desktop-file path as the
        // local identity; launching uses the parsed `xdg.exec` command below.
        item.local_path = Some(entry.path.to_string_lossy().into_owned());
        item.provider_ids
            .insert("xdg.desktop".to_owned(), entry.id().to_owned());
        if let Ok(exec) = serde_json::to_string(&command) {
            item.provider_ids.insert("xdg.exec".to_owned(), exec);
        }
        if let Some(working_directory) = entry.path() {
            item.provider_ids
                .insert("xdg.path".to_owned(), expand_home(working_directory));
        }
        if let Some(icon) = entry.icon().and_then(resolve_icon) {
            item.assets.push(LibraryAsset {
                kind: LibraryAssetKind::CoverSmall,
                source: None,
                local_path: Some(icon),
            });
        }
        item.summary = entry.comment(&locales).map(|comment| comment.into_owned());
        item.tags = entry
            .categories()
            .unwrap_or_default()
            .into_iter()
            .filter(|category| !category.is_empty())
            .map(str::to_owned)
            .collect();
        items.push(item);
    }

    items.sort_by_cached_key(|item| item.title.to_lowercase());
    items
}

/// Resolves an XDG icon name or path to a concrete image path.
pub fn resolve_icon(icon: &str) -> Option<String> {
    let icon = expand_home(icon);
    let path = std::path::Path::new(&icon);
    if path.is_file() {
        return Some(icon);
    }
    freedesktop_icons::lookup(&icon)
        .with_size(256)
        .with_cache()
        .find()
        .map(|path| path.to_string_lossy().into_owned())
}

/// Resolves an icon from a specific installed XDG icon theme.
pub fn resolve_icon_in_theme(icon: &str, theme: &str) -> Option<String> {
    let icon = expand_home(icon);
    let path = std::path::Path::new(&icon);
    if path.is_file() {
        return Some(icon);
    }
    freedesktop_icons::lookup(&icon)
        .with_theme(theme)
        .with_size(256)
        .with_cache()
        .find()
        .map(|path| path.to_string_lossy().into_owned())
}

fn expand_home(path: &str) -> String {
    let Some(home) = std::env::var_os("HOME") else {
        return path.to_owned();
    };
    if path == "~" {
        return std::path::PathBuf::from(home)
            .to_string_lossy()
            .into_owned();
    }
    path.strip_prefix("~/")
        .map(|relative| std::path::PathBuf::from(home).join(relative))
        .map(|expanded| expanded.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

/// Reconciles discovered desktop entries with the persistent library backend.
pub async fn sync<L>(library: &L, discovered: Vec<LibraryItem>) -> Result<(), LibraryError>
where
    L: LibraryRead + LibraryWrite + PlatformWrite,
{
    library
        .add_platform(marina_core::Platform::new(PLATFORM_SLUG, PLATFORM_NAME))
        .await?;
    let existing = library
        .search(SearchQuery::new().platform(PLATFORM_SLUG).limit(usize::MAX))
        .await?
        .into_iter()
        .map(|item| (item.id.clone(), item))
        .collect::<std::collections::HashMap<_, _>>();
    let discovered_ids = discovered
        .iter()
        .map(|item| item.id.clone())
        .collect::<std::collections::HashSet<_>>();
    for item in existing.values() {
        if item.provider_ids.contains_key("xdg.desktop") && !discovered_ids.contains(&item.id) {
            library.remove(&item.id).await?;
        }
    }
    for item in discovered {
        match existing.get(&item.id) {
            Some(previous) if previous == &item => {}
            Some(_) => {
                library.update(item).await?;
            }
            None => {
                library.add(item).await?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_platform_is_stable() {
        assert_eq!(PLATFORM_SLUG, "apps");
        assert_eq!(PLATFORM_NAME, "Apps");
    }
}
