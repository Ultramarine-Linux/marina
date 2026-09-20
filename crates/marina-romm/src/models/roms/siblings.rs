use serde::{Deserialize, Serialize};

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
