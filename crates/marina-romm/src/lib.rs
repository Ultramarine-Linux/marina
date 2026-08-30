//! # marina-romm
//!
//! A small asynchronous client for the RomM HTTP API.
//!
//! The client is built around the RomM OpenAPI schema, but exposes only the
//! operations Marina currently needs. RomM-specific response models stay in
//! this crate; they should be converted into `marina-core` types by a provider
//! layer rather than leaking into the core domain model.
//!
//! ## Basic usage
//!
//! > NOTE: The base URL is the RomM server URL, not the `/api` URL. The client adds
//! > endpoint paths such as `/api/heartbeat` and `/api/roms` itself.
//!
//! ```no_run
//! use marina_romm::{Auth, Client, RomQueryBuilder};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), marina_romm::Error> {
//!     let token = std::env::var("ROMM_TOKEN")
//!         .expect("ROMM_TOKEN must contain a RomM bearer token");
//!
//!     let client = Client::new("https://romm.example.com")
//!         .with_auth(Auth::Bearer(token));
//!
//!     let heartbeat = client.heartbeat().await?;
//!     println!("connected to RomM {}", heartbeat.system.version);
//!
//!     let page = client
//!         .list_roms(
//!             &RomQueryBuilder::new()
//!                 .search_term("zelda")
//!                 .limit(25)
//!                 .build(),
//!         )
//!         .await?;
//!
//!     for rom in page.items {
//!         println!("{} ({})", rom.name.as_deref().unwrap_or(&rom.files.fs_name), rom.platform.platform_slug.as_str());
//!     }
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Authentication
//!
//! RomM bearer-token authentication is the normal choice for Marina:
//!
//! ```
//! use marina_romm::{Auth, Client};
//!
//! let client = Client::new("https://romm.example.com")
//!     .with_auth(Auth::Bearer("token-from-config".into()));
//! ```
//!
//! HTTP Basic authentication is also available:
//!
//! ```
//! use marina_romm::{Auth, Client};
//!
//! let client = Client::new("https://romm.example.com").with_auth(Auth::Basic {
//!     username: "user".into(),
//!     password: "password".into(),
//! });
//! ```
//!
//! The client does not read environment variables, keyrings, or config files
//! itself. Callers own credential storage and pass an [`Auth`] value explicitly.
//!
//! ## ROM queries
//!
//! [`RomQuery`] maps the first useful subset of RomM's `GET /api/roms` query
//! parameters. Repeated values such as `platform_ids` are emitted as repeated
//! query parameters, matching RomM's API:
//!
//! ```
//! use marina_romm::RomQuery;
//!
//! let query = RomQuery {
//!     platform_ids: vec![1, 2],
//!     favorite: Some(true),
//!     offset: Some(50),
//!     limit: Some(50),
//!     ..Default::default()
//! };
//! ```
//!
//! `list_roms` returns a [`RomPage`]. Use `offset` and `limit` to request later
//! pages; `total` may be absent when the server does not calculate a total for
//! the request.
//!
//! ## Current scope
//!
//! The current client implements heartbeat, ROM listing, and platform listing.
//! Downloading ROMs, save synchronization, collections, firmware, screenshots,
//! and metadata operations are intentionally not included yet. They can be added
//! as typed operations without changing the transport or authentication surface.

mod client;
mod error;
mod models;

pub use client::{Auth, Client};
pub use error::Error;
pub use models::metadata::*;
pub use models::{
    Heartbeat, Platform, PlatformQuery, PlatformQueryBuilder, Rom, RomPage, RomQuery,
    RomQueryBuilder, SystemInfo,
};

#[cfg(test)]
mod tests {
    use super::{RomQueryBuilder, SystemInfo};

    #[test]
    fn rom_query_repeats_platform_ids() {
        let query = RomQueryBuilder::new()
            .platform_ids([7, 12])
            .favorite(true)
            .build();

        assert_eq!(
            serde_urlencoded::to_string(&query).expect("query encoding"),
            "favorite=true"
        );
    }

    #[test]
    fn heartbeat_system_fields_deserialize_from_romm_names() {
        let system: SystemInfo = serde_json::from_value(serde_json::json!({
            "VERSION": "5.2.0",
            "SHOW_SETUP_WIZARD": false
        }))
        .expect("system info");

        assert_eq!(system.version, "5.2.0");
        assert!(!system.show_setup_wizard);
    }
}
