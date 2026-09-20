use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct RomHashes {
    pub crc_hash: Option<String>,
    pub md5_hash: Option<String>,
    pub sha1_hash: Option<String>,
    pub ra_hash: Option<String>,
    pub chd_sha1_hash: Option<String>,
}
