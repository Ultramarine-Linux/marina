use std::{
    io::{Cursor, Read},
    path::{Component, Path, PathBuf},
};

use md5::{Digest, Md5};
use reqwest::Client;
use thiserror::Error;
use tokio::{io::AsyncWriteExt, task};
use zip::ZipArchive;

#[derive(Debug, Error)]
pub enum Error {
    #[error("PortMaster download failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("PortMaster archive error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("PortMaster archive contains an unsafe path: {0}")]
    UnsafePath(String),
    #[error("PortMaster runtime metadata is invalid")]
    InvalidMetadata,
    #[error("runtime bundle does not contain: {0}")]
    RuntimeBundleMissing(String),
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

pub async fn ensure_runtimes(
    client: &Client,
    metadata_url: String,
    runtimes: Vec<String>,
    libs_dir: PathBuf,
) -> Result<(), Error> {
    if runtimes.is_empty() {
        return Ok(());
    }
    let runtimes = runtimes
        .into_iter()
        .map(|runtime| {
            if runtime.ends_with(".squashfs") {
                runtime
            } else {
                format!("{runtime}.squashfs")
            }
        })
        .collect::<Vec<_>>();
    tokio::fs::create_dir_all(&libs_dir).await?;
    let metadata = client
        .get(metadata_url)
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    let bundles = metadata.as_array().ok_or_else(|| Error::InvalidMetadata)?;
    let bundle = bundles
        .iter()
        .filter(|bundle| {
            bundle
                .get("included_files")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|files| {
                    runtimes
                        .iter()
                        .all(|runtime| files.iter().any(|file| file.as_str() == Some(runtime)))
                })
        })
        .min_by_key(|bundle| {
            bundle
                .get("size")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(u64::MAX)
        })
        .ok_or_else(|| Error::RuntimeBundleMissing(runtimes.join(", ")))?;
    let url = bundle
        .get("url")
        .and_then(serde_json::Value::as_str)
        .ok_or(Error::InvalidMetadata)?
        .to_owned();
    let md5 = bundle.get("md5").and_then(serde_json::Value::as_str);
    let archive = libs_dir.join(".runtime-bundle.part");
    download_to_path(client, url, &archive).await?;
    if let Some(expected) = md5 {
        let actual = task::spawn_blocking({
            let archive = archive.clone();
            move || hash_file(&archive)
        })
        .await??;
        if !actual.eq_ignore_ascii_case(expected) {
            let _ = tokio::fs::remove_file(&archive).await;
            return Err(Error::Checksum {
                expected: expected.to_owned(),
                actual,
            });
        }
    }
    let output = libs_dir.clone();
    let archive_for_extract = archive.clone();
    task::spawn_blocking(move || extract_selected(&archive_for_extract, &output, &runtimes))
        .await??;
    let _ = tokio::fs::remove_file(&archive).await;
    Ok(())
}

pub async fn write_runtime_manifest(install_dir: &Path, runtimes: &[String]) -> Result<(), Error> {
    let mut names = runtimes
        .iter()
        .map(|runtime| runtime.strip_suffix(".squashfs").unwrap_or(runtime))
        .map(str::trim)
        .filter(|runtime| !runtime.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if names.iter().any(|runtime| {
        runtime.contains("..")
            || !runtime
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_.-".contains(character))
    }) {
        return Err(Error::InvalidMetadata);
    }
    names.sort_unstable();
    names.dedup();

    let manifest = install_dir.join(".marina-runtimes");
    if names.is_empty() {
        match tokio::fs::remove_file(manifest).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        return Ok(());
    }

    let temporary = install_dir.join(".marina-runtimes.new");
    tokio::fs::write(&temporary, format!("{}\n", names.join("\n"))).await?;
    tokio::fs::rename(temporary, manifest).await?;
    Ok(())
}

pub async fn install_port(
    client: &Client,
    url: String,
    expected_md5: Option<&str>,
    package: String,
    items: Vec<String>,
    install_dir: PathBuf,
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
    let extract_root = install_dir.clone();
    task::spawn_blocking(move || extract_archive(&bytes, &extract_root, true)).await??;
    let permission_root = install_dir.clone();
    let permission_items = items.clone();
    task::spawn_blocking(move || mark_declared_executables(&permission_root, &permission_items))
        .await??;
    find_launcher(&install_dir, &package, &items).await
}

fn extract_selected(archive_path: &Path, root: &Path, selected: &[String]) -> Result<(), Error> {
    std::fs::create_dir_all(root)?;
    let archive_file = std::fs::File::open(archive_path)?;
    let mut archive = ZipArchive::new(archive_file)?;
    for runtime in selected {
        let mut entry = archive.by_name(runtime).map_err(|error| match error {
            zip::result::ZipError::FileNotFound => Error::RuntimeBundleMissing(runtime.clone()),
            other => Error::Zip(other),
        })?;
        let destination = root.join(runtime);
        let mut output = std::fs::File::create(&destination)?;
        std::io::copy(&mut entry, &mut output)?;
    }
    Ok(())
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

        // chmod +x ELF binaries
        let mut prefix = [0_u8; 8192];
        let prefix_len = entry.read(&mut prefix)?;
        let inferred_executable = infer::get(&prefix[..prefix_len])
            .is_some_and(|kind| kind.mime_type() == "application/x-executable");
        let mut output = std::fs::File::create(&destination)?;
        std::io::Write::write_all(&mut output, &prefix[..prefix_len])?;
        std::io::copy(&mut entry, &mut output)?;
        let executable_name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if inferred_executable
            || destination.extension().and_then(|ext| ext.to_str()) == Some("sh")
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

fn hash_file(path: &Path) -> Result<String, Error> {
    let mut file = std::fs::File::open(path)?;
    let mut digest = Md5::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

async fn download_to_path(client: &Client, url: String, path: &Path) -> Result<(), Error> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    let temporary = path.with_extension("part");
    let mut file = tokio::fs::File::create(&temporary).await?;
    while let Some(chunk) = response.chunk().await? {
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    tokio::fs::rename(temporary, path).await?;
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
