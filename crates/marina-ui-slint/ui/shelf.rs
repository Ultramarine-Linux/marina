use futures_util::future;
use marina_library::{LibraryError, LibraryRead};
use marina_store_surrealdb::SurrealLibrary;
use tracing::{debug, instrument, warn};

use crate::GameCardData;

use super::card;

const CARD_BASE_WIDTH: f32 = 180.0;
const CARD_METADATA_HEIGHT: f32 = 58.0;
// Keep in sync with max-cover-height in card.slint.
const MAX_COVER_HEIGHT: f32 = 240.0;
const SHELF_VERTICAL_PADDING: f32 = 20.0;
const SCROLLBAR_PADDING: f32 = 8.0;

#[instrument(skip_all)]
pub async fn load_games(
    library: &SurrealLibrary,
    http: &reqwest::Client,
    romm_base_url: Option<&str>,
) -> Result<Vec<GameCardData>, LibraryError> {
    let items = library.list(100).await?;
    debug!(count = items.len(), "fetched library items");

    // Split metadata from cover URLs so we can fetch covers concurrently.
    let (cards, urls): (Vec<_>, Vec<_>) = items.into_iter().map(card::from_library_item).unzip();

    // Resolve relative cover paths against the RomM base URL.
    let resolved_urls: Vec<Option<String>> = urls
        .into_iter()
        .map(|url| resolve_cover_url(url.as_deref(), romm_base_url))
        .collect();

    // Fetch all cover URLs concurrently. Bytes are Send; Image creation happens
    // below on the main thread to avoid slint::Image's !Send bound.
    let fetches = resolved_urls.iter().map(|url| async move {
        let url = url.as_deref()?;
        debug!(url, "fetching cover");
        match http.get(url).send().await.ok() {
            Some(resp) => resp.bytes().await.ok(),
            None => {
                warn!(url, "cover fetch failed");
                None
            }
        }
    });
    let cover_bytes = future::join_all(fetches).await;

    // Decode covers on the main thread and merge with card metadata.
    let games = cards
        .into_iter()
        .zip(cover_bytes)
        .map(|(mut card, bytes)| {
            if let Some((image, ratio)) = bytes.as_deref().and_then(card::decode_cover) {
                card.cover = image;
                card.cover_ratio = ratio;
            } else {
                warn!(title = %card.title, "no cover loaded, using placeholder");
            }
            card
        })
        .collect();

    Ok(games)
}

fn resolve_cover_url(cover: Option<&str>, base_url: Option<&str>) -> Option<String> {
    let cover = cover?;
    if cover.starts_with("http://") || cover.starts_with("https://") {
        return Some(cover.to_owned());
    }
    let base = base_url?;
    let base = base.trim_end_matches('/');
    let path = cover.trim_start_matches('/');
    Some(format!("{base}/{path}"))
}

/// Height of the tallest card in the shelf, used to size the row viewport.
/// Cards keep their natural heights and are bottom-aligned within the row.
/// Covers are height-capped, mirroring the clamp in card.slint.
fn max_card_height(games: &[GameCardData]) -> f32 {
    games
        .iter()
        .map(|game| {
            let cover_height = (CARD_BASE_WIDTH / game.cover_ratio.max(0.01)).min(MAX_COVER_HEIGHT);
            cover_height + CARD_METADATA_HEIGHT
        })
        .fold(CARD_BASE_WIDTH + CARD_METADATA_HEIGHT, f32::max)
}

pub fn height_for_games(games: &[GameCardData]) -> f32 {
    max_card_height(games) + SHELF_VERTICAL_PADDING + SCROLLBAR_PADDING
}
