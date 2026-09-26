//! Cached-first UI hydration and background catalog reconciliation.

use std::sync::{Arc, Mutex};

use marina_library::{
    query::SearchQuery,
    read::{LibraryRead, PlatformRead},
};
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};
use tokio::sync::Notify;
use tracing::{error, info};

use crate::ui::pages::library as shelf;
use crate::{
    HomeState, LibraryState, MainWindow, PlatformCardData, PlatformCardMetadata, ShellPage,
    ShellState, app, covers, game_cards, image, platform_asset_path,
};

type CoverSourceStore = Arc<Mutex<Vec<covers::CoverSource>>>;

pub(crate) async fn hydrate_cached_home(
    state: app::AppStateHandle,
    window: slint::Weak<MainWindow>,
    loader_sources: CoverSourceStore,
    home_sources: CoverSourceStore,
    ready: Arc<Notify>,
) {
    // The persisted card projection is the first usable Home model. Local
    // reconciliation refreshes it in its own phase afterward.
    let config = state.config.snapshot();
    match shelf::load_games(&state.library, config.romm_url.as_deref()).await {
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

pub(crate) async fn reconcile_local(
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
    let config = state.config.snapshot();
    match shelf::load_games(&state.library, config.romm_url.as_deref()).await {
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

pub(crate) async fn reconcile_stores(state: app::AppStateHandle) {
    // Runs for every configured backend; each one syncs into its own
    // `<backend>.db` cache file — never into the main library database.
    // RomM import-on-startup stays opt-in, other backends sync
    // unconditionally once configured.
    let config = state.config.snapshot();
    for backend in state.store_backends() {
        if backend.id() == "romm" && !config.import_romm_on_startup {
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

pub(crate) async fn hydrate_startup_platforms(
    state: app::AppStateHandle,
    window: slint::Weak<MainWindow>,
) {
    let platforms = match state.library.platforms().await {
        Ok(platforms) => platforms,
        Err(error) => {
            error!(%error, "startup platform hydration failed");
            return;
        }
    };

    let cards = platforms
        .into_iter()
        .map(|platform| PlatformCardMetadata {
            icon_path: platform_asset_path(&platform.slug),
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
