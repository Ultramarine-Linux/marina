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
        item.files.push(marina_core::LibraryItemFile {
            name: "mario.sfc".to_owned(),
            path: "/roms/mario.sfc".to_owned(),
            size_bytes: Some(1024),
        });
        item.assets.push(marina_core::LibraryAsset {
            url: None,
            path: Some("/assets/mario.jpg".to_owned()),
        });
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

    #[tokio::test]
    async fn list_cards_falls_back_to_platform_slug_for_blank_name() {
        let library = SurrealLibrary::connect("mem://").await.unwrap();
        library
            .add_platforms([marina_library::Platform::new("gba", "")])
            .await
            .unwrap();

        let mut item = LibraryItem::new_game("Metroid Fusion");
        item.platform_slug = Some("gba".to_owned());
        library.add(item).await.unwrap();

        let cards = library.list_cards(10).await.unwrap();
        assert_eq!(cards[0].platform_name.as_deref(), Some("gba"));
    }

    #[tokio::test]
    async fn list_cards_resolves_platform_name() {
        let library = SurrealLibrary::connect("mem://").await.unwrap();
        library
            .add_platforms([marina_library::Platform::new("snes", "Super Nintendo")])
            .await
            .unwrap();

        let mut item = LibraryItem::new_game("Super Mario World");
        item.platform_slug = Some("snes".to_owned());
        library.add(item).await.unwrap();

        let cards = library.list_cards(10).await.unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].title, "Super Mario World");
        assert_eq!(cards[0].platform_name.as_deref(), Some("Super Nintendo"));

        let search_results = library
            .search_cards(
                marina_library::SearchQuery::new()
                    .text("mario")
                    .platform("snes"),
            )
            .await
            .unwrap();
        assert_eq!(search_results, cards);
    }
}
