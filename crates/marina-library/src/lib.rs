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

#[cfg(test)]
mod tests;
