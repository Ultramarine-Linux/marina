use std::sync::{Arc, Mutex};

use marina_core::LibraryItemId;
use marina_library::read::{LibraryRead, PlatformRead};
use serde::Serialize;
use slint::{Image, Model, ModelRc, SharedString, VecModel};
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

    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--export-ui-fixture") {
        let path = args.get(2).map(String::as_str).unwrap_or("ui-fixture.json");
        if let Err(error) = export_ui_fixture(path).await {
            eprintln!("failed to export UI fixture: {error}");
        }
        return Ok(());
    }

    let (username, display_name, initials) = profile_identity();
    let window = MainWindow::new()?;
    window.set_profile_name(SharedString::from(display_name));
    window.set_profile_username(SharedString::from(username.clone()));
    window.set_profile_initials(SharedString::from(initials));
    window.set_profile_image(Image::default());

    let profile_window = window.as_weak();
    tokio::spawn(async move {
        let icon_path = format!("/var/lib/AccountsService/icons/{username}");
        let Some(bytes) = tokio::fs::read(icon_path).await.ok() else {
            return;
        };
        let _ = profile_window.upgrade_in_event_loop(move |window| {
            if let Some((image, _)) = covers::decode(&bytes) {
                window.set_profile_image(image);
            }
        });
    });
    window.set_games(ModelRc::from(std::rc::Rc::new(VecModel::from(Vec::new()))));
    window.set_platform_games(ModelRc::from(std::rc::Rc::new(VecModel::from(Vec::new()))));
    window.set_selected_game(empty_game_card());
    window.set_game_details(empty_preview_details());
    window.set_shelf_height(shelf::shelf_height());
    window.set_loading(true);
    window.set_library_loading(false);

    let library_state: Arc<Mutex<Option<app::AppStateHandle>>> = Arc::new(Mutex::new(None));

    // Keep the loader alive for the lifetime of the window. Metadata and
    // artwork are populated after the shell is already visible.
    let (loader, source_store) = covers::spawn_loader(&window, reqwest::Client::new(), Vec::new());
    let home_source_store = Arc::new(Mutex::new(Vec::new()));
    let loader_for_scroll = loader.clone();
    window.on_viewport_changed(move |scroll_x, viewport_width| {
        loader_for_scroll
            .borrow_mut()
            .update(scroll_x, viewport_width);
    });

    let loader_for_context = loader.clone();
    let context_sources = source_store.clone();
    let context_home_sources = home_source_store.clone();
    window.on_cover_context_changed(move |active_tab| {
        if active_tab == 1 {
            return;
        }
        let home_sources = context_home_sources
            .lock()
            .expect("home cover source state poisoned")
            .clone();
        *context_sources.lock().expect("cover source state poisoned") = home_sources;
        let mut loader = loader_for_context.borrow_mut();
        loader.reset();
        loader.update(0.0, 1_280.0);
    });

    let weak_window = window.as_weak();
    let detail_state = library_state.clone();
    let detail_window = window.as_weak();
    let selected_loader = loader.clone();
    window.on_game_selected(move |id, index| {
        // Load the selected row's cover on demand. The detail request below
        // supplies the rest of the metadata.
        selected_loader
            .borrow_mut()
            .update(index as f32 * 216.0, 240.0);
        let state = detail_state
            .lock()
            .expect("library state lock poisoned")
            .clone();
        let Some(state) = state else {
            return;
        };
        let Ok(item_id) = LibraryItemId::parse(id.as_str()).ok_or(()) else {
            return;
        };
        let window = detail_window.clone();
        tokio::spawn(async move {
            match state.library.get(&item_id).await {
                Ok(Some(item)) => {
                    let details = preview_details(item);
                    let _ = window.upgrade_in_event_loop(move |window| {
                        window.set_game_details(details);
                        window.set_game_details_loading(false);
                    });
                }
                Ok(None) | Err(_) => {
                    let _ = window.upgrade_in_event_loop(|window| {
                        window.set_game_details_loading(false);
                    });
                }
            }
        });
    });

    let open_state = library_state.clone();
    let open_window = window.as_weak();
    window.on_game_opened(move |id| {
        let Some(window) = open_window.upgrade() else {
            return;
        };
        let games = window.get_games();
        let Some(game) = (0..games.row_count())
            .filter_map(|index| games.row_data(index))
            .find(|game| game.id == id)
        else {
            return;
        };
        window.set_selected_game(game);
        window.set_game_page_visible(true);
        window.set_game_details_loading(true);

        let state = open_state
            .lock()
            .expect("library state lock poisoned")
            .clone();
        let Some(state) = state else {
            return;
        };
        let Ok(item_id) = LibraryItemId::parse(id.as_str()).ok_or(()) else {
            return;
        };
        let detail_window = window.as_weak();
        tokio::spawn(async move {
            match state.library.get(&item_id).await {
                Ok(Some(item)) => {
                    let details = preview_details(item);
                    let _ = detail_window.upgrade_in_event_loop(move |window| {
                        window.set_game_details(details);
                        window.set_game_details_loading(false);
                    });
                }
                Ok(None) | Err(_) => {
                    let _ = detail_window.upgrade_in_event_loop(|window| {
                        window.set_game_details_loading(false);
                    });
                }
            }
        });
    });

    let query_state = library_state.clone();
    let query_sources = source_store.clone();
    let query_window = window.as_weak();
    window.on_platform_query(move |platform_slug| {
        let state = query_state
            .lock()
            .expect("library state lock poisoned")
            .clone();
        let Some(state) = state else {
            return;
        };
        let base_url = state.config.romm_url.clone();
        let sources = query_sources.clone();
        let window = query_window.clone();
        tokio::spawn(async move {
            let loaded = shelf::load_platform_games(
                &state.library,
                base_url.as_deref(),
                platform_slug.as_str(),
            )
            .await;
            let (metadata, cover_sources) = match loaded {
                Ok(loaded) => loaded,
                Err(error) => {
                    error!(%error, platform = %platform_slug, "platform games loading failed");
                    let _ = window.upgrade_in_event_loop(|window| {
                        window.set_library_loading(false);
                    });
                    return;
                }
            };
            *sources.lock().expect("cover source state poisoned") = cover_sources;
            let _ = window.upgrade_in_event_loop(move |window| {
                let model = window.get_platform_games();
                let model = model
                    .as_any()
                    .downcast_ref::<VecModel<GameCardData>>()
                    .expect("platform game model should be a VecModel");
                let games = game_cards(metadata);
                model.set_vec(games);
                window.set_library_loading(false);
            });
        });
    });

    let state_store = library_state.clone();
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
        *source_store.lock().expect("cover source state poisoned") = cover_sources.clone();
        *home_source_store
            .lock()
            .expect("home cover source state poisoned") = cover_sources;
        *state_store.lock().expect("library state lock poisoned") = Some(state.clone());

        let platform_metadata = match state.library.platforms().await {
            Ok(platforms) => platforms,
            Err(error) => {
                error!(%error, "platform metadata loading failed");
                Vec::new()
            }
        };
        let icon_root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/assets/platforms/systematic");
        let mut platform_cards = Vec::with_capacity(platform_metadata.len());
        for platform in platform_metadata {
            let game_count = metadata
                .iter()
                .filter(|game| game.platform.eq_ignore_ascii_case(&platform.name))
                .count();
            let icon_path = platform_asset_path(&icon_root, &platform.slug);
            platform_cards.push(PlatformCardMetadata {
                slug: platform.slug,
                name: platform.name,
                game_count: format!(
                    "{} {}",
                    game_count,
                    if game_count == 1 { "game" } else { "games" }
                ),
                icon_path,
            });
        }

        let _ = weak_window.upgrade_in_event_loop(move |window| {
            let platforms: Vec<PlatformCardData> = platform_cards
                .into_iter()
                .map(|platform| PlatformCardData {
                    slug: SharedString::from(platform.slug),
                    name: SharedString::from(platform.name),
                    icon: platform
                        .icon_path
                        .and_then(|path| Image::load_from_path(std::path::Path::new(&path)).ok())
                        .unwrap_or_default(),
                    game_count: SharedString::from(platform.game_count),
                })
                .collect();
            window.set_platforms(ModelRc::from(std::rc::Rc::new(VecModel::from(platforms))));
            let games = game_cards(metadata);
            window.set_games(ModelRc::from(std::rc::Rc::new(VecModel::from(games))));
            window.set_loading(false);
            // The Shelf's init callback requests the initial visible range.
        });
    });

    window.run()
}

struct PlatformCardMetadata {
    slug: String,
    name: String,
    game_count: String,
    icon_path: Option<String>,
}

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

async fn export_ui_fixture(path: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

fn profile_identity() -> (String, String, String) {
    let username = std::env::var("USER").unwrap_or_else(|_| "user".into());
    let gecos = std::fs::read_to_string("/etc/passwd")
        .ok()
        .and_then(|passwd| {
            passwd.lines().find_map(|line| {
                let fields: Vec<_> = line.split(':').collect();
                if fields.first().copied() != Some(username.as_str()) {
                    return None;
                }
                fields
                    .get(4)
                    .and_then(|gecos| gecos.split(',').next())
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
            })
        });
    let display_name = gecos.unwrap_or_else(|| username.clone());
    let initials = profile_initials(&display_name);
    (username, display_name, initials)
}

fn profile_initials(name: &str) -> String {
    let initials: String = name
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect();
    if initials.is_empty() {
        name.chars()
            .next()
            .unwrap_or('U')
            .to_uppercase()
            .to_string()
    } else {
        initials.to_uppercase()
    }
}

fn empty_game_card() -> GameCardData {
    GameCardData {
        id: SharedString::default(),
        title: SharedString::default(),
        platform: SharedString::default(),
        cover: Image::default(),
        cover_ratio: 1.0,
    }
}

fn game_cards(metadata: Vec<shelf::GameMetadata>) -> Vec<GameCardData> {
    metadata
        .into_iter()
        .map(|item| GameCardData {
            id: SharedString::from(item.id),
            title: SharedString::from(item.title),
            platform: SharedString::from(item.platform),
            cover: Image::default(),
            cover_ratio: 1.0,
        })
        .collect()
}

fn empty_preview_details() -> PreviewDetailsData {
    PreviewDetailsData {
        title: SharedString::default(),
        summary: SharedString::default(),
        released_at: SharedString::default(),
        languages: SharedString::default(),
        regions: SharedString::default(),
        tags: SharedString::default(),
    }
}

fn preview_details(item: marina_core::LibraryItem) -> PreviewDetailsData {
    PreviewDetailsData {
        title: SharedString::from(item.title),
        summary: SharedString::from(item.summary.unwrap_or_default()),
        released_at: SharedString::from(
            item.released_at
                .map(|date| date.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
        ),
        languages: SharedString::from(item.languages.join(", ")),
        regions: SharedString::from(item.regions.join(", ")),
        tags: SharedString::from(item.tags.join(", ")),
    }
}

fn platform_asset_path(root: &std::path::Path, slug: &str) -> Option<String> {
    let exact_name = match slug {
        "ndsi" => "nintendo-dsi",
        "win" => "pc-50x-family",
        _ => slug,
    };
    let exact_path = root.join(format!("{exact_name}.svg"));
    if exact_path.is_file() {
        return Some(exact_path.to_string_lossy().into_owned());
    }

    if let Some(prefix) = slug.split('-').next() {
        let prefix_path = root.join(format!("{prefix}.svg"));
        if prefix_path.is_file() {
            return Some(prefix_path.to_string_lossy().into_owned());
        }
    }

    let default_path = root.join("default.svg");
    default_path
        .is_file()
        .then(|| default_path.to_string_lossy().into_owned())
}
