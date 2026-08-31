use marina_library::{LibraryError, LibraryRead};
use marina_store_surrealdb::SurrealLibrary;
use slint::{ComponentHandle, Model};
use tracing::{debug, instrument, warn};

use crate::{GameCardData, MainWindow};

use super::card;

const CARD_METADATA_HEIGHT: f32 = 58.0;
// Uniform cover height. Keep in sync with cover-height-scale in card.slint.
const COVER_HEIGHT: f32 = 200.0;
const SHELF_VERTICAL_PADDING: f32 = 20.0;
const SCROLLBAR_PADDING: f32 = 8.0;

/// Loads game metadata with square placeholder covers. Returns the cards and
/// each card's resolved cover URL for asynchronous fetching.
#[instrument(skip_all)]
pub async fn load_games(
    library: &SurrealLibrary,
    romm_base_url: Option<&str>,
) -> Result<(Vec<GameCardData>, Vec<Option<String>>), LibraryError> {
    let items = library.list(100).await?;
    debug!(count = items.len(), "fetched library items");

    let (cards, urls): (Vec<_>, Vec<_>) = items.into_iter().map(card::from_library_item).unzip();
    let urls = urls
        .into_iter()
        .map(|url| resolve_cover_url(url.as_deref(), romm_base_url))
        .collect();

    Ok((cards, urls))
}

/// Spawns a background fetch per cover and patches the corresponding model
/// row as each one arrives, so the UI shows immediately with skeletons and
/// covers stream in progressively.
///
/// Bytes are fetched on tokio workers; the `slint::Image` is created on the
/// UI thread (it is `!Send`) via `upgrade_in_event_loop`.
pub fn spawn_cover_loader(window: &MainWindow, http: reqwest::Client, urls: Vec<Option<String>>) {
    let weak = window.as_weak();

    for (index, url) in urls.into_iter().enumerate() {
        let Some(url) = url else { continue };
        let http = http.clone();
        let weak = weak.clone();

        tokio::spawn(async move {
            debug!(url, "fetching cover");
            let bytes = match http.get(&url).send().await {
                Ok(response) => match response.bytes().await {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        warn!(url, %error, "cover read failed");
                        return;
                    }
                },
                Err(error) => {
                    warn!(url, %error, "cover fetch failed");
                    return;
                }
            };

            let result = weak.upgrade_in_event_loop(move |window| {
                let Some((image, ratio)) = card::decode_cover(&bytes) else {
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

/// Fixed shelf viewport height. Covers all render at the same height, so
/// the row height is constant and never shifts as covers stream in.
pub fn shelf_height() -> f32 {
    COVER_HEIGHT + CARD_METADATA_HEIGHT + SHELF_VERTICAL_PADDING + SCROLLBAR_PADDING
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
