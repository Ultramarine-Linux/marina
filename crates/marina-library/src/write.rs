//! Write access to Marina library metadata.

use marina_core::{LibraryItem, LibraryItemId, Platform};

use crate::error::LibraryError;

/// Write access to platform metadata.
#[allow(async_fn_in_trait)]
pub trait PlatformWrite {
    async fn add_platform(&self, platform: Platform) -> Result<Platform, LibraryError>;
    async fn update_platform(&self, platform: Platform) -> Result<Platform, LibraryError>;
    async fn remove_platform(&self, slug: &str) -> Result<(), LibraryError>;
}

/// Write access to library metadata.
#[allow(async_fn_in_trait)]
pub trait LibraryWrite {
    async fn add(&self, item: LibraryItem) -> Result<LibraryItem, LibraryError>;
    async fn update(&self, item: LibraryItem) -> Result<LibraryItem, LibraryError>;
    async fn remove(&self, id: &LibraryItemId) -> Result<(), LibraryError>;
}

#[cfg(test)]
mod tests {
    use super::{LibraryWrite, PlatformWrite};
    use crate::tests::EmptyLibrary;

    #[test]
    fn a_backend_can_implement_the_write_interfaces() {
        fn assert_platform_write<T: PlatformWrite>() {}
        fn assert_library_write<T: LibraryWrite>() {}

        assert_platform_write::<EmptyLibrary>();
        assert_library_write::<EmptyLibrary>();
    }
}
