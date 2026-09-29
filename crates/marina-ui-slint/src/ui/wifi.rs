use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use tracing::warn;

use marina_networking::{
    WifiConnectError, WifiSnapshot, connect_wifi, disconnect_wifi, monitor_wifi,
    read_wifi_snapshot, request_wifi_scan, set_wifi_enabled,
};

use crate::{MainWindow, WifiNetworkInfo, WifiState};

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(items)))
}

fn apply_snapshot(window: &slint::Weak<MainWindow>, snapshot: WifiSnapshot) {
    let _ = window.upgrade_in_event_loop(move |window| {
        let wifi = window.global::<WifiState>();
        wifi.set_available(snapshot.available);
        wifi.set_enabled(snapshot.enabled);
        wifi.set_networks(model(
            snapshot
                .networks
                .into_iter()
                .map(|network| WifiNetworkInfo {
                    ssid: SharedString::from(network.ssid),
                    strength: network.strength.map_or(-1, i32::from),
                    secured: network.secured,
                    active: network.active,
                    saved: network.saved,
                })
                .collect(),
        ));
    });
}

fn set_connecting(window: &slint::Weak<MainWindow>, connecting: bool, ssid: &str) {
    let ssid = SharedString::from(ssid);
    let _ = window.upgrade_in_event_loop(move |window| {
        let wifi = window.global::<WifiState>();
        wifi.set_connecting(connecting);
        wifi.set_connecting_ssid(ssid);
    });
}

pub(crate) fn initialize(window: &MainWindow) {
    let wifi = window.global::<WifiState>();
    wifi.set_available(false);
    wifi.set_enabled(false);

    let weak = window.as_weak();
    slint::Timer::single_shot(Duration::ZERO, move || {
        tokio::spawn(monitor_wifi(move |snapshot| {
            apply_snapshot(&weak, snapshot);
        }));
    });

    let weak = window.as_weak();
    window.global::<WifiState>().on_set_enabled(move |enabled| {
        let weak = weak.clone();
        tokio::spawn(async move {
            if let Err(error) = set_wifi_enabled(enabled).await {
                warn!(%error, "could not set Wi-Fi radio state");
                crate::ui::notifications::NOTIFICATION.error(format!(
                    "Could not turn Wi-Fi {}: {error}",
                    if enabled { "on" } else { "off" }
                ));
            }
            apply_snapshot(&weak, read_wifi_snapshot().await);
        });
    });

    let weak = window.as_weak();
    window.global::<WifiState>().on_refresh(move || {
        let weak = weak.clone();
        tokio::spawn(async move {
            request_wifi_scan().await;
            tokio::time::sleep(Duration::from_secs(2)).await;
            apply_snapshot(&weak, read_wifi_snapshot().await);
        });
    });

    let weak = window.as_weak();
    window.global::<WifiState>().on_connect(move |ssid, psk| {
        let ssid_string = ssid.to_string();
        let psk = (!psk.is_empty()).then(|| psk.to_string());
        set_connecting(&weak, true, &ssid_string);
        let loading = crate::ui::notifications::NOTIFICATION
            .loading(format!("Connecting to {ssid_string}…"));
        let weak = weak.clone();
        tokio::spawn(async move {
            match connect_wifi(&ssid_string, psk.as_deref()).await {
                Ok(()) => loading.success(format!("Connected to {ssid_string}")),
                Err(WifiConnectError::AuthFailed) => loading
                    .error(format!("Could not connect to {ssid_string}: incorrect password")),
                Err(error) => {
                    loading.error(format!("Could not connect to {ssid_string}: {error}"))
                }
            }
            set_connecting(&weak, false, "");
            apply_snapshot(&weak, read_wifi_snapshot().await);
        });
    });

    let weak = window.as_weak();
    window.global::<WifiState>().on_disconnect(move || {
        let weak = weak.clone();
        tokio::spawn(async move {
            if let Err(error) = disconnect_wifi().await {
                warn!(%error, "Wi-Fi disconnect failed");
                crate::ui::notifications::NOTIFICATION
                    .error(format!("Could not disconnect Wi-Fi: {error}"));
            }
            apply_snapshot(&weak, read_wifi_snapshot().await);
        });
    });
}
