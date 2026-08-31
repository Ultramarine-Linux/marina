use std::error::Error;

use slint::{ModelRc, VecModel};
use tracing::info;
use tracing_subscriber::EnvFilter;

slint::include_modules!();

mod app;
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

    let state = app::AppState::initialize().await?;

    info!("loading game metadata");
    let (games, cover_sources) =
        shelf::load_games(&state.library, state.config.romm_url.as_deref()).await?;
    info!(count = games.len(), "library loaded");

    let window = MainWindow::new()?;
    window.set_games(ModelRc::from(std::rc::Rc::new(VecModel::from(games))));
    window.set_shelf_height(shelf::shelf_height());

    // Covers stream in asynchronously (disk cache first, then network);
    // cards show skeletons until then.
    let loader = covers::spawn_loader(&window, state.http.clone(), cover_sources);
    let loader_for_scroll = loader.clone();
    window.on_viewport_changed(move |scroll_x, viewport_width| {
        loader_for_scroll
            .borrow_mut()
            .update(scroll_x, viewport_width);
    });

    window.run()?;

    Ok(())
}
