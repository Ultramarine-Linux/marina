//! A "ROM" in RomM.
//!
//!
//! An invididual ROM contains a LOT of metadata so it's useful to have a typed
//! representation of it available to callers.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct RomMetadata {
    pub rom_id: i32,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub franchises: Vec<String>,
    #[serde(default)]
    pub collections: Vec<String>,
    #[serde(default)]
    pub companies: Vec<String>,
    #[serde(default)]
    pub game_modes: Vec<String>,
    #[serde(default)]
    pub age_ratings: Vec<String>,
    pub player_count: String,
    pub first_release_date: Option<i64>,
    pub average_rating: Option<f64>,
}

/// Age rating according to IGDB.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct IgdbAgeRating {
    pub rating: String,
    pub category: String,
    pub rating_cover_url: String,
}

/// Platform according to IGDB.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct IgdbPlatform {
    pub igdb_id: i32,
    pub name: String,
}

/// Multiplayer mode according to IGDB.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct IgdbMultiplayerMode {
    pub campaigncoop: bool,
    pub dropin: bool,
    pub lancoop: bool,
    pub offlinecoop: bool,
    pub offlinecoopmax: i32,
    pub offlinemax: i32,
    pub onlinecoop: i32,
    pub onlinecoopmax: i32,
    pub onlinemax: i32,
    pub splitscreen: bool,
    pub splitscreenonline: bool,
    pub platform: IgdbPlatform,
}
/// Related game according to IGDB.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct IgdbRelatedGame {
    pub id: i32,
    pub name: String,
    pub slug: String,
    r#type: String,
    pub cover_url: String,
}
/// Metadata according to IGDB.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct IgdbMetadata {
    pub total_rating: Option<String>,
    pub aggregated_rating: Option<String>,
    pub first_release_date: Option<i64>,
    pub youtube_video_id: Option<String>,
    pub genres: Vec<String>,
    pub franchises: Vec<String>,
    pub alternative_names: Vec<String>,
    pub collections: Vec<String>,
    pub companies: Vec<String>,
    pub game_modes: Vec<String>,
    pub age_ratings: Vec<IgdbAgeRating>,
    pub platforms: Vec<IgdbPlatform>,
    pub multiplayer_modes: Vec<IgdbMultiplayerMode>,
    pub player_count: String,
    pub expansions: Vec<IgdbRelatedGame>,
    pub dlcs: Vec<IgdbRelatedGame>,
    pub remasters: Vec<IgdbRelatedGame>,
    pub remakes: Vec<IgdbRelatedGame>,
    pub expanded_games: Vec<IgdbRelatedGame>,
    pub ports: Vec<IgdbRelatedGame>,
    pub similar_games: Vec<IgdbRelatedGame>,
}

/// Platform according to MobyGames.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct MobyPlatform {
    pub moby_id: i32,
    pub name: String,
}
/// Metadata according to MobyGames.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct MobyMetadata {
    pub moby_score: Option<String>,
    pub genres: Vec<String>,
    pub alternate_titles: Vec<String>,
    pub platforms: Vec<MobyPlatform>,
}

/// Age rating according to ScreenScraper
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct SsAgeRating {
    pub rating: String,
    pub category: String,
}

/// Metadata according to ScreenScraper.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct SsMetadata {
    pub bezel_url: Option<String>,
    pub box2d_url: Option<String>,
    pub box2d_side_url: Option<String>,
    pub box2d_back_url: Option<String>,
    pub box3d_url: Option<String>,
    pub fanart_url: Option<String>,
    pub fullbox_url: Option<String>,
    pub logo_url: Option<String>,
    pub manual_url: Option<String>,
    pub marquee_url: Option<String>,
    pub miximage_url: Option<String>,
    pub miximage_v2_url: Option<String>,
    pub physical_url: Option<String>,
    pub screenshot_url: Option<String>,
    pub steamgrid_url: Option<String>,
    pub title_screen_url: Option<String>,
    pub video_url: Option<String>,
    pub video_normalized_url: Option<String>,
    pub bezel_path: Option<String>,
    pub box2d_path: Option<String>,
    pub box2d_back_path: Option<String>,
    pub box2d_side_path: Option<String>,
    pub box3d_path: Option<String>,
    pub fanart_path: Option<String>,
    pub miximage_path: Option<String>,
    pub miximage_v2_path: Option<String>,
    pub physical_path: Option<String>,
    pub marquee_path: Option<String>,
    pub logo_path: Option<String>,
    pub title_screen_path: Option<String>,
    pub video_path: Option<String>,
    pub video_normalized_path: Option<String>,
    pub ss_score: Option<String>,
    pub first_release_date: Option<i64>,
    pub alternative_names: Vec<String>,
    pub age_ratings: Vec<SsAgeRating>,
    pub companies: Vec<String>,
    pub franchises: Vec<String>,
    pub game_modes: Vec<String>,
    pub genres: Vec<String>,
    pub player_count: String,
}

/// Image from Launchbox
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct LaunchboxImage {
    pub url: String,
    r#type: String,
    pub region: String,
}

/// Metadata from Launchbox.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct LaunchboxMetadata {
    pub first_release_date: Option<i64>,
    pub max_players: i32,
    pub release_type: String,
    pub cooperative: bool,
    pub youtube_video_id: String,
    pub community_rating: f64,
    pub community_rating_count: i32,
    pub wikipedia_url: String,
    pub esrb: String,
    pub genres: Vec<String>,
    pub companies: Vec<String>,
    pub images: Vec<LaunchboxImage>,
    pub video_url: String,
    pub video_path: String,
}

/// Metadata from Hasheous.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct HasheousMetadata {
    pub tosec_match: bool,
    pub mame_arcade_match: bool,
    pub mame_mess_match: bool,
    pub nointro_match: bool,
    pub redump_match: bool,
    pub mame_redump_match: bool,
    pub whdload_match: bool,
    pub ra_match: bool,
    pub fbneo_match: bool,
    pub puredos_match: bool,
}

/// Metadata from the Flashpoint database.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct FlashpointMetadata {
    pub franchises: Vec<String>,
    pub companies: Vec<String>,
    pub source: Option<String>,
    pub genres: Vec<String>,
    pub first_release_date: String,
    pub game_modes: Vec<String>,
    pub status: Option<String>,
    pub version: Option<String>,
    pub language: Option<String>,
    pub notes: Option<String>,
}
/// Metadata from How Long to Beat.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct HltbMetadata {
    pub main_story: i32,
    pub main_story_count: i32,
    pub main_plus_extra: i32,
    pub main_plus_extra_count: i32,
    pub completionist: i32,
    pub completionist_count: i32,
    pub all_styles: i32,
    pub all_styles_count: i32,
    pub release_year: i32,
    pub review_score: i32,
    pub review_count: i32,
    pub popularity: i32,
    pub completions: i32,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct GamelistMetadata {
    pub box2d_url: Option<String>,
    pub box2d_back_url: Option<String>,
    pub box3d_url: Option<String>,
    pub fanart_url: Option<String>,
    pub image_url: Option<String>,
    pub manual_url: Option<String>,
    pub marquee_url: Option<String>,
    pub miximage_url: Option<String>,
    pub miximage_v2_url: Option<String>,
    pub physical_url: Option<String>,
    pub screenshot_url: Option<String>,
    pub thumbnail_url: Option<String>,
    pub title_screen_url: Option<String>,
    pub video_url: Option<String>,
    pub rating: Option<f64>,
    pub first_release_date: Option<String>,
    pub sort_name: Option<String>,
    pub companies: Option<Vec<String>>,
    pub franchises: Option<Vec<String>>,
    pub genres: Option<Vec<String>>,
    pub player_count: Option<String>,
    pub md5_hash: Option<String>,
    pub box2d_back_path: Option<String>,
    pub box3d_path: Option<String>,
    pub fanart_path: Option<String>,
    pub miximage_path: Option<String>,
    pub miximage_v2_path: Option<String>,
    pub physical_path: Option<String>,
    pub marquee_path: Option<String>,
    pub title_screen_path: Option<String>,
    pub video_path: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct ManualMetadata {
    pub genres: Option<Vec<String>>,
    pub franchises: Option<Vec<String>>,
    pub companies: Option<Vec<String>>,
    pub game_modes: Option<Vec<String>>,
    pub age_ratings: Option<Vec<String>>,
    pub first_release_date: Option<i64>,
    pub youtube_video_id: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct RaMetadata {
    pub first_release_date: Option<i64>,
    pub genres: Vec<String>,
    pub companies: Vec<String>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct ArchiveMember {
    pub name: String,
    pub size: i64,
    pub crc_hash: String,
    pub md5_hash: String,
    pub sha1_hash: String,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct TrackMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub track: Option<i32>,
    pub disc: Option<i32>,
    pub duration_seconds: Option<f64>,
    pub has_embedded_cover: bool,
    pub cover_path: Option<String>,
}

/// A file-backed object embedded in a detailed ROM response.  RomM uses the
/// same shape for screenshots and save/state files; the enriched variants add
/// the owner fields below.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Screenshot {
    pub id: i32,
    pub rom_id: i32,
    pub user_id: i32,
    pub file_name: String,
    pub file_name_no_tags: String,
    pub file_name_no_ext: String,
    pub file_extension: String,
    pub file_path: String,
    pub file_size_bytes: i64,
    pub full_path: String,
    pub download_path: String,
    pub missing_from_fs: bool,
    pub created_at: String,
    pub updated_at: String,
    pub is_gallery: bool,
    pub is_public: bool,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct UserScreenshot {
    pub id: i32,
    pub rom_id: i32,
    pub user_id: i32,
    pub file_name: String,
    pub file_name_no_tags: String,
    pub file_name_no_ext: String,
    pub file_extension: String,
    pub file_path: String,
    pub file_size_bytes: i64,
    pub full_path: String,
    pub download_path: String,
    pub missing_from_fs: bool,
    pub created_at: String,
    pub updated_at: String,
    pub is_gallery: bool,
    pub is_public: bool,
    pub username: String,
    pub user_avatar_path: String,
    pub user_updated_at: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct UserNote {
    pub id: i32,
    pub title: String,
    pub content: String,
    pub is_public: bool,
    pub tags: Option<Vec<String>>,
    pub created_at: String,
    pub updated_at: String,
    pub user_id: i32,
    pub username: String,
    pub user_avatar_path: String,
    pub user_updated_at: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct UserCollection {
    pub id: i32,
    pub name: String,
    pub is_smart: bool,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RomUser {
    pub id: i32,
    pub user_id: i32,
    pub rom_id: i32,
    pub created_at: String,
    pub updated_at: String,
    pub last_played: Option<String>,
    pub is_main_sibling: bool,
    pub backlogged: bool,
    pub now_playing: bool,
    pub hidden: bool,
    pub rating: i32,
    pub difficulty: i32,
    pub completion: i32,
    pub status: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Save {
    pub id: i32,
    pub rom_id: i32,
    pub user_id: i32,
    pub file_name: String,
    pub file_name_no_tags: String,
    pub file_name_no_ext: String,
    pub file_extension: String,
    pub file_path: String,
    pub file_size_bytes: i64,
    pub full_path: String,
    pub download_path: String,
    pub missing_from_fs: bool,
    pub created_at: String,
    pub updated_at: String,
    pub emulator: Option<String>,
    pub slot: Option<String>,
    pub content_hash: Option<String>,
    pub is_public: bool,
    pub screenshot: Option<Screenshot>,
    pub origin_device_id: Option<String>,
}
/// State files have the same wire shape as saves, without save slots/hashes.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct State {
    pub id: i32,
    pub rom_id: i32,
    pub user_id: i32,
    pub file_name: String,
    pub file_name_no_tags: String,
    pub file_name_no_ext: String,
    pub file_extension: String,
    pub file_path: String,
    pub file_size_bytes: i64,
    pub full_path: String,
    pub download_path: String,
    pub missing_from_fs: bool,
    pub created_at: String,
    pub updated_at: String,
    pub emulator: Option<String>,
    pub is_public: bool,
    pub screenshot: Option<Screenshot>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct UserSave {
    pub id: i32,
    pub rom_id: i32,
    pub user_id: i32,
    pub file_name: String,
    pub file_name_no_tags: String,
    pub file_name_no_ext: String,
    pub file_extension: String,
    pub file_path: String,
    pub file_size_bytes: i64,
    pub full_path: String,
    pub download_path: String,
    pub missing_from_fs: bool,
    pub created_at: String,
    pub updated_at: String,
    pub emulator: Option<String>,
    pub slot: Option<String>,
    pub content_hash: Option<String>,
    pub is_public: bool,
    pub screenshot: Option<Screenshot>,
    pub origin_device_id: Option<String>,
    pub username: String,
    pub user_avatar_path: String,
    pub user_updated_at: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct UserState {
    pub id: i32,
    pub rom_id: i32,
    pub user_id: i32,
    pub file_name: String,
    pub file_name_no_tags: String,
    pub file_name_no_ext: String,
    pub file_extension: String,
    pub file_path: String,
    pub file_size_bytes: i64,
    pub full_path: String,
    pub download_path: String,
    pub missing_from_fs: bool,
    pub created_at: String,
    pub updated_at: String,
    pub emulator: Option<String>,
    pub is_public: bool,
    pub screenshot: Option<Screenshot>,
    pub username: String,
    pub user_avatar_path: String,
    pub user_updated_at: Option<String>,
}
