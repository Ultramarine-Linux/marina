//! Read-only access to Marina library metadata.

use marina_core::{LibraryCard, LibraryItem, LibraryItemId, Platform};

use crate::{error::LibraryError, query::SearchQuery};

/// Read-only access to platform metadata.
#[allow(async_fn_in_trait)]
pub trait PlatformRead {
    async fn platforms(&self) -> Result<Vec<Platform>, LibraryError>;
}

/// Read-only access to library metadata.
///
/// This trait contains no database-specific types. A backend may implement it using
/// any storage engine and may perform work asynchronously.
#[allow(async_fn_in_trait)]
pub trait LibraryRead: PlatformRead {
    async fn search(&self, query: SearchQuery) -> Result<Vec<LibraryItem>, LibraryError>;

    /// Count entries matching a query without loading their records.
    async fn count(&self, query: SearchQuery) -> Result<usize, LibraryError>;

    async fn get(&self, id: &LibraryItemId) -> Result<Option<LibraryItem>, LibraryError>;

    async fn list(&self, limit: u32) -> Result<Vec<LibraryItem>, LibraryError>;

    async fn list_cards(&self, limit: u32) -> Result<Vec<LibraryCard>, LibraryError>;

    async fn search_cards(&self, query: SearchQuery) -> Result<Vec<LibraryCard>, LibraryError>;
}

#[cfg(test)]
mod tests {
    use super::LibraryRead;
    use crate::tests::EmptyLibrary;

    #[test]
    fn a_backend_can_implement_the_read_interface() {
        fn assert_library_read<T: LibraryRead>() {}
        assert_library_read::<EmptyLibrary>();
    }
}
