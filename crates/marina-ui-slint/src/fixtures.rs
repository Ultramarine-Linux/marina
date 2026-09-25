//! JSON export used by Slint viewer and design fixtures.

use marina_library::read::{LibraryRead, PlatformRead};
use serde::Serialize;

use crate::app;

#[derive(Serialize)]
struct UiFixture {
    platforms: Vec<UiFixturePlatform>,
    games: Vec<UiFixtureGame>,
}

#[derive(Serialize)]
struct UiFixturePlatform {
    slug: String,
    name: String,
}

#[derive(Serialize)]
struct UiFixtureGame {
    id: String,
    title: String,
    platform: String,
    cover: Option<String>,
    regions: Vec<String>,
}

pub(crate) async fn export_ui_fixture(
    path: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = app::AppState::initialize().await?;
    let platforms = state
        .library
        .platforms()
        .await?
        .into_iter()
        .map(|platform| UiFixturePlatform {
            slug: platform.slug,
            name: platform.name,
        })
        .collect();
    let games = state
        .library
        .list_cards(u32::MAX)
        .await?
        .into_iter()
        .map(|game| UiFixtureGame {
            id: game.id.to_string(),
            title: game.title,
            platform: game.platform_name.unwrap_or_else(|| "Unknown".into()),
            cover: game.cover,
            regions: game.regions,
        })
        .collect();
    let fixture = UiFixture { platforms, games };
    let json = serde_json::to_string_pretty(&fixture)?;
    std::fs::write(path, json)?;
    println!("wrote UI fixture to {path}");
    Ok(())
}
