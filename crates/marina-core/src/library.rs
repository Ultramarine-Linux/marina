use std::{collections::HashMap, fmt};
use uuid::Uuid;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LibraryItemId(Uuid);

impl Default for LibraryItemId {
    fn default() -> Self {
        Self::new()
    }
}

impl LibraryItemId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn from_provider(provider: &str, entity: &str, id: &str) -> Self {
        let name = format!("marina:{provider}:{entity}:{id}");
        Self(Uuid::new_v5(&Uuid::NAMESPACE_URL, name.as_bytes()))
    }

    /// Parses an ID previously formatted with [`Display`](fmt::Display).
    pub fn parse(value: &str) -> Option<Self> {
        Uuid::parse_str(value).ok().map(Self)
    }

    pub fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl fmt::Display for LibraryItemId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub enum ItemKind {
    #[default]
    Game,
    App,
}

/// a minimal card of a library item, used for quick display in the UI
/// without loading the full item details
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct LibraryCard {
    pub id: LibraryItemId,
    pub title: String,
    pub kind: ItemKind,
    /// full pretty name for platform, e.g. "PC (Steam)", "Game Boy Advance"
    pub platform_name: Option<String>,
    pub regions: Vec<String>,
    pub cover: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct LibraryItem {
    pub id: LibraryItemId,
    pub title: String,
    pub kind: ItemKind,
    /// RomM's filesystem slug, used as Marina's platform/directory ID.
    pub platform_slug: Option<String>,
    // insert more stuff here like uh covers n shit idk
    pub provider_ids: HashMap<String, String>,

    /// A summary/description of the item.
    pub summary: Option<String>,
    pub alternative_names: Vec<String>,
    pub tags: Vec<String>,
    pub languages: Vec<String>,
    pub regions: Vec<String>,

    /// The cover image URL/path of the item.
    pub cover: Option<String>,
    pub created_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub released_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub updated_at: Option<chrono::DateTime<chrono::FixedOffset>>,

    pub files: Vec<LibraryItemFile>,
    pub assets: Vec<LibraryAsset>,
}

impl LibraryItem {
    pub fn new_game(title: impl Into<String>) -> Self {
        Self {
            id: LibraryItemId::default(),
            title: title.into(),
            kind: ItemKind::Game,
            platform_slug: None,
            provider_ids: HashMap::new(),
            summary: None,
            alternative_names: Vec::new(),
            tags: Vec::new(),
            languages: Vec::new(),
            regions: Vec::new(),
            cover: None,
            created_at: None,
            released_at: None,
            updated_at: None,
            files: Vec::new(),
            assets: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct LibraryItemFile {
    pub name: String,
    pub path: String,
    pub size_bytes: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct LibraryAsset {
    pub url: Option<String>,
    pub path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::LibraryItemId;

    #[test]
    fn provider_ids_are_deterministic() {
        assert_eq!(
            LibraryItemId::from_provider("romm", "rom", "1234"),
            LibraryItemId::from_provider("romm", "rom", "1234")
        );
        assert_ne!(
            LibraryItemId::from_provider("romm", "rom", "1234"),
            LibraryItemId::from_provider("romm", "rom", "5678")
        );
    }
}
