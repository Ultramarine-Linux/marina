use marina_library::{LibraryError, LibraryRead};
use marina_store_surrealdb::SurrealLibrary;

use crate::GameCardData;

const CARD_WIDTH: f32 = 180.0;
const CARD_METADATA_HEIGHT: f32 = 58.0;
const SHELF_VERTICAL_PADDING: f32 = 20.0;
const SCROLLBAR_PADDING: f32 = 8.0;

use super::card;

pub async fn load_games(library: &SurrealLibrary) -> Result<Vec<GameCardData>, LibraryError> {
    Ok(library
        .list(100)
        .await?
        .into_iter()
        .map(card::from_library_item)
        .collect())
}

pub fn height_for_games(games: &[GameCardData]) -> f32 {
    let tallest_card = games
        .iter()
        .map(|game| CARD_WIDTH / game.cover_ratio.max(0.01) + CARD_METADATA_HEIGHT)
        .fold(0.0, f32::max);

    tallest_card + SHELF_VERTICAL_PADDING + SCROLLBAR_PADDING
}
