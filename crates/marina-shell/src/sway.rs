use swayipc::{Connection, Node, NodeType};

use crate::{ManagedWindow, WindowId, WindowManager, WindowManagerError};

const MARINA_APP_ID: &str = "org.ultramarinelinux.MarinaShell";
const MARINA_OVERLAY_NAMESPACE: &str = "org.ultramarinelinux.MarinaShell.shell-overlay";

/// Sway IPC implementation of Marina's compositor window-management boundary.
#[derive(Clone, Copy, Debug, Default)]
pub struct SwayWindowManager;

impl SwayWindowManager {
    pub fn new() -> Result<Self, WindowManagerError> {
        Connection::new()
            .map(|_| Self)
            .map_err(|error| WindowManagerError::Connection(error.to_string()))
    }

    fn connection(&self) -> Result<Connection, WindowManagerError> {
        Connection::new().map_err(|error| WindowManagerError::Connection(error.to_string()))
    }

    fn run_container_command(&self, id: WindowId, command: &str) -> Result<(), WindowManagerError> {
        let mut connection = self.connection()?;
        let command = format!("[con_id={}] {command}", id.get());
        let outcomes = connection
            .run_command(command)
            .map_err(|error| WindowManagerError::Command(error.to_string()))?;
        for outcome in outcomes {
            outcome.map_err(|error| WindowManagerError::Command(error.to_string()))?;
        }
        Ok(())
    }
}

impl WindowManager for SwayWindowManager {
    fn windows(&self) -> Result<Vec<ManagedWindow>, WindowManagerError> {
        let mut connection = self.connection()?;
        let tree = connection
            .get_tree()
            .map_err(|error| WindowManagerError::Query(error.to_string()))?;
        let mut windows = Vec::new();
        collect_windows(&tree, None, None, &mut windows);
        windows.sort_by_key(|window| !window.focused);
        Ok(windows)
    }

    fn focus(&self, id: WindowId) -> Result<(), WindowManagerError> {
        self.run_container_command(id, "focus")
    }

    fn close(&self, id: WindowId) -> Result<(), WindowManagerError> {
        self.run_container_command(id, "kill")
    }
}

fn collect_windows(
    node: &Node,
    workspace: Option<&str>,
    output: Option<&str>,
    windows: &mut Vec<ManagedWindow>,
) {
    let workspace = if node.node_type == NodeType::Workspace {
        node.name.as_deref().or(workspace)
    } else {
        workspace
    };
    let output = if node.node_type == NodeType::Output {
        node.name.as_deref().or(output)
    } else {
        output
    };

    let app_id = node.app_id.clone().or_else(|| {
        node.window_properties
            .as_ref()
            .and_then(|properties| properties.class.clone())
    });
    let is_view = node.app_id.is_some() || node.window.is_some();
    let is_marina = app_id
        .as_deref()
        .is_some_and(|id| id == MARINA_APP_ID || id == MARINA_OVERLAY_NAMESPACE);

    if is_view && !is_marina {
        let title = node
            .name
            .clone()
            .or_else(|| app_id.clone())
            .unwrap_or_else(|| "Untitled window".to_owned());
        windows.push(ManagedWindow {
            id: WindowId::new(node.id),
            title,
            app_id,
            workspace: workspace.map(str::to_owned),
            output: output.map(str::to_owned),
            focused: node.focused,
            urgent: node.urgent,
        });
    }

    for child in node.nodes.iter().chain(&node.floating_nodes) {
        collect_windows(child, workspace, output, windows);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_id_round_trips() {
        let id = WindowId::new(42);
        assert_eq!(id.get(), 42);
    }

    #[test]
    fn marina_identifiers_are_distinct_and_reserved() {
        assert_ne!(MARINA_APP_ID, MARINA_OVERLAY_NAMESPACE);
        assert!(MARINA_OVERLAY_NAMESPACE.starts_with(MARINA_APP_ID));
    }
}
