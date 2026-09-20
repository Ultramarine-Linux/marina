pub mod metadata;
pub mod platforms;
pub mod roms;

pub use platforms::{Platform, PlatformQuery, PlatformQueryBuilder};
pub use roms::{Rom, RomPage, RomQuery, RomQueryBuilder};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SystemInfo {
    #[serde(rename = "VERSION")]
    pub version: String,
    #[serde(rename = "SHOW_SETUP_WIZARD")]
    pub show_setup_wizard: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Heartbeat {
    #[serde(rename = "SYSTEM")]
    pub system: SystemInfo,
}
