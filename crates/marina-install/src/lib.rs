use std::path::PathBuf;

use marina_core::LibraryItem;
use marina_library::{query::SearchQuery, read::LibraryRead, write::LibraryWrite};
use marina_romm::{Client, Error as RommError, Rom, RomFile};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InstallError {
    #[error(transparent)]
    Romm(#[from] RommError),
    #[error(transparent)]
    Library(#[from] marina_library::error::LibraryError),
    #[error("invalid RomM file size {0}")]
    InvalidSize(i64),
    #[error("downloaded file size mismatch: expected {expected}, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug)]
pub struct InstallRequest {
    pub rom: Rom,
    /// Exact RomM artifacts selected for this installation.
    pub files: Vec<RomFile>,
    pub library_root: PathBuf,
}

pub async fn install<L>(
    client: &Client,
    library: &L,
    request: InstallRequest,
) -> Result<LibraryItem, InstallError>
where
    L: LibraryRead + LibraryWrite,
{
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
            client
                .download_file(file.rom_id, &file.file_name, Some(file.id), &temporary)
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

    let rom_id = request.rom.id;
    let mut item: LibraryItem = request.rom.into();
    item.local_path = Some(game_dir.to_string_lossy().into_owned());
    item.files = installed_files;
    let existing = library
        .search(SearchQuery::new().platform(platform).limit(usize::MAX))
        .await?
        .into_iter()
        .find(|candidate| candidate.local_path.as_deref() == item.local_path.as_deref());
    if let Some(mut existing) = existing {
        let mut files = existing.files;
        for file in item.files {
            let duplicate = files.iter().any(|current| {
                current.provider_id == file.provider_id || current.path == file.path
            });
            if !duplicate {
                files.push(file);
            }
        }
        existing.files = files;
        existing
            .provider_ids
            .insert("romm_id".into(), rom_id.to_string());
        Ok(library.update(existing).await?)
    } else {
        Ok(library.add(item).await?)
    }
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

#[cfg(test)]
mod tests {
    use super::safe_component;

    #[test]
    fn sanitizes_only_path_breaking_characters() {
        assert_eq!(safe_component("A:B?C"), "A:B_C");
    }
}
