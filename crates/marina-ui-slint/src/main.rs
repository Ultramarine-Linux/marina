use std::{env, error::Error};

use marina_store_surrealdb::SurrealLibrary;
use slint::{ModelRc, VecModel};

slint::include_modules!();

#[path = "../ui/mod.rs"]
mod ui;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenvy::dotenv().ok();

    let storage_uri = env::var("MARINA_STORAGE_URI").unwrap_or_else(|_| "mem://".to_owned());
    let library = if storage_uri.starts_with("ws://") || storage_uri.starts_with("wss://") {
        let username = env::var("MARINA_STORAGE_USERNAME").unwrap_or_else(|_| "root".to_owned());
        let password = env::var("MARINA_STORAGE_PASSWORD").unwrap_or_else(|_| "root".to_owned());
        SurrealLibrary::connect_with_root(storage_uri.as_str(), username, password).await?
    } else {
        SurrealLibrary::connect(storage_uri.as_str()).await?
    };

    println!("Loading games...");
    let games = ui::shelf::load_games(&library).await?;
    let shelf_height = ui::shelf::height_for_games(&games);
    let window = MainWindow::new()?;
    window.set_games(ModelRc::from(std::rc::Rc::new(VecModel::from(games))));
    window.set_shelf_height(shelf_height);
    window.run()?;

    Ok(())
}
