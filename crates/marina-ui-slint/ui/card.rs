use marina_core::LibraryItem;
use slint::{Image, SharedPixelBuffer, SharedString};

use crate::GameCardData;

/// Converts a library item into initial card data with a square placeholder
/// cover. The real cover is fetched and decoded separately by `shelf::load_games`.
pub fn from_library_item(item: LibraryItem) -> (GameCardData, Option<String>) {
    let cover_url = item.cover.clone();
    let data = GameCardData {
        title: SharedString::from(item.title),
        platform: SharedString::from(item.platform_slug.unwrap_or_else(|| "Unknown".into())),
        cover: Image::default(),
        cover_ratio: 1.0,
    };
    (data, cover_url)
}

/// Decodes raw image bytes into a Slint `Image` and returns the natural aspect
/// ratio (width / height). Returns `None` if the bytes are not a supported format.
pub fn decode_cover(bytes: &[u8]) -> Option<(Image, f32)> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let buffer = SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(img.as_raw(), w, h);
    Some((Image::from_rgba8(buffer), w as f32 / h as f32))
}
