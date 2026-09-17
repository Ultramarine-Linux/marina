//! Shelf view-model and layout sizing.

use marina_library::{error::LibraryError, query::SearchQuery, read::LibraryRead};
use marina_store_sqlite::SqliteLibrary;

use crate::covers::{self, CoverSource};

/// Sendable metadata that can cross from the Tokio task to the UI event loop.
/// The Slint image is created only after crossing onto the UI thread.
#[derive(Clone, Debug)]
pub struct GameMetadata {
    pub id: String,
    pub title: String,
    pub platform: String,
}

pub async fn load_games(
    library: &SqliteLibrary,
    romm_base_url: Option<&str>,
) -> Result<(Vec<GameMetadata>, Vec<CoverSource>), LibraryError> {
    let started = std::time::Instant::now();
    tracing::info!("loading home game metadata");
    let result = covers::load_games_metadata(library, romm_base_url).await;
    tracing::info!(
        elapsed_ms = started.elapsed().as_millis() as u64,
        success = result.is_ok(),
        "home game metadata request finished"
    );
    result
}

/// Loads one alphabetized page of games for a platform.
///
/// Pagination is deliberately applied in the backend query so the UI never
/// needs to hold the complete platform library in memory.
pub async fn load_platform_games(
    library: &SqliteLibrary,
    romm_base_url: Option<&str>,
    platform_slug: &str,
) -> Result<(Vec<GameMetadata>, Vec<CoverSource>), LibraryError> {
    let mut items = library
        .search_cards(SearchQuery::new().platform(platform_slug).limit(usize::MAX))
        .await?;
    items.sort_by(|left, right| left.title.to_lowercase().cmp(&right.title.to_lowercase()));

    Ok(items
        .into_iter()
        .map(|item| {
            let source = covers::source_for(
                item.cover.as_deref(),
                item.cover_small_local_path
                    .as_deref()
                    .or(item.cover_large_local_path.as_deref()),
                romm_base_url,
            );
            let metadata = GameMetadata {
                id: item.id.to_string(),
                title: item.title,
                platform: item.platform_name.unwrap_or_else(|| "Unknown".into()),
            };
            (metadata, source)
        })
        .unzip())
}
