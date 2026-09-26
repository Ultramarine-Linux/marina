//! Marina's typed systemd D-Bus integration boundary.

use zbus_systemd::{
    systemd1::ManagerProxy,
    zbus,
    zvariant::{self, OwnedObjectPath, OwnedValue, Value},
};

pub use zbus_systemd;

const JOB_MODE_REPLACE: &str = "replace";
const COLLECT_MODE: &str = "inactive-or-failed";

/// Description of a transient user service to start.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransientService {
    pub unit_name: String,
    pub description: String,
    pub executable: String,
    pub arguments: Vec<String>,
    pub working_directory: Option<String>,
    pub slice: String,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("systemd D-Bus operation failed: {0}")]
    Dbus(#[from] zbus::Error),
    #[error("failed to encode a systemd unit property: {0}")]
    Property(#[from] zvariant::Error),
}

/// Start an installed unit through the per-user systemd manager.
pub async fn start_user_unit(unit_name: &str) -> Result<OwnedObjectPath, Error> {
    let connection = zbus::Connection::session().await?;
    let manager = ManagerProxy::new(&connection).await?;
    Ok(manager
        .start_unit(unit_name.to_owned(), JOB_MODE_REPLACE.to_owned())
        .await?)
}

/// Return whether an installed or transient user unit is still active.
///
/// `ListUnitsByNames` only includes loaded units; a collected transient unit is
/// absent and is therefore considered inactive.
pub async fn user_unit_is_active(unit_name: &str) -> Result<bool, Error> {
    let connection = zbus::Connection::session().await?;
    let manager = ManagerProxy::new(&connection).await?;
    let units = manager
        .list_units_by_names(vec![unit_name.to_owned()])
        .await?;
    Ok(units.into_iter().any(|unit| unit_state_is_active(&unit.3)))
}

fn unit_state_is_active(state: &str) -> bool {
    matches!(
        state,
        "active" | "activating" | "reloading" | "deactivating"
    )
}

/// Create and start a transient service through the per-user systemd manager.
pub async fn start_transient_user_service(
    service: &TransientService,
) -> Result<OwnedObjectPath, Error> {
    let connection = zbus::Connection::session().await?;
    let manager = ManagerProxy::new(&connection).await?;
    let properties = transient_service_properties(service)?;
    Ok(manager
        .start_transient_unit(
            service.unit_name.clone(),
            JOB_MODE_REPLACE.to_owned(),
            properties,
            Vec::new(),
        )
        .await?)
}

fn transient_service_properties(
    service: &TransientService,
) -> Result<Vec<(String, OwnedValue)>, zvariant::Error> {
    let mut argv = Vec::with_capacity(service.arguments.len() + 1);
    argv.push(service.executable.clone());
    argv.extend(service.arguments.iter().cloned());

    let mut properties = vec![
        property("Description", service.description.clone())?,
        property("Slice", service.slice.clone())?,
        property("CollectMode", COLLECT_MODE.to_owned())?,
        property("ExecStart", vec![(service.executable.clone(), argv, false)])?,
    ];
    if let Some(working_directory) = &service.working_directory {
        properties.push(property("WorkingDirectory", working_directory.clone())?);
    }
    Ok(properties)
}

fn property<T>(name: &str, value: T) -> Result<(String, OwnedValue), zvariant::Error>
where
    T: Into<Value<'static>>,
{
    let value: Value<'static> = value.into();
    Ok((name.to_owned(), value.try_to_owned()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> TransientService {
        TransientService {
            unit_name: "app-example.service".to_owned(),
            description: "Example Game".to_owned(),
            executable: "/usr/bin/example".to_owned(),
            arguments: vec!["--fullscreen".to_owned()],
            working_directory: Some("/games/example".to_owned()),
            slice: "graphical-apps.slice".to_owned(),
        }
    }

    fn take_property(properties: &mut Vec<(String, OwnedValue)>, name: &str) -> OwnedValue {
        let index = properties
            .iter()
            .position(|(property_name, _)| property_name == name)
            .expect("property");
        properties.remove(index).1
    }

    #[test]
    fn active_unit_states_include_transitions_until_a_service_is_gone() {
        for state in ["active", "activating", "reloading", "deactivating"] {
            assert!(unit_state_is_active(state));
        }
        for state in ["inactive", "failed", "unknown"] {
            assert!(!unit_state_is_active(state));
        }
    }

    #[test]
    fn transient_service_encodes_systemd_exec_properties() {
        let mut properties = transient_service_properties(&service()).expect("properties");

        let description = String::try_from(take_property(&mut properties, "Description"))
            .expect("description string");
        let slice =
            String::try_from(take_property(&mut properties, "Slice")).expect("slice string");
        let collect_mode = String::try_from(take_property(&mut properties, "CollectMode"))
            .expect("collect mode string");
        let working_directory =
            String::try_from(take_property(&mut properties, "WorkingDirectory"))
                .expect("working directory string");
        let exec_start = Vec::<(String, Vec<String>, bool)>::try_from(take_property(
            &mut properties,
            "ExecStart",
        ))
        .expect("exec start array");

        assert_eq!(description, "Example Game");
        assert_eq!(slice, "graphical-apps.slice");
        assert_eq!(collect_mode, COLLECT_MODE);
        assert_eq!(working_directory, "/games/example");
        assert_eq!(
            exec_start,
            vec![(
                "/usr/bin/example".to_owned(),
                vec!["/usr/bin/example".to_owned(), "--fullscreen".to_owned()],
                false,
            )]
        );
        assert!(properties.is_empty());
    }

    #[test]
    fn transient_service_omits_an_absent_working_directory() {
        let mut service = service();
        service.working_directory = None;
        let properties = transient_service_properties(&service).expect("properties");

        assert!(
            properties
                .iter()
                .all(|(name, _)| name != "WorkingDirectory")
        );
    }
}
