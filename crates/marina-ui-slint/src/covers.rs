//! Page-scoped artwork loading and decoding.

use std::{
    collections::HashSet,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use marina_library::{
    error::LibraryError,
    query::{SearchQuery, SearchSort},
    read::LibraryRead,
};
use slint::{ComponentHandle, Image, Model, ModelRc, SharedPixelBuffer};
use tracing::{debug, warn};

use crate::{GameCardData, HomeState, MainWindow, cache};

#[derive(Clone)]
pub struct CoverSource {
    pub url: Option<String>,
    pub cache_path: Option<PathBuf>,
    pub local_path: Option<PathBuf>,
}

pub fn source_for(
    cover: Option<&str>,
    local_path: Option<&str>,
    base_url: Option<&str>,
) -> CoverSource {
    let cover = cover.and_then(normalize_cover_source);
    CoverSource {
        url: resolve_url(cover.as_deref(), base_url),
        cache_path: cover.as_deref().and_then(cache::cover_cache_path),
        local_path: local_path.map(PathBuf::from),
    }
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
                let decoded = match load_bytes(&source).await {
                    Some(bytes) => {
                        debug!(?key_page, index, %title, bytes = bytes.len(), "cover bytes loaded");
                        tokio::task::spawn_blocking(move || decode_rgba(&bytes))
                            .await
                            .ok()
                            .flatten()
                    }
                    None => {
                        warn!(?key_page, index, %title, "cover source produced no bytes");
                        None
                    }
                };
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
                        let (image, ratio) = image_from_rgba(decoded);
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

pub struct DecodedImage {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub fn decode(bytes: &[u8]) -> Option<(Image, f32)> {
    decode_rgba(bytes).map(image_from_rgba)
}

pub async fn decode_pixels(bytes: Vec<u8>) -> Option<DecodedImage> {
    tokio::task::spawn_blocking(move || decode_rgba(&bytes))
        .await
        .ok()
        .flatten()
}

fn decode_rgba(bytes: &[u8]) -> Option<DecodedImage> {
    let svg_prefix = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]);
    if svg_prefix.contains("<svg") {
        let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default()).ok()?;
        let size = tree.size().to_int_size();
        let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height())?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::default(),
            &mut pixmap.as_mut(),
        );
        return Some(DecodedImage {
            pixels: pixmap.data().to_vec(),
            width: pixmap.width(),
            height: pixmap.height(),
        });
    }
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    (width > 0 && height > 0).then(|| DecodedImage {
        pixels: img.into_raw(),
        width,
        height,
    })
}

pub fn image_from_rgba(decoded: DecodedImage) -> (Image, f32) {
    let buffer = SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        &decoded.pixels,
        decoded.width,
        decoded.height,
    );
    (
        Image::from_rgba8(buffer),
        decoded.width as f32 / decoded.height as f32,
    )
}

#[tracing::instrument(skip(source), fields(loader = "covers"))]
pub(crate) async fn load_bytes(source: &CoverSource) -> Option<Vec<u8>> {
    if let Some(path) = &source.local_path {
        if let Ok(bytes) = tokio::fs::read(path).await {
            debug!(path = %path.display(), bytes = bytes.len(), "local cover hit");
            return Some(bytes);
        }
        debug!(path = %path.display(), "local cover miss");
    }
    if let Some(path) = &source.cache_path {
        if let Ok(bytes) = tokio::fs::read(path).await {
            debug!(path = %path.display(), bytes = bytes.len(), "cover cache hit");
            return Some(bytes);
        }
        debug!(path = %path.display(), "cover cache miss");
    }
    let url = source.url.as_deref()?;
    static HTTP: OnceLock<reqwest::Client> = OnceLock::new();
    let bytes = HTTP
        .get_or_init(reqwest::Client::new)
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .await
        .ok()?;
    if let Some(path) = &source.cache_path {
        if let Some(parent) = path.parent() {
            if tokio::fs::create_dir_all(parent).await.is_ok() {
                let _ = tokio::fs::write(path, &bytes).await;
            }
        }
    }
    Some(bytes.to_vec())
}

fn resolve_url(cover: Option<&str>, base_url: Option<&str>) -> Option<String> {
    let cover = cover?;
    if cover.starts_with("http://") || cover.starts_with("https://") {
        return Some(cover.to_owned());
    }
    let cover = cover.trim_matches('/');
    let base_url = base_url?.trim_end_matches('/');
    (!cover.is_empty()).then(|| format!("{base_url}/{cover}"))
}

fn normalize_cover_source(cover: &str) -> Option<String> {
    let cover = cover.trim();
    let cover = cover
        .strip_prefix("@url:`")
        .and_then(|value| value.strip_suffix('`'))
        .unwrap_or(cover)
        .trim();
    (!cover.is_empty()).then(|| cover.to_owned())
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
