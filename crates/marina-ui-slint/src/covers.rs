//! Asynchronous cover pipeline: disk cache -> network fetch -> decode -> model patch.

use std::path::PathBuf;

use slint::{ComponentHandle, Image, Model, SharedPixelBuffer};
use tracing::{debug, warn};

use crate::{MainWindow, cache};

/// Where a card's cover can be found: a deterministic disk cache location
/// and/or a remote URL to fetch (and backfill the cache) from.
pub struct CoverSource {
    pub url: Option<String>,
    pub cache_path: Option<PathBuf>,
}

/// Builds the cover source for a stored cover path/URL.
pub fn source_for(cover: Option<&str>, base_url: Option<&str>) -> CoverSource {
    CoverSource {
        url: resolve_url(cover, base_url),
        cache_path: cover.and_then(cache::cover_cache_path),
    }
}

/// Spawns a background load per cover and patches the corresponding model
/// row as each one arrives, so the UI shows immediately with skeletons and
/// covers stream in progressively.
///
/// Bytes are read/fetched on tokio workers; the `slint::Image` is created on
/// the UI thread (it is `!Send`) via `upgrade_in_event_loop`.
pub fn spawn_loader(window: &MainWindow, http: reqwest::Client, sources: Vec<CoverSource>) {
    let weak = window.as_weak();

    for (index, source) in sources.into_iter().enumerate() {
        if source.url.is_none() && source.cache_path.is_none() {
            continue;
        }
        let http = http.clone();
        let weak = weak.clone();

        tokio::spawn(async move {
            let Some(bytes) = load_bytes(&http, &source).await else {
                return;
            };

            let result = weak.upgrade_in_event_loop(move |window| {
                let Some((image, ratio)) = decode(&bytes) else {
                    warn!("cover decode failed, keeping placeholder");
                    return;
                };
                let games = window.get_games();
                if let Some(mut row) = games.row_data(index) {
                    row.cover = image;
                    row.cover_ratio = ratio;
                    games.set_row_data(index, row);
                }
            });
            if result.is_err() {
                debug!("event loop gone, dropping cover");
            }
        });
    }
}

/// Decodes raw image bytes into a Slint `Image` and returns the natural
/// aspect ratio (width / height). `None` if the format is unsupported.
pub fn decode(bytes: &[u8]) -> Option<(Image, f32)> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let buffer = SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(img.as_raw(), w, h);
    Some((Image::from_rgba8(buffer), w as f32 / h as f32))
}

/// Reads a cover from the disk cache, falling back to a network fetch that
/// backfills the cache (best-effort).
async fn load_bytes(http: &reqwest::Client, source: &CoverSource) -> Option<Vec<u8>> {
    if let Some(path) = &source.cache_path {
        if let Ok(bytes) = tokio::fs::read(path).await {
            debug!(path = %path.display(), "cover cache hit");
            return Some(bytes);
        }
    }

    let url = source.url.as_deref()?;
    debug!(url, "fetching cover");
    let bytes = match http
        .get(url)
        .send()
        .await
        .and_then(|response| response.error_for_status())
    {
        Ok(response) => match response.bytes().await {
            Ok(bytes) => bytes,
            Err(error) => {
                warn!(url, %error, "cover read failed");
                return None;
            }
        },
        Err(error) => {
            warn!(url, %error, "cover fetch failed");
            return None;
        }
    };

    if let Some(path) = &source.cache_path {
        match path.parent() {
            Some(parent) => {
                if let Err(error) = tokio::fs::create_dir_all(parent).await {
                    warn!(path = %path.display(), %error, "cover cache dir creation failed");
                } else if let Err(error) = tokio::fs::write(path, &bytes).await {
                    warn!(path = %path.display(), %error, "cover cache write failed");
                } else {
                    debug!(path = %path.display(), "cover cached");
                }
            }
            None => warn!(path = %path.display(), "cover cache path has no parent"),
        }
    }

    Some(bytes.to_vec())
}

fn resolve_url(cover: Option<&str>, base_url: Option<&str>) -> Option<String> {
    let cover = cover?;
    if cover.starts_with("http://") || cover.starts_with("https://") {
        return Some(cover.to_owned());
    }
    let base = base_url?.trim_end_matches('/');
    let path = cover.trim_start_matches('/');
    Some(format!("{base}/{path}"))
}
