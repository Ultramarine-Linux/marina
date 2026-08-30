use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

use crate::Platform;

impl From<Rom> for marina_core::LibraryItem {
    fn from(rom: Rom) -> Self {
        let title = rom.name.unwrap_or(rom.files.fs_name);
        let id = marina_core::LibraryItemId::from_provider("romm", "rom", &rom.id.to_string());
        let mut item = marina_core::LibraryItem {
            id,
            title,
            kind: marina_core::ItemKind::Game,
            provider_ids: std::collections::HashMap::new(),
        };
        item.provider_ids
            .insert("romm_id".into(), rom.id.to_string());
        item
    }
}
impl RomResponse {
    pub fn cover_path(&self) -> Option<&str> {
        self.assets
            .path_cover_small
            .as_deref()
            .or(self.assets.path_cover_large.as_deref())
            .or(self.assets.url_cover.as_deref())
    }
}
/// Full response of a ROM entry in RomM
// very messy i know, RomM keeps a LOT of data on these
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RomResponse {
    pub id: i32,
    pub name: Option<String>,
    pub name_sort_key: Option<String>,
    pub slug: Option<String>,
    pub summary: Option<String>,
    #[serde(default)]
    pub alternative_names: Vec<String>,
    pub revision: Option<String>,
    #[serde(default)]
    pub regions: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub is_identifying: Option<bool>,
    pub is_unidentified: Option<bool>,
    pub is_identified: Option<bool>,
    pub created_at: Option<DateTime<FixedOffset>>,
    pub updated_at: Option<DateTime<FixedOffset>>,
    pub missing_from_fs: Option<bool>,
    pub has_notes: Option<bool>,
    #[serde(flatten)]
    pub platform: RomPlatform,
    #[serde(flatten)]
    pub files: RomFiles,
    #[serde(flatten)]
    pub assets: RomAssets,
    #[serde(flatten)]
    pub provider_metadata: RomProviderMetadata,
    #[serde(flatten)]
    pub notes: RomNotes,
    #[serde(flatten)]
    pub hashes: RomHashes,
    #[serde(flatten)]
    pub saves: RomSaves,
    #[serde(flatten)]
    pub savestates: RomSavestates,
    #[serde(flatten)]
    pub siblings: RomSiblings,
}
pub type Rom = RomResponse;
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RomPlatform {
    pub platform_id: i32,
    pub platform_slug: String,
    pub platform_fs_slug: String,
    pub platform_custom_name: Option<String>,
    pub platform_display_name: Option<String>,
}

impl RomPlatform {
    async fn get_platform(&self) -> Platform {
        todo!()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RomFiles {
    pub fs_name: String,
    pub fs_name_no_tags: String,
    pub fs_name_no_ext: String,
    pub fs_extension: String,
    pub fs_path: String,
    pub fs_size_bytes: i64,
    pub full_path: String,
    pub has_simple_single_file: bool,
    pub has_nested_single_file: bool,
    pub has_multiple_files: bool,
    #[serde(default)]
    pub files: Vec<RomFile>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RomFile {
    pub id: i32,
    pub rom_id: i32,
    pub file_name: String,
    pub file_path: String,
    pub file_size_bytes: i64,
    pub full_path: String,
    pub is_top_level: bool,
    pub created_at: DateTime<FixedOffset>,
    pub updated_at: DateTime<FixedOffset>,
    pub last_modified: String,
    #[serde(flatten)]
    pub hashes: RomHashes,
    #[serde(default)]
    pub archive_members: Vec<serde_json::Value>,
    pub category: String,
    pub track_meta: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RomAssets {
    pub path_cover_small: Option<String>,
    pub path_cover_large: Option<String>,
    pub url_cover: Option<String>,
    pub has_manual: Option<bool>,
    pub has_soundtrack: Option<bool>,
    pub path_manual: Option<String>,
    pub url_manual: Option<String>,
    pub path_video: Option<String>,
    #[serde(default)]
    pub merged_screenshots: Vec<String>,
    #[serde(default)]
    pub user_screenshots: Vec<serde_json::Value>,
    #[serde(default)]
    pub all_user_screenshots: Vec<serde_json::Value>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RomProviderMetadata {
    pub youtube_video_id: Option<String>,
    pub metadatum: Option<serde_json::Value>,
    pub igdb_metadata: Option<serde_json::Value>,
    pub moby_metadata: Option<serde_json::Value>,
    pub ss_metadata: Option<serde_json::Value>,
    pub launchbox_metadata: Option<serde_json::Value>,
    pub hasheous_metadata: Option<serde_json::Value>,
    pub flashpoint_metadata: Option<serde_json::Value>,
    pub hltb_metadata: Option<serde_json::Value>,
    pub gamelist_metadata: Option<serde_json::Value>,
    pub manual_metadata: Option<serde_json::Value>,
    pub rom_user: Option<serde_json::Value>,
    pub merged_ra_metadata: Option<serde_json::Value>,
    pub igdb_id: Option<serde_json::Value>,
    pub sgdb_id: Option<serde_json::Value>,
    pub moby_id: Option<serde_json::Value>,
    pub ss_id: Option<serde_json::Value>,
    pub ra_id: Option<serde_json::Value>,
    pub launchbox_id: Option<serde_json::Value>,
    pub hasheous_id: Option<serde_json::Value>,
    pub tgdb_id: Option<serde_json::Value>,
    pub flashpoint_id: Option<serde_json::Value>,
    pub hltb_id: Option<serde_json::Value>,
    pub gamelist_id: Option<serde_json::Value>,
    pub libretro_id: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RomHashes {
    pub crc_hash: Option<String>,
    pub md5_hash: Option<String>,
    pub sha1_hash: Option<String>,
    pub ra_hash: Option<String>,
    pub chd_sha1_hash: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RomSaves {
    pub user_saves: Option<serde_json::Value>,
    pub all_user_saves: Option<serde_json::Value>,
    pub user_collections: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RomNotes {
    pub all_user_notes: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RomSavestates {
    pub user_states: Option<serde_json::Value>,
    pub all_user_states: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RomSibling {
    pub id: i32,
    pub name: String,
    pub fs_name_no_tags: String,
    pub fs_name_no_ext: String,
    pub is_main_sibling: bool,
    pub sort_comparator: String,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RomSiblings {
    pub sibling_roms: Option<Vec<RomSibling>>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RomPage {
    pub items: Vec<Rom>,
    pub total: Option<i64>,
    pub limit: i64,
    pub offset: i64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RomQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_term: Option<String>,
    #[serde(skip)]
    pub platform_ids: Vec<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub virtual_collection_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smart_collection_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub favorite: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_played: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct RomQueryBuilder {
    query: RomQuery,
}

impl RomQueryBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn search_term(mut self, value: impl Into<String>) -> Self {
        self.query.search_term = Some(value.into());
        self
    }

    pub fn platform_id(mut self, value: i64) -> Self {
        self.query.platform_ids.push(value);
        self
    }

    pub fn platform_ids(mut self, values: impl IntoIterator<Item = i64>) -> Self {
        self.query.platform_ids.extend(values);
        self
    }

    pub fn collection_id(mut self, value: i64) -> Self {
        self.query.collection_id = Some(value);
        self
    }

    pub fn virtual_collection_id(mut self, value: impl Into<String>) -> Self {
        self.query.virtual_collection_id = Some(value.into());
        self
    }

    pub fn smart_collection_id(mut self, value: i64) -> Self {
        self.query.smart_collection_id = Some(value);
        self
    }

    pub fn matched(mut self, value: bool) -> Self {
        self.query.matched = Some(value);
        self
    }

    pub fn favorite(mut self, value: bool) -> Self {
        self.query.favorite = Some(value);
        self
    }

    pub fn last_played(mut self, value: bool) -> Self {
        self.query.last_played = Some(value);
        self
    }

    pub fn playable(mut self, value: bool) -> Self {
        self.query.playable = Some(value);
        self
    }

    pub fn missing(mut self, value: bool) -> Self {
        self.query.missing = Some(value);
        self
    }

    pub fn limit(mut self, value: i64) -> Self {
        self.query.limit = Some(value);
        self
    }

    pub fn offset(mut self, value: i64) -> Self {
        self.query.offset = Some(value);
        self
    }

    pub fn build(self) -> RomQuery {
        self.query
    }
}
