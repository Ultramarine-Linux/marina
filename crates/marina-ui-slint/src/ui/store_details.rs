//! RomM detail projection for the store page.

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::{
    MainWindow, PreviewDetailsData, StoreArtifact, StoreState, covers, image, string_model,
};

pub(crate) fn populate_store_details(
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
    let selected_rom_id = crate::ui::pages::store::card_id("romm", &rom_id);
    let details_card_id = selected_rom_id.clone();
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
            .is_none_or(|game| game.id.as_str() != details_card_id)
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
    let preview_rom_id = selected_rom_id.clone();
    let row_rom_id = selected_rom_id.clone();
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
