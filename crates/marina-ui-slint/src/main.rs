use std::error::Error;

use slint::{ModelRc, VecModel};
use tracing::info;
use tracing_subscriber::EnvFilter;

slint::include_modules!();

mod cache;
mod config;
mod covers;
mod shelf;
mod storage;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    dotenvy::dotenv().ok();

    let config = config::Config::from_env();

    info!(uri = %config.storage_uri, "connecting to library store");
    let library = storage::connect(&config).await?;

    info!("loading game metadata");
    let (games, cover_sources) = shelf::load_games(&library, config.romm_url.as_deref()).await?;
    info!(count = games.len(), "library loaded");

    let window = MainWindow::new()?;
    window.set_games(ModelRc::from(std::rc::Rc::new(VecModel::from(games))));
    window.set_shelf_height(shelf::shelf_height());

    // Covers stream in asynchronously (disk cache first, then network);
    // cards show skeletons until then.
    covers::spawn_loader(&window, reqwest::Client::new(), cover_sources);

    window.run()?;

    Ok(())
}
