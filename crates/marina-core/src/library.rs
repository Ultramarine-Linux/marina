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

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct LibraryItem {
    pub id: LibraryItemId,
    pub title: String,
    pub kind: ItemKind,
    // insert more stuff here like uh covers n shit idk
    pub provider_ids: HashMap<String, String>,
}

impl LibraryItem {
    pub fn game(title: impl Into<String>) -> Self {
        Self {
            id: LibraryItemId::default(),
            title: title.into(),
            kind: ItemKind::Game,
            provider_ids: HashMap::new(),
        }
    }
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
