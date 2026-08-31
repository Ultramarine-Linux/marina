use slint::{Image, ModelRc, SharedString, VecModel};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

slint::include_modules!();

mod app;
mod cache;
mod config;
mod covers;
mod shelf;
mod storage;

#[tokio::main]
async fn main() -> Result<(), slint::PlatformError> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    dotenvy::dotenv().ok();

    let window = MainWindow::new()?;
    window.set_games(ModelRc::from(std::rc::Rc::new(VecModel::from(Vec::new()))));
    window.set_shelf_height(shelf::shelf_height());
    window.set_loading(true);

    // Keep the loader alive for the lifetime of the window. Metadata and
    // artwork are populated after the shell is already visible.
    let (loader, source_store) = covers::spawn_loader(&window, reqwest::Client::new(), Vec::new());
    let loader_for_scroll = loader.clone();
    window.on_viewport_changed(move |scroll_x, viewport_width| {
        loader_for_scroll
            .borrow_mut()
            .update(scroll_x, viewport_width);
    });

    let weak_window = window.as_weak();
    tokio::spawn(async move {
        let state = match app::AppState::initialize().await {
            Ok(state) => state,
            Err(error) => {
                error!(%error, "application initialization failed");
                return;
            }
        };

        info!("loading game metadata");
        let loaded = shelf::load_games(&state.library, state.config.romm_url.as_deref()).await;
        let (metadata, cover_sources) = match loaded {
            Ok(loaded) => loaded,
            Err(error) => {
                error!(%error, "game metadata loading failed");
                return;
            }
        };
        info!(count = metadata.len(), "library loaded");
        *source_store.lock().expect("cover source state poisoned") = cover_sources;

        let _ = weak_window.upgrade_in_event_loop(move |window| {
            let games: Vec<GameCardData> = metadata
                .into_iter()
                .map(|item| GameCardData {
                    title: SharedString::from(item.title),
                    platform: SharedString::from(item.platform),
                    cover: Image::default(),
                    cover_ratio: 1.0,
                })
                .collect();
            window.set_games(ModelRc::from(std::rc::Rc::new(VecModel::from(games))));
            window.set_loading(false);
            // The Shelf's init callback requests the initial visible range.
        });
    });

    window.run()
}
