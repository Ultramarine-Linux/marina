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
/// `RUST_LOG` controls the filter. When stdout is connected to the journal by
/// systemd, the matching `JOURNAL_STREAM` marker selects direct structured
/// journald logging over terminal output. A pre-existing global subscriber is
/// left in place.
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
    // Both INVOCATION_ID and JOURNAL_STREAM can be inherited by an interactive
    // child process. JOURNAL_STREAM is only authoritative when it identifies
    // this process's current stdout, rather than the parent service's stream.
    let Some(stream) = std::env::var_os("JOURNAL_STREAM") else {
        return false;
    };
    let Some((device, inode)) = stream
        .to_str()
        .and_then(|stream| stream.split_once(':'))
        .and_then(|(device, inode)| {
            Some((device.parse::<u64>().ok()?, inode.parse::<u64>().ok()?))
        })
    else {
        return false;
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let Ok(stdout) = std::fs::metadata("/proc/self/fd/1") else {
            return false;
        };
        stdout.dev() == device && stdout.ino() == inode
    }

    #[cfg(not(unix))]
    {
        let _ = (device, inode);
        false
    }
}
