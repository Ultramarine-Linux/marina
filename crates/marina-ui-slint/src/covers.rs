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
use tracing::{debug, warn};

use crate::{GameCardData, HomeState, MainWindow, image as image_loader};

pub use crate::image::ImageSource as CoverSource;

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

    pub fn reset(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.loading
            .lock()
            .expect("cover loading state poisoned")
            .clear();
        self.resident
            .lock()
            .expect("cover resident state poisoned")
            .clear();
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
        let prefetch = 2usize;
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
        let generation = self.generation.load(Ordering::Relaxed);
        debug!(
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
        for index in 0..games.row_count() {
            if !wanted.contains(&index) {
                let was_resident = resident
                    .lock()
                    .expect("cover resident state poisoned")
                    .remove(&(key_page, key_shelf, index));
                if was_resident {
                    if let Some(mut game) = games.row_data(index) {
                        game.cover = Image::default();
                        game.cover_ratio = 1.0;
                        games.set_row_data(index, game);
                    }
                }
            }
        }
        for index in wanted {
            if resident
                .lock()
                .expect("cover resident state poisoned")
                .contains(&(key_page, key_shelf, index))
                || !loading
                    .lock()
                    .expect("cover loading state poisoned")
                    .insert((generation, key_page, key_shelf, index))
            {
                continue;
            }
            let Some(source) = sources.get(index).cloned() else {
                warn!(?key_page, ?key_shelf, index, "cover source index missing");
                continue;
            };
            let title = games
                .row_data(index)
                .map(|game| game.title.to_string())
                .unwrap_or_else(|| "<missing>".to_owned());
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
            tokio::spawn(async move {
                let decoded = image_loader::load_scaled(
                    &image_loader::ImageSource::from(&source),
                    "shelf-cover",
                    256,
                )
                .await;
                loading
                    .lock()
                    .expect("cover loading state poisoned")
                    .remove(&(generation, key_page, key_shelf, index));
                if generation_state.load(Ordering::Relaxed) != generation {
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
                    debug!(?key_page, ?key_shelf, index, %title, "discarding cover for hidden shelf");
                    return;
                }
                let Some(decoded) = decoded else {
                    warn!(?key_page, ?key_shelf, index, %title, "cover decode failed");
                    return;
                };
                debug!(?key_page, ?key_shelf, index, %title, width = decoded.width, height = decoded.height, "cover decoded");
                let _ = window.upgrade_in_event_loop(move |window| {
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
            });
        }
    }

    /// Drops every resident cover of one shelf and cancels its pending loads.
    /// In-flight decodes still finish but discard their result via the
    /// visibility check above.
    fn evict_shelf(&self, page: Page, shelf: Shelf) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let games = page_games(&window, page, shelf);
        self.loading
            .lock()
            .expect("cover loading state poisoned")
            .retain(|(_, key_page, key_shelf, _)| *key_page != page || *key_shelf != shelf);
        self.resident
            .lock()
            .expect("cover resident state poisoned")
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
