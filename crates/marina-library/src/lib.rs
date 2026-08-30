//! Backend-agnostic interfaces for reading Marina library metadata.
//!
//! Storage implementations should depend on this crate and implement [`LibraryRead`].
//! The interface deliberately returns domain types from `marina-core`, as a little abstraction
//! layer over the storage backend so we can swap out the database implementation without
//! actually rewriting calls to the library.

use std::error::Error;

pub use marina_core::{ItemKind, LibraryItem, LibraryItemId, Platform};
use thiserror::Error as DeriveError;

/// Parameters used when searching library entries.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SearchQuery {
    /// Text to match against an entry's title or searchable metadata.
    pub text: Option<String>,
    /// Restrict results to a platform slug.
    pub platform: Option<String>,
    /// Number of results to return. Backends may apply their own maximum.
    pub limit: Option<usize>,
    /// Number of matching results to skip.
    pub offset: usize,
}

impl SearchQuery {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    pub fn platform(mut self, platform: impl Into<String>) -> Self {
        self.platform = Some(platform.into());
        self
    }

    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = offset;
        self
    }
}

/// Errors returned while reading library data.
#[derive(Debug, DeriveError)]
pub enum LibraryError {
    #[error("library backend failed: {0}")]
    Backend(#[source] Box<dyn Error + Send + Sync>),

    #[error("invalid library query: {0}")]
    InvalidQuery(String),
}

impl LibraryError {
    /// Wrap an error returned by a concrete storage backend.
    pub fn backend(error: impl Error + Send + Sync + 'static) -> Self {
        Self::Backend(Box::new(error))
    }
}

/// Read-only access to library metadata.
///
/// This trait contains no database-specific types. A backend may implement it using
/// any storage engine and may perform work asynchronously.
#[allow(async_fn_in_trait)]
pub trait LibraryRead {
    async fn search(&self, query: SearchQuery) -> Result<Vec<LibraryItem>, LibraryError>;

    async fn get(&self, id: &LibraryItemId) -> Result<Option<LibraryItem>, LibraryError>;

    async fn platforms(&self) -> Result<Vec<Platform>, LibraryError>;
}

#[allow(async_fn_in_trait)]
pub trait LibraryWrite {
    async fn add(&self, item: LibraryItem) -> Result<LibraryItem, LibraryError>;
    async fn update(&self, item: LibraryItem) -> Result<LibraryItem, LibraryError>;
    async fn remove(&self, id: &LibraryItemId) -> Result<(), LibraryError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Debug)]
    struct BackendFailure;

    impl std::fmt::Display for BackendFailure {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("backend failure")
        }
    }

    impl std::error::Error for BackendFailure {}

    struct EmptyLibrary;

    impl LibraryRead for EmptyLibrary {
        async fn search(&self, _query: SearchQuery) -> Result<Vec<LibraryItem>, LibraryError> {
            Ok(Vec::new())
        }

        async fn get(&self, _id: &LibraryItemId) -> Result<Option<LibraryItem>, LibraryError> {
            Ok(None)
        }

        async fn platforms(&self) -> Result<Vec<Platform>, LibraryError> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn search_query_builder_is_composable() {
        let query = SearchQuery::new()
            .text("zelda")
            .platform("snes")
            .limit(20)
            .offset(40);

        assert_eq!(query.text.as_deref(), Some("zelda"));
        assert_eq!(query.platform.as_deref(), Some("snes"));
        assert_eq!(query.limit, Some(20));
        assert_eq!(query.offset, 40);
    }

    #[test]
    fn backend_errors_are_preserved_as_sources() {
        let error = LibraryError::backend(BackendFailure);
        assert!(error.source().is_some());
    }

    #[test]
    fn a_backend_can_implement_the_read_interface() {
        fn assert_library_read<T: LibraryRead>() {}
        assert_library_read::<EmptyLibrary>();
    }
}
