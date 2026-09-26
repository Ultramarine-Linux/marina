//! Typed client bindings for InputPlumber's D-Bus API.
//!
//! The interface declarations are intentionally limited to the operations Marina
//! needs. They follow the introspection XML shipped by InputPlumber 0.81.0.

use std::fmt;

use zbus::{
    Connection,
    fdo::ObjectManagerProxy,
    zvariant::{ObjectPath, OwnedObjectPath},
};

pub const BUS_NAME: &str = "org.shadowblip.InputPlumber";
pub const ROOT_PATH: &str = "/org/shadowblip/InputPlumber";
pub const COMPOSITE_DEVICE_INTERFACE: &str = "org.shadowblip.Input.CompositeDevice";
pub const DBUS_DEVICE_INTERFACE: &str = "org.shadowblip.Input.DBusDevice";

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
