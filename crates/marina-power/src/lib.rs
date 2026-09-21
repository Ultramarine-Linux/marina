//! System power state for Marina.
//!
//! This crate owns the UPower D-Bus backend and exposes only plain battery
//! snapshots. UI crates should depend on [`BatteryStatus`] and
//! [`monitor_battery_status`] rather than touching D-Bus types directly.
//!
//! The composite DisplayDevice (`org.freedesktop.UPower.GetDisplayDevice` +
//! `org.freedesktop.UPower.Device` properties) aggregates the system battery.
//! A missing daemon, missing system bus, or absent battery all resolve to an
//! unavailable status so callers can hide their indicator.

use std::time::Duration;

use futures_util::StreamExt;

/// Interval for the fallback re-read. Live `PropertiesChanged` /
/// `DeviceAdded` / `DeviceRemoved` signals normally deliver updates instantly;
/// the poll only guards against a missed signal.
const FALLBACK_POLL: Duration = Duration::from_secs(30);
/// Delay before retrying the system bus after a connection failure.
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

/// UPower `org.freedesktop.UPower.Device` state values.
pub const STATE_UNKNOWN: u32 = 0;
pub const STATE_CHARGING: u32 = 1;
pub const STATE_DISCHARGING: u32 = 2;
pub const STATE_EMPTY: u32 = 3;
pub const STATE_FULLY_CHARGED: u32 = 4;
pub const STATE_PENDING_CHARGE: u32 = 5;
pub const STATE_PENDING_DISCHARGE: u32 = 6;

/// Snapshot of the system battery state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatteryStatus {
    /// False when UPower is unreachable or no battery is present.
    pub available: bool,
    /// Battery level in percent (0–100). Only meaningful when `available`.
    pub percentage: f64,
    /// True while the battery is actively charging.
    pub charging: bool,
}

impl BatteryStatus {
    /// Status used when no battery can be reported; callers hide the UI.
    pub fn unavailable() -> Self {
        Self {
            available: false,
            percentage: 0.0,
            charging: false,
        }
    }

    /// True when the battery is low and not charging (default 20% threshold).
    pub fn is_low(&self) -> bool {
        self.available && !self.charging && self.percentage <= 20.0
    }
}

/// Maps raw UPower DisplayDevice properties to a [`BatteryStatus`].
///
/// `is_present` is the `IsPresent` property, `percentage` the `Percentage`
/// property (0–100), and `state` the numeric `State` property.
pub fn status_from_upower(is_present: bool, percentage: f64, state: u32) -> BatteryStatus {
    if !is_present {
        return BatteryStatus::unavailable();
    }
    BatteryStatus {
        available: true,
        percentage: percentage.clamp(0.0, 100.0),
        charging: matches!(state, STATE_CHARGING | STATE_PENDING_CHARGE),
    }
}

#[zbus::proxy(
    interface = "org.freedesktop.UPower",
    default_service = "org.freedesktop.UPower",
    default_path = "/org/freedesktop/UPower"
)]
trait UPower {
    fn get_display_device(&self) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;

    /// Emitted when a power device appears; the DisplayDevice aggregate may
    /// have changed with it.
    #[zbus(signal)]
    fn device_added(&self, device: zbus::zvariant::OwnedObjectPath) -> zbus::Result<()>;

    /// Emitted when a power device disappears; the DisplayDevice aggregate
    /// may have changed with it.
    #[zbus(signal)]
    fn device_removed(&self, device: zbus::zvariant::OwnedObjectPath) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.UPower.Device",
    default_service = "org.freedesktop.UPower"
)]
trait UpowerDevice {
    #[zbus(property)]
    fn percentage(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn is_present(&self) -> zbus::Result<bool>;
}

async fn query_display_device() -> Option<BatteryStatus> {
    let connection = zbus::Connection::system().await.ok()?;
    let upower = UPowerProxy::new(&connection).await.ok()?;
    let path = upower.get_display_device().await.ok()?;
    Some(read_device(&connection, &path).await)
}

async fn read_device(
    connection: &zbus::Connection,
    path: &zbus::zvariant::OwnedObjectPath,
) -> BatteryStatus {
    let Some(device) = build_device_proxy(connection, path).await else {
        return BatteryStatus::unavailable();
    };
    read_device_proxy(&device).await
}

async fn build_device_proxy<'a>(
    connection: &'a zbus::Connection,
    path: &zbus::zvariant::OwnedObjectPath,
) -> Option<UpowerDeviceProxy<'a>> {
    UpowerDeviceProxy::builder(connection)
        .path(path.clone())
        .ok()?
        .build()
        .await
        .ok()
}

async fn read_device_proxy(device: &UpowerDeviceProxy<'_>) -> BatteryStatus {
    let (is_present, percentage, state) =
        tokio::join!(device.is_present(), device.percentage(), device.state(),);
    status_from_upower(
        is_present.unwrap_or(false),
        percentage.unwrap_or(0.0),
        state.unwrap_or(STATE_UNKNOWN),
    )
}

/// Reads the composite UPower DisplayDevice.
///
/// Never fails: any D-Bus error, missing property, or absent battery resolves
/// to [`BatteryStatus::unavailable`].
pub async fn read_battery_status() -> BatteryStatus {
    match query_display_device().await {
        Some(status) => status,
        None => {
            tracing::debug!("UPower DisplayDevice unavailable; hiding battery indicator");
            BatteryStatus::unavailable()
        }
    }
}

/// Outcome of one monitor session: either the DisplayDevice path moved (or a
/// stream ended) and subscriptions must be rebuilt immediately, or the bus
/// failed and the caller should back off before retrying.
enum SessionOutcome {
    Rebuild,
    Backoff,
}

/// Continuously reports battery status: an immediate initial read, live
/// updates from UPower `PropertiesChanged` / `DeviceAdded` / `DeviceRemoved`
/// signals, and a slow poll fallback against missed signals.
///
/// `on_change` fires once with the initial status and again on every change.
/// This future never resolves under normal operation; spawn it as a
/// process-lifetime task.
pub async fn monitor_battery_status(on_change: impl Fn(BatteryStatus) + Send + Sync + 'static) {
    let mut last = None;
    tracing::debug!("UPower monitor started");
    loop {
        match monitor_session(&on_change, &mut last).await {
            SessionOutcome::Rebuild => continue,
            SessionOutcome::Backoff => {
                tracing::debug!("UPower monitor backing off before reconnect");
                tokio::time::sleep(RECONNECT_DELAY).await;
            }
        }
    }
}

async fn monitor_session(
    on_change: &(impl Fn(BatteryStatus) + Send + Sync),
    last: &mut Option<BatteryStatus>,
) -> SessionOutcome {
    let mut emit = |status: BatteryStatus| {
        if *last != Some(status) {
            tracing::debug!(?status, "battery status changed");
            on_change(status);
            *last = Some(status);
        }
    };

    let Ok(connection) = zbus::Connection::system().await else {
        return SessionOutcome::Backoff;
    };
    let Ok(upower) = UPowerProxy::new(&connection).await else {
        return SessionOutcome::Backoff;
    };
    let Ok(path) = upower.get_display_device().await else {
        return SessionOutcome::Backoff;
    };
    let mut path = path;
    let Some(device) = build_device_proxy(&connection, &path).await else {
        return SessionOutcome::Backoff;
    };

    // Subscribe through the generated device-property streams. Besides
    // filtering out unrelated org.freedesktop.DBus.Properties traffic, these
    // streams keep this long-lived proxy's property cache synchronized before
    // waking the monitor. Subscribe before the initial snapshot so a change
    // cannot be lost between reading the device and installing listeners.
    let mut percentage_changed = device.receive_percentage_changed().await;
    let mut state_changed = device.receive_state_changed().await;
    let mut presence_changed = device.receive_is_present_changed().await;
    emit(read_device_proxy(&device).await);

    let Ok(mut added) = upower.receive_device_added().await else {
        return SessionOutcome::Backoff;
    };
    let Ok(mut removed) = upower.receive_device_removed().await else {
        return SessionOutcome::Backoff;
    };
    let mut poll = tokio::time::interval(FALLBACK_POLL);

    loop {
        tokio::select! {
            event = percentage_changed.next() => {
                if event.is_none() {
                    return SessionOutcome::Rebuild;
                }
                tracing::debug!("UPower DisplayDevice percentage changed; re-reading");
                emit(read_device_proxy(&device).await);
            }
            event = state_changed.next() => {
                if event.is_none() {
                    return SessionOutcome::Rebuild;
                }
                tracing::debug!("UPower DisplayDevice charging state changed; re-reading");
                emit(read_device_proxy(&device).await);
            }
            event = presence_changed.next() => {
                if event.is_none() {
                    return SessionOutcome::Rebuild;
                }
                tracing::debug!("UPower DisplayDevice presence changed; re-reading");
                emit(read_device_proxy(&device).await);
            }
            event = added.next() => {
                if event.is_none() {
                    return SessionOutcome::Rebuild;
                }
                tracing::debug!("UPower device added; re-reading DisplayDevice");
                if display_path_moved(&upower, &mut path).await {
                    return SessionOutcome::Rebuild;
                }
                emit(read_device(&connection, &path).await);
            }
            event = removed.next() => {
                if event.is_none() {
                    return SessionOutcome::Rebuild;
                }
                tracing::debug!("UPower device removed; re-reading DisplayDevice");
                if display_path_moved(&upower, &mut path).await {
                    return SessionOutcome::Rebuild;
                }
                emit(read_device(&connection, &path).await);
            }
            _ = poll.tick() => {
                if display_path_moved(&upower, &mut path).await {
                    return SessionOutcome::Rebuild;
                }
                // Build a fresh proxy for the fallback read. Reusing the
                // listener proxy here would only consult its cache and could
                // repeat a stale value when the signal was the thing missed.
                emit(read_device(&connection, &path).await);
            }
        }
    }
}

/// Re-resolves the DisplayDevice path, updating `path` in place. Returns true
/// when the path moved and signal subscriptions must be rebuilt.
async fn display_path_moved(
    upower: &UPowerProxy<'_>,
    path: &mut zbus::zvariant::OwnedObjectPath,
) -> bool {
    let Ok(fresh) = upower.get_display_device().await else {
        return false;
    };
    if fresh != *path {
        *path = fresh;
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_when_not_present() {
        assert_eq!(
            status_from_upower(false, 85.0, STATE_DISCHARGING),
            BatteryStatus::unavailable()
        );
    }

    #[test]
    fn charging_states_map_to_charging() {
        assert!(status_from_upower(true, 50.0, STATE_CHARGING).charging);
        assert!(status_from_upower(true, 50.0, STATE_PENDING_CHARGE).charging);
        assert!(!status_from_upower(true, 50.0, STATE_DISCHARGING).charging);
        // Fully charged is present but not actively charging.
        let full = status_from_upower(true, 100.0, STATE_FULLY_CHARGED);
        assert!(full.available);
        assert!(!full.charging);
    }

    #[test]
    fn percentage_is_clamped() {
        assert_eq!(
            status_from_upower(true, 150.0, STATE_DISCHARGING).percentage,
            100.0
        );
        assert_eq!(
            status_from_upower(true, -5.0, STATE_DISCHARGING).percentage,
            0.0
        );
    }

    #[test]
    fn low_threshold() {
        let low = BatteryStatus {
            available: true,
            percentage: 20.0,
            charging: false,
        };
        assert!(low.is_low());
        assert!(
            !BatteryStatus {
                available: true,
                percentage: 20.0,
                charging: true,
            }
            .is_low()
        );
        assert!(!BatteryStatus::unavailable().is_low());
    }
}
