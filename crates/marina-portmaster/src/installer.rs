use std::{
    io::Cursor,
    path::{Component, Path, PathBuf},
};

use md5::{Digest, Md5};
use reqwest::Client;
use thiserror::Error;
use tokio::task;
use zip::ZipArchive;

#[derive(Debug, Error)]
pub enum Error {
    #[error("PortMaster download failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("PortMaster archive error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("PortMaster archive contains an unsafe path: {0}")]
    UnsafePath(String),
    #[error("PortMaster checksum mismatch: expected {expected}, got {actual}")]
    Checksum { expected: String, actual: String },
    #[error("PortMaster launcher was not found after installation")]
    MissingLauncher,

    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Join(#[from] task::JoinError),
}

pub async fn ensure_payload(client: &Client, url: String, ports_dir: PathBuf) -> Result<(), Error> {
    let control = ports_dir.join("PortMaster/control.txt");
    if tokio::fs::try_exists(&control).await.unwrap_or(false) {
        return Ok(());
    }
    let bytes = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?
        .to_vec();
    task::spawn_blocking(move || extract_archive(&bytes, &ports_dir, false)).await??;
    Ok(())
}

pub async fn install_port(
    client: &Client,
    url: String,
    expected_md5: Option<&str>,
    package: String,
    items: Vec<String>,
    ports_dir: PathBuf,
) -> Result<PathBuf, Error> {
    let bytes = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?
        .to_vec();
    if let Some(expected) = expected_md5.filter(|value| !value.is_empty()) {
        let mut digest = Md5::new();
        digest.update(&bytes);
        let actual = format!("{:x}", digest.finalize());
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(Error::Checksum {
                expected: expected.to_owned(),
                actual,
            });
        }
    }
    let extract_root = ports_dir.clone();
    task::spawn_blocking(move || extract_archive(&bytes, &extract_root, true)).await??;
    let permission_root = ports_dir.clone();
    let permission_items = items.clone();
    task::spawn_blocking(move || mark_declared_executables(&permission_root, &permission_items))
        .await??;
    find_launcher(&ports_dir, &package, &items).await
}

fn extract_archive(bytes: &[u8], root: &Path, port_archive: bool) -> Result<(), Error> {
    std::fs::create_dir_all(root)?;
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_owned();
        let relative = safe_relative_path(&name)?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let destination = if port_archive
            && relative
                .parent()
                .is_some_and(|parent| parent.as_os_str().is_empty())
            && relative.extension().and_then(|ext| ext.to_str()) == Some("sh")
        {
            root.join(&relative)
        } else {
            root.join(&relative)
        };
        if entry.is_dir() {
            std::fs::create_dir_all(&destination)?;
            continue;
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut output = std::fs::File::create(&destination)?;
        std::io::copy(&mut entry, &mut output)?;
        let executable_name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if destination.extension().and_then(|ext| ext.to_str()) == Some("sh")
            || matches!(
                executable_name,
                "gptokeyb" | "gptokeyb2" | "PortMaster.sh" | "pugwash" | "mapper.txt"
            )
        {
            set_executable(&destination)?;
        }
    }
    Ok(())
}

fn mark_declared_executables(root: &Path, items: &[String]) -> Result<(), Error> {
    for item in items {
        let path = root.join(item);
        if !path.starts_with(root) || !path.is_file() {
            continue;
        }
        let executable = path.extension().and_then(|ext| ext.to_str()) == Some("sh")
            || path.extension().is_none();
        if executable {
            set_executable(&path)?;
        }
    }
    Ok(())
}

fn safe_relative_path(name: &str) -> Result<PathBuf, Error> {
    let path = Path::new(name);
    if path.is_absolute() {
        return Err(Error::UnsafePath(name.to_owned()));
    }
    for component in path.components() {
        if matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        ) {
            return Err(Error::UnsafePath(name.to_owned()));
        }
    }
    Ok(path.to_owned())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), Error> {
    Ok(())
}

async fn find_launcher(root: &Path, package: &str, items: &[String]) -> Result<PathBuf, Error> {
    for item in items
        .iter()
        .filter(|item| item.to_ascii_lowercase().ends_with(".sh"))
    {
        let candidate = root.join(item);
        if candidate.starts_with(root) && tokio::fs::try_exists(&candidate).await.unwrap_or(false) {
            return Ok(candidate);
        }
    }
    let stem = package
        .strip_suffix(".zip")
        .unwrap_or(package)
        .to_ascii_lowercase();
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        let mut entries = tokio::fs::read_dir(&directory).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) == Some("sh")
                && path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|name| name.to_ascii_lowercase() == stem)
            {
                return Ok(path);
            }
        }
    }
    Err(Error::MissingLauncher)
}
