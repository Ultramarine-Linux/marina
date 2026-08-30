use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

use super::super::metadata::*;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RomProviderMetadata {
    pub youtube_video_id: Option<String>,
    pub metadatum: Option<RomMetadata>,
    pub igdb_metadata: Option<IgdbMetadata>,
    pub moby_metadata: Option<MobyMetadata>,
    pub ss_metadata: Option<SsMetadata>,
    pub launchbox_metadata: Option<LaunchboxMetadata>,
    pub hasheous_metadata: Option<HasheousMetadata>,
    pub flashpoint_metadata: Option<FlashpointMetadata>,
    pub hltb_metadata: Option<HltbMetadata>,
    pub gamelist_metadata: Option<GamelistMetadata>,
    pub manual_metadata: Option<ManualMetadata>,
    pub rom_user: Option<RomUser>,
    pub merged_ra_metadata: Option<RaMetadata>,
    pub igdb_id: Option<i32>,
    pub sgdb_id: Option<i32>,
    pub moby_id: Option<i32>,
    pub ss_id: Option<i32>,
    pub ra_id: Option<i32>,
    pub launchbox_id: Option<i32>,
    pub hasheous_id: Option<i32>,
    pub tgdb_id: Option<i32>,
    pub flashpoint_id: Option<i32>,
    pub hltb_id: Option<i32>,
    pub gamelist_id: Option<i32>,
    pub libretro_id: Option<i32>,
}
