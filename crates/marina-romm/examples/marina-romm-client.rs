use marina_romm::{Auth, Client, PlatformQueryBuilder, RomQueryBuilder};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    let base_url = std::env::var("ROMM_URL")?;
    let client = match (
        std::env::var("ROMM_TOKEN").ok(),
        std::env::var("ROMM_USERNAME").ok(),
        std::env::var("ROMM_PASSWORD").ok(),
    ) {
        (Some(token), None, None) => Client::new(base_url).with_auth(Auth::Bearer(token)),
        (None, Some(username), Some(password)) => {
            Client::new(base_url).with_auth(Auth::Basic { username, password })
        }
        (None, None, None) => Client::new(base_url),
        _ => {
            return Err(
                "set either ROMM_TOKEN or both ROMM_USERNAME and ROMM_PASSWORD, not both".into(),
            );
        }
    };

    let heartbeat = client.heartbeat().await?;
    println!("RomM {}", heartbeat.system.version);

    let platforms = client
        .list_platforms(&PlatformQueryBuilder::new().build())
        .await?;
    println!("platforms: {}", platforms.len());
    for platform in &platforms {
        println!(
            "  {} ({}) [directory: {}]",
            platform.display_name, platform.slug, platform.fs_slug
        );
    }

    let roms = client
        .list_roms(&RomQueryBuilder::new().limit(25).build())
        .await?;
    println!("ROMs: {} of {:?}", roms.items.len(), roms.total);
    for rom in &roms.items {
        println!("{:#?}", rom);
    }

    Ok(())
}
