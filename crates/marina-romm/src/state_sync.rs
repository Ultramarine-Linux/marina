use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration as ChronoDuration, Utc};

use crate::{Client, Error, State};

use super::save_sync::{MARINA_SAVE_TAG, SaveDownloadReport, SaveSyncFailure, SaveSyncReport};

const LOCAL_BACKUP_DIRECTORY: &str = ".marina-backups";
const REMOTE_STATE_GRACE: ChronoDuration = ChronoDuration::minutes(30);

/// Downloads the newest Marina state for each RetroArch slot when the server
/// copy is newer. State slots are materialized as `<ROM basename>.state`,
/// `.stateN`, or `.state.auto` in `state_directory`.
pub async fn download_state_directory(
    client: &Client,
    rom_id: i32,
    state_directory: impl AsRef<Path>,
    rom_basename: &str,
) -> Result<SaveDownloadReport, Error> {
    let state_directory = state_directory.as_ref();
    let states = client.list_states(rom_id).await?;
    let candidates = states
        .into_iter()
        .filter(|state| {
            !state.missing_from_fs && state.emulator.as_deref() == Some(MARINA_SAVE_TAG)
        })
        .filter_map(|state| state_slot_suffix(&state.file_name).map(|slot| (slot, state)))
        .collect::<Vec<_>>();
    let mut report = SaveDownloadReport {
        available: candidates.len(),
        ..Default::default()
    };

    let mut slots = candidates
        .iter()
        .map(|(slot, _)| slot.clone())
        .collect::<Vec<_>>();
    slots.sort_unstable();
    slots.dedup();

    for slot in slots {
        let Some(state) = candidates
            .iter()
            .filter(|(candidate_slot, _)| candidate_slot == &slot)
            .map(|(_, state)| state)
            .max_by_key(|state| state_written_at(state))
        else {
            continue;
        };

        let destination = state_directory.join(format!("{rom_basename}{slot}"));
        if !server_state_is_newer(state, &destination).await {
            continue;
        }
        if let Err(error) = tokio::fs::create_dir_all(state_directory).await {
            report.failures.push(SaveSyncFailure {
                path: destination,
                error: format!("failed to create state directory: {error}"),
            });
            continue;
        }
        if let Err(error) = backup_existing_state(&destination, Utc::now()).await {
            report.failures.push(SaveSyncFailure {
                path: destination,
                error: format!("failed to back up existing state: {error}"),
            });
            continue;
        }
        let download_url = client.resource_url(&state.download_path);
        match client.download_url(&download_url, &destination, None).await {
            Ok(()) => report.downloaded += 1,
            Err(error) => report.failures.push(SaveSyncFailure {
                path: destination,
                error: error.to_string(),
            }),
        }
    }

    Ok(report)
}

/// Uploads every RetroArch state file below `state_directory` as an immutable
/// RomM state. Screenshot sidecars and unrelated files are ignored.
pub async fn upload_state_directory(
    client: &Client,
    rom_id: i32,
    state_directory: impl AsRef<Path>,
    rom_basename: &str,
) -> Result<SaveSyncReport, Error> {
    let state_directory = state_directory.as_ref();
    match tokio::fs::metadata(state_directory).await {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "state path is not a directory: {}",
                    state_directory.display()
                ),
            )
            .into());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SaveSyncReport::default());
        }
        Err(error) => return Err(error.into()),
    }

    let files = state_files(state_directory).await?;
    let timestamp = Utc::now();
    let mut report = SaveSyncReport {
        discovered: files.len(),
        ..Default::default()
    };

    for path in files {
        let Some(snapshot_name) = state_snapshot_name(rom_basename, &path, timestamp) else {
            continue;
        };
        match client
            .upload_state_snapshot(rom_id, &path, &snapshot_name, MARINA_SAVE_TAG)
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

async fn state_files(root: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut entries = tokio::fs::read_dir(root).await?;
    let mut files = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_file()
            && entry
                .file_name()
                .to_str()
                .and_then(state_slot_suffix)
                .is_some()
        {
            files.push(entry.path());
        }
    }
    files.sort_unstable();
    Ok(files)
}

fn state_slot_suffix(file_name: &str) -> Option<String> {
    let (_, slot) = file_name.rsplit_once(".state")?;
    if slot.is_empty()
        || slot == ".auto"
        || slot.chars().all(|character| character.is_ascii_digit())
    {
        Some(format!(".state{slot}"))
    } else {
        None
    }
}

fn state_snapshot_name(
    rom_basename: &str,
    path: &Path,
    timestamp: DateTime<Utc>,
) -> Option<String> {
    let suffix = state_slot_suffix(path.file_name()?.to_str()?)?;
    let tag = timestamp.format("%Y-%m-%d_%H-%M-%S");
    Some(format!("{rom_basename} [{tag}]{suffix}"))
}

async fn server_state_is_newer(state: &State, destination: &Path) -> bool {
    let Ok(metadata) = tokio::fs::metadata(destination).await else {
        return true;
    };
    let Ok(modified) = metadata.modified() else {
        return true;
    };
    let Some(server_timestamp) = state_written_at(state) else {
        return false;
    };
    server_timestamp.signed_duration_since(DateTime::<Utc>::from(modified)) > REMOTE_STATE_GRACE
}

fn state_written_at(state: &State) -> Option<DateTime<Utc>> {
    state_file_timestamp(&state.file_name).or_else(|| {
        DateTime::parse_from_rfc3339(&state.updated_at)
            .ok()
            .map(|timestamp| timestamp.with_timezone(&Utc))
    })
}

fn state_file_timestamp(file_name: &str) -> Option<DateTime<Utc>> {
    let start = file_name.rfind(" [")? + 2;
    let end = file_name[start..].find(']')? + start;
    chrono::NaiveDateTime::parse_from_str(&file_name[start..end], "%Y-%m-%d_%H-%M-%S")
        .ok()
        .map(|timestamp| timestamp.and_utc())
}

async fn backup_existing_state(
    destination: &Path,
    timestamp: DateTime<Utc>,
) -> Result<(), std::io::Error> {
    match tokio::fs::metadata(destination).await {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "state path is not a regular file: {}",
                    destination.display()
                ),
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
    let file_name = destination
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let tag = timestamp.format("%Y-%m-%d_%H-%M-%S");
    let mut suffix = 0_u32;
    loop {
        let collision = if suffix == 0 {
            String::new()
        } else {
            format!("-{suffix}")
        };
        let backup = backup_directory.join(format!("{file_name} [{tag}{collision}]"));
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

    use super::{state_file_timestamp, state_slot_suffix, state_snapshot_name};

    #[test]
    fn recognizes_retroarch_state_slots() {
        assert_eq!(state_slot_suffix("Example.state"), Some(".state".into()));
        assert_eq!(state_slot_suffix("Example.state0"), Some(".state0".into()));
        assert_eq!(
            state_slot_suffix("Example.state12"),
            Some(".state12".into())
        );
        assert_eq!(
            state_slot_suffix("Example.state.auto"),
            Some(".state.auto".into())
        );
        assert_eq!(state_slot_suffix("Example.state.png"), None);
        assert_eq!(state_slot_suffix("Example.srm"), None);
    }

    #[test]
    fn timestamps_state_uploads_without_losing_the_slot() {
        let timestamp = Utc.with_ymd_and_hms(2026, 9, 29, 6, 28, 27).unwrap();
        assert_eq!(
            state_snapshot_name(
                "Example Game (Region)",
                Path::new("Example Game (Region).state"),
                timestamp
            ),
            Some("Example Game (Region) [2026-09-29_06-28-27].state".into())
        );
        assert_eq!(
            state_snapshot_name(
                "Example Game (Region)",
                Path::new("Example Game (Region).state.auto"),
                timestamp
            ),
            Some("Example Game (Region) [2026-09-29_06-28-27].state.auto".into())
        );
        assert_eq!(
            state_file_timestamp("Example Game (Region) [2026-09-29_06-28-27].state"),
            Some(timestamp)
        );
    }
}
