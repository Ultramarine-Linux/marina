use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

use super::super::metadata::*;
use super::hashes::RomHashes;

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
    pub archive_members: Option<Vec<ArchiveMember>>,
    pub category: Option<String>,
    pub track_meta: Option<TrackMetadata>,
}
