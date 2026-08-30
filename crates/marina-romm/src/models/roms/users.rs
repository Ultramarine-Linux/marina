use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

use super::super::metadata::*;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RomSaves {
    pub user_saves: Option<Vec<Save>>,
    pub all_user_saves: Option<Vec<UserSave>>,
    pub user_collections: Option<Vec<UserCollection>>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RomNotes {
    pub all_user_notes: Option<Vec<UserNote>>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RomSavestates {
    pub user_states: Option<Vec<State>>,
    pub all_user_states: Option<Vec<UserState>>,
}
