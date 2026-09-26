//! Typed client bindings for InputPlumber's D-Bus API.
//!
//! The interface declarations are intentionally limited to the operations Marina
//! needs. They follow the introspection XML shipped by InputPlumber 0.81.0.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    time::Duration,
};

use futures_util::StreamExt;
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};
use tracing::{debug, info, warn};
use zbus::{
    Connection,
    fdo::ObjectManagerProxy,
    zvariant::{ObjectPath, OwnedObjectPath},
};

use crate::{InputAction, InputEvent as SemanticInputEvent, InputEventKind};

pub const BUS_NAME: &str = "org.shadowblip.InputPlumber";
pub const ROOT_PATH: &str = "/org/shadowblip/InputPlumber";
pub const COMPOSITE_DEVICE_INTERFACE: &str = "org.shadowblip.Input.CompositeDevice";
pub const DBUS_DEVICE_INTERFACE: &str = "org.shadowblip.Input.DBusDevice";

const RECONNECT_DELAY: Duration = Duration::from_secs(2);
// Object topology changes rarely on the built-in handheld controller. Keep a
// low-frequency fallback reconciliation without continuously loading
// InputPlumber's comparatively expensive ObjectManager/property path.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(5);

/// Controls which events InputPlumber forwards to its ordinary virtual devices.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u32)]
pub enum InterceptMode {
    /// Forward all events to the ordinary virtual devices.
    #[default]
    None = 0,
    /// Forward normal events, but reserve Guide to enter [`Self::All`].
    Pass = 1,
    /// Route all events only to InputPlumber's D-Bus target.
    All = 2,
    /// Route gamepad events only to InputPlumber's D-Bus target.
    GamepadOnly = 3,
}

impl From<InterceptMode> for u32 {
    fn from(mode: InterceptMode) -> Self {
        mode as u32
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidInterceptMode(pub u32);

impl fmt::Display for InvalidInterceptMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid InputPlumber intercept mode: {}", self.0)
    }
}

impl std::error::Error for InvalidInterceptMode {}

impl TryFrom<u32> for InterceptMode {
    type Error = InvalidInterceptMode;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Pass),
            2 => Ok(Self::All),
            3 => Ok(Self::GamepadOnly),
            value => Err(InvalidInterceptMode(value)),
        }
    }
}

/// The subset of `org.shadowblip.Input.CompositeDevice` used by Marina.
#[zbus::proxy(
    interface = "org.shadowblip.Input.CompositeDevice",
    default_service = "org.shadowblip.InputPlumber"
)]
pub trait CompositeDevice {
    /// Paths of the D-Bus event targets attached to this composite device.
    #[zbus(property, name = "DbusDevices")]
    fn dbus_devices(&self) -> zbus::Result<Vec<String>>;

    /// Current InputPlumber interception mode.
    #[zbus(property)]
    fn intercept_mode(&self) -> zbus::Result<u32>;

    /// Change the InputPlumber interception mode.
    #[zbus(property)]
    fn set_intercept_mode(&self, mode: u32) -> zbus::Result<()>;
}

/// The subset of `org.shadowblip.Input.DBusDevice` used by Marina.
#[zbus::proxy(
    interface = "org.shadowblip.Input.DBusDevice",
    default_service = "org.shadowblip.InputPlumber"
)]
pub trait DbusDevice {
    /// Emitted for an intercepted input capability and its normalized value.
    #[zbus(signal)]
    fn input_event(&self, event: String, value: f64) -> zbus::Result<()>;
}

/// Connection and discovery helper for InputPlumber's system-bus objects.
#[derive(Clone, Debug)]
pub struct Client {
    connection: Connection,
}

impl Client {
    /// Connect to InputPlumber over the system bus.
    pub async fn connect() -> zbus::Result<Self> {
        Ok(Self::new(Connection::system().await?))
    }

    /// Wrap an existing system-bus connection.
    pub fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Return every object currently exporting InputPlumber's composite-device
    /// interface. Callers should also monitor ObjectManager signals when they
    /// need to track hotplug or daemon restarts.
    pub async fn composite_devices(&self) -> zbus::Result<Vec<OwnedObjectPath>> {
        let manager = ObjectManagerProxy::builder(&self.connection)
            .destination(BUS_NAME)?
            .path(ROOT_PATH)?
            .build()
            .await?;
        let objects = manager.get_managed_objects().await?;
        let mut paths = objects
            .into_iter()
            .filter_map(|(path, interfaces)| {
                interfaces
                    .contains_key(COMPOSITE_DEVICE_INTERFACE)
                    .then_some(path)
            })
            .collect::<Vec<_>>();
        paths.sort_unstable_by(|left, right| left.as_str().cmp(right.as_str()));
        Ok(paths)
    }

    /// Build a proxy for a discovered composite device.
    pub async fn composite_device<'a>(
        &'a self,
        path: &'a ObjectPath<'_>,
    ) -> zbus::Result<CompositeDeviceProxy<'a>> {
        CompositeDeviceProxy::builder(&self.connection)
            .path(path)?
            .build()
            .await
    }

    /// Build a proxy for a D-Bus event target returned by
    /// [`CompositeDeviceProxy::dbus_devices`].
    pub async fn dbus_device<'a>(
        &'a self,
        path: &'a ObjectPath<'_>,
    ) -> zbus::Result<DbusDeviceProxy<'a>> {
        DbusDeviceProxy::builder(&self.connection)
            .path(path)?
            .build()
            .await
    }

    /// Set the interception mode for every currently managed controller.
    pub async fn set_intercept_mode(&self, mode: InterceptMode) -> zbus::Result<()> {
        for path in self.composite_devices().await? {
            self.composite_device(&path)
                .await?
                .set_intercept_mode(mode.into())
                .await?;
        }
        Ok(())
    }
}

/// Cloneable control handle for the InputPlumber interception monitor.
#[derive(Clone, Debug)]
pub struct InterceptControl {
    mode: watch::Sender<InterceptMode>,
}

impl InterceptControl {
    /// Request a new interception mode. The monitor applies it to current
    /// devices immediately and to hotplugged devices during reconciliation.
    pub fn set_mode(&self, mode: InterceptMode) {
        self.mode.send_if_modified(|current| {
            if *current == mode {
                return false;
            }
            *current = mode;
            true
        });
    }

    /// Record a mode transition performed internally by InputPlumber without
    /// sending the same property change back over D-Bus.
    pub fn observe_mode(&self, mode: InterceptMode) {
        self.mode.send_if_modified(|current| {
            *current = mode;
            false
        });
    }

    pub fn mode(&self) -> InterceptMode {
        *self.mode.borrow()
    }
}

/// Create the control channel consumed by [`monitor_input_events`].
pub fn intercept_control(
    initial_mode: InterceptMode,
) -> (InterceptControl, watch::Receiver<InterceptMode>) {
    let (mode, receiver) = watch::channel(initial_mode);
    (InterceptControl { mode }, receiver)
}

/// Monitor InputPlumber controllers, reconnect across daemon restarts, and emit
/// Marina semantic input events from each attached D-Bus target.
///
/// Dropping every [`InterceptControl`] ends the monitor. The image's Polkit
/// policy must allow Marina to set `InterceptMode` without interaction.
pub async fn monitor_input_events(
    mut modes: watch::Receiver<InterceptMode>,
    mut handler: impl FnMut(SemanticInputEvent) + Send + 'static,
) {
    let (events_tx, mut events_rx) = mpsc::unbounded_channel::<(String, f64)>();
    let dispatcher = tokio::spawn(async move {
        while let Some((event, value)) = events_rx.recv().await {
            if let Some(semantic) = semantic_event(&event, value) {
                debug!(raw_event = %event, value, action = ?semantic.action, kind = ?semantic.kind, "received InputPlumber input event");
                handler(semantic);
            }
        }
    });

    loop {
        match Client::connect().await {
            Ok(client) => {
                if !monitor_connection(&client, &mut modes, &events_tx).await {
                    break;
                }
                debug!("InputPlumber connection was lost; reconnecting");
            }
            Err(error) => {
                debug!(%error, "InputPlumber is unavailable; retrying");
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(RECONNECT_DELAY) => {}
            changed = modes.changed() => {
                if changed.is_err() {
                    break;
                }
            }
        }
    }

    drop(events_tx);
    let _ = dispatcher.await;
}

async fn monitor_connection(
    client: &Client,
    modes: &mut watch::Receiver<InterceptMode>,
    events_tx: &mpsc::UnboundedSender<(String, f64)>,
) -> bool {
    let mut targets: HashMap<String, JoinHandle<()>> = HashMap::new();
    let mut composites = Vec::new();
    let mut announced = false;
    let mut interval = tokio::time::interval(RECONCILE_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let keep_running = loop {
        tokio::select! {
            _ = interval.tick() => {
                let mode = *modes.borrow();
                match reconcile(client, mode, &composites, events_tx, &mut targets).await {
                    Ok(paths) => {
                        if !announced {
                            info!(devices = paths.len(), "connected to InputPlumber");
                            announced = true;
                        }
                        composites = paths;
                    }
                    Err(error) => {
                        debug!(%error, "failed to reconcile InputPlumber devices");
                        break true;
                    }
                }
            }
            changed = modes.changed() => {
                if changed.is_err() {
                    break false;
                }
                let mode = *modes.borrow_and_update();
                for path in &composites {
                    match client.composite_device(path).await {
                        Ok(proxy) => {
                            if let Err(error) = proxy.set_intercept_mode(mode.into()).await {
                                warn!(%error, device = %path, ?mode, "failed to change InputPlumber interception mode");
                            }
                        }
                        Err(error) => {
                            debug!(%error, device = %path, "InputPlumber device disappeared while changing mode");
                        }
                    }
                }
            }

        }
    };

    for (_, task) in targets {
        task.abort();
    }
    keep_running
}

async fn reconcile(
    client: &Client,
    mode: InterceptMode,
    known_composites: &[OwnedObjectPath],
    events: &mpsc::UnboundedSender<(String, f64)>,
    targets: &mut HashMap<String, JoinHandle<()>>,
) -> zbus::Result<Vec<OwnedObjectPath>> {
    let composites = client.composite_devices().await?;
    let known_composites = known_composites
        .iter()
        .map(|path| path.as_str())
        .collect::<HashSet<_>>();
    let mut active_targets = HashSet::new();

    for path in &composites {
        let composite = client.composite_device(path).await?;
        for target in composite.dbus_devices().await? {
            active_targets.insert(target.clone());
            if !targets.contains_key(&target) {
                let task = spawn_target_listener(
                    client.connection().clone(),
                    target.clone(),
                    events.clone(),
                );
                targets.insert(target, task);
            }
        }
        if !known_composites.contains(path.as_str()) {
            composite.set_intercept_mode(mode.into()).await?;
        }
    }

    targets.retain(|path, task| {
        if active_targets.contains(path) && !task.is_finished() {
            true
        } else {
            task.abort();
            false
        }
    });
    Ok(composites)
}

fn spawn_target_listener(
    connection: Connection,
    path: String,
    events: mpsc::UnboundedSender<(String, f64)>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let builder = match DbusDeviceProxy::builder(&connection).path(path.as_str()) {
            Ok(builder) => builder,
            Err(error) => {
                debug!(%error, device = %path, "invalid InputPlumber event target path");
                return;
            }
        };
        let proxy = match builder.build().await {
            Ok(proxy) => proxy,
            Err(error) => {
                debug!(%error, device = %path, "failed to build InputPlumber event proxy");
                return;
            }
        };
        let mut stream = match proxy.receive_input_event().await {
            Ok(stream) => stream,
            Err(error) => {
                debug!(%error, device = %path, "failed to subscribe to InputPlumber events");
                return;
            }
        };

        while let Some(signal) = stream.next().await {
            match signal.args() {
                Ok(args) => {
                    if events
                        .send((args.event().to_owned(), *args.value()))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(error) => {
                    debug!(%error, device = %path, "ignored malformed InputPlumber event");
                }
            }
        }
    })
}

fn semantic_event(event: &str, value: f64) -> Option<SemanticInputEvent> {
    let action = match event {
        "ui_guide" | "ui_option" => InputAction::Menu,
        "ui_accept" => InputAction::Accept,
        "ui_back" => InputAction::Back,
        "ui_context" => InputAction::Context,
        "ui_up" => InputAction::Up,
        "ui_down" => InputAction::Down,
        "ui_left" => InputAction::Left,
        "ui_right" => InputAction::Right,
        "ui_l1" => InputAction::PreviousTab,
        "ui_r1" => InputAction::NextTab,
        "ui_l2" => InputAction::PageUp,
        "ui_r2" => InputAction::PageDown,
        _ => return None,
    };
    Some(SemanticInputEvent {
        action,
        kind: if value.abs() > f64::EPSILON {
            InputEventKind::Pressed
        } else {
            InputEventKind::Released
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intercept_modes_match_inputplumber_wire_values() {
        for (raw, mode) in [
            (0, InterceptMode::None),
            (1, InterceptMode::Pass),
            (2, InterceptMode::All),
            (3, InterceptMode::GamepadOnly),
        ] {
            assert_eq!(InterceptMode::try_from(raw), Ok(mode));
            assert_eq!(u32::from(mode), raw);
        }
        assert_eq!(InterceptMode::try_from(4), Err(InvalidInterceptMode(4)));
    }

    #[test]
    fn observed_modes_do_not_trigger_redundant_dbus_updates() {
        let (control, receiver) = intercept_control(InterceptMode::Pass);
        assert!(!receiver.has_changed().expect("mode sender"));

        control.observe_mode(InterceptMode::All);
        assert_eq!(control.mode(), InterceptMode::All);
        assert!(!receiver.has_changed().expect("mode sender"));

        control.set_mode(InterceptMode::Pass);
        assert!(receiver.has_changed().expect("mode sender"));
    }

    #[test]
    fn dbus_actions_map_to_semantic_input() {
        assert_eq!(
            semantic_event("ui_guide", 1.0),
            Some(SemanticInputEvent {
                action: InputAction::Menu,
                kind: InputEventKind::Pressed,
            })
        );
        assert_eq!(
            semantic_event("ui_accept", 0.0),
            Some(SemanticInputEvent {
                action: InputAction::Accept,
                kind: InputEventKind::Released,
            })
        );
        assert_eq!(
            semantic_event("ui_context", 1.0),
            Some(SemanticInputEvent {
                action: InputAction::Context,
                kind: InputEventKind::Pressed,
            })
        );
        assert_eq!(semantic_event("ui_quick", 1.0), None);
    }

    #[test]
    fn inputplumber_object_names_match_the_published_api() {
        assert_eq!(BUS_NAME, "org.shadowblip.InputPlumber");
        assert_eq!(ROOT_PATH, "/org/shadowblip/InputPlumber");
        assert_eq!(
            COMPOSITE_DEVICE_INTERFACE,
            "org.shadowblip.Input.CompositeDevice"
        );
        assert_eq!(DBUS_DEVICE_INTERFACE, "org.shadowblip.Input.DBusDevice");
    }
}
