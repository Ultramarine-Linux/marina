use std::fmt;

use futures_util::StreamExt;
use nmrs::{ConnectionError, Network, NetworkManager, WifiSecurity};

use crate::RECONNECT_DELAY;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiNetwork {
    pub ssid: String,
    pub strength: Option<u8>,
    pub secured: bool,
    pub active: bool,
    pub saved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiSnapshot {
    pub available: bool,
    pub enabled: bool,
    pub networks: Vec<WifiNetwork>,
}

impl WifiSnapshot {
    pub fn unavailable() -> Self {
        Self {
            available: false,
            enabled: false,
            networks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WifiControlError {
    Unavailable,
    Failed(String),
}

impl fmt::Display for WifiControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("NetworkManager is not available"),
            Self::Failed(cause) => formatter.write_str(cause),
        }
    }
}

impl std::error::Error for WifiControlError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WifiConnectError {
    Unavailable,
    NotFound,
    AuthFailed, // no password/wrong password
    Timeout,
    Failed(String),
}

impl fmt::Display for WifiConnectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("NetworkManager is not available"),
            Self::NotFound => formatter.write_str("network not found"),
            Self::AuthFailed => formatter.write_str("authentication failed"),
            Self::Timeout => formatter.write_str("connection timed out"),
            Self::Failed(cause) => formatter.write_str(cause),
        }
    }
}

impl std::error::Error for WifiConnectError {}

fn network_from_nm(network: &Network) -> WifiNetwork {
    WifiNetwork {
        ssid: network.ssid.clone(),
        strength: network.strength,
        secured: network.secured,
        active: network.is_active,
        saved: network.known,
    }
}

fn sort_networks(networks: &mut [WifiNetwork]) {
    networks.sort_by(|left, right| {
        right
            .active
            .cmp(&left.active)
            .then_with(|| right.strength.unwrap_or(0).cmp(&left.strength.unwrap_or(0)))
            .then_with(|| left.ssid.cmp(&right.ssid))
    });
}

async fn read_snapshot(network_manager: &NetworkManager) -> Result<WifiSnapshot, ConnectionError> {
    let radio = network_manager.wifi_state().await?;
    if !radio.enabled {
        return Ok(WifiSnapshot {
            available: true,
            enabled: false,
            networks: Vec::new(),
        });
    }
    let networks = network_manager.list_networks(None).await?;
    let mut networks: Vec<WifiNetwork> = networks.iter().map(network_from_nm).collect();
    sort_networks(&mut networks);
    Ok(WifiSnapshot {
        available: true,
        enabled: true,
        networks,
    })
}

pub async fn read_wifi_snapshot() -> WifiSnapshot {
    let Ok(network_manager) = NetworkManager::new().await else {
        return WifiSnapshot::unavailable();
    };
    match read_snapshot(&network_manager).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            tracing::debug!(%error, "Wi-Fi snapshot read failed");
            WifiSnapshot::unavailable()
        }
    }
}

pub async fn monitor_wifi(on_change: impl Fn(WifiSnapshot) + Send + Sync + 'static) {
    let mut last = None;

    loop {
        let network_manager = match NetworkManager::new().await {
            Ok(network_manager) => network_manager,
            Err(error) => {
                tracing::debug!(%error, "NetworkManager unavailable");
                crate::emit(&on_change, &mut last, WifiSnapshot::unavailable());
                tokio::time::sleep(RECONNECT_DELAY).await;
                continue;
            }
        };

        let mut events = match network_manager.network_events().await {
            Ok(events) => events,
            Err(error) => {
                tracing::debug!(%error, "NetworkManager event subscription failed");
                crate::emit(&on_change, &mut last, WifiSnapshot::unavailable());
                tokio::time::sleep(RECONNECT_DELAY).await;
                continue;
            }
        };

        if !publish_snapshot(&network_manager, &on_change, &mut last).await {
            tokio::time::sleep(RECONNECT_DELAY).await;
            continue;
        }

        while let Some(event) = events.next().await {
            match event {
                Ok(_) if !publish_snapshot(&network_manager, &on_change, &mut last).await => break,
                Ok(_) => {}
                Err(error) => {
                    tracing::debug!(%error, "NetworkManager event stream failed");
                    break;
                }
            }
        }

        crate::emit(&on_change, &mut last, WifiSnapshot::unavailable());
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

async fn publish_snapshot(
    network_manager: &NetworkManager,
    on_change: &(impl Fn(WifiSnapshot) + Send + Sync),
    last: &mut Option<WifiSnapshot>,
) -> bool {
    match read_snapshot(network_manager).await {
        Ok(snapshot) => {
            crate::emit(on_change, last, snapshot);
            true
        }
        Err(error) => {
            tracing::debug!(%error, "Wi-Fi snapshot read failed");
            crate::emit(on_change, last, WifiSnapshot::unavailable());
            false
        }
    }
}

pub async fn set_wifi_enabled(enabled: bool) -> Result<(), WifiControlError> {
    let network_manager = NetworkManager::new()
        .await
        .map_err(|_| WifiControlError::Unavailable)?;
    network_manager
        .set_wireless_enabled(enabled)
        .await
        .map_err(|error| {
            tracing::debug!(%error, "could not set Wi-Fi radio state");
            WifiControlError::Failed(error.to_string())
        })
}

pub async fn request_wifi_scan() {
    let Ok(network_manager) = NetworkManager::new().await else {
        return;
    };
    if let Err(error) = network_manager.scan_networks(None).await {
        tracing::debug!(%error, "Wi-Fi scan request failed");
    }
}

pub async fn connect_wifi(ssid: &str, psk: Option<&str>) -> Result<(), WifiConnectError> {
    let network_manager = NetworkManager::new()
        .await
        .map_err(|_| WifiConnectError::Unavailable)?;
    let security = match psk {
        Some(psk) if !psk.is_empty() => WifiSecurity::WpaPsk {
            psk: psk.to_owned(),
        },
        _ => WifiSecurity::Open,
    };
    network_manager
        .connect(ssid, None, security)
        .await
        .map_err(|error| match error {
            ConnectionError::NotFound => WifiConnectError::NotFound,
            ConnectionError::AuthFailed | ConnectionError::MissingPassword => {
                WifiConnectError::AuthFailed
            }
            ConnectionError::Timeout | ConnectionError::SupplicantTimeout => {
                WifiConnectError::Timeout
            }
            other => {
                tracing::debug!(error = %other, "Wi-Fi connect failed");
                WifiConnectError::Failed(other.to_string())
            }
        })
}

pub async fn disconnect_wifi() -> Result<(), WifiControlError> {
    let network_manager = NetworkManager::new()
        .await
        .map_err(|_| WifiControlError::Unavailable)?;
    network_manager.disconnect(None).await.map_err(|error| {
        tracing::debug!(%error, "Wi-Fi disconnect failed");
        WifiControlError::Failed(error.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(ssid: &str, strength: Option<u8>, active: bool) -> WifiNetwork {
        WifiNetwork {
            ssid: ssid.to_owned(),
            strength,
            secured: true,
            active,
            saved: false,
        }
    }

    #[test]
    fn sort_networks_puts_active_first_then_descending_strength() {
        let mut networks = vec![
            network("weak", Some(20), false),
            network("strong", Some(90), false),
            network("active", Some(10), true),
            network("unknown", None, false),
        ];
        sort_networks(&mut networks);
        let order: Vec<&str> = networks.iter().map(|network| network.ssid.as_str()).collect();
        assert_eq!(order, ["active", "strong", "weak", "unknown"]);
    }

    #[test]
    fn unavailable_snapshot_has_no_radio_or_networks() {
        assert_eq!(
            WifiSnapshot::unavailable(),
            WifiSnapshot {
                available: false,
                enabled: false,
                networks: Vec::new(),
            }
        );
    }
}
