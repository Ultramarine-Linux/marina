//! Backend-agnostic interfaces for reading Marina library metadata.
//!
//! Storage implementations should depend on this crate and implement [`read::LibraryRead`].
//! The interface deliberately returns domain types from `marina-core`, as a little abstraction
//! layer over the storage backend so we can swap out the database implementation without
//! actually rewriting calls to the library.

pub mod error;
pub mod query;
pub mod read;
pub mod write;

use async_trait::async_trait;
use read::{LibraryRead, PlatformRead};
use write::{LibraryWrite, PlatformWrite};

/// A complete library backend: read + write access to items and platforms.
///
/// Blanket-implemented for every backend; exists so generic code (e.g. store
/// installs) can take a single `&(dyn Library + Send + Sync)` instead of
/// juggling four traits. Native `async fn` traits aren't `dyn`-safe, which
/// is why the read/write traits use `async_trait`.
#[async_trait]
pub trait Library: LibraryRead + LibraryWrite + PlatformRead + PlatformWrite {}

impl<T> Library for T where T: LibraryRead + LibraryWrite + PlatformRead + PlatformWrite {}

#[cfg(test)]
mod tests;
