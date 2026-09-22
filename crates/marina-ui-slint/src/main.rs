use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio::sync::Notify;

use marina_input::{InputConfig, InputLoop};
use marina_library::{
    query::SearchQuery,
    read::{LibraryRead, PlatformRead},
};
use serde::Serialize;
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

slint::include_modules!();

mod app;
mod cache;
mod config;
mod covers;
mod image;
mod storage;
mod ui;

use ui::pages::library as shelf;

const CONTROLLER_STARTUP_DELAY: Duration = Duration::from_secs(1);

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
    let controller_enabled = Arc::new(AtomicBool::new(true));
    let focus_state = controller_enabled.clone();
    i_slint_core::context::set_window_event_hook(Some(Box::new(
        move |_adapter, event, _result| {
            if let i_slint_core::platform::WindowEvent::WindowActiveChanged(active) = event {
                focus_state.store(*active, Ordering::Release);
            }
        },
    )))?;
    slint::set_xdg_app_id("org.ultramarinelinux.MarinaShell")?;
    info!("main window constructed; completing lightweight UI setup");
    ui::controls::configure_toasts(&window);
    ui::controls::configure_navigation(&window);

    // Controller discovery can block inside gilrs while probing input devices.
    // Never make window creation wait for it; initialize it after the event loop
    // has started and keep the returned loop alive in its background task.
    let controller_window = window.as_weak();
    let controller_enabled = controller_enabled.clone();
    tokio::spawn(async move {
        // gilrs probes every input device during construction. Let the first
        // frame and initial library query get CPU priority before doing that
        // work, especially on handhelds with slow input device enumeration.
        tokio::time::sleep(CONTROLLER_STARTUP_DELAY).await;
        let started = std::time::Instant::now();
        info!("starting controller input discovery");
        let result = tokio::task::spawn_blocking(move || {
            InputLoop::spawn(InputConfig::default(), move |event| {
                if !controller_enabled.load(Ordering::Acquire) {
                    return;
                }
                let controller_window = controller_window.clone();
                let _ = controller_window.upgrade_in_event_loop(move |window| {
                    // gilrs remains alive while another application owns focus,
                    // but controller actions must only reach Marina when its
                    // native window is active.
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
    ui::battery::initialize(&window);
    ui::clock::initialize(&window);
    window
        .global::<HomeState>()
        .set_games(ModelRc::from(std::rc::Rc::new(VecModel::from(Vec::new()))));
    window
        .global::<HomeState>()
        .set_played_games(ModelRc::from(std::rc::Rc::new(VecModel::from(Vec::new()))));
    window
        .global::<LibraryState>()
        .set_games(ModelRc::from(std::rc::Rc::new(VecModel::from(Vec::new()))));
    window
        .global::<GameState>()
        .set_selected_game(empty_game_card());
    window
        .global::<GameState>()
        .set_details(empty_preview_details());
    window
        .global::<GameState>()
        .set_tags(ModelRc::from(std::rc::Rc::new(VecModel::from(Vec::<
            SharedString,
        >::new(
        )))));

    window.global::<HomeState>().set_loading(true);
    window.global::<LibraryState>().set_loading(false);

    let library_state: Arc<Mutex<Option<app::AppStateHandle>>> = Arc::new(Mutex::new(None));
    let cover_loader = covers::ViewportLoader::new(&window);
    let loader_sources = cover_loader.borrow().added_sources();
    let played_sources = cover_loader.borrow().played_sources();
    let home_source_store = Arc::new(Mutex::new(Vec::new()));
    let scroll_loader = cover_loader.clone();
    window.global::<HomeState>().on_viewport_changed(
        move |shelf, scroll_x, width, cover_height, visible| {
            scroll_loader.borrow_mut().update(
                covers::Page::Home,
                covers::Shelf::from_index(shelf),
                scroll_x,
                width,
                cover_height,
                visible,
            );
        },
    );
    let context_loader = cover_loader.clone();
    let context_loader_sources = loader_sources.clone();
    let context_home_sources = home_source_store.clone();
    window
        .global::<HomeState>()
        .on_cover_context_changed(move |active_tab| {
            let mut loader = context_loader.borrow_mut();
            if active_tab != 0 {
                loader.suspend(covers::Page::Home);
                return;
            }
            // Refresh is idempotent: it preserves resident and in-flight
            // covers, avoiding duplicate decode waves when model and route
            // notifications arrive during the same Home entry.
            *context_loader_sources
                .lock()
                .expect("cover source state poisoned") = context_home_sources
                .lock()
                .expect("home cover source state poisoned")
                .clone();
            loader.refresh(covers::Page::Home);
        });

    let played_store = ui::pages::home::new_played_store();
    ui::launch::install(&window, &library_state, &played_store, &played_sources);
    ui::pages::home::install(&window, &library_state, &home_source_store, &played_store);
    ui::pages::library::install(&window, &library_state);

    let weak_window = window.as_weak();
    let state_store = library_state.clone();
    let played_hydration = played_store.clone();
    let played_source_hydration = played_sources.clone();
    let played_window = weak_window.clone();
    let startup_sources = loader_sources.clone();
    let startup_home_sources = home_source_store.clone();
    let cached_home_ready = Arc::new(Notify::new());
    slint::Timer::single_shot(Duration::ZERO, move || {
        tokio::spawn(async move {
            let state = match app::AppState::initialize().await {
                Ok(state) => state,
                Err(error) => {
                    error!(%error, "application initialization failed");
                    return;
                }
            };
            *state_store.lock().expect("library state lock poisoned") = Some(state.clone());

            // Restore persisted play activity into the recently-played shelf
            // alongside the other independent hydration phases.
            tokio::spawn(ui::pages::home::hydrate_played(
                state.clone(),
                played_window.clone(),
                played_hydration.clone(),
                played_source_hydration.clone(),
            ));

            // Each phase owns its own work and can publish as soon as it is ready.
            tokio::spawn(hydrate_cached_home(
                state.clone(),
                weak_window.clone(),
                startup_sources.clone(),
                startup_home_sources.clone(),
                cached_home_ready.clone(),
            ));
            tokio::spawn(reconcile_local(
                state.clone(),
                weak_window.clone(),
                startup_sources,
                startup_home_sources,
                cached_home_ready,
            ));
            tokio::spawn(hydrate_startup_platforms(
                state.clone(),
                weak_window.clone(),
            ));
            tokio::spawn(reconcile_stores(state));
        });
    });

    ui::pages::store::install(&window, &library_state);

    info!("starting Slint event loop; deferred startup work will run in background");
    window.run()
}

type CoverSourceStore = Arc<Mutex<Vec<covers::CoverSource>>>;

async fn hydrate_cached_home(
    state: app::AppStateHandle,
    window: slint::Weak<MainWindow>,
    loader_sources: CoverSourceStore,
    home_sources: CoverSourceStore,
    ready: Arc<Notify>,
) {
    // The persisted card projection is the first usable Home model. Local
    // reconciliation refreshes it in its own phase afterward.
    match shelf::load_games(&state.library, state.config.romm_url.as_deref()).await {
        Ok((metadata, cover_sources)) => {
            apply_home_metadata(
                &window,
                metadata,
                cover_sources,
                &loader_sources,
                &home_sources,
            );
        }
        Err(error) => error!(%error, "cached game metadata loading failed"),
    }
    ready.notify_one();
}

async fn reconcile_local(
    state: app::AppStateHandle,
    window: slint::Weak<MainWindow>,
    loader_sources: CoverSourceStore,
    home_sources: CoverSourceStore,
    cached_home_ready: Arc<Notify>,
) {
    // Keep reconciliation independent without allowing it to race the initial
    // cached projection and replace it with an older query result.
    cached_home_ready.notified().await;

    let discovered_apps = tokio::task::spawn_blocking(marina_apps::discover)
        .await
        .unwrap_or_else(|error| {
            error!(%error, "XDG application discovery task failed");
            Vec::new()
        });
    if let Err(error) = marina_apps::sync(&state.library, discovered_apps).await {
        error!(%error, "failed to sync XDG applications");
    }

    app::reconcile_local_games(&state).await;

    // Reconciliation changes the persisted platform/game rows. Refresh the
    // Library projection as well as Home so a newly discovered local platform
    // appears immediately even when its startup hydration finished earlier.
    if let Err(error) = shelf::refresh_platform_cards(&state.library, &window).await {
        error!(%error, "post-scan library platform refresh failed");
    }
    refresh_home(&state, &window, loader_sources, home_sources).await;
}

async fn refresh_home(
    state: &app::AppStateHandle,
    window: &slint::Weak<MainWindow>,
    loader_sources: CoverSourceStore,
    home_sources: CoverSourceStore,
) {
    info!("loading game metadata");
    match shelf::load_games(&state.library, state.config.romm_url.as_deref()).await {
        Ok((metadata, cover_sources)) => {
            info!(count = metadata.len(), "library loaded");
            apply_home_metadata(
                window,
                metadata,
                cover_sources,
                &loader_sources,
                &home_sources,
            );
        }
        Err(error) => error!(%error, "game metadata loading failed"),
    }
}

fn apply_home_metadata(
    window: &slint::Weak<MainWindow>,
    metadata: Vec<shelf::GameMetadata>,
    cover_sources: Vec<covers::CoverSource>,
    loader_sources: &CoverSourceStore,
    home_sources: &CoverSourceStore,
) {
    let loader_sources = loader_sources.clone();
    let home_sources = home_sources.clone();
    let _ = window.upgrade_in_event_loop(move |window| {
        if window.global::<ShellState>().get_page() != ShellPage::Home {
            return;
        }
        *loader_sources.lock().expect("cover source state poisoned") = cover_sources.clone();
        *home_sources
            .lock()
            .expect("home cover source state poisoned") = cover_sources;
        window
            .global::<HomeState>()
            .set_games(ModelRc::from(std::rc::Rc::new(VecModel::from(game_cards(
                metadata,
            )))));
        window.global::<HomeState>().set_loading(false);
        window.global::<HomeState>().invoke_cover_context_changed(0);
    });
}

async fn reconcile_stores(state: app::AppStateHandle) {
    // Runs for every configured backend; each one syncs into its own
    // `<backend>.db` cache file — never into the main library database.
    // RomM import-on-startup stays opt-in, other backends sync
    // unconditionally once configured.
    for backend in state.stores.values().cloned() {
        if backend.id() == "romm" && !state.config.import_romm_on_startup {
            info!("RomM startup catalog import disabled by config");
            continue;
        }
        let Ok(cache) = state.store_caches.cache_for(backend.id()) else {
            continue;
        };

        match backend.list_platforms().await {
            Ok(platforms) => {
                for platform in platforms {
                    let mut offset = 0_usize;
                    loop {
                        let entries = match backend
                            .browse(marina_store::StoreQuery {
                                platform_slug: Some(platform.slug.clone()),
                                limit: 100,
                                offset,
                                ..Default::default()
                            })
                            .await
                        {
                            Ok(entries) => entries,
                            Err(error) => {
                                error!(%error, backend = backend.id(), platform = %platform.slug, "store catalog sync failed");
                                break;
                            }
                        };
                        let count = entries.len();
                        if let Err(error) = cache.upsert_entries(&entries) {
                            error!(%error, backend = backend.id(), "store catalog cache write failed");
                            break;
                        }
                        info!(backend = backend.id(), platform = %platform.slug, offset, rows = count, "store catalog page cached");
                        if count == 0 || count < 100 {
                            break;
                        }
                        offset += count;
                    }
                }
            }
            Err(error) => {
                error!(%error, backend = backend.id(), "store catalog platform sync failed")
            }
        }
    }
}

async fn hydrate_startup_platforms(state: app::AppStateHandle, window: slint::Weak<MainWindow>) {
    let platforms = match state.library.platforms().await {
        Ok(platforms) => platforms,
        Err(error) => {
            error!(%error, "startup platform hydration failed");
            return;
        }
    };
    let icon_root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/assets/platforms/systematic");
    let cards = platforms
        .into_iter()
        .map(|platform| PlatformCardMetadata {
            icon_path: platform_asset_path(&icon_root, &platform.slug),
            slug: platform.slug,
            name: platform.name,
            game_count: "Loading…".to_owned(),
        })
        .collect::<Vec<_>>();
    let icon_jobs = cards
        .iter()
        .enumerate()
        .filter_map(|(index, card)| card.icon_path.clone().map(|path| (index, path)))
        .collect::<Vec<_>>();
    let count_jobs = cards
        .iter()
        .enumerate()
        .map(|(index, card)| (index, card.slug.clone()))
        .collect::<Vec<_>>();
    let _ = window.upgrade_in_event_loop(move |window| {
        let cards = cards
            .into_iter()
            .map(|card| PlatformCardData {
                slug: SharedString::from(card.slug),
                name: SharedString::from(card.name),
                game_count: SharedString::from(card.game_count),
                icon: Image::default(),
            })
            .collect::<Vec<_>>();
        let library = window.global::<LibraryState>();
        if library.get_platforms().row_count() == 0 {
            library.set_platforms(ModelRc::from(std::rc::Rc::new(VecModel::from(cards))));
        }
        library.set_loading(false);
    });

    for (index, path) in icon_jobs {
        let icon_window = window.clone();
        tokio::spawn(async move {
            let Some(decoded) = image::load_path_scaled(path, "platform-icon", 256).await else {
                return;
            };
            let _ = icon_window.upgrade_in_event_loop(move |window| {
                let platforms = window.global::<LibraryState>().get_platforms();
                if let Some(mut platform) = platforms.row_data(index) {
                    platform.icon = image::into_slint_image(decoded).0;
                    platforms.set_row_data(index, platform);
                }
            });
        });
    }

    for (index, slug) in count_jobs {
        let Ok(count) = state
            .library
            .count(SearchQuery::new().platform(&slug))
            .await
        else {
            continue;
        };
        let _ = window.upgrade_in_event_loop(move |window| {
            let platforms = window.global::<LibraryState>().get_platforms();
            if let Some(mut platform) = platforms.row_data(index) {
                platform.game_count = SharedString::from(format!(
                    "{} {}",
                    count,
                    if count == 1 { "game" } else { "games" }
                ));
                platforms.set_row_data(index, platform);
            }
        });
    }
}

fn populate_store_details(
    window: &slint::Weak<MainWindow>,
    rom: marina_romm::Rom,
    base_url: &str,
    generation_guard: Arc<AtomicU64>,
    generation: u64,
    load_preview: bool,
) -> Option<tokio::task::AbortHandle> {
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
    let item: marina_core::LibraryItem = rom.into();
    let tags = item.tags.clone();
    let details = PreviewDetailsData {
        title: SharedString::from(item.title),
        summary: SharedString::from(item.summary.unwrap_or_default()),
        released_at: SharedString::default(),
        languages: SharedString::from(item.languages.join(", ")),
        regions: SharedString::from(item.regions.join(", ")),
        tags: SharedString::from(item.tags.join(", ")),
    };
    let selected_rom_id = rom_id.clone();
    let details_generation = generation_guard.clone();
    let _ = window.upgrade_in_event_loop(move |window| {
        if details_generation.load(Ordering::Relaxed) != generation {
            return;
        }
        let games = window.global::<StoreState>().get_games();
        let selected_index = window
            .global::<StoreState>()
            .get_selected_game_index()
            .max(0) as usize;
        if games
            .row_data(selected_index)
            .is_none_or(|game| game.id.as_str() != selected_rom_id)
        {
            return;
        }
        window.global::<StoreState>().set_details(details);
        window.global::<StoreState>().set_tags(string_model(tags));
        // Note: no preview reset here. The selection handler already unloaded
        // the pane; this runs twice per selection (cached, then fresh) and a
        // second clear would flicker the just-loaded preview.
        window
            .global::<StoreState>()
            .set_artifacts(ModelRc::from(std::rc::Rc::new(VecModel::from(artifacts))));
        window.global::<StoreState>().set_selected_artifacts(
            std::rc::Rc::new(VecModel::from(vec![false; artifact_count])).into(),
        );
        window.global::<StoreState>().set_details_loading(false);
    });

    // One download per selection: the preview shows the first screenshot,
    // falling back to the cover, and the list row reuses the same image.
    // Decided up front from the API record — never fetch both.
    let preview_source = [&screenshot_source, &cover_source]
        .into_iter()
        .find(|source| !source.is_empty())
        .cloned();

    if !load_preview {
        return None;
    }

    let preview_window = window.clone();
    let preview_rom_id = rom_id.clone();
    let row_rom_id = rom_id.clone();
    if let Some(preview_source) = preview_source {
        let task = tokio::spawn(async move {
            let Some(decoded) = image::load_scaled(
                &image::ImageSource::from(&preview_source),
                "store-preview",
                image::PREVIEW_MAX_DIMENSION,
            )
            .await
            else {
                return;
            };
            let _ = preview_window.upgrade_in_event_loop(move |window| {
                if generation_guard.load(Ordering::Relaxed) != generation {
                    return;
                }
                let games = window.global::<StoreState>().get_games();
                let selected_index = window
                    .global::<StoreState>()
                    .get_selected_game_index()
                    .max(0) as usize;
                if games
                    .row_data(selected_index)
                    .is_none_or(|game| game.id.as_str() != preview_rom_id)
                {
                    return;
                }
                let (image, ratio) = image::into_slint_image(decoded);
                window
                    .global::<StoreState>()
                    .set_preview_image(image.clone());
                let Some(index) = (0..games.row_count()).find(|&index| {
                    games
                        .row_data(index)
                        .is_some_and(|game| game.id.as_str() == row_rom_id)
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
        Some(task.abort_handle())
    } else {
        None
    }
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

pub(crate) fn empty_preview_details() -> PreviewDetailsData {
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

fn string_model(values: Vec<String>) -> ModelRc<SharedString> {
    ModelRc::from(std::rc::Rc::new(VecModel::from(
        values
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )))
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
