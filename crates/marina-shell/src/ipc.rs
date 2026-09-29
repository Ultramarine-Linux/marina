use std::{
    env, fs, io,
    path::PathBuf,
    sync::Arc,
    thread::{self, JoinHandle},
};

use thiserror::Error;
use varlink::{Connection, VarlinkService};

#[allow(clippy::all, warnings)]
mod protocol {
    include!(concat!(
        env!("OUT_DIR"),
        "/org.ultramarinelinux.MarinaShell.Overlay.rs"
    ));
}

use protocol::{Call_Hide, Call_QuickSettings, Call_Show, Call_Toggle, VarlinkClientInterface};

const SOCKET_DIR: &str = "marina";
const SOCKET_NAME: &str = "shell-overlay.varlink";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverlayRequest {
    Show,
    Hide,
    Toggle,
    QuickSettings,
}

#[derive(Debug, Error)]
pub enum OverlayIpcError {
    #[error("XDG_RUNTIME_DIR is not set")]
    MissingRuntimeDirectory,
    #[error("failed to prepare overlay IPC directory: {0}")]
    Directory(#[source] io::Error),
    #[error("overlay Varlink request failed: {0}")]
    Request(String),
    #[error("failed to spawn overlay Varlink server: {0}")]
    Thread(#[source] io::Error),
}

pub struct OverlayClient;

impl OverlayClient {
    pub fn show() -> Result<(), OverlayIpcError> {
        Self::call(|client| client.show().call().map(|_| ()))
    }

    pub fn hide() -> Result<(), OverlayIpcError> {
        Self::call(|client| client.hide().call().map(|_| ()))
    }

    pub fn toggle() -> Result<(), OverlayIpcError> {
        Self::call(|client| client.toggle().call().map(|_| ()))
    }

    pub fn quick_settings() -> Result<(), OverlayIpcError> {
        Self::call(|client| client.quick_settings().call().map(|_| ()))
    }

    fn call(
        call: impl FnOnce(&mut protocol::VarlinkClient) -> protocol::Result<()>,
    ) -> Result<(), OverlayIpcError> {
        let address = socket_address()?;
        let connection = Connection::with_address(&address)
            .map_err(|error| OverlayIpcError::Request(error.to_string()))?;
        let mut client = protocol::VarlinkClient::new(connection);
        call(&mut client).map_err(|error| OverlayIpcError::Request(error.to_string()))
    }
}

struct OverlayInterface {
    handler: Arc<dyn Fn(OverlayRequest) + Send + Sync>,
}

impl protocol::VarlinkInterface for OverlayInterface {
    fn show(&self, call: &mut dyn Call_Show) -> varlink::Result<()> {
        (self.handler)(OverlayRequest::Show);
        call.reply()
    }

    fn hide(&self, call: &mut dyn Call_Hide) -> varlink::Result<()> {
        (self.handler)(OverlayRequest::Hide);
        call.reply()
    }

    fn toggle(&self, call: &mut dyn Call_Toggle) -> varlink::Result<()> {
        (self.handler)(OverlayRequest::Toggle);
        call.reply()
    }

    fn quick_settings(&self, call: &mut dyn Call_QuickSettings) -> varlink::Result<()> {
        (self.handler)(OverlayRequest::QuickSettings);
        call.reply()
    }
}

pub fn spawn_overlay_server(
    handler: impl Fn(OverlayRequest) + Send + Sync + 'static,
) -> Result<JoinHandle<()>, OverlayIpcError> {
    let socket_path = socket_path()?;
    let parent = socket_path
        .parent()
        .expect("overlay socket path always has a parent");
    fs::create_dir_all(parent).map_err(OverlayIpcError::Directory)?;
    let address = format!("unix:{}", socket_path.display());
    let handler = Arc::new(handler);

    thread::Builder::new()
        .name("marina-overlay-varlink".to_owned())
        .spawn(move || {
            let interface = protocol::new(Box::new(OverlayInterface { handler }));
            let service = VarlinkService::new(
                "Ultramarine Linux",
                "Marina window overlay",
                env!("CARGO_PKG_VERSION"),
                "https://github.com/Ultramarine-Linux/marina",
                vec![Box::new(interface)],
            );
            if let Err(error) =
                varlink::listen(service, &address, &varlink::ListenConfig::default())
            {
                tracing::error!(%error, %address, "window overlay Varlink server stopped");
            }
        })
        .map_err(OverlayIpcError::Thread)
}

fn socket_path() -> Result<PathBuf, OverlayIpcError> {
    let runtime_dir = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or(OverlayIpcError::MissingRuntimeDirectory)?;
    Ok(runtime_dir.join(SOCKET_DIR).join(SOCKET_NAME))
}

fn socket_address() -> Result<String, OverlayIpcError> {
    Ok(format!("unix:{}", socket_path()?.display()))
}
