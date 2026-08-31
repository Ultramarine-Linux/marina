//! Bounded cover residency: cache/network loading plus visible-range eviction.

use std::{
    collections::HashSet,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
};

use marina_library::{LibraryError, LibraryRead};
use slint::{ComponentHandle, Image, Model, SharedPixelBuffer};
use tracing::{debug, warn};

use crate::{GameCardData, MainWindow, cache};

const COVER_HEIGHT: f32 = 200.0;
const CARD_SPACING: f32 = 16.0;
const CONTENT_PADDING_LEFT: f32 = 4.0;
const PREFETCH_CARDS: usize = 3;

#[derive(Clone)]
pub struct CoverSource {
    pub url: Option<String>,
    pub cache_path: Option<PathBuf>,
}

pub fn source_for(cover: Option<&str>, base_url: Option<&str>) -> CoverSource {
    CoverSource {
        url: resolve_url(cover, base_url),
        cache_path: cover.and_then(cache::cover_cache_path),
    }
}

/// Loads metadata only; artwork is requested by `CoverLoader` as cards enter
/// the viewport plus a small prefetch buffer.
pub fn spawn_loader(
    window: &MainWindow,
    http: reqwest::Client,
    sources: Vec<CoverSource>,
) -> Rc<std::cell::RefCell<CoverLoader>> {
    let loader = Rc::new(std::cell::RefCell::new(CoverLoader {
        window: window.as_weak(),
        http,
        sources,
        state: Arc::new(Mutex::new(LoaderState::default())),
    }));
    loader
}

pub struct CoverLoader {
    window: slint::Weak<MainWindow>,
    http: reqwest::Client,
    sources: Vec<CoverSource>,
    state: Arc<Mutex<LoaderState>>,
}

#[derive(Default)]
struct LoaderState {
    loading: HashSet<usize>,
    resident: HashSet<usize>,
}

impl CoverLoader {
    pub fn update(&mut self, scroll_x: f32, viewport_width: f32) {
        // Slint exposes Flickable::viewport-x as the content translation, so
        // scrolling right produces negative values. Convert to a positive
        // distance through the content before calculating card positions.
        let scroll_x = (-scroll_x).max(0.0);
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let games = window.get_games();

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
            .map(|index| (index + PREFETCH_CARDS + 1).min(self.sources.len()))
            .unwrap_or(0);
        let wanted: HashSet<_> = (first..last).collect();
        debug!(
            scroll_x,
            viewport_width, first, last, "updating cover residency range"
        );
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
            let Some(source) = self.sources.get(index).cloned() else {
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

            tokio::spawn(async move {
                let Some(bytes) = load_bytes(&http, &source).await else {
                    state
                        .lock()
                        .expect("cover loader state poisoned")
                        .loading
                        .remove(&index);
                    return;
                };
                {
                    let mut state = state.lock().expect("cover loader state poisoned");
                    state.loading.remove(&index);
                    state.resident.insert(index);
                }
                debug!(index, "cover bytes loaded");
                let _ = weak.upgrade_in_event_loop(move |window| {
                    let Some((image, ratio)) = decode(&bytes) else {
                        warn!("cover decode failed, keeping placeholder");
                        return;
                    };
                    let games = window.get_games();
                    if let Some(mut row) = games.row_data(index) {
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

async fn load_bytes(http: &reqwest::Client, source: &CoverSource) -> Option<Vec<u8>> {
    if let Some(path) = &source.cache_path {
        if let Ok(bytes) = tokio::fs::read(path).await {
            debug!(path = %path.display(), "cover cache hit");
            return Some(bytes);
        }
    }
    let url = source.url.as_deref()?;
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
    Some(format!("{}/{cover}", base_url?.trim_end_matches('/')))
}

pub async fn load_games_metadata(
    library: &marina_store_surrealdb::SurrealLibrary,
    base_url: Option<&str>,
) -> Result<(Vec<GameCardData>, Vec<CoverSource>), LibraryError> {
    let items = library.list(100).await?;
    Ok(items
        .into_iter()
        .map(|item| {
            let source = source_for(item.cover.as_deref(), base_url);
            let card = GameCardData {
                title: item.title.into(),
                platform: item
                    .platform_slug
                    .unwrap_or_else(|| "Unknown".into())
                    .into(),
                cover: Image::default(),
                cover_ratio: 1.0,
            };
            (card, source)
        })
        .unzip())
}
