//! Restores Sway's input devices after resume.
//!
//! Sway can come back from suspend with an empty seat until a physical event on
//! one of its own devices wakes libinput (see
//! `marina_input::virtual_keyboard`). Without a keyboard on the seat no window
//! holds focus, so the shell drops every controller event. After each resume
//! this polls Sway briefly and nudges libinput only while the seat is empty.

use std::time::Duration;

use marina_shell::SwayWindowManager;
use tracing::{debug, info, warn};

const POLL_INTERVAL: Duration = Duration::from_millis(500);
const RECOVERY_WINDOW: Duration = Duration::from_secs(10);

pub(crate) fn spawn_monitor() {
    tokio::spawn(marina_power::monitor_resume(|| {
        tokio::spawn(recover_seat());
    }));
}

async fn recover_seat() {
    let deadline = tokio::time::Instant::now() + RECOVERY_WINDOW;
    let mut nudged = false;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(POLL_INTERVAL).await;
        let keyboards = tokio::task::spawn_blocking(|| {
            SwayWindowManager::new().and_then(|sway| sway.keyboard_count())
        })
        .await;
        match keyboards {
            Ok(Ok(0)) => {
                if !nudged {
                    info!("Sway lost its input devices after resume; nudging libinput");
                    nudged = true;
                }
                let nudge = tokio::task::spawn_blocking(
                    marina_input::virtual_keyboard::nudge_virtual_keyboard,
                )
                .await;
                match nudge {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        warn!(%error, "failed to nudge libinput after resume");
                        return;
                    }
                    Err(error) => {
                        warn!(%error, "libinput nudge task failed");
                        return;
                    }
                }
            }
            Ok(Ok(keyboards)) if nudged => {
                info!(keyboards, "Sway input devices restored after resume");
                return;
            }
            Ok(Ok(_)) => {}
            Ok(Err(error)) => debug!(%error, "could not query Sway inputs after resume"),
            Err(error) => {
                warn!(%error, "Sway input query task failed");
                return;
            }
        }
    }
    if nudged {
        warn!("Sway input devices did not return after resume");
    }
}
