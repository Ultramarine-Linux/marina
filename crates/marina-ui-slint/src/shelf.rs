//! Shelf view-model and layout sizing.

use marina_library::LibraryError;
use marina_store_surrealdb::SurrealLibrary;

use crate::GameCardData;
use crate::covers::{self, CoverSource};

const CARD_METADATA_HEIGHT: f32 = 58.0;
const COVER_HEIGHT: f32 = 200.0;
const SHELF_VERTICAL_PADDING: f32 = 20.0;
const SCROLLBAR_PADDING: f32 = 8.0;

pub async fn load_games(
    library: &SurrealLibrary,
    romm_base_url: Option<&str>,
) -> Result<(Vec<GameCardData>, Vec<CoverSource>), LibraryError> {
    covers::load_games_metadata(library, romm_base_url).await
}

pub fn shelf_height() -> f32 {
    COVER_HEIGHT + CARD_METADATA_HEIGHT + SHELF_VERTICAL_PADDING + SCROLLBAR_PADDING
}
