//! Marina's NetworkManager integration boundary.
//!
//! The crate re-exports `nmrs` and its high-level [`NetworkManager`] client.
//! Higher-level, Marina-specific connectivity operations can be added here as
//! settings requirements become concrete.

pub use nmrs;
pub use nmrs::NetworkManager;
