//! Network-status hydration for the AppShell top bar.
//!
//! NetworkManager D-Bus integration, event subscriptions, and reconnection live
//! in `marina-networking`; this module only moves plain [`NetworkStatus`]
//! snapshots into the applet-owned `NetworkState` global.

use slint::ComponentHandle;

use marina_networking::{NetworkStatus, monitor_network_status};

use crate::{MainWindow, NetworkState};

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
        tokio::spawn(monitor_network_status(move |status| {
            apply_status(&weak, status);
        }));
    });
}
