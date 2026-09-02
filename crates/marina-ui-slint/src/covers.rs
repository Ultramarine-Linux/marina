//! Bounded cover residency: cache/network loading plus visible-range eviction.

use std::{
    collections::HashSet,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
};

use marina_library::{
    error::LibraryError,
    query::{SearchQuery, SearchSort},
    read::LibraryRead,
};
use slint::{ComponentHandle, Image, Model, SharedPixelBuffer};
use tracing::{debug, warn};

use crate::{MainWindow, cache};

const COVER_HEIGHT: f32 = 200.0;
const CARD_SPACING: f32 = 16.0;
const CONTENT_PADDING_LEFT: f32 = 4.0;
const PREFETCH_CARDS: usize = 3;

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

/// Loads metadata only; artwork is requested by `CoverLoader` as cards enter
/// the viewport plus a small prefetch buffer.
pub fn spawn_loader(
    window: &MainWindow,
    http: reqwest::Client,
    sources: Vec<CoverSource>,
) -> (
    Rc<std::cell::RefCell<CoverLoader>>,
    Arc<Mutex<Vec<CoverSource>>>,
) {
    let sources = Arc::new(Mutex::new(sources));
    let loader = Rc::new(std::cell::RefCell::new(CoverLoader {
        window: window.as_weak(),
        http,
        sources: sources.clone(),
        state: Arc::new(Mutex::new(LoaderState::default())),
        last_viewport: None,
    }));
    (loader, sources)
}

pub struct CoverLoader {
    window: slint::Weak<MainWindow>,
    http: reqwest::Client,
    sources: Arc<Mutex<Vec<CoverSource>>>,
    state: Arc<Mutex<LoaderState>>,
    last_viewport: Option<(f32, f32)>,
}

#[derive(Default)]
struct LoaderState {
    loading: HashSet<usize>,
    resident: HashSet<usize>,
    generation: u64,
}

impl CoverLoader {
    /// Clears residency after replacing the game model with another page.
    #[tracing::instrument(skip(self), fields(loader = "covers"))]
    pub fn reset(&mut self) {
        if let Some(window) = self.window.upgrade() {
            debug!(
                home_rows = window.get_games().row_count(),
                platform_rows = window.get_platform_games().row_count(),
                "resetting cover loader"
            );
            for model in [window.get_games(), window.get_platform_games()] {
                for index in 0..model.row_count() {
                    if let Some(mut row) = model.row_data(index) {
                        row.cover = Image::default();
                        model.set_row_data(index, row);
                    }
                }
            }
        }
        let mut state = self.state.lock().expect("cover loader state poisoned");
        state.loading.clear();
        state.resident.clear();
        state.generation = state.generation.wrapping_add(1);
        debug!("cover loader state cleared");
    }

    #[tracing::instrument(skip(self), fields(loader = "covers"))]
    pub fn update(&mut self, scroll_x: f32, viewport_width: f32) {
        self.last_viewport = Some((scroll_x, viewport_width));
        // Slint exposes Flickable::viewport-x as the content translation, so
        // scrolling right produces negative values. Convert to a positive
        // distance through the content before calculating card positions.
        let scroll_x = (-scroll_x).max(0.0);
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let games = if window.get_active_tab() == 1 {
            window.get_platform_games()
        } else {
            window.get_games()
        };
        let sources = self
            .sources
            .lock()
            .expect("cover source state poisoned")
            .clone();

        // Cards have variable widths, so derive their actual positions from
        // the current ratios instead of assuming a fixed slot size.
        let viewport_end = scroll_x + viewport_width;
        let mut cursor = CONTENT_PADDING_LEFT;
        let mut first_visible = None;
        let mut last_visible = None;
        for index in 0..games.row_count() {
            let ratio = games
                .row_data(index)
                .map(|row| row.cover_ratio.clamp(0.4, 2.0))
                .unwrap_or(1.0);
            let width = COVER_HEIGHT * ratio;
            let card_end = cursor + width;
            if card_end >= scroll_x && cursor <= viewport_end {
                first_visible.get_or_insert(index);
                last_visible = Some(index);
            }
            cursor = card_end + CARD_SPACING;
        }

        let first = first_visible.unwrap_or(0).saturating_sub(PREFETCH_CARDS);
        let last = last_visible
            .map(|index| (index + PREFETCH_CARDS + 1).min(sources.len()))
            .unwrap_or(0);
        let wanted: HashSet<_> = (first..last).collect();
        let state_snapshot = self.state.lock().expect("cover loader state poisoned");
        debug!(
            scroll_x,
            viewport_width,
            first,
            last,
            active_tab = window.get_active_tab(),
            rows = games.row_count(),
            sources = sources.len(),
            resident = ?state_snapshot.resident,
            loading = ?state_snapshot.loading,
            wanted = ?wanted,
            "updating cover residency range"
        );
        drop(state_snapshot);
        let evict: Vec<_> = self
            .state
            .lock()
            .expect("cover loader state poisoned")
            .resident
            .difference(&wanted)
            .copied()
            .collect();
        debug!(?evict, "evicting cover rows");
        for index in evict {
            let title = games
                .row_data(index)
                .map(|row| row.title.to_string())
                .unwrap_or_else(|| "<missing>".into());
            debug!(index, %title, "evicting cover row");
            if let Some(mut row) = games.row_data(index) {
                row.cover = Image::default();
                games.set_row_data(index, row);
            }
            self.state
                .lock()
                .expect("cover loader state poisoned")
                .resident
                .remove(&index);
        }

        for index in wanted {
            let should_load = {
                let mut state = self.state.lock().expect("cover loader state poisoned");
                !state.resident.contains(&index) && state.loading.insert(index)
            };
            if !should_load {
                continue;
            }
            debug!(index, "queueing cover row");
            let Some(source) = sources.get(index).cloned() else {
                self.state
                    .lock()
                    .expect("cover loader state poisoned")
                    .loading
                    .remove(&index);
                continue;
            };
            let http = self.http.clone();
            let weak = self.window.clone();
            let state = self.state.clone();
            let generation = state
                .lock()
                .expect("cover loader state poisoned")
                .generation;
            let source_kind = if source.local_path.is_some() {
                "local"
            } else if source.cache_path.is_some() {
                "cache-or-url"
            } else if source.url.is_some() {
                "url"
            } else {
                "none"
            };
            debug!(
                index,
                source_kind,
                has_local_path = source.local_path.is_some(),
                has_cache_path = source.cache_path.is_some(),
                has_url = source.url.is_some(),
                "starting cover load"
            );

            tokio::spawn(async move {
                let Some(bytes) = load_bytes(&http, &source).await else {
                    debug!(index, "cover load produced no bytes");
                    state
                        .lock()
                        .expect("cover loader state poisoned")
                        .loading
                        .remove(&index);
                    return;
                };
                {
                    let mut state = state.lock().expect("cover loader state poisoned");
                    if state.generation != generation {
                        debug!(
                            index,
                            generation,
                            current_generation = state.generation,
                            "discarding stale cover load"
                        );
                        state.loading.remove(&index);
                        return;
                    }
                    state.loading.remove(&index);
                    state.resident.insert(index);
                }
                debug!(index, "cover bytes loaded");
                let _ = weak.upgrade_in_event_loop(move |window| {
                    let Some((image, ratio)) = decode(&bytes) else {
                        warn!(index, "cover decode failed, keeping placeholder");
                        return;
                    };
                    let games = if window.get_active_tab() == 1 {
                        window.get_platform_games()
                    } else {
                        window.get_games()
                    };
                    if let Some(mut row) = games.row_data(index) {
                        debug!(index, title = %row.title, "applying decoded cover to row");
                        row.cover = image;
                        row.cover_ratio = ratio;
                        games.set_row_data(index, row);
                    }
                });
            });
        }
    }
}

pub fn decode(bytes: &[u8]) -> Option<(Image, f32)> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let buffer = SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(img.as_raw(), w, h);
    Some((Image::from_rgba8(buffer), w as f32 / h as f32))
}

#[tracing::instrument(skip(http, source), fields(loader = "covers"))]
async fn load_bytes(http: &reqwest::Client, source: &CoverSource) -> Option<Vec<u8>> {
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
    debug!(url, "starting cover URL request");
    let bytes = http
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .await
        .ok()?;
    debug!(bytes = bytes.len(), "cover URL response loaded");

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
        let trimmed = cover.trim_end_matches('/');
        if trimmed == "https:" || trimmed == "http:" {
            return None;
        }
        return Some(cover.to_owned());
    }
    let cover = cover.trim_matches('/');
    if cover.is_empty() {
        return None;
    }
    Some(format!("{}/{cover}", base_url?.trim_end_matches('/')))
}

fn normalize_cover_source(cover: &str) -> Option<String> {
    let cover = cover.trim();
    if cover.is_empty() {
        return None;
    }
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
) -> Result<(Vec<crate::shelf::GameMetadata>, Vec<CoverSource>), LibraryError> {
    let items = library
        .search_cards(SearchQuery::new().sort(SearchSort::LastUpdated).limit(100))
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
            let card = crate::shelf::GameMetadata {
                id: item.id.to_string(),
                title: item.title,
                platform: item.platform_name.unwrap_or_else(|| "Unknown".into()),
            };
            (card, source)
        })
        .unzip())
}
