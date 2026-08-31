//! SurrealDB-backed storage for Marina's library interfaces.
//!
//! The backend accepts SurrealDB endpoint URIs, for example `mem://` for an
//! ephemeral test database or `surrealkv://./marina.db` for a local persistent
//! database.

surrealkit::embed_schema!();

mod connection;
mod library;
mod platform;

pub(crate) use connection::{ITEMS_TABLE, PLATFORMS_TABLE};
pub use connection::{SurrealLibrary, SurrealLibraryError};

#[cfg(test)]
mod tests {
    use marina_library::{LibraryItem, LibraryRead, LibraryWrite, PlatformRead, SearchQuery};

    use super::SurrealLibrary;

    #[tokio::test]
    async fn memory_uri_supports_library_crud_and_search() {
        let library = SurrealLibrary::connect("mem://").await.unwrap();
        let mut item = LibraryItem::new_game("Super Mario World");
        item.platform_slug = Some("snes".to_owned());
        let id = item.id.clone();

        library.add(item).await.unwrap();
        assert_eq!(
            library.get(&id).await.unwrap().unwrap().title,
            "Super Mario World"
        );
        assert_eq!(
            library
                .search(SearchQuery::new().text("mario"))
                .await
                .unwrap()
                .len(),
            1
        );

        library.remove(&id).await.unwrap();
        assert!(library.get(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn platforms_are_stored_and_read_separately() {
        let library = SurrealLibrary::connect("mem://").await.unwrap();
        library
            .add_platforms([marina_library::Platform::new("snes", "Super Nintendo")])
            .await
            .unwrap();

        let platforms = library.platforms().await.unwrap();
        assert_eq!(platforms[0].slug, "snes");
    }
}
