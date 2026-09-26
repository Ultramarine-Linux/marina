use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration as ChronoDuration, Utc};

use crate::{Client, Error, Save};

pub const MARINA_SAVE_TAG: &str = "marina";
pub const AUTOSAVE_SLOT: &str = "autosave";
const LOCAL_BACKUP_DIRECTORY: &str = ".marina-backups";
const REMOTE_SAVE_GRACE: ChronoDuration = ChronoDuration::minutes(30);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SaveSyncReport {
    pub discovered: usize,
    pub uploaded: usize,
    pub failures: Vec<SaveSyncFailure>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SaveDownloadReport {
    pub available: usize,
    pub downloaded: usize,
    pub failures: Vec<SaveSyncFailure>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SaveSyncFailure {
    pub path: PathBuf,
    pub error: String,
}

/// Download the newest Marina autosave snapshot when the server copy is newer
/// than the local save. The destination is always `<ROM basename>.srm`,
/// regardless of the uploaded snapshot's source extension. Other emulators and
/// slots are left untouched.
pub async fn download_save_directory(
    client: &Client,
    rom_id: i32,
    save_directory: impl AsRef<Path>,
    rom_basename: &str,
) -> Result<SaveDownloadReport, Error> {
    let save_directory = save_directory.as_ref();
    let saves = client.list_saves(rom_id, AUTOSAVE_SLOT).await?;
    let candidates = saves
        .into_iter()
        .filter(|save| {
            !save.missing_from_fs
                && save.slot.as_deref() == Some(AUTOSAVE_SLOT)
                && save.emulator.as_deref() == Some(MARINA_SAVE_TAG)
        })
        .collect::<Vec<_>>();
    let mut report = SaveDownloadReport {
        available: candidates.len(),
        ..Default::default()
    };
    let Some(save) = candidates
        .into_iter()
        .max_by_key(|save| save_updated_at(save))
    else {
        return Ok(report);
    };

    let destination = download_save_path(save_directory, rom_basename);
    if !server_save_is_newer(&save, &destination).await {
        return Ok(report);
    }
    if let Err(error) = backup_existing_save(&destination, Utc::now()).await {
        report.failures.push(SaveSyncFailure {
            path: destination,
            error: format!("failed to back up existing save: {error}"),
        });
        return Ok(report);
    }
    match client.download_save(save.id, &destination).await {
        Ok(()) => report.downloaded += 1,
        Err(error) => report.failures.push(SaveSyncFailure {
            path: destination,
            error: error.to_string(),
        }),
    }

    Ok(report)
}

/// Upload every regular file below `save_directory` as a new RomM autosave
/// snapshot. Symlinks are ignored and previous snapshots are never replaced or
/// automatically removed.
pub async fn upload_save_directory(
    client: &Client,
    rom_id: i32,
    save_directory: impl AsRef<Path>,
    rom_basename: &str,
) -> Result<SaveSyncReport, Error> {
    let save_directory = save_directory.as_ref();
    match tokio::fs::metadata(save_directory).await {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("save path is not a directory: {}", save_directory.display()),
            )
            .into());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SaveSyncReport::default());
        }
        Err(error) => return Err(error.into()),
    }

    let files = regular_files(save_directory).await?;
    let timestamp = Utc::now();
    let mut report = SaveSyncReport {
        discovered: files.len(),
        ..Default::default()
    };

    for path in files {
        let snapshot_name = snapshot_name(rom_basename, &path, timestamp);
        match client
            .upload_save_snapshot(
                rom_id,
                &path,
                &snapshot_name,
                MARINA_SAVE_TAG,
                AUTOSAVE_SLOT,
            )
            .await
        {
            Ok(_) => report.uploaded += 1,
            Err(error) => report.failures.push(SaveSyncFailure {
                path,
                error: error.to_string(),
            }),
        }
    }

    Ok(report)
}

async fn regular_files(root: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut pending = vec![root.to_owned()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        let mut entries = tokio::fs::read_dir(directory).await?;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                if entry.file_name() != LOCAL_BACKUP_DIRECTORY {
                    pending.push(entry.path());
                }
            } else if file_type.is_file() {
                files.push(entry.path());
            }
        }
    }
    files.sort_unstable();
    Ok(files)
}

async fn server_save_is_newer(save: &Save, destination: &Path) -> bool {
    let Ok(metadata) = tokio::fs::metadata(destination).await else {
        return true;
    };
    let Ok(modified) = metadata.modified() else {
        return true;
    };
    let Some(server_timestamp) = save_updated_at(save) else {
        // Without a trustworthy remote timestamp, never replace an existing
        // local save. A missing local save still downloads above.
        return false;
    };
    server_timestamp_is_significantly_newer(DateTime::<Utc>::from(modified), server_timestamp)
}

fn save_updated_at(save: &Save) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&save.updated_at)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

fn server_timestamp_is_significantly_newer(
    local_timestamp: DateTime<Utc>,
    server_timestamp: DateTime<Utc>,
) -> bool {
    server_timestamp.signed_duration_since(local_timestamp) > REMOTE_SAVE_GRACE
}

fn snapshot_name(rom_basename: &str, path: &Path, timestamp: DateTime<Utc>) -> String {
    let tag = timestamp.format("%Y-%m-%d_%H-%M-%S");
    match path.extension().filter(|extension| !extension.is_empty()) {
        Some(extension) => format!("{rom_basename} [{tag}].{}", extension.to_string_lossy()),
        None => format!("{rom_basename} [{tag}]"),
    }
}

fn download_save_path(save_directory: &Path, rom_basename: &str) -> PathBuf {
    save_directory.join(format!("{rom_basename}.srm"))
}

async fn backup_existing_save(
    destination: &Path,
    timestamp: DateTime<Utc>,
) -> Result<(), std::io::Error> {
    match tokio::fs::metadata(destination).await {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("save path is not a regular file: {}", destination.display()),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }

    let backup_directory = destination
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(LOCAL_BACKUP_DIRECTORY);
    tokio::fs::create_dir_all(&backup_directory).await?;
    let stem = destination
        .file_stem()
        .filter(|stem| !stem.is_empty())
        .unwrap_or_default()
        .to_string_lossy();
    let extension = destination.extension().and_then(|value| value.to_str());
    let tag = timestamp.format("%Y-%m-%d_%H-%M-%S");
    let mut suffix = 0_u32;
    loop {
        let collision = if suffix == 0 {
            String::new()
        } else {
            format!("-{suffix}")
        };
        let file_name = match extension {
            Some(extension) => format!("{stem} [{tag}{collision}].{extension}"),
            None => format!("{stem} [{tag}{collision}]"),
        };
        let backup = backup_directory.join(file_name);
        if !tokio::fs::try_exists(&backup).await? {
            tokio::fs::copy(destination, backup).await?;
            return Ok(());
        }
        suffix += 1;
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use chrono::{TimeZone, Utc};

    use super::{
        backup_existing_save, download_save_path, regular_files,
        server_timestamp_is_significantly_newer, snapshot_name,
    };

    #[test]
    fn names_upload_from_rom_and_preserves_local_save_extension() {
        let timestamp = Utc.with_ymd_and_hms(2026, 9, 12, 21, 6, 42).unwrap();
        assert_eq!(
            snapshot_name(
                "Mother 3 (Japan) [T-en]",
                Path::new("profile/anything.sav"),
                timestamp
            ),
            "Mother 3 (Japan) [T-en] [2026-09-12_21-06-42].sav"
        );
        assert_eq!(
            snapshot_name(
                "Star Fox (USA)",
                Path::new("profile/core-save.srm"),
                timestamp
            ),
            "Star Fox (USA) [2026-09-12_21-06-42].srm"
        );
    }

    #[test]
    fn remote_save_needs_to_be_more_than_thirty_minutes_newer() {
        let local = Utc.with_ymd_and_hms(2026, 9, 12, 21, 0, 0).unwrap();
        assert!(!server_timestamp_is_significantly_newer(
            local,
            Utc.with_ymd_and_hms(2026, 9, 12, 21, 30, 0).unwrap()
        ));
        assert!(!server_timestamp_is_significantly_newer(
            local,
            Utc.with_ymd_and_hms(2026, 9, 12, 21, 29, 59).unwrap()
        ));
        assert!(server_timestamp_is_significantly_newer(
            local,
            Utc.with_ymd_and_hms(2026, 9, 12, 21, 30, 1).unwrap()
        ));
    }

    #[test]
    fn downloads_to_rom_basename_with_srm_extension() {
        assert_eq!(
            download_save_path(
                Path::new("/var/games/saves/Mother 3"),
                "Mother 3 (Japan) [T-en]"
            ),
            Path::new("/var/games/saves/Mother 3/Mother 3 (Japan) [T-en].srm")
        );
    }

    #[tokio::test]
    async fn backs_up_existing_saves_without_rediscovering_backups() {
        let root = std::env::temp_dir().join(format!(
            "marina-romm-save-sync-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let destination = root.join("Mother 3 (Japan) [T-en].srm");
        tokio::fs::write(&destination, b"local-save").await.unwrap();
        let timestamp = Utc.with_ymd_and_hms(2026, 9, 12, 21, 6, 42).unwrap();

        backup_existing_save(&destination, timestamp).await.unwrap();
        backup_existing_save(&destination, timestamp).await.unwrap();

        let backup_directory = root.join(".marina-backups");
        assert_eq!(
            tokio::fs::read(
                backup_directory.join("Mother 3 (Japan) [T-en] [2026-09-12_21-06-42].srm")
            )
            .await
            .unwrap(),
            b"local-save"
        );
        assert!(
            backup_directory
                .join("Mother 3 (Japan) [T-en] [2026-09-12_21-06-42-1].srm")
                .is_file()
        );
        assert_eq!(regular_files(&root).await.unwrap(), vec![destination]);

        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
