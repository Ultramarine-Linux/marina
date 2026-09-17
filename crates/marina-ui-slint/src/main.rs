use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use marina_input::{InputConfig, InputLoop};
use marina_library::{
    query::SearchQuery,
    read::{LibraryRead, PlatformRead},
    write::{LibraryWrite, PlatformWrite},
};
use marina_romm::{PlatformQuery, RomQuery};

use marina_scanner::scan;
use serde::Serialize;
use slint::{Image, Model, ModelRc, SharedString, VecModel};
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

slint::include_modules!();

mod app;
mod cache;
mod config;
mod covers;
mod romm_auth;
mod storage;
mod ui;

use ui::pages::library as shelf;

const CONTROLLER_STARTUP_DELAY: Duration = Duration::from_secs(1);

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

    // Keep the first window construction independent of filesystem-backed profile
    // discovery. The shell starts with a useful fallback and hydrates the profile
    // asynchronously once the event loop is running.
    let username = std::env::var("USER").unwrap_or_else(|_| "user".into());
    let window = MainWindow::new()?;
    slint::set_xdg_app_id("org.ultramarinelinux.MarinaShell")?;
    info!("main window constructed; completing lightweight UI setup");
    ui::controls::configure_toasts(&window);

    // Controller discovery can block inside gilrs while probing input devices.
    // Never make window creation wait for it; initialize it after the event loop
    // has started and keep the returned loop alive in its background task.
    let controller_window = window.as_weak();
    tokio::spawn(async move {
        // gilrs probes every input device during construction. Let the first
        // frame and initial library query get CPU priority before doing that
        // work, especially on handhelds with slow input device enumeration.
        tokio::time::sleep(CONTROLLER_STARTUP_DELAY).await;
        let started = std::time::Instant::now();
        info!("starting controller input discovery");
        let result = tokio::task::spawn_blocking(move || {
            InputLoop::spawn(InputConfig::default(), move |event| {
                let controller_window = controller_window.clone();
                let _ = controller_window.upgrade_in_event_loop(move |window| {
                    ui::controls::dispatch_controller_action(&window, event);
                });
            })
        })
        .await;

        match result {
            Ok(Ok(input)) => {
                info!(
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "controller input ready"
                );
                // InputLoop owns its polling thread and must stay alive for the
                // lifetime of the application.
                std::future::pending::<()>().await;
                drop(input);
            }
            Ok(Err(error)) => {
                warn!(
                    %error,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "controller input unavailable; keyboard and pointer input remain active"
                );
            }
            Err(error) => {
                warn!(
                    %error,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "controller input task failed; keyboard and pointer input remain active"
                );
            }
        }
    });
    ui::profile::initialize(&window, username.clone());
    window.set_games(ModelRc::from(std::rc::Rc::new(VecModel::from(Vec::new()))));
    window.set_platform_games(ModelRc::from(std::rc::Rc::new(VecModel::from(Vec::new()))));
    window.set_selected_game(empty_game_card());
    window.set_game_details(empty_preview_details());

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
        debug!(active_tab, "cover context changed");
        if active_tab == 1 {
            let mut loader = loader_for_context.borrow_mut();
            loader.reset();
            loader.update(0.0, 1_280.0);
            debug!("cover context reset complete; refreshing platform residency");
            return;
        }
        let home_sources = context_home_sources
            .lock()
            .expect("home cover source state poisoned")
            .clone();
        let source_count = home_sources.len();
        *context_sources.lock().expect("cover source state poisoned") = home_sources;
        debug!(source_count, "cover context switched to home sources");
        let mut loader = loader_for_context.borrow_mut();
        loader.reset();
        loader.update(0.0, 1_280.0);
        debug!("cover context reset complete; refreshing home residency");
    });

    ui::launch::install(&window, &library_state);
    ui::pages::home::install(&window, &library_state, &source_store, &home_source_store);
    ui::pages::library::install(&window, &library_state, &source_store, &loader);

    let weak_window = window.as_weak();
    let state_store = library_state.clone();
    slint::Timer::single_shot(Duration::ZERO, move || {
        tokio::spawn(async move {
            let state = match app::AppState::initialize().await {
                Ok(state) => state,
                Err(error) => {
                    error!(%error, "application initialization failed");
                    return;
                }
            };

            if state.config.scan_on_startup {
                if let Some(root) = state.config.library_root.as_ref() {
                    match tokio::fs::try_exists(root).await {
                        Ok(false) => {
                            info!(path = %root.display(), "local library root does not exist yet; skipping scan");
                        }
                        Ok(true) => match scan(root) {
                            Ok(items) => {
                                info!(count = items.len(), "local game scan completed");
                                for item in items {
                                    let platform_slug = item.platform_slug.clone();
                                    if let Some(slug) = platform_slug.as_deref() {
                                        let _ = state
                                            .library
                                            .add_platform(marina_core::Platform::new(slug, slug))
                                            .await;
                                    }
                                    let existing = state
                                        .library
                                        .search(
                                            SearchQuery::new()
                                                .platform(
                                                    platform_slug.as_deref().unwrap_or_default(),
                                                )
                                                .limit(usize::MAX),
                                        )
                                        .await
                                        .ok()
                                        .and_then(|items| {
                                            items.into_iter().find(|candidate| {
                                                candidate.local_path == item.local_path
                                            })
                                        });
                                    let result = if let Some(mut existing) = existing {
                                        // Scanner data describes filesystem presence only. Preserve
                                        // provider metadata/assets from an enriched installed record.
                                        existing.files = item.files;
                                        state.library.update(existing).await
                                    } else {
                                        state.library.add(item).await
                                    };
                                    if let Err(error) = result {
                                        error!(%error, "failed to store scanned local game");
                                    }
                                }
                            }
                            Err(error) => error!(%error, "local game scan failed"),
                        },
                        Err(error) => {
                            error!(%error, path = %root.display(), "could not inspect local library root")
                        }
                    }
                }
            }

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

            if state.config.import_romm_on_startup {
                if let Some(base_url) = state.config.romm_url.clone() {
                    let sync_state = state.clone();
                    tokio::spawn(async move {
                        let client =
                            romm_auth::client(base_url, sync_state.config.romm_token.as_deref());
                        match client.list_platforms(&PlatformQuery::default()).await {
                            Ok(platforms) => {
                                for platform in platforms {
                                    let mut offset = 0_i64;
                                    loop {
                                        let query = RomQuery {
                                            platform_ids: vec![platform.id],
                                            limit: Some(100),
                                            offset: Some(offset),
                                            with_files: Some(true),
                                            ..Default::default()
                                        };
                                        let page = match client.list_roms(&query).await {
                                            Ok(page) => page,
                                            Err(error) => {
                                                error!(%error, platform = %platform.fs_slug, "RomM catalog sync failed");
                                                break;
                                            }
                                        };
                                        let rows = page
                                            .items
                                            .iter()
                                            .filter_map(|rom| {
                                                Some((
                                                    rom.id.to_string(),
                                                    rom.name.clone().unwrap_or_else(|| {
                                                        rom.files.fs_name.clone()
                                                    }),
                                                    rom.platform.platform_fs_slug.clone(),
                                                    serde_json::to_string(rom).ok()?,
                                                ))
                                            })
                                            .collect::<Vec<_>>();
                                        if let Err(error) =
                                            sync_state.library.upsert_remote_json("romm", &rows)
                                        {
                                            error!(%error, "RomM catalog cache write failed");
                                            break;
                                        }
                                        let count = page.items.len() as i64;
                                        info!(platform = %platform.fs_slug, offset, rows = count, total = ?page.total, "RomM catalog page cached");
                                        if count == 0
                                            || page
                                                .total
                                                .is_some_and(|total| offset + count >= total)
                                        {
                                            break;
                                        }
                                        offset += count;
                                    }
                                }
                            }
                            Err(error) => error!(%error, "RomM catalog platform sync failed"),
                        }
                    });
                }
            } else {
                info!("RomM startup catalog import disabled by MARINA_IMPORT_ROMM_ON_STARTUP");
            }

            let platform_metadata = match state.library.platforms().await {
                Ok(platforms) => platforms,
                Err(error) => {
                    error!(%error, "platform metadata loading failed");
                    Vec::new()
                }
            };
            info!(
                count = platform_metadata.len(),
                "local platform records loaded"
            );
            let icon_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("ui/assets/platforms/systematic");
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
                            .and_then(|path| {
                                Image::load_from_path(std::path::Path::new(&path)).ok()
                            })
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
    });

    ui::pages::store::install(&window, &library_state);

    info!("starting Slint event loop; deferred startup work will run in background");
    window.run()
}

fn populate_store_details(window: &slint::Weak<MainWindow>, rom: marina_romm::Rom, base_url: &str) {
    let rom_id = rom.id.to_string();
    let cover_source = covers::source_for(rom.cover_path().as_deref(), None, Some(base_url));
    let screenshot = rom
        .assets
        .merged_screenshots
        .first()
        .cloned()
        .or_else(|| {
            rom.assets
                .user_screenshots
                .first()
                .map(|screenshot| screenshot.download_path.clone())
        })
        .or_else(|| {
            rom.assets
                .all_user_screenshots
                .first()
                .map(|screenshot| screenshot.download_path.clone())
        });
    let screenshot_source = covers::source_for(screenshot.as_deref(), None, Some(base_url));
    let item: marina_core::LibraryItem = rom.clone().into();
    let rom_prefix = rom.files.full_path.trim_end_matches('/').to_owned();
    let artifact_count = rom.files.files.len();
    let mut artifact_tree = ArtifactTree::default();
    for (file_index, file) in rom.files.files.iter().enumerate() {
        artifact_tree.insert(
            &display_artifact_path(file, &rom_prefix),
            file_index,
            file.file_size_bytes,
        );
    }
    let mut artifacts = Vec::new();
    artifact_tree.flatten(0, &mut artifacts);
    let details = PreviewDetailsData {
        title: SharedString::from(item.title),
        summary: SharedString::from(item.summary.unwrap_or_default()),
        released_at: SharedString::default(),
        languages: SharedString::from(item.languages.join(", ")),
        regions: SharedString::from(item.regions.join(", ")),
        tags: SharedString::from(item.tags.join(", ")),
    };
    let selected_rom_id = rom_id.clone();
    let _ = window.upgrade_in_event_loop(move |window| {
        let games = window.get_store_games();
        let selected_index = window.get_store_selected_game_index().max(0) as usize;
        if games
            .row_data(selected_index)
            .is_none_or(|game| game.id.as_str() != selected_rom_id)
        {
            return;
        }
        window.set_store_details(details);
        window.set_store_preview_image(Image::default());
        window.set_store_artifacts(ModelRc::from(std::rc::Rc::new(VecModel::from(artifacts))));
        window.set_store_selected_artifacts(
            std::rc::Rc::new(VecModel::from(vec![false; artifact_count])).into(),
        );
        window.set_store_details_loading(false);
    });

    let preview_window = window.clone();
    let preview_rom_id = rom_id.clone();
    tokio::spawn(async move {
        let Some(bytes) = covers::load_bytes(&reqwest::Client::new(), &screenshot_source).await
        else {
            return;
        };
        let _ = preview_window.upgrade_in_event_loop(move |window| {
            let games = window.get_store_games();
            let selected_index = window.get_store_selected_game_index().max(0) as usize;
            if games
                .row_data(selected_index)
                .is_none_or(|game| game.id.as_str() != preview_rom_id)
            {
                return;
            }
            let Some((image, _)) = covers::decode(&bytes) else {
                return;
            };
            window.set_store_preview_image(image);
        });
    });

    let cover_window = window.clone();
    tokio::spawn(async move {
        let Some(bytes) = covers::load_bytes(&reqwest::Client::new(), &cover_source).await else {
            return;
        };
        let _ = cover_window.upgrade_in_event_loop(move |window| {
            let Some((image, ratio)) = covers::decode(&bytes) else {
                return;
            };
            let games = window.get_store_games();
            let Some(index) = (0..games.row_count()).find(|&index| {
                games
                    .row_data(index)
                    .is_some_and(|game| game.id.as_str() == rom_id)
            }) else {
                return;
            };
            if let Some(mut game) = games.row_data(index) {
                game.cover = image;
                game.cover_ratio = ratio;
                games.set_row_data(index, game);
            }
        });
    });
}

#[derive(Default)]
struct ArtifactTree {
    directories: BTreeMap<String, Self>,
    files: Vec<(String, usize, i64)>,
}

impl ArtifactTree {
    fn insert(&mut self, path: &str, file_index: usize, file_size_bytes: i64) {
        let parts = path
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let Some((file_name, directories)) = parts.split_last() else {
            return;
        };

        let mut node = self;
        for directory in directories {
            node = node.directories.entry((*directory).to_owned()).or_default();
        }
        node.files
            .push(((*file_name).to_owned(), file_index, file_size_bytes));
    }

    fn flatten(&self, depth: i32, rows: &mut Vec<StoreArtifact>) {
        for (directory, child) in &self.directories {
            rows.push(StoreArtifact {
                path: SharedString::from(directory),
                size: SharedString::default(),
                depth,
                is_directory: true,
                file_index: -1,
            });
            child.flatten(depth + 1, rows);
        }

        let mut files = self.files.clone();
        files.sort_by(|left, right| left.0.cmp(&right.0));
        for (file_name, file_index, file_size_bytes) in files {
            rows.push(StoreArtifact {
                path: SharedString::from(file_name),
                size: SharedString::from(format_file_size(file_size_bytes)),
                depth,
                is_directory: false,
                file_index: file_index as i32,
            });
        }
    }
}

fn format_file_size(bytes: i64) -> String {
    u64::try_from(bytes)
        .map(|bytes| bytesize::ByteSize::b(bytes).display().iec().to_string())
        .unwrap_or_else(|_| "Unknown size".to_owned())
}

fn display_artifact_path(file: &marina_romm::RomFile, rom_prefix: &str) -> String {
    let source = if file.full_path.is_empty() {
        &file.file_path
    } else {
        &file.full_path
    };
    let stripped = source
        .strip_prefix(rom_prefix)
        .unwrap_or(source)
        .trim_start_matches('/');
    if stripped.is_empty() {
        file.file_name.clone()
    } else {
        stripped.to_owned()
    }
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
