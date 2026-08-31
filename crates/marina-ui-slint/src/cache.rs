//! Deterministic, provider-namespaced disk cache paths for cover art.
//!
//! Layout: `$CACHE/covers/<provider>/<provider-specific path>`, e.g. for RomM:
//! `$CACHE/covers/romm/{platform_id}/{rom_id}/cover/small.png`.

use std::env;
use std::path::PathBuf;

/// Cache root: `$MARINA_CACHE_DIR`, else `$XDG_CACHE_HOME/marina`,
/// else `~/.cache/marina`.
pub fn cache_root() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("MARINA_CACHE_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Some(dir) = env::var_os("XDG_CACHE_HOME") {
        return Some(PathBuf::from(dir).join("marina"));
    }
    env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache").join("marina"))
}

/// Deterministic cache path for a cover, namespaced by provider.
///
/// RomM resource paths (`/assets/romm/resources/roms/{platform_id}/{rom_id}/cover/*.png`)
/// map to `covers/romm/{platform_id}/{rom_id}/cover/*.png` under the cache
/// root. Absolute URLs from external providers are not cached yet.
pub fn cover_cache_path(cover: &str) -> Option<PathBuf> {
    let rel = romm_cover_rel_path(cover)?;
    Some(cache_root()?.join("covers").join("romm").join(rel))
}

/// Extracts the `{platform_id}/{rom_id}/cover/*.png` tail from a RomM
/// resource path, rejecting anything that could traverse outside the cache.
fn romm_cover_rel_path(cover: &str) -> Option<&str> {
    let rel = cover
        .trim_start_matches('/')
        .strip_prefix("assets/romm/resources/roms/")?;
    let safe = rel
        .split('/')
        .all(|segment| !segment.is_empty() && segment != "." && segment != "..");
    safe.then_some(rel)
}

#[cfg(test)]
mod tests {
    use super::romm_cover_rel_path;

    #[test]
    fn maps_romm_resource_paths() {
        assert_eq!(
            romm_cover_rel_path("/assets/romm/resources/roms/46/3972/cover/small.png"),
            Some("46/3972/cover/small.png")
        );
    }

    #[test]
    fn rejects_non_romm_paths_and_urls() {
        assert_eq!(romm_cover_rel_path("https://example.com/cover.png"), None);
        assert_eq!(romm_cover_rel_path("/somewhere/else.png"), None);
    }

    #[test]
    fn rejects_path_traversal() {
        assert_eq!(
            romm_cover_rel_path("/assets/romm/resources/roms/../../../etc/passwd"),
            None
        );
        assert_eq!(
            romm_cover_rel_path("/assets/romm/resources/roms/46//cover.png"),
            None
        );
    }
}
