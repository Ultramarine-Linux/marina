//! Digital clock hydration for the shared clock indicator.

use slint::{ComponentHandle, SharedString};

use crate::{ClockState, MainWindow, config};

const TICK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

fn apply_time(window: &slint::Weak<MainWindow>, time: String) {
    let _ = window.upgrade_in_event_loop(move |window| {
        window
            .global::<ClockState>()
            .set_time(SharedString::from(time));
    });
}

pub(crate) fn initialize(window: &MainWindow) {
    let twelve_hour = config::shared().snapshot().clock_twelve_hour;
    window.global::<ClockState>().set_time(SharedString::from(
        marina_ui_slint::clock_format::current_time_string(twelve_hour),
    ));

    let weak = window.as_weak();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        tokio::spawn(async move {
            tracing::debug!("clock task started");
            let mut last = marina_ui_slint::clock_format::current_time_string(
                config::shared().snapshot().clock_twelve_hour,
            );
            apply_time(&weak, last.clone());
            let mut interval = tokio::time::interval(TICK_INTERVAL);
            loop {
                interval.tick().await;
                let now = marina_ui_slint::clock_format::current_time_string(
                    config::shared().snapshot().clock_twelve_hour,
                );
                if now != last {
                    last = now.clone();
                    apply_time(&weak, now);
                }
            }
        });
    });
}
