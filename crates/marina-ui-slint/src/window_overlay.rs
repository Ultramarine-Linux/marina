//! Varlink control for the independently supervised layer-shell overlay.

use std::thread;

use marina_shell::OverlayClient;
use tracing::warn;

pub(crate) fn toggle() {
    thread::Builder::new()
        .name("marina-overlay-varlink-client".to_owned())
        .spawn(|| {
            for attempt in 0..20 {
                match OverlayClient::toggle() {
                    Ok(()) => return,
                    Err(error) if attempt < 19 => {
                        thread::sleep(std::time::Duration::from_millis(50));
                        tracing::debug!(%error, attempt, "waiting for window overlay Varlink service");
                    }
                    Err(error) => warn!(%error, "failed to toggle window overlay"),
                }
            }
        })
        .map(|_| ())
        .unwrap_or_else(|error| warn!(%error, "failed to start overlay Varlink client"));
}
