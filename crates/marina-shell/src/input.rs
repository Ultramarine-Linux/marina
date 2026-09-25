use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use evdev::{Device, EventType, InputEvent, KeyCode};

const DEVICE_SCAN_INTERVAL: Duration = Duration::from_secs(2);
const MODE_RELEASE_SUPPRESSION: Duration = Duration::from_millis(500);

type ShortcutHandler = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShortcutAction {
    LeftMenu,
    RightMenu,
}

struct ShortcutState {
    mode_held: bool,
    mode_chorded: bool,
    suppress_mode_until: Instant,
}

impl ShortcutState {
    fn new() -> Self {
        Self {
            mode_held: false,
            mode_chorded: false,
            suppress_mode_until: Instant::now(),
        }
    }

    fn handle(&mut self, event: InputEvent, now: Instant) -> Option<ShortcutAction> {
        if is_key(event, KeyCode::BTN_MODE) {
            if event.value() == 1 {
                self.mode_held = true;
                self.mode_chorded = false;
            } else if event.value() == 0 {
                let open_left =
                    self.mode_held && !self.mode_chorded && now >= self.suppress_mode_until;
                self.mode_held = false;
                self.mode_chorded = false;
                return open_left.then_some(ShortcutAction::LeftMenu);
            }
        } else if is_homepage_release(event) {
            self.suppress_mode_until = now + MODE_RELEASE_SUPPRESSION;
            return Some(ShortcutAction::RightMenu);
        } else if self.mode_held && is_key_release(event, KeyCode::BTN_SOUTH) {
            self.mode_chorded = true;
            self.suppress_mode_until = now + MODE_RELEASE_SUPPRESSION;
            return Some(ShortcutAction::RightMenu);
        }
        None
    }
}

/// Watches readable evdev devices for Marina's global overlay shortcuts.
///
/// Releasing MODE opens the left game menu. Releasing South while MODE is held
/// opens right-side quick settings and suppresses the following MODE release.
/// `KEY_HOMEPAGE` release also opens quick settings. State is shared across
/// event nodes because handheld firmware may report one button on several
/// devices. Devices are not grabbed and newly connected devices are rescanned.
pub fn spawn_overlay_shortcut_listener(
    left_menu_handler: impl Fn() + Send + Sync + 'static,
    right_menu_handler: impl Fn() + Send + Sync + 'static,
) -> io::Result<JoinHandle<()>> {
    let left_menu_handler = Arc::new(left_menu_handler) as ShortcutHandler;
    let right_menu_handler = Arc::new(right_menu_handler) as ShortcutHandler;
    let shortcut_state = Arc::new(Mutex::new(ShortcutState::new()));
    thread::Builder::new()
        .name("marina-overlay-shortcut-discovery".to_owned())
        .spawn(move || {
            let mut workers: HashMap<PathBuf, JoinHandle<()>> = HashMap::new();
            loop {
                workers.retain(|path, worker| {
                    if worker.is_finished() {
                        tracing::debug!(path = %path.display(), "overlay input device disconnected");
                        false
                    } else {
                        true
                    }
                });

                for (path, device) in evdev::enumerate() {
                    if workers.contains_key(&path) || !supports_shortcuts(&device) {
                        continue;
                    }
                    let worker_path = path.clone();
                    let left_menu_handler = left_menu_handler.clone();
                    let right_menu_handler = right_menu_handler.clone();
                    let shortcut_state = shortcut_state.clone();
                    match thread::Builder::new()
                        .name("marina-overlay-shortcuts".to_owned())
                        .spawn(move || {
                            watch_device(
                                worker_path,
                                device,
                                shortcut_state,
                                left_menu_handler,
                                right_menu_handler,
                            )
                        }) {
                        Ok(worker) => {
                            tracing::info!(path = %path.display(), "listening for overlay shortcuts");
                            workers.insert(path, worker);
                        }
                        Err(error) => {
                            tracing::warn!(%error, path = %path.display(), "failed to start overlay shortcut listener");
                        }
                    }
                }

                thread::sleep(DEVICE_SCAN_INTERVAL);
            }
        })
}

fn supports_shortcuts(device: &Device) -> bool {
    device.supported_keys().is_some_and(|keys| {
        keys.contains(KeyCode::KEY_HOMEPAGE) || keys.contains(KeyCode::BTN_MODE)
    })
}

fn watch_device(
    path: PathBuf,
    mut device: Device,
    shortcut_state: Arc<Mutex<ShortcutState>>,
    left_menu_handler: ShortcutHandler,
    right_menu_handler: ShortcutHandler,
) {
    loop {
        match device.fetch_events() {
            Ok(events) => {
                for event in events {
                    let action = shortcut_state
                        .lock()
                        .expect("shortcut state lock poisoned")
                        .handle(event, Instant::now());
                    match action {
                        Some(ShortcutAction::LeftMenu) => {
                            tracing::debug!(path = %path.display(), "BTN_MODE released");
                            left_menu_handler();
                        }
                        Some(ShortcutAction::RightMenu) => {
                            tracing::debug!(path = %path.display(), "quick-settings shortcut released");
                            right_menu_handler();
                        }
                        None => {}
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "overlay shortcut listener stopped");
                return;
            }
        }
    }
}

fn is_key(event: InputEvent, key: KeyCode) -> bool {
    event.event_type() == EventType::KEY && event.code() == key.0
}

fn is_key_release(event: InputEvent, key: KeyCode) -> bool {
    is_key(event, key) && event.value() == 0
}

fn is_homepage_release(event: InputEvent) -> bool {
    is_key_release(event, KeyCode::KEY_HOMEPAGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(key: KeyCode, value: i32) -> InputEvent {
        InputEvent::new(EventType::KEY.0, key.0, value)
    }

    #[test]
    fn homepage_release_opens_right_and_suppresses_mode_release() {
        let start = Instant::now();
        let mut state = ShortcutState::new();
        assert_eq!(state.handle(event(KeyCode::BTN_MODE, 1), start), None);
        assert_eq!(
            state.handle(event(KeyCode::KEY_HOMEPAGE, 0), start),
            Some(ShortcutAction::RightMenu)
        );
        assert_eq!(state.handle(event(KeyCode::BTN_MODE, 0), start), None);
    }

    #[test]
    fn mode_release_opens_left_menu() {
        let start = Instant::now();
        let mut state = ShortcutState::new();
        state.handle(event(KeyCode::BTN_MODE, 1), start);
        assert_eq!(
            state.handle(event(KeyCode::BTN_MODE, 0), start),
            Some(ShortcutAction::LeftMenu)
        );
    }

    #[test]
    fn mode_south_chord_opens_only_right_menu() {
        let start = Instant::now();
        let mut state = ShortcutState::new();
        state.handle(event(KeyCode::BTN_MODE, 1), start);
        assert_eq!(
            state.handle(event(KeyCode::BTN_SOUTH, 0), start),
            Some(ShortcutAction::RightMenu)
        );
        assert_eq!(state.handle(event(KeyCode::BTN_MODE, 0), start), None);
    }
}
