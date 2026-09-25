//! Shared tracing setup for Marina executable targets.
//!
//! [`init`] loads a local `.env` file before reading `RUST_LOG`. It sends
//! structured events directly to journald for systemd services and uses a
//! formatted terminal subscriber for interactive launches. It is safe to call
//! from binaries that may run under a test harness or another host that has
//! already installed a global subscriber.

use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

/// Loads environment configuration and installs the workspace tracing subscriber.
///
/// `RUST_LOG` controls the filter. systemd sets `INVOCATION_ID` for processes
/// it starts as a unit, which selects direct structured journald logging over
/// terminal output. A pre-existing global subscriber is left in place.
pub fn init() {
    dotenvy::dotenv().ok();
    let filter = EnvFilter::from_default_env();

    if is_systemd_service() {
        match tracing_journald::layer() {
            Ok(journald) => {
                let _ = tracing_subscriber::registry()
                    .with(filter)
                    .with(journald)
                    .try_init();
                return;
            }
            Err(error) => {
                eprintln!(
                    "failed to initialize journald logging; falling back to terminal logging: {error}"
                );
            }
        }
    }

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .try_init();
}

fn is_systemd_service() -> bool {
    std::env::var_os("INVOCATION_ID").is_some_and(|value| !value.is_empty())
}
