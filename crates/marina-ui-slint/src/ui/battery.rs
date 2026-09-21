//! Battery level hydration for the AppShell top bar.
//!
//! The UPower D-Bus backend lives in `marina-power`, including live
//! `PropertiesChanged` / `DeviceAdded` / `DeviceRemoved` signal monitoring;
//! this module only moves [`marina_power::BatteryStatus`] snapshots into
//! `ShellState`. The indicator stays hidden when no battery is reported, so
//! desktops without a battery simply show no icon.

use slint::ComponentHandle;

use marina_power::{BatteryStatus, monitor_battery_status};

use crate::{MainWindow, ShellState};

pub(crate) fn initialize(window: &MainWindow) {
    window.global::<ShellState>().set_battery_available(false);
    window.global::<ShellState>().set_battery_percentage(100.0);
    window.global::<ShellState>().set_battery_charging(false);

    if let Some(fake) = fake_battery_status() {
        tracing::warn!(
            percentage = fake.percentage,
            charging = fake.charging,
            "MARINA_FAKE_BATTERY set; showing dummy battery level"
        );
        apply_status(&window.as_weak(), fake);
        return;
    }

    let weak = window.as_weak();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        // The monitor reports the initial status immediately, then live
        // signal updates with a slow poll fallback. It runs for the
        // lifetime of the process; closing the window exits main and drops
        // the task with the runtime.
        tokio::spawn(monitor_battery_status(move |status| {
            apply_status(&weak, status);
        }));
    });
}

fn apply_status(window: &slint::Weak<MainWindow>, status: BatteryStatus) {
    let _ = window.upgrade_in_event_loop(move |window| {
        let shell = window.global::<ShellState>();
        shell.set_battery_available(status.available);
        shell.set_battery_percentage(status.percentage as f32);
        shell.set_battery_charging(status.charging);
    });
}

/// Debug dummy: `MARINA_FAKE_BATTERY=85` or `MARINA_FAKE_BATTERY=42,charging`
/// forces the indicator visible with a fake level on machines without a
/// battery, for visually inspecting the widget. Unset for real UPower
/// readings.
fn fake_battery_status() -> Option<BatteryStatus> {
    let raw = std::env::var("MARINA_FAKE_BATTERY").ok()?;
    let (percentage, suffix) = match raw.split_once(',') {
        Some((percentage, suffix)) => (percentage, Some(suffix)),
        None => (raw.as_str(), None),
    };
    let percentage: f64 = percentage.trim().parse().ok()?;
    Some(BatteryStatus {
        available: true,
        percentage: percentage.clamp(0.0, 100.0),
        charging: suffix.is_some_and(|s| s.trim().eq_ignore_ascii_case("charging")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the tests that mutate process environment; Rust runs
    /// tests on threads sharing one environment.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_fake_env(value: Option<&str>, f: impl FnOnce()) {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os("MARINA_FAKE_BATTERY");
        // SAFETY: guarded by ENV_LOCK and this is the only test touching
        // MARINA_FAKE_BATTERY.
        unsafe {
            match value {
                Some(value) => std::env::set_var("MARINA_FAKE_BATTERY", value),
                None => std::env::remove_var("MARINA_FAKE_BATTERY"),
            }
        }
        f();
        unsafe {
            match previous {
                Some(value) => std::env::set_var("MARINA_FAKE_BATTERY", value),
                None => std::env::remove_var("MARINA_FAKE_BATTERY"),
            }
        }
    }

    #[test]
    fn fake_battery_parses_plain_percentage() {
        with_fake_env(Some("85"), || {
            assert_eq!(
                fake_battery_status(),
                Some(BatteryStatus {
                    available: true,
                    percentage: 85.0,
                    charging: false,
                })
            );
        });
    }

    #[test]
    fn fake_battery_parses_charging_suffix() {
        with_fake_env(Some("42,charging"), || {
            let status = fake_battery_status().expect("must parse");
            assert!(status.available);
            assert_eq!(status.percentage, 42.0);
            assert!(status.charging);
        });
    }

    #[test]
    fn fake_battery_clamps_and_rejects_garbage() {
        with_fake_env(Some("150"), || {
            assert_eq!(fake_battery_status().map(|s| s.percentage), Some(100.0));
        });
        with_fake_env(Some("full"), || {
            assert_eq!(fake_battery_status(), None);
        });
        with_fake_env(None, || {
            assert_eq!(fake_battery_status(), None);
        });
    }
}
