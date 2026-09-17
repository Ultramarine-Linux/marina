//! Universal asynchronous image loading and decoding.

use std::{path::PathBuf, sync::OnceLock};

use slint::{Image, SharedPixelBuffer};
use tracing::{debug, warn};

use crate::cache;

const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 4096;
const MAX_IMAGE_PIXELS: u64 = 16_777_216;

#[derive(Clone, Debug)]
pub struct ImageSource {
    pub url: Option<String>,
    pub cache_path: Option<PathBuf>,
    pub local_path: Option<PathBuf>,
}

impl From<&crate::covers::CoverSource> for ImageSource {
    fn from(source: &crate::covers::CoverSource) -> Self {
        Self {
            url: source.url.clone(),
            cache_path: source.cache_path.clone(),
            local_path: source.local_path.clone(),
        }
    }
}

pub struct DecodedImage {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub fn source_for(
    source: Option<&str>,
    local_path: Option<&str>,
    base_url: Option<&str>,
) -> ImageSource {
    let source = source.and_then(normalize_source);
    ImageSource {
        url: resolve_url(source.as_deref(), base_url),
        cache_path: source.as_deref().and_then(cache::cover_cache_path),
        local_path: local_path.map(PathBuf::from),
    }
}

pub async fn load(source: &ImageSource, purpose: &'static str) -> Option<DecodedImage> {
    debug!(
        purpose,
        local_path = ?source.local_path,
        cache_path = ?source.cache_path,
        url = ?source.url,
        "image load requested"
    );
    let bytes = load_bytes(source).await?;
    let byte_count = bytes.len();
    let decoded = tokio::task::spawn_blocking(move || decode_rgba(&bytes))
        .await
        .ok()
        .flatten();
    match &decoded {
        Some(image) => debug!(
            purpose,
            byte_count,
            width = image.width,
            height = image.height,
            "image decoded"
        ),
        None => warn!(purpose, byte_count, "image decode failed"),
    }
    decoded
}

pub async fn load_path(path: impl Into<PathBuf>, purpose: &'static str) -> Option<DecodedImage> {
    load(
        &ImageSource {
            url: None,
            cache_path: None,
            local_path: Some(path.into()),
        },
        purpose,
    )
    .await
}

pub fn into_slint_image(decoded: DecodedImage) -> (Image, f32) {
    let buffer = SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        &decoded.pixels,
        decoded.width,
        decoded.height,
    );
    (
        Image::from_rgba8(buffer),
        decoded.width as f32 / decoded.height as f32,
    )
}

async fn load_bytes(source: &ImageSource) -> Option<Vec<u8>> {
    if let Some(path) = &source.local_path {
        match tokio::fs::read(path).await {
            Ok(bytes) if bytes.len() <= MAX_IMAGE_BYTES => {
                debug!(path = %path.display(), bytes = bytes.len(), "local image hit");
                return Some(bytes);
            }
            Ok(bytes) => {
                warn!(path = %path.display(), bytes = bytes.len(), "local image exceeds byte limit")
            }
            Err(error) => debug!(path = %path.display(), %error, "local image miss"),
        }
    }
    if let Some(path) = &source.cache_path {
        match tokio::fs::read(path).await {
            Ok(bytes) if bytes.len() <= MAX_IMAGE_BYTES => {
                debug!(path = %path.display(), bytes = bytes.len(), "image cache hit");
                return Some(bytes);
            }
            Ok(bytes) => {
                warn!(path = %path.display(), bytes = bytes.len(), "cached image exceeds byte limit")
            }
            Err(error) => debug!(path = %path.display(), %error, "image cache miss"),
        }
    }
    let url = source.url.as_deref()?;
    static HTTP: OnceLock<reqwest::Client> = OnceLock::new();
    let bytes = HTTP
        .get_or_init(reqwest::Client::new)
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .await
        .ok()?;
    if bytes.len() > MAX_IMAGE_BYTES {
        warn!(url, bytes = bytes.len(), "remote image exceeds byte limit");
        return None;
    }
    if let Some(path) = &source.cache_path {
        if let Some(parent) = path.parent() {
            if tokio::fs::create_dir_all(parent).await.is_ok() {
                let _ = tokio::fs::write(path, &bytes).await;
            }
        }
    }
    Some(bytes.to_vec())
}

fn decode_rgba(bytes: &[u8]) -> Option<DecodedImage> {
    let svg_prefix = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]);
    if svg_prefix.contains("<svg") {
        let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default()).ok()?;
        let size = tree.size().to_int_size();
        let scale = (MAX_IMAGE_DIMENSION as f32 / size.width().max(size.height()) as f32).min(1.0);
        let width = ((size.width() as f32 * scale).round() as u32).max(1);
        let height = ((size.height() as f32 * scale).round() as u32).max(1);
        let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        return Some(DecodedImage {
            pixels: pixmap.data().to_vec(),
            width,
            height,
        });
    }

    let img = ::image::load_from_memory(bytes).ok()?;
    let (width, height) = (img.width(), img.height());
    if width == 0
        || height == 0
        || width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS
    {
        return None;
    }
    Some(DecodedImage {
        pixels: img.to_rgba8().into_raw(),
        width,
        height,
    })
}

fn resolve_url(source: Option<&str>, base_url: Option<&str>) -> Option<String> {
    let source = source?;
    if source.starts_with("http://") || source.starts_with("https://") {
        return Some(source.to_owned());
    }
    let source = source.trim_matches('/');
    let base_url = base_url?.trim_end_matches('/');
    (!source.is_empty()).then(|| format!("{base_url}/{source}"))
}

fn normalize_source(source: &str) -> Option<String> {
    let source = source.trim();
    let source = source
        .strip_prefix("@url:`")
        .and_then(|value| value.strip_suffix('`'))
        .unwrap_or(source)
        .trim();
    (!source.is_empty()).then(|| source.to_owned())
}
