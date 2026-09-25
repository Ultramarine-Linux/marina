//! Rust adapters for Slint UI data structures.

use slint::{Image, ModelRc, SharedString, VecModel};

use crate::ui::pages::library as shelf;
use crate::{GameCardData, PreviewDetailsData};

pub(crate) struct PlatformCardMetadata {
    pub(crate) slug: String,
    pub(crate) name: String,
    pub(crate) game_count: String,
    pub(crate) icon_path: Option<String>,
}

pub(crate) fn empty_game_card() -> GameCardData {
    GameCardData {
        id: SharedString::default(),
        title: SharedString::default(),
        platform: SharedString::default(),
        cover: Image::default(),
        cover_ratio: 1.0,
    }
}

pub(crate) fn game_cards(metadata: Vec<shelf::GameMetadata>) -> Vec<GameCardData> {
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

pub(crate) fn preview_details(item: marina_core::LibraryItem) -> PreviewDetailsData {
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

pub(crate) fn string_model(values: Vec<String>) -> ModelRc<SharedString> {
    ModelRc::from(std::rc::Rc::new(VecModel::from(
        values
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )))
}

pub(crate) fn platform_asset_path(root: &std::path::Path, slug: &str) -> Option<String> {
    let exact_name = match slug {
        "ndsi" => "nintendo-dsi",
        "win" => "pc-50x-family",
        _ => slug,
    };
    let exact_path = root.join(format!("platform-{exact_name}.svg"));
    if let Some(path) = marina_apps::resolve_icon(&exact_path.to_string_lossy()) {
        return Some(path);
    }

    if let Some(prefix) = slug.split('-').next() {
        let prefix_path = root.join(format!("platform-{prefix}.svg"));
        if let Some(path) = marina_apps::resolve_icon(&prefix_path.to_string_lossy()) {
            return Some(path);
        }
    }

    let default_path = root.join("platform-default.svg");
    marina_apps::resolve_icon(&default_path.to_string_lossy())
}
