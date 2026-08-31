use std::{env, error::Error};

use futures_util::StreamExt;

use marina_core::{LibraryItem, Platform as MarinaPlatform};
use marina_library::{LibraryRead, LibraryWrite, SearchQuery};
use marina_romm::{Auth, Client, PlatformQuery, RomQueryBuilder};
use marina_store_surrealdb::SurrealLibrary;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenvy::dotenv().ok();

    let romm_url = required_env("ROMM_URL")?;
    let storage_uri = env::var("MARINA_STORAGE_URI").unwrap_or_else(|_| "mem://".to_owned());
    let romm_token = env::var("ROMM_TOKEN").ok();

    let romm = match romm_token {
        Some(token) => Client::new(romm_url).with_auth(Auth::Bearer(token)),
        None => Client::new(romm_url),
    };
    let library = if storage_uri.starts_with("ws://") || storage_uri.starts_with("wss://") {
        let username = env::var("MARINA_STORAGE_USERNAME").unwrap_or_else(|_| "root".to_owned());
        let password = env::var("MARINA_STORAGE_PASSWORD").unwrap_or_else(|_| "root".to_owned());
        SurrealLibrary::connect_with_root(storage_uri.as_str(), username, password).await?
    } else {
        SurrealLibrary::connect(storage_uri.as_str()).await?
    };
    let heartbeat = romm.heartbeat().await?;
    println!("Connected to RomM {}", heartbeat.system.version);

    let romm_import: bool = env::var("ROMM_IMPORT").map_or(false, |v| v == "true");
    if romm_import {
        let platforms = romm.list_platforms(&PlatformQuery::default()).await?;

        let imported_platforms = library
            .add_platforms(platforms.iter().map(|platform| {
                let name = platform
                    .custom_name
                    .as_deref()
                    .unwrap_or(&platform.display_name);
                MarinaPlatform::new(&platform.slug, name)
            }))
            .await?;
        println!("Imported {} platform(s)", imported_platforms.len());
        let limit = env::var("ROMM_LIMIT")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.parse::<i64>())
            .transpose()?;
        let mut imported = 0_usize;
        match limit {
            Some(limit) => {
                let page = romm
                    .list_roms(&RomQueryBuilder::new().limit(limit).build())
                    .await?;
                imported += import_rom_page(&library, &page.items).await?;
            }
            None => {
                println!("Fetching and importing ROM pages...");
                let pages = romm.paginate_roms(RomQueryBuilder::new().build());
                futures_util::pin_mut!(pages);
                while let Some(page) = pages.next().await {
                    let page = page?;
                    imported += import_rom_page(&library, &page.items).await?;
                    println!("Imported {} ROM(s) so far", imported);
                }
            }
        }

        println!("Imported {} ROM(s)", imported);
    }

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // select first game in the library
    // let results = library.list(1).await?;
    let gameid = uuid::Uuid::parse_str("00003b74-57c9-558f-9dca-1cb13876f656").unwrap();

    let results = library
        .get(&marina_core::LibraryItemId::from_uuid(gameid))
        .await?;
    if let Some(item) = results {
        println!("First item: {} [{}]", item.title, item.id);
    } else {
        println!("No item found for {}", gameid);
    }

    // let search_text = env::var("MARINA_SEARCH").unwrap_or_else(|_| "".to_owned());
    // let results = library
    //     .search(SearchQuery::new().text(search_text).limit(10))
    //     .await?;

    // println!("SurrealDB round-trip returned {} item(s):", results.len());
    // for item in results {
    //     println!("- {} [{}]", item.title, item.id);
    // }

    Ok(())
}

async fn import_rom_page(
    library: &SurrealLibrary,
    roms: &[marina_romm::Rom],
) -> Result<usize, Box<dyn Error>> {
    for rom in roms {
        println!("Importing ROM: {}", rom.name.as_deref().unwrap_or_default());
        let item: LibraryItem = rom.clone().into();
        library.add(item).await?;
    }

    Ok(roms.len())
}

fn required_env(name: &str) -> Result<String, Box<dyn Error>> {
    env::var(name).map_err(|_| format!("{name} must be set").into())
}
