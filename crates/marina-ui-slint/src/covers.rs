//! Page-scoped artwork loading and decoding.

use std::{
    collections::HashSet,
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

pub struct ViewportLoader {
    window: slint::Weak<MainWindow>,
    sources: Arc<Mutex<Vec<CoverSource>>>,
    loading: Arc<Mutex<HashSet<(u64, Page, usize)>>>,
    resident: Arc<Mutex<HashSet<(Page, usize)>>>,
    generation: Arc<AtomicU64>,
    last_viewport: Option<(f32, f32, f32)>,
}

impl ViewportLoader {
    pub fn new(
        window: &MainWindow,
    ) -> (Rc<std::cell::RefCell<Self>>, Arc<Mutex<Vec<CoverSource>>>) {
        let sources = Arc::new(Mutex::new(Vec::new()));
        let loader = Self {
            window: window.as_weak(),
            sources: sources.clone(),
            loading: Arc::new(Mutex::new(HashSet::new())),
            resident: Arc::new(Mutex::new(HashSet::new())),
            generation: Arc::new(AtomicU64::new(0)),
            last_viewport: None,
        };
        (Rc::new(std::cell::RefCell::new(loader)), sources)
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
        let viewport = self
            .last_viewport
            .filter(|(_, width, height)| *width > 0.0 && *height > 0.0)
            .or_else(|| {
                let window = self.window.upgrade()?;
                let size = window.window().size();
                let scale = window.window().scale_factor().max(1.0);
                let width = size.width as f32 / scale;
                let height = size.height as f32 / scale;
                let cover_height = (height * 0.5 - 64.0).clamp(80.0, 200.0);
                Some((0.0, width, cover_height))
            });
        if let Some((scroll_x, viewport_width, cover_height)) = viewport {
            self.update(page, scroll_x, viewport_width, cover_height);
        }
    }

    pub fn update(&mut self, page: Page, scroll_x: f32, viewport_width: f32, cover_height: f32) {
        self.last_viewport = Some((scroll_x, viewport_width, cover_height));
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let games = page_games(&window, page);
        let sources = self
            .sources
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
            scroll_x,
            viewport_width,
            cover_height,
            rows = games.row_count(),
            sources = sources.len(),
            first,
            last,
            wanted = ?wanted,
            generation,
            "cover viewport calculated"
        );
        let key_page = page;
        let resident = self.resident.clone();
        let loading = self.loading.clone();
        for index in 0..games.row_count() {
            if !wanted.contains(&index) {
                let was_resident = resident
                    .lock()
                    .expect("cover resident state poisoned")
                    .remove(&(key_page, index));
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
                .contains(&(key_page, index))
                || !loading
                    .lock()
                    .expect("cover loading state poisoned")
                    .insert((generation, key_page, index))
            {
                continue;
            }
            let Some(source) = sources.get(index).cloned() else {
                warn!(?key_page, index, "cover source index missing");
                continue;
            };
            let title = games
                .row_data(index)
                .map(|game| game.title.to_string())
                .unwrap_or_else(|| "<missing>".to_owned());
            debug!(
                ?key_page,
                index,
                %title,
                local_path = ?source.local_path,
                cache_path = ?source.cache_path,
                url = ?source.url,
                "starting cover load"
            );
            let window = self.window.clone();
            let generation_state = self.generation.clone();
            let resident = resident.clone();
            let loading = loading.clone();
            tokio::spawn(async move {
                let decoded =
                    image_loader::load(&image_loader::ImageSource::from(&source), "shelf-cover")
                        .await;
                loading
                    .lock()
                    .expect("cover loading state poisoned")
                    .remove(&(generation, key_page, index));
                if generation_state.load(Ordering::Relaxed) != generation {
                    debug!(?key_page, index, %title, generation, "discarding stale cover result");
                    return;
                }
                let Some(decoded) = decoded else {
                    warn!(?key_page, index, %title, "cover decode failed");
                    return;
                };
                debug!(?key_page, index, %title, width = decoded.width, height = decoded.height, "cover decoded");
                let _ = window.upgrade_in_event_loop(move |window| {
                    if let Some(mut game) = page_games(&window, key_page).row_data(index) {
                        let (image, ratio) = image_loader::into_slint_image(decoded);
                        game.cover = image;
                        game.cover_ratio = ratio;
                        page_games(&window, key_page).set_row_data(index, game);
                        resident
                            .lock()
                            .expect("cover resident state poisoned")
                            .insert((key_page, index));
                        debug!(?key_page, index, %title, "cover applied");
                    } else {
                        warn!(?key_page, index, %title, "cover row missing when applying image");
                    }
                });
            });
        }
    }
}

fn page_games(window: &MainWindow, page: Page) -> ModelRc<GameCardData> {
    match page {
        Page::Home => window.global::<HomeState>().get_games(),
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
