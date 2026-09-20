//! Read-only access to Marina library metadata.

use marina_core::{LibraryCard, LibraryItem, LibraryItemId, Platform};

/// A platform and the number of library items assigned to it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlatformCount {
    pub platform: Platform,
    pub count: usize,
}

use crate::{error::LibraryError, query::SearchQuery};
use async_trait::async_trait;

/// Read-only access to platform metadata.
#[async_trait]
pub trait PlatformRead {
    async fn platforms(&self) -> Result<Vec<Platform>, LibraryError>;
}

/// Read-only access to library metadata.
///
/// This trait contains no database-specific types. A backend may implement it using
/// any storage engine and may perform work asynchronously.
#[async_trait]
pub trait LibraryRead: PlatformRead {
    async fn search(&self, query: SearchQuery) -> Result<Vec<LibraryItem>, LibraryError>;

    /// Count entries matching a query without loading their records.
    async fn count(&self, query: SearchQuery) -> Result<usize, LibraryError>;

    async fn get(&self, id: &LibraryItemId) -> Result<Option<LibraryItem>, LibraryError>;

    /// Finds library items whose local paths are in the supplied batch.
    ///
    /// The default keeps existing backends source-compatible; storage backends should override
    /// this when they can use a path index.
    async fn find_by_local_paths(
        &self,
        paths: &[String],
    ) -> Result<Vec<LibraryItem>, LibraryError> {
        let items = self.search(SearchQuery::new()).await?;
        Ok(items
            .into_iter()
            .filter(|item| {
                item.local_path
                    .as_deref()
                    .is_some_and(|path| paths.iter().any(|candidate| candidate == path))
            })
            .collect())
    }

    async fn list(&self, limit: u32) -> Result<Vec<LibraryItem>, LibraryError>;

    async fn list_cards(&self, limit: u32) -> Result<Vec<LibraryCard>, LibraryError>;

    async fn search_cards(&self, query: SearchQuery) -> Result<Vec<LibraryCard>, LibraryError>;

    /// Find an item by its exact installed/local path.
    ///
    /// The default keeps existing backends source-compatible; storage backends should override
    /// this when they can use a path index.
    async fn get_by_local_path(
        &self,
        local_path: &str,
    ) -> Result<Option<LibraryItem>, LibraryError> {
        Ok(self
            .search(SearchQuery::new())
            .await?
            .into_iter()
            .find(|item| item.local_path.as_deref() == Some(local_path)))
    }

    /// Return counts for platforms represented in the library.
    ///
    /// The default is intentionally compatible with existing implementations. Backends with
    /// grouped-query support should override it.
    async fn platform_counts(&self) -> Result<Vec<PlatformCount>, LibraryError> {
        let platforms = self.platforms().await?;
        let mut counts = Vec::with_capacity(platforms.len());
        for platform in platforms {
            let count = self
                .count(SearchQuery::new().platform(platform.slug.clone()))
                .await?;
            counts.push(PlatformCount { platform, count });
        }
        Ok(counts)
    }
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
