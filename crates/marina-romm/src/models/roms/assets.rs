use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

use super::super::metadata::*;

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
    pub user_screenshots: Vec<Screenshot>,
    #[serde(default)]
    pub all_user_screenshots: Vec<UserScreenshot>,
}
