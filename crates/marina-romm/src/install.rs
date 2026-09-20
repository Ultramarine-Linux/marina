//! Installing a RomM entry into the local library.
//!
//! Downloads the selected ROM files and artwork into `library_root`
//! (`roms/<platform>/<game>` and `media/<platform>/<game>`), then creates
//! or reconciles the library database record.

use std::path::PathBuf;

use marina_core::LibraryItem;
use marina_library::{Library, query::SearchQuery};
use marina_store::StoreError;
use thiserror::Error;
use tracing::debug;

use crate::{Client, Rom, RomFile};

#[derive(Debug, Error)]
pub enum InstallError {
    #[error(transparent)]
    Romm(#[from] crate::Error),
    #[error(transparent)]
    Library(#[from] marina_library::error::LibraryError),
    #[error("entry payload is missing or invalid")]
    InvalidEntry,
    #[error("invalid RomM file size {0}")]
    InvalidSize(i64),
    #[error("downloaded file size mismatch: expected {expected}, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub struct ResolvedInstall {
    pub rom: Rom,
    /// Exact RomM artifacts selected for this installation.
    pub files: Vec<RomFile>,
    pub library_root: PathBuf,
}

/// Resolves an [`InstallRequest`](marina_store::InstallRequest) against this
/// backend: hydrates the ROM (from the entry payload, refetching when it is
/// missing) and matches the selected backend-opaque file ids.
pub async fn resolve(
    client: &Client,
    entry: &marina_store::StoreEntry,
    file_ids: &[String],
    library_root: PathBuf,
) -> Result<ResolvedInstall, InstallError> {
    let rom = match entry
        .payload_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<Rom>(json).ok())
    {
        Some(rom) => rom,
        None => {
            let id = entry
                .entry_id
                .parse::<i32>()
                .map_err(|_| InstallError::InvalidEntry)?;
            client.get_rom(id).await?
        }
    };
    let files = rom
        .files
        .files
        .iter()
        .filter(|file| file_ids.iter().any(|id| *id == file.id.to_string()))
        .cloned()
        .collect::<Vec<_>>();
    Ok(ResolvedInstall {
        rom,
        files,
        library_root,
    })
}

pub async fn install(
    client: &Client,
    library: &(dyn Library + Send + Sync),
    request: ResolvedInstall,
) -> Result<LibraryItem, InstallError> {
    let title = request
        .rom
        .name
        .clone()
        .unwrap_or_else(|| request.rom.files.fs_name.clone());
    let platform = safe_component(&request.rom.platform.platform_fs_slug);
    let game = safe_component(&title);
    let game_dir = request
        .library_root
        .join("roms")
        .join(&platform)
        .join(&game);
    // The scanner may run before the library exists at all. Installation is
    // the operation that materializes the layout, so create the game folder
    // before asking the RomM client to open its temporary output file.
    tokio::fs::create_dir_all(&game_dir).await?;
    let mut installed_files = Vec::with_capacity(request.files.len());
    for file in &request.files {
        if file.file_size_bytes < 0 {
            return Err(InstallError::InvalidSize(file.file_size_bytes));
        }
        let destination = game_dir.join(safe_component(&file.file_name));
        let expected = file.file_size_bytes as u64;
        let existing_size = tokio::fs::metadata(&destination)
            .await
            .ok()
            .map(|metadata| metadata.len());
        if existing_size != Some(expected) {
            let temporary = destination.with_extension(format!("part-{}", std::process::id()));
            let rom_id = if file.rom_id == 0 {
                request.rom.id
            } else {
                file.rom_id
            };
            client
                .download_file(rom_id, &file.file_name, Some(file.id), &temporary)
                .await?;
            let actual = tokio::fs::metadata(&temporary).await?.len();
            if actual != expected {
                let _ = tokio::fs::remove_file(&temporary).await;
                return Err(InstallError::SizeMismatch { expected, actual });
            }
            tokio::fs::rename(&temporary, &destination).await?;
        }
        installed_files.push(marina_core::LibraryItemFile {
            provider_id: Some(format!("romm:file:{}", file.id)),
            name: file.file_name.clone(),
            path: destination.to_string_lossy().into_owned(),
            size_bytes: Some(expected),
        });
    }
    debug!(
        title = %title,
        files = ?installed_files
            .iter()
            .map(|file| format!("{} ({} bytes)", file.path, file.size_bytes.unwrap_or(0)))
            .collect::<Vec<_>>(),
        "installed game files written"
    );

    let rom_id = request.rom.id;
    let mut item: LibraryItem = request.rom.into();
    item.local_path = Some(game_dir.to_string_lossy().into_owned());
    item.files = installed_files;
    let asset_dir = request
        .library_root
        .join("media")
        .join(&platform)
        .join(&game);
    for (index, asset) in item.assets.iter_mut().enumerate() {
        let Some(source) = asset
            .source
            .clone()
            .filter(|source| !source.trim().is_empty())
        else {
            continue;
        };
        let source_url = client.resource_url(&source);
        let suffix = match asset.kind {
            marina_core::LibraryAssetKind::CoverSmall => "cover-small",
            marina_core::LibraryAssetKind::CoverLarge => "cover-large",
            marina_core::LibraryAssetKind::Manual => "manual",
            marina_core::LibraryAssetKind::Video => "video",
            marina_core::LibraryAssetKind::Screenshot => "screenshot",
            marina_core::LibraryAssetKind::UserScreenshot => "user-screenshot",
        };
        let extension = asset_extension(&source);
        let destination = asset_dir.join(format!("{suffix}-{index}.{extension}"));
        client.download_url(&source_url, &destination).await?;
        asset.local_path = Some(destination.to_string_lossy().into_owned());
    }
    let existing = library
        .search(SearchQuery::new().platform(platform).limit(usize::MAX))
        .await?
        .into_iter()
        .find(|candidate| candidate.local_path.as_deref() == item.local_path.as_deref());
    if let Some(mut existing) = existing {
        // Reconciliation must also refresh provider metadata. The in-memory
        // Store has the hydrated RomM record, while an older scanner entry
        // may contain only title/path data. Keep the stable local identity,
        // but persist the newly hydrated metadata before saving.
        existing.title = item.title;
        existing.kind = item.kind;
        existing.platform_slug = item.platform_slug;
        existing.summary = item.summary;
        existing.alternative_names = item.alternative_names;
        existing.tags = item.tags;
        existing.languages = item.languages;
        existing.regions = item.regions;
        existing.cover = item.cover;
        existing.created_at = item.created_at;
        existing.released_at = item.released_at;
        existing.updated_at = item.updated_at;
        existing.assets = item.assets;
        existing.provider_ids.extend(item.provider_ids);
        // Merge file records by path: freshly installed files replace stale
        // rows (e.g. untagged scanner entries) so provider identities are
        // recorded; previously installed files for other paths are kept.
        let mut files: Vec<marina_core::LibraryItemFile> = existing
            .files
            .into_iter()
            .filter(|current| !item.files.iter().any(|file| file.path == current.path))
            .collect();
        files.extend(item.files);
        existing.files = files;
        existing
            .provider_ids
            .insert("romm_id".into(), rom_id.to_string());
        let saved = library.update(existing).await?;
        debug!(title = %saved.title, local_path = ?saved.local_path, "installed game reconciled into library");
        Ok(saved)
    } else {
        let saved = library.add(item).await?;
        debug!(title = %saved.title, local_path = ?saved.local_path, "installed game added to library");
        Ok(saved)
    }
}

pub(crate) fn map_error(error: InstallError) -> StoreError {
    StoreError::backend(error)
}

fn safe_component(value: &str) -> String {
    let value: String = value
        .chars()
        .map(|character| match character {
            '<' | '>' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            _ => character,
        })
        .collect();
    let value = value.trim().trim_matches('.');
    if value.is_empty() {
        "unknown".to_owned()
    } else {
        value.to_owned()
    }
}

fn asset_extension(source: &str) -> &str {
    let path = source.split(['?', '#']).next().unwrap_or(source);
    std::path::Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .filter(|extension| !extension.is_empty())
        .unwrap_or("bin")
}

#[cfg(test)]
mod tests {
    use super::{asset_extension, safe_component};

    #[test]
    fn sanitizes_only_path_breaking_characters() {
        assert_eq!(safe_component("A:B?C"), "A:B_C");
    }

    #[test]
    fn extracts_asset_extension_before_query_and_fragment() {
        assert_eq!(asset_extension("cover/big.png?ts=2026-08-15"), "png");
        assert_eq!(asset_extension("cover/big.webp#gallery"), "webp");
        assert_eq!(asset_extension("cover/no-extension?ts=1"), "bin");
    }
}
