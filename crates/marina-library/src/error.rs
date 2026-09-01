//! Errors returned by library interfaces.

use std::error::Error;

use thiserror::Error as DeriveError;

/// Errors returned while reading or writing library data.
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

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::LibraryError;

    #[derive(Debug)]
    struct BackendFailure;

    impl std::fmt::Display for BackendFailure {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("backend failure")
        }
    }

    impl Error for BackendFailure {}

    #[test]
    fn backend_errors_are_preserved_as_sources() {
        let error = LibraryError::backend(BackendFailure);
        assert!(error.source().is_some());
    }
}
