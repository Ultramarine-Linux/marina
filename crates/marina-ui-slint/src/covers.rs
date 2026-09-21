//! Page-scoped artwork loading and decoding.

use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use marina_library::{
    error::LibraryError,
    query::{SearchQuery, SearchSort},
    read::LibraryRead,
};
use slint::{ComponentHandle, Image, Model, ModelRc};
use tracing::{debug, trace, warn};

use crate::{GameCardData, HomeState, MainWindow, image as image_loader};

pub use crate::image::ImageSource as CoverSource;

// Handheld memory is more constrained than storage latency. Do not retain
// decoded neighbors once they leave the visible horizontal viewport.
const COVER_PREFETCH_ITEMS: usize = 0;

pub fn source_for(
    source: Option<&str>,
    local_path: Option<&str>,
    base_url: Option<&str>,
) -> CoverSource {
    crate::image::source_for(source, local_path, base_url)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Page {
    Home,
}

/// A vertically stacked shelf on the home page. The loader tracks each
/// shelf independently so covers for vertically hidden shelves are evicted
/// instead of held resident.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Shelf {
    Played,
    Added,
}

impl Shelf {
    pub fn from_index(index: i32) -> Self {
        if index == 0 {
            Self::Played
        } else {
            Self::Added
        }
    }
}

pub struct ViewportLoader {
    window: slint::Weak<MainWindow>,
    added_sources: Arc<Mutex<Vec<CoverSource>>>,
    played_sources: Arc<Mutex<Vec<CoverSource>>>,
    loading: Arc<Mutex<HashSet<(u64, Page, Shelf, usize)>>>,
    resident: Arc<Mutex<HashSet<(Page, Shelf, usize)>>>,
    wanted: Arc<Mutex<HashSet<(Page, Shelf, usize)>>>,
    /// Indexes whose load failed — or had no source at all — in this
    /// generation. Consulted alongside `resident` so failures are not
    /// retried on every viewport update (notably every scroll frame, when
    /// `update` runs per frame and each attempt spawns a doomed task).
    /// Cleared by `reset()`, so replaced sources get a fresh attempt.
    failed: Arc<Mutex<HashSet<(u64, Page, Shelf, usize)>>>,
    tasks: Arc<Mutex<HashMap<(u64, Page, Shelf, usize), tokio::task::AbortHandle>>>,
    visible: Arc<Mutex<HashMap<(Page, Shelf), f32>>>,
    generation: Arc<AtomicU64>,
    last_viewport: HashMap<Shelf, (f32, f32, f32)>,
}

impl ViewportLoader {
    pub fn new(window: &MainWindow) -> Rc<std::cell::RefCell<Self>> {
        let loader = Self {
            window: window.as_weak(),
            added_sources: Arc::new(Mutex::new(Vec::new())),
            played_sources: Arc::new(Mutex::new(Vec::new())),
            loading: Arc::new(Mutex::new(HashSet::new())),
            resident: Arc::new(Mutex::new(HashSet::new())),
            wanted: Arc::new(Mutex::new(HashSet::new())),
            failed: Arc::new(Mutex::new(HashSet::new())),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            visible: Arc::new(Mutex::new(HashMap::new())),
            generation: Arc::new(AtomicU64::new(0)),
            last_viewport: HashMap::new(),
        };
        Rc::new(std::cell::RefCell::new(loader))
    }

    pub fn added_sources(&self) -> Arc<Mutex<Vec<CoverSource>>> {
        self.added_sources.clone()
    }

    pub fn played_sources(&self) -> Arc<Mutex<Vec<CoverSource>>> {
        self.played_sources.clone()
    }

    fn shelf_sources(&self, shelf: Shelf) -> Arc<Mutex<Vec<CoverSource>>> {
        match shelf {
            Shelf::Played => self.played_sources.clone(),
            Shelf::Added => self.added_sources.clone(),
        }
    }

    fn cancel_pending(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        let tasks = std::mem::take(&mut *self.tasks.lock().expect("cover task state poisoned"));
        for task in tasks.into_values() {
            task.abort();
        }
        self.loading
            .lock()
            .expect("cover loading state poisoned")
            .clear();
        self.wanted
            .lock()
            .expect("cover wanted state poisoned")
            .clear();
        self.failed
            .lock()
            .expect("cover failed state poisoned")
            .clear();
    }

    /// Cancels hidden-page work while retaining the bounded set of already
    /// visible images. Re-entry reuses these image identities and avoids
    /// repeatedly warming renderer caches.
    pub fn suspend(&mut self, page: Page) {
        self.cancel_pending();
        self.visible
            .lock()
            .expect("cover visibility state poisoned")
            .retain(|(key_page, _), _| *key_page != page);
    }

    pub fn refresh(&mut self, page: Page) {
        // Replay the last known geometry per shelf so a context return
        // restores coverage without waiting for the next scroll event.
        // Shelves that never reported fall back to the window size below.
        let mut replayed = HashSet::new();
        for (&shelf, &(scroll_x, viewport_width, cover_height)) in self.last_viewport.clone().iter()
        {
            let visible = self
                .visible
                .lock()
                .expect("cover visibility state poisoned")
                .get(&(page, shelf))
                .copied()
                .unwrap_or(1.0);
            self.update(page, shelf, scroll_x, viewport_width, cover_height, visible);
            replayed.insert(shelf);
        }
        if !replayed.contains(&Shelf::Added) {
            let viewport = self.window.upgrade().map(|window| {
                let size = window.window().size();
                let scale = window.window().scale_factor().max(1.0);
                let width = size.width as f32 / scale;
                let height = size.height as f32 / scale;
                let cover_height = (height * 0.5 - 64.0).clamp(80.0, 200.0);
                (0.0, width, cover_height)
            });
            if let Some((scroll_x, viewport_width, cover_height)) = viewport {
                self.update(
                    page,
                    Shelf::Added,
                    scroll_x,
                    viewport_width,
                    cover_height,
                    1.0,
                );
            }
        }
    }

    pub fn update(
        &mut self,
        page: Page,
        shelf: Shelf,
        scroll_x: f32,
        viewport_width: f32,
        cover_height: f32,
        visible_fraction: f32,
    ) {
        self.visible
            .lock()
            .expect("cover visibility state poisoned")
            .insert((page, shelf), visible_fraction);
        if visible_fraction <= 0.0 {
            // Vertically hidden: drop resident covers so the off-screen
            // shelf holds no decoded images.
            self.evict_shelf(page, shelf);
            return;
        }
        self.last_viewport
            .insert(shelf, (scroll_x, viewport_width, cover_height));
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let games = page_games(&window, page, shelf);
        let sources = self
            .shelf_sources(shelf)
            .lock()
            .expect("cover source state poisoned")
            .clone();
        let scroll_x = (-scroll_x).max(0.0);
        let cover_height = cover_height.max(1.0);
        let prefetch = COVER_PREFETCH_ITEMS;
        let mut cursor = 4.0_f32;
        let end = scroll_x + viewport_width;
        let mut first = None;
        let mut last = None;
        for index in 0..games.row_count().min(sources.len()) {
            let ratio = games
                .row_data(index)
                .map(|game| game.cover_ratio.clamp(0.4, 2.0))
                .unwrap_or(1.0);
            let width = cover_height * ratio;
            if cursor + width >= scroll_x && cursor <= end {
                first.get_or_insert(index);
                last = Some(index);
            }
            cursor += width + 16.0;
        }
        let first = first.unwrap_or(0).saturating_sub(prefetch);
        let last = last
            .map(|index| (index + prefetch + 1).min(sources.len()))
            .unwrap_or(0);
        let wanted = (first..last).collect::<HashSet<_>>();
        {
            let mut wanted_state = self.wanted.lock().expect("cover wanted state poisoned");
            wanted_state.retain(|(wanted_page, wanted_shelf, _)| {
                *wanted_page != page || *wanted_shelf != shelf
            });
            wanted_state.extend(wanted.iter().map(|index| (page, shelf, *index)));
        }
        let generation = self.generation.load(Ordering::Relaxed);
        trace!(
            ?page,
            ?shelf,
            scroll_x,
            viewport_width,
            cover_height,
            visible_fraction,
            rows = games.row_count(),
            sources = sources.len(),
            first,
            last,
            wanted = ?wanted,
            generation,
            "cover viewport calculated"
        );
        let key_page = page;
        let key_shelf = shelf;
        let resident = self.resident.clone();
        let loading = self.loading.clone();
        let failed = self.failed.clone();
        for index in 0..games.row_count() {
            if !wanted.contains(&index) {
                let key = (generation, key_page, key_shelf, index);
                if let Some(task) = self
                    .tasks
                    .lock()
                    .expect("cover task state poisoned")
                    .remove(&key)
                {
                    task.abort();
                }
                loading
                    .lock()
                    .expect("cover loading state poisoned")
                    .remove(&key);
                resident
                    .lock()
                    .expect("cover resident state poisoned")
                    .remove(&(key_page, key_shelf, index));
                // The Slint model owns the Image. Clear it regardless of our
                // bookkeeping so its pixel buffer drops as soon as no other
                // visible component references it.
                if let Some(mut game) = games.row_data(index)
                    && game.cover.size().width > 0
                {
                    game.cover = Image::default();
                    game.cover_ratio = 1.0;
                    games.set_row_data(index, game);
                }
            }
        }
        for index in wanted {
            let key = (generation, key_page, key_shelf, index);
            let resident_key = (key_page, key_shelf, index);
            let has_image = games
                .row_data(index)
                .is_some_and(|game| game.cover.size().width > 0);
            if !has_image {
                resident
                    .lock()
                    .expect("cover resident state poisoned")
                    .remove(&resident_key);
            }
            if (has_image
                && resident
                    .lock()
                    .expect("cover resident state poisoned")
                    .contains(&resident_key))
                || failed
                    .lock()
                    .expect("cover failed state poisoned")
                    .contains(&key)
                || !loading
                    .lock()
                    .expect("cover loading state poisoned")
                    .insert(key)
            {
                continue;
            }
            let title = games
                .row_data(index)
                .map(|game| game.title.to_string())
                .unwrap_or_else(|| "<missing>".to_owned());
            let Some(source) = sources.get(index).cloned() else {
                loading
                    .lock()
                    .expect("cover loading state poisoned")
                    .remove(&key);
                failed
                    .lock()
                    .expect("cover failed state poisoned")
                    .insert(key);
                warn!(?key_page, ?key_shelf, index, %title, "cover source index missing");
                continue;
            };
            if source.is_empty() {
                loading
                    .lock()
                    .expect("cover loading state poisoned")
                    .remove(&key);
                failed
                    .lock()
                    .expect("cover failed state poisoned")
                    .insert(key);
                debug!(?key_page, ?key_shelf, index, %title, "no cover source; skipping load");
                continue;
            }
            debug!(
                ?key_page,
                ?key_shelf,
                index,
                %title,
                local_path = ?source.local_path,
                cache_path = ?source.cache_path,
                url = ?source.url,
                "starting cover load"
            );
            let window = self.window.clone();
            let generation_state = self.generation.clone();
            let visible = self.visible.clone();
            let resident = resident.clone();
            let loading = loading.clone();
            let failed = failed.clone();
            let wanted_state = self.wanted.clone();
            let task_registry = self.tasks.clone();
            let task = tokio::spawn(async move {
                let decoded = image_loader::load_scaled(
                    &image_loader::ImageSource::from(&source),
                    "shelf-cover",
                    256,
                )
                .await;
                task_registry
                    .lock()
                    .expect("cover task state poisoned")
                    .remove(&(generation, key_page, key_shelf, index));
                if generation_state.load(Ordering::Relaxed) != generation {
                    loading
                        .lock()
                        .expect("cover loading state poisoned")
                        .remove(&(generation, key_page, key_shelf, index));
                    debug!(?key_page, ?key_shelf, index, %title, generation, "discarding stale cover result");
                    return;
                }
                let visible_now = visible
                    .lock()
                    .expect("cover visibility state poisoned")
                    .get(&(key_page, key_shelf))
                    .copied()
                    .unwrap_or(0.0);
                if visible_now <= 0.0 {
                    loading
                        .lock()
                        .expect("cover loading state poisoned")
                        .remove(&(generation, key_page, key_shelf, index));
                    debug!(?key_page, ?key_shelf, index, %title, "discarding cover for hidden shelf");
                    return;
                }
                let Some(decoded) = decoded else {
                    loading
                        .lock()
                        .expect("cover loading state poisoned")
                        .remove(&(generation, key_page, key_shelf, index));
                    failed
                        .lock()
                        .expect("cover failed state poisoned")
                        .insert((generation, key_page, key_shelf, index));
                    warn!(?key_page, ?key_shelf, index, %title, "cover decode failed");
                    return;
                };
                debug!(?key_page, ?key_shelf, index, %title, width = decoded.width, height = decoded.height, "cover decoded");
                let ui_loading = loading.clone();
                let queued = window.upgrade_in_event_loop(move |window| {
                    ui_loading
                        .lock()
                        .expect("cover loading state poisoned")
                        .remove(&(generation, key_page, key_shelf, index));
                    if generation_state.load(Ordering::Relaxed) != generation
                        || !wanted_state
                            .lock()
                            .expect("cover wanted state poisoned")
                            .contains(&(key_page, key_shelf, index))
                    {
                        return;
                    }
                    if let Some(mut game) = page_games(&window, key_page, key_shelf).row_data(index) {
                        let (image, ratio) = image_loader::into_slint_image(decoded);
                        resident
                            .lock()
                            .expect("cover resident state poisoned")
                            .insert((key_page, key_shelf, index));
                        game.cover = image;
                        game.cover_ratio = ratio;
                        page_games(&window, key_page, key_shelf).set_row_data(index, game);
                        debug!(?key_page, ?key_shelf, index, %title, "cover applied");
                    } else {
                        warn!(?key_page, ?key_shelf, index, %title, "cover row missing when applying image");
                    }
                });
                if queued.is_err() {
                    loading
                        .lock()
                        .expect("cover loading state poisoned")
                        .remove(&(generation, key_page, key_shelf, index));
                }
            });
            self.tasks
                .lock()
                .expect("cover task state poisoned")
                .insert(key, task.abort_handle());
        }
    }

    /// Drops every resident cover of one shelf and aborts pending loads. A
    /// blocking decode that already started may finish, but its result is
    /// discarded by the generation and visibility guards.
    fn evict_shelf(&self, page: Page, shelf: Shelf) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let games = page_games(&window, page, shelf);
        let mut tasks = self.tasks.lock().expect("cover task state poisoned");
        let stale_keys = tasks
            .keys()
            .filter(|(_, key_page, key_shelf, _)| *key_page == page && *key_shelf == shelf)
            .copied()
            .collect::<Vec<_>>();
        for key in stale_keys {
            if let Some(task) = tasks.remove(&key) {
                task.abort();
            }
        }
        drop(tasks);
        self.loading
            .lock()
            .expect("cover loading state poisoned")
            .retain(|(_, key_page, key_shelf, _)| *key_page != page || *key_shelf != shelf);
        self.resident
            .lock()
            .expect("cover resident state poisoned")
            .retain(|(key_page, key_shelf, _)| *key_page != page || *key_shelf != shelf);
        self.wanted
            .lock()
            .expect("cover wanted state poisoned")
            .retain(|(key_page, key_shelf, _)| *key_page != page || *key_shelf != shelf);
        for index in 0..games.row_count() {
            if let Some(mut game) = games.row_data(index) {
                if game.cover.size().width > 0 {
                    game.cover = Image::default();
                    game.cover_ratio = 1.0;
                    games.set_row_data(index, game);
                }
            }
        }
        debug!(?page, ?shelf, "evicted hidden shelf covers");
    }
}

fn page_games(window: &MainWindow, page: Page, shelf: Shelf) -> ModelRc<GameCardData> {
    match (page, shelf) {
        (Page::Home, Shelf::Added) => window.global::<HomeState>().get_games(),
        (Page::Home, Shelf::Played) => window.global::<HomeState>().get_played_games(),
    }
}

pub async fn load_games_metadata(
    library: &marina_store_sqlite::SqliteLibrary,
    base_url: Option<&str>,
) -> Result<
    (
        Vec<crate::ui::pages::library::GameMetadata>,
        Vec<CoverSource>,
    ),
    LibraryError,
> {
    let items = library
        .search_cards(SearchQuery::new().sort(SearchSort::LastUpdated).limit(20))
        .await?;
    Ok(items
        .into_iter()
        .map(|item| {
            let source = source_for(
                item.cover.as_deref(),
                item.cover_small_local_path
                    .as_deref()
                    .or(item.cover_large_local_path.as_deref()),
                base_url,
            );
            (
                crate::ui::pages::library::GameMetadata {
                    id: item.id.to_string(),
                    title: item.title,
                    platform: item.platform_name.unwrap_or_else(|| "Unknown".into()),
                },
                source,
            )
        })
        .unzip())
}
