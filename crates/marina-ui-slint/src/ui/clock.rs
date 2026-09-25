//! Digital clock hydration for the AppShell top bar.
//!
//! Publishes the local time into `ShellState.clock-time`: 24-hour `HH:MM` by
//! default, or 12-hour `h:MM AM/PM` when `[clock] twelve_hour` (or `12hr`) is
//! set.

use chrono::Timelike;
use slint::{ComponentHandle, SharedString};

use crate::{MainWindow, ShellState, config};

/// Poll once per second so minute transitions appear promptly and system time
/// changes are reflected without waiting for a minute-long interval.
const TICK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Formats a clock reading: `HH:MM` in 24-hour mode, `h:MM AM/PM` when
/// `twelve_hour` is set.
pub(crate) fn format_clock(hours: u32, minutes: u32, twelve_hour: bool) -> String {
    if !twelve_hour {
        return format!("{hours:02}:{minutes:02}");
    }
    let period = if hours < 12 { "AM" } else { "PM" };
    let hour = match hours % 12 {
        0 => 12,
        hour => hour,
    };
    format!("{hour}:{minutes:02} {period}")
}

/// Returns the current local time as a display string.
pub(crate) fn current_time_string(twelve_hour: bool) -> String {
    let now = chrono::Local::now();
    format_clock(now.hour(), now.minute(), twelve_hour)
}

fn apply_time(window: &slint::Weak<MainWindow>, time: String) {
    let _ = window.upgrade_in_event_loop(move |window| {
        window
            .global::<ShellState>()
            .set_clock_time(SharedString::from(time));
    });
}

pub(crate) fn initialize(window: &MainWindow) {
    window
        .global::<ShellState>()
        .set_clock_time(SharedString::from(current_time_string(false)));

    let weak = window.as_weak();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        tokio::spawn(async move {
            tracing::debug!("clock task started");
            let mut last = current_time_string(config::shared().snapshot().clock_twelve_hour);
            apply_time(&weak, last.clone());
            let mut interval = tokio::time::interval(TICK_INTERVAL);
            loop {
                interval.tick().await;
                // `Weak::upgrade()` only succeeds on the Slint event-loop
                // thread. This task runs on a Tokio worker, so dispatch the
                // update directly and let `upgrade_in_event_loop()` skip it
                // when the window no longer exists.
                let now = current_time_string(config::shared().snapshot().clock_twelve_hour);
                if now != last {
                    last = now.clone();
                    apply_time(&weak, now);
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_single_digits_24h() {
        assert_eq!(format_clock(9, 5, false), "09:05");
    }

    #[test]
    fn keeps_double_digits_24h() {
        assert_eq!(format_clock(23, 59, false), "23:59");
    }

    #[test]
    fn midnight_is_zeroed_24h() {
        assert_eq!(format_clock(0, 0, false), "00:00");
    }

    #[test]
    fn morning_12h() {
        assert_eq!(format_clock(9, 5, true), "9:05 AM");
    }

    #[test]
    fn afternoon_12h() {
        assert_eq!(format_clock(21, 5, true), "9:05 PM");
    }

    #[test]
    fn noon_and_midnight_12h() {
        assert_eq!(format_clock(12, 0, true), "12:00 PM");
        assert_eq!(format_clock(0, 0, true), "12:00 AM");
    }
}
