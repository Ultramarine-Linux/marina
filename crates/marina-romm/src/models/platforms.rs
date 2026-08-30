use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Platform {
    pub id: i64,
    pub slug: String,
    pub fs_slug: String,
    pub rom_count: i64,
    pub name: String,
    pub custom_name: Option<String>,
    pub category: Option<String>,
    // RomM uses -1 for platforms without a generation.
    pub generation: Option<i64>,
    pub family_name: Option<String>,
    pub family_slug: Option<String>,
    pub url: Option<String>,
    pub url_logo: Option<String>,
    pub created_at: DateTime<FixedOffset>,
    pub updated_at: DateTime<FixedOffset>,
    pub fs_size_bytes: i64,
    pub is_unidentified: bool,
    pub is_identified: bool,
    pub missing_from_fs: bool,
    pub display_name: String,
    pub firmware_count: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PlatformQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_after: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct PlatformQueryBuilder {
    query: PlatformQuery,
}

impl PlatformQueryBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn updated_after(mut self, value: impl Into<String>) -> Self {
        self.query.updated_after = Some(value.into());
        self
    }

    pub fn build(self) -> PlatformQuery {
        self.query
    }
}

#[cfg(test)]
mod tests {
    use super::Platform;

    #[test]
    fn accepts_romm_generation_sentinel() {
        let platform: Platform = serde_json::from_str(
            r#"{
                "id": 1,
                "slug": "arcade",
                "fs_slug": "arcade",
                "rom_count": 0,
                "name": "Arcade",
                "custom_name": null,
                "category": "arcade",
                "generation": -1,
                "family_name": null,
                "family_slug": null,
                "url": null,
                "url_logo": null,
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z",
                "fs_size_bytes": 0,
                "is_unidentified": false,
                "is_identified": true,
                "missing_from_fs": false,
                "display_name": "Arcade",
                "firmware_count": 0
            }"#,
        )
        .expect("RomM platform with generation=-1 should deserialize");

        assert_eq!(platform.generation, Some(-1));
    }
}
