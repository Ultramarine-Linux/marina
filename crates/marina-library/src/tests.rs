//! Shared test support for the library interfaces.

use marina_core::{LibraryCard, LibraryItem, LibraryItemId, Platform};

use crate::{
    error::LibraryError,
    query::SearchQuery,
    read::{LibraryRead, PlatformRead},
    write::{LibraryWrite, PlatformWrite},
};

pub(crate) struct EmptyLibrary;

impl PlatformRead for EmptyLibrary {
    async fn platforms(&self) -> Result<Vec<Platform>, LibraryError> {
        Ok(Vec::new())
    }
}

impl LibraryRead for EmptyLibrary {
    async fn search(&self, _query: SearchQuery) -> Result<Vec<LibraryItem>, LibraryError> {
        Ok(Vec::new())
    }

    async fn get(&self, _id: &LibraryItemId) -> Result<Option<LibraryItem>, LibraryError> {
        Ok(None)
    }

    async fn list(&self, _limit: u32) -> Result<Vec<LibraryItem>, LibraryError> {
        Ok(Vec::new())
    }

    async fn list_cards(&self, _limit: u32) -> Result<Vec<LibraryCard>, LibraryError> {
        Ok(Vec::new())
    }

    async fn search_cards(&self, _query: SearchQuery) -> Result<Vec<LibraryCard>, LibraryError> {
        Ok(Vec::new())
    }
}

impl PlatformWrite for EmptyLibrary {
    async fn add_platform(&self, platform: Platform) -> Result<Platform, LibraryError> {
        Ok(platform)
    }

    async fn update_platform(&self, platform: Platform) -> Result<Platform, LibraryError> {
        Ok(platform)
    }

    async fn remove_platform(&self, _slug: &str) -> Result<(), LibraryError> {
        Ok(())
    }
}

impl LibraryWrite for EmptyLibrary {
    async fn add(&self, item: LibraryItem) -> Result<LibraryItem, LibraryError> {
        Ok(item)
    }

    async fn update(&self, item: LibraryItem) -> Result<LibraryItem, LibraryError> {
        Ok(item)
    }

    async fn remove(&self, _id: &LibraryItemId) -> Result<(), LibraryError> {
        Ok(())
    }
}
