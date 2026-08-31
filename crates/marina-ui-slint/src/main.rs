use std::{env, error::Error};

use marina_store_surrealdb::SurrealLibrary;
use slint::{ModelRc, VecModel};
use tracing::{info, instrument};
use tracing_subscriber::EnvFilter;

slint::include_modules!();

#[path = "../ui/mod.rs"]
mod ui;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    dotenvy::dotenv().ok();

    let storage_uri = env::var("MARINA_STORAGE_URI").unwrap_or_else(|_| "mem://".to_owned());

    info!(uri = %storage_uri, "connecting to library store");
    let library = connect_library(&storage_uri).await?;

    let romm_url = env::var("ROMM_URL").ok();
    if romm_url.is_none() {
        tracing::warn!("ROMM_URL not set — relative cover paths will not resolve");
    }

    info!("loading game metadata");
    let (games, cover_urls) = ui::shelf::load_games(&library, romm_url.as_deref()).await?;
    info!(count = games.len(), "library loaded");

    let window = MainWindow::new()?;
    window.set_games(ModelRc::from(std::rc::Rc::new(VecModel::from(games))));
    window.set_shelf_height(ui::shelf::shelf_height());

    // Covers stream in asynchronously; cards show skeletons until then.
    ui::shelf::spawn_cover_loader(&window, reqwest::Client::new(), cover_urls);

    window.run()?;

    Ok(())
}

#[instrument(skip_all, fields(uri))]
async fn connect_library(storage_uri: &str) -> Result<SurrealLibrary, Box<dyn Error>> {
    if storage_uri.starts_with("ws://") || storage_uri.starts_with("wss://") {
        let username = env::var("MARINA_STORAGE_USERNAME").unwrap_or_else(|_| "root".to_owned());
        let password = env::var("MARINA_STORAGE_PASSWORD").unwrap_or_else(|_| "root".to_owned());
        Ok(SurrealLibrary::connect_with_root(storage_uri, username, password).await?)
    } else {
        Ok(SurrealLibrary::connect(storage_uri).await?)
    }
}
