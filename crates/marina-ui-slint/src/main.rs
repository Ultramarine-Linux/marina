mod app;
mod bootstrap;
mod cache;
mod config;
mod covers;
mod fixtures;
mod image;
mod startup;
mod storage;
mod ui;

use tracing_subscriber::EnvFilter;

slint::include_modules!();

pub(crate) use ui::data::{
    PlatformCardMetadata, empty_game_card, empty_preview_details, game_cards, platform_asset_path,
    preview_details, string_model,
};
pub(crate) use ui::store_details::populate_store_details;

// Marina is I/O-bound, and image decoding has its own bounded blocking pool.
// Keeping the async pool small avoids one glibc allocation arena per CPU core
// being warmed by short-lived image buffers on high-core-count handhelds.
#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), slint::PlatformError> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    dotenvy::dotenv().ok();

    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--export-ui-fixture") {
        let path = args.get(2).map(String::as_str).unwrap_or("ui-fixture.json");
        if let Err(error) = fixtures::export_ui_fixture(path).await {
            eprintln!("failed to export UI fixture: {error}");
        }
        return Ok(());
    }

    bootstrap::run().await
}
