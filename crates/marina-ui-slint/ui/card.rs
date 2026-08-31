use marina_core::LibraryItem;
use slint::SharedString;

use crate::GameCardData;

pub fn from_library_item(item: LibraryItem) -> GameCardData {
    GameCardData {
        title: SharedString::from(item.title),
        platform: SharedString::from(item.platform_slug.unwrap_or_else(|| "Unknown".into())),
        // The local image loader will replace this fallback with the cover ratio.
        cover_ratio: 1.0,
    }
}
