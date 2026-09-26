//! Wayland overlay-shell and compositor window-management integration.
//!
//! Marina's own overlay surfaces use Wayland layer-shell through `layer-shika`.
//! Managing other applications is compositor-owned behavior and therefore goes
//! through a backend such as Sway IPC rather than pretending a standard Wayland
//! protocol can focus arbitrary clients.

mod ipc;
mod sway;

pub use ipc::{OverlayClient, OverlayIpcError, OverlayRequest, spawn_overlay_server};
pub use sway::SwayWindowManager;

use std::{collections::HashMap, env, path::Path};

use layer_shika::{
    prelude::{AnchorEdges, KeyboardInteractivity, Layer, OutputPolicy, Shell},
    slint_interpreter::Compiler,
};
use thiserror::Error;

/// Stable compositor window identifier used by a [`WindowManager`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WindowId(i64);

impl WindowId {
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

/// Lightweight window metadata suitable for an overlay browser.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedWindow {
    pub id: WindowId,
    pub title: String,
    pub app_id: Option<String>,
    pub workspace: Option<String>,
    pub output: Option<String>,
    pub focused: bool,
    pub urgent: bool,
}

/// Compositor operations required by Marina's controller-first window overlay.
pub trait WindowManager {
    fn windows(&self) -> Result<Vec<ManagedWindow>, WindowManagerError>;
    fn focus(&self, id: WindowId) -> Result<(), WindowManagerError>;
    fn close(&self, id: WindowId) -> Result<(), WindowManagerError>;
}

#[derive(Debug, Error)]
pub enum WindowManagerError {
    #[error("failed to connect to the compositor: {0}")]
    Connection(String),
    #[error("compositor command failed: {0}")]
    Command(String),
    #[error("failed to query compositor windows: {0}")]
    Query(String),
}

/// Builds Marina's persistent fullscreen layer-shell surface.
///
/// The surface remains mapped and transparent so Sheet animations always have
/// a configured render target. The overlay process gives it an empty input
/// region and no keyboard interactivity while the Sheet is closed.
pub fn build_overlay_shell(ui_path: impl AsRef<Path>) -> Result<Shell, layer_shika::Error> {
    let scale_factor = env::var("SLINT_SCALE_FACTOR")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(1.0);

    let ui_path = ui_path.as_ref();
    Shell::from_file_with_compiler(ui_path, overlay_compiler(ui_path))
        .surface("WindowOverlay")
        .size(0, 0)
        .anchor(AnchorEdges::all())
        .layer(Layer::Overlay)
        .keyboard_interactivity(KeyboardInteractivity::None)
        .exclusive_zone(-1)
        .scale_factor(scale_factor)
        .output_policy(OutputPolicy::PrimaryOnly)
        .namespace("org.ultramarinelinux.MarinaShell.shell-overlay")
        .build()
}

/// Validates the runtime-interpreted overlay UI and its bundled libraries.
pub fn validate_overlay_ui(ui_path: impl AsRef<Path>) -> Result<(), String> {
    let ui_path = ui_path.as_ref();
    let result = spin_on::spin_on(overlay_compiler(ui_path).build_from_path(ui_path));
    let diagnostics = result
        .diagnostics()
        .map(|diagnostic| diagnostic.to_string())
        .collect::<Vec<_>>();
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics.join("\n"))
    }
}

fn overlay_compiler(ui_path: &Path) -> Compiler {
    let mut compiler = Compiler::default();
    if let Some(ui_root) = ui_path.parent().and_then(Path::parent) {
        compiler.set_library_paths(HashMap::from([(
            "lucide".to_owned(),
            ui_root.join("libraries/lucide"),
        )]));
    }
    compiler
}

/// Upstream layer-shika API used to wire the runtime-interpreted overlay UI.
pub mod layer_shell {
    pub use layer_shika::{
        calloop,
        prelude::{Shell, Surface},
        slint_interpreter::Value,
    };
}
