//! NetworkManager connectivity hydration for the AppShell top bar.
//!
//! This module exposes only a connected/disconnected status. Network discovery,
//! credentials, and connection controls belong to the future Settings surface.

use futures_util::StreamExt;
use slint::ComponentHandle;

use marina_networking::{
    NetworkManager,
    nmrs::{ActiveConnection, ActiveConnectionState},
};

use crate::{MainWindow, NetworkState};

/// Delay before retrying NetworkManager after its system-bus service is absent
/// or an event stream ends (for example, during a daemon restart).
const RECONNECT_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NetworkStatus {
    available: bool,
    connected: bool,
    wifi_signal_strength: Option<u8>,
}

fn connection_state(connection: &ActiveConnection) -> ActiveConnectionState {
    match connection {
        ActiveConnection::Wired(connection) => connection.state,
        ActiveConnection::Wifi(connection) => connection.state,
        ActiveConnection::Vpn(connection) => connection.state,
        ActiveConnection::Other(connection) => connection.state,
        _ => ActiveConnectionState::Unknown,
    }
}

fn connected_from_state(state: ActiveConnectionState) -> bool {
    matches!(state, ActiveConnectionState::Activated)
}

fn status_from_connections(connections: &[ActiveConnection]) -> NetworkStatus {
    let connected = connections
        .iter()
        .any(|connection| connected_from_state(connection_state(connection)));
    let wifi_signal_strength = connections.iter().find_map(|connection| match connection {
        ActiveConnection::Wifi(connection) if connected_from_state(connection.state) => {
            connection.strength
        }
        _ => None,
    });

    NetworkStatus {
        available: true,
        connected,
        wifi_signal_strength,
    }
}

fn apply_status(window: &slint::Weak<MainWindow>, status: NetworkStatus) {
    let _ = window.upgrade_in_event_loop(move |window| {
        let network = window.global::<NetworkState>();
        network.set_available(status.available);
        network.set_connected(status.connected);
        network.set_signal_strength(status.wifi_signal_strength.map_or(-1, i32::from));
    });
}

pub(crate) fn initialize(window: &MainWindow) {
    let network = window.global::<NetworkState>();
    network.set_available(false);
    network.set_connected(false);
    network.set_signal_strength(-1);

    let weak = window.as_weak();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        tokio::spawn(monitor_network_status(weak));
    });
}

async fn monitor_network_status(window: slint::Weak<MainWindow>) {
    loop {
        let network_manager = match NetworkManager::new().await {
            Ok(network_manager) => network_manager,
            Err(error) => {
                tracing::debug!(%error, "NetworkManager unavailable; hiding network indicator");
                apply_status(
                    &window,
                    NetworkStatus {
                        available: false,
                        connected: false,
                        wifi_signal_strength: None,
                    },
                );
                tokio::time::sleep(RECONNECT_DELAY).await;
                continue;
            }
        };

        let mut events = match network_manager.network_events().await {
            Ok(events) => events,
            Err(error) => {
                tracing::debug!(%error, "NetworkManager event subscription failed");
                apply_status(
                    &window,
                    NetworkStatus {
                        available: false,
                        connected: false,
                        wifi_signal_strength: None,
                    },
                );
                tokio::time::sleep(RECONNECT_DELAY).await;
                continue;
            }
        };

        if !publish_status(&window, &network_manager).await {
            tokio::time::sleep(RECONNECT_DELAY).await;
            continue;
        }

        while let Some(event) = events.next().await {
            match event {
                Ok(_) if !publish_status(&window, &network_manager).await => break,
                Ok(_) => {}
                Err(error) => {
                    tracing::debug!(%error, "NetworkManager event stream failed");
                    break;
                }
            }
        }

        apply_status(
            &window,
            NetworkStatus {
                available: false,
                connected: false,
                wifi_signal_strength: None,
            },
        );
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

/// Reads NetworkManager's active connections. Returning false asks the caller
/// to rebuild the D-Bus connection and subscriptions.
async fn publish_status(
    window: &slint::Weak<MainWindow>,
    network_manager: &NetworkManager,
) -> bool {
    match network_manager.list_active_connections().await {
        Ok(connections) => {
            apply_status(window, status_from_connections(&connections));
            true
        }
        Err(error) => {
            tracing::debug!(%error, "NetworkManager active-connection read failed");
            apply_status(
                window,
                NetworkStatus {
                    available: false,
                    connected: false,
                    wifi_signal_strength: None,
                },
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_connection_states_map_to_connection_status() {
        assert!(connected_from_state(ActiveConnectionState::Activated));
        assert!(!connected_from_state(ActiveConnectionState::Activating));
        assert!(!connected_from_state(ActiveConnectionState::Deactivating));
        assert!(!connected_from_state(ActiveConnectionState::Deactivated));
        assert!(!connected_from_state(ActiveConnectionState::Unknown));
    }
}
