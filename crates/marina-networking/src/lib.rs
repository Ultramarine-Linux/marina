//! NetworkManager integration for Marina.
//!
//! This crate owns NetworkManager D-Bus access, including active-connection
//! status, Wi-Fi signal strength, live update subscriptions, and reconnection
//! after a daemon or system-bus failure. Consumers receive plain
//! [`NetworkStatus`] snapshots and need not depend on NetworkManager types.

use std::time::Duration;

use futures_util::StreamExt;
use nmrs::{ActiveConnection, ActiveConnectionState, NetworkManager};

/// Delay before retrying NetworkManager after its system-bus service is absent
/// or an event stream ends (for example, during a daemon restart).
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

/// Snapshot of NetworkManager's active connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkStatus {
    /// False when NetworkManager is unreachable.
    pub available: bool,
    /// True when any NetworkManager connection is activated.
    pub connected: bool,
    /// Signal strength (0–100) of an active Wi-Fi connection, when reported.
    /// `None` represents a wired/VPN connection or unavailable Wi-Fi strength.
    pub wifi_signal_strength: Option<u8>,
}

impl NetworkStatus {
    /// Status used when NetworkManager cannot be reached.
    pub fn unavailable() -> Self {
        Self {
            available: false,
            connected: false,
            wifi_signal_strength: None,
        }
    }
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

fn emit(
    on_change: &(impl Fn(NetworkStatus) + Send + Sync),
    last: &mut Option<NetworkStatus>,
    status: NetworkStatus,
) {
    if *last != Some(status) {
        on_change(status);
        *last = Some(status);
    }
}

/// Reads the current NetworkManager connection status once.
pub async fn read_network_status() -> NetworkStatus {
    let Ok(network_manager) = NetworkManager::new().await else {
        return NetworkStatus::unavailable();
    };
    match network_manager.list_active_connections().await {
        Ok(connections) => status_from_connections(&connections),
        Err(_) => NetworkStatus::unavailable(),
    }
}

/// Continuously reports NetworkManager connection status.
///
/// The callback fires with the initial state, on NetworkManager events, and
/// after a reconnect. This future normally runs for the application lifetime.
pub async fn monitor_network_status(on_change: impl Fn(NetworkStatus) + Send + Sync + 'static) {
    let mut last = None;

    loop {
        let network_manager = match NetworkManager::new().await {
            Ok(network_manager) => network_manager,
            Err(error) => {
                tracing::debug!(%error, "NetworkManager unavailable");
                emit(&on_change, &mut last, NetworkStatus::unavailable());
                tokio::time::sleep(RECONNECT_DELAY).await;
                continue;
            }
        };

        let mut events = match network_manager.network_events().await {
            Ok(events) => events,
            Err(error) => {
                tracing::debug!(%error, "NetworkManager event subscription failed");
                emit(&on_change, &mut last, NetworkStatus::unavailable());
                tokio::time::sleep(RECONNECT_DELAY).await;
                continue;
            }
        };

        if !publish_status(&network_manager, &on_change, &mut last).await {
            tokio::time::sleep(RECONNECT_DELAY).await;
            continue;
        }

        while let Some(event) = events.next().await {
            match event {
                Ok(_) if !publish_status(&network_manager, &on_change, &mut last).await => break,
                Ok(_) => {}
                Err(error) => {
                    tracing::debug!(%error, "NetworkManager event stream failed");
                    break;
                }
            }
        }

        emit(&on_change, &mut last, NetworkStatus::unavailable());
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

/// Reads active connections. Returning false asks the caller to rebuild the
/// D-Bus connection and subscriptions.
async fn publish_status(
    network_manager: &NetworkManager,
    on_change: &(impl Fn(NetworkStatus) + Send + Sync),
    last: &mut Option<NetworkStatus>,
) -> bool {
    match network_manager.list_active_connections().await {
        Ok(connections) => {
            emit(on_change, last, status_from_connections(&connections));
            true
        }
        Err(error) => {
            tracing::debug!(%error, "NetworkManager active-connection read failed");
            emit(on_change, last, NetworkStatus::unavailable());
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

    #[test]
    fn unavailable_status_has_no_connection_or_signal() {
        assert_eq!(
            NetworkStatus::unavailable(),
            NetworkStatus {
                available: false,
                connected: false,
                wifi_signal_strength: None,
            }
        );
    }
}
