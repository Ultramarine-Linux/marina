//! Marina's typed systemd D-Bus integration boundary.
//!
//! The crate re-exports the generated `zbus_systemd` bindings enabled by the
//! workspace. Higher-level, Marina-specific operations can be added here as
//! settings and runtime requirements become concrete.

pub use zbus_systemd;
