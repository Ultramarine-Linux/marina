//! Human-readable defaults for RomM platform slugs and documented folder aliases.

use std::{collections::BTreeMap, sync::OnceLock};

use serde::Deserialize;

const CATALOG_JSON: &str = include_str!(concat!(env!("OUT_DIR"), "/platform-names.json"));

#[derive(Debug, Deserialize)]
struct PlatformCatalog {
    #[serde(rename = "source")]
    _source: String,
    platforms: BTreeMap<String, String>,
    aliases: BTreeMap<String, Vec<String>>,
}

fn catalog() -> &'static PlatformCatalog {
    static CATALOG: OnceLock<PlatformCatalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(CATALOG_JSON).expect("embedded RomM platform catalog must be valid")
    })
}

pub(crate) fn default_name(slug: &str) -> Option<&'static str> {
    let slug = slug.trim().to_ascii_lowercase();
    let catalog = catalog();
    catalog
        .platforms
        .get(&slug)
        .or_else(|| {
            catalog.aliases.get(&slug).and_then(|targets| {
                targets
                    .iter()
                    .find_map(|target| catalog.platforms.get(target))
            })
        })
        .map(String::as_str)
}

pub(crate) fn display_name(slug: &str, configured: &str) -> String {
    let configured = configured.trim();
    if !configured.is_empty() && !configured.eq_ignore_ascii_case(slug) {
        return configured.to_owned();
    }
    default_name(slug)
        .map(str::to_owned)
        .unwrap_or_else(|| humanize_slug(slug))
}

fn humanize_slug(slug: &str) -> String {
    slug.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::{default_name, display_name};

    #[test]
    fn maps_common_romm_slugs_and_aliases() {
        assert_eq!(default_name("gba"), Some("Game Boy Advance"));
        assert_eq!(default_name("n64"), Some("Nintendo 64"));
        assert_eq!(default_name("nds"), Some("Nintendo DS"));
        assert_eq!(default_name("amiga500"), Some("Amiga"));
        assert_eq!(default_name("ndsi"), Some("Nintendo DSi"));
    }

    #[test]
    fn preserves_explicit_names_and_humanizes_unknown_slugs() {
        assert_eq!(display_name("gba", "Custom Advance"), "Custom Advance");
        assert_eq!(display_name("future-box", "future-box"), "Future Box");
    }
}
