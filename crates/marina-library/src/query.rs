//! Query parameters for searching library entries.

/// Backend-owned sort order for library queries.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SearchSort {
    #[default]
    Title,
    LastUpdated,
}

/// Parameters used when searching library entries.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SearchQuery {
    /// Text to match against an entry's title or searchable metadata.
    pub text: Option<String>,
    /// Restrict results to a platform slug.
    pub platform: Option<String>,
    /// Ordering applied by the backend before pagination.
    pub sort: SearchSort,
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

    pub fn sort(mut self, sort: SearchSort) -> Self {
        self.sort = sort;
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

#[cfg(test)]
mod tests {
    use super::{SearchQuery, SearchSort};

    #[test]
    fn builder_is_composable() {
        let query = SearchQuery::new()
            .text("zelda")
            .platform("snes")
            .sort(SearchSort::LastUpdated)
            .limit(20)
            .offset(40);

        assert_eq!(query.text.as_deref(), Some("zelda"));
        assert_eq!(query.platform.as_deref(), Some("snes"));
        assert_eq!(query.sort, SearchSort::LastUpdated);
        assert_eq!(query.limit, Some(20));
        assert_eq!(query.offset, 40);
    }
}
