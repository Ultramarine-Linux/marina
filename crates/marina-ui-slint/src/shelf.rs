//! Shelf view-model: library items → `GameCardData` rows.

use marina_core::LibraryItem;
use marina_library::{LibraryError, LibraryRead};
use marina_store_surrealdb::SurrealLibrary;
use slint::{Image, SharedString};
use tracing::{debug, instrument};

use crate::GameCardData;
use crate::covers::{self, CoverSource};

const CARD_METADATA_HEIGHT: f32 = 58.0;
// Uniform cover height. Keep in sync with cover-height-scale in card.slint.
const COVER_HEIGHT: f32 = 200.0;
const SHELF_VERTICAL_PADDING: f32 = 20.0;
const SCROLLBAR_PADDING: f32 = 8.0;

/// Loads game metadata with square placeholder covers. Returns the cards and
/// each card's cover source for asynchronous loading.
#[instrument(skip_all)]
pub async fn load_games(
    library: &SurrealLibrary,
    romm_base_url: Option<&str>,
) -> Result<(Vec<GameCardData>, Vec<CoverSource>), LibraryError> {
    let items = library.list(100).await?;
    debug!(count = items.len(), "fetched library items");

    Ok(items
        .into_iter()
        .map(|item| card_for(item, romm_base_url))
        .unzip())
}

/// Fixed shelf viewport height. Covers all render at the same height, so
/// the row height is constant and never shifts as covers stream in.
pub fn shelf_height() -> f32 {
    COVER_HEIGHT + CARD_METADATA_HEIGHT + SHELF_VERTICAL_PADDING + SCROLLBAR_PADDING
}

/// Maps a library item to its card view-model (square placeholder cover)
/// plus the cover source used to hydrate the artwork asynchronously.
fn card_for(item: LibraryItem, base_url: Option<&str>) -> (GameCardData, CoverSource) {
    let source = covers::source_for(item.cover.as_deref(), base_url);
    let data = GameCardData {
        title: SharedString::from(item.title),
        platform: SharedString::from(item.platform_slug.unwrap_or_else(|| "Unknown".into())),
        cover: Image::default(),
        cover_ratio: 1.0,
    };
    (data, source)
}
