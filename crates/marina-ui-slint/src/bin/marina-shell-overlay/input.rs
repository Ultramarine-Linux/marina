use std::{io, sync::Arc, thread};

use marina_input::{InputAction, InputConfig, InputEvent, InputEventKind, InputLoop, inputplumber};
use tracing::error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DbusOverlayAction {
    ShowGameMenu,
    ShowQuickSettings,
    Dispatch(InputEvent),
    Ignore,
}

#[derive(Debug, Default)]
pub(super) struct DbusOverlayRouter {
    guide_held: bool,
    guide_started_visible: bool,
    guide_chorded: bool,
}

impl DbusOverlayRouter {
    pub(super) fn route(&mut self, event: InputEvent, visible: bool) -> DbusOverlayAction {
        if event.action == InputAction::Menu {
            match event.kind {
                InputEventKind::Pressed => {
                    self.guide_held = true;
                    self.guide_started_visible = visible;
                    self.guide_chorded = false;
                    return DbusOverlayAction::Ignore;
                }
                InputEventKind::Released => {
                    self.guide_held = false;
                    let chorded = self.guide_chorded;
                    let close_existing = self.guide_started_visible && !chorded;
                    self.guide_started_visible = false;
                    self.guide_chorded = false;
                    return if chorded {
                        DbusOverlayAction::Ignore
                    } else if close_existing {
                        DbusOverlayAction::Dispatch(InputEvent {
                            action: InputAction::Menu,
                            kind: InputEventKind::Pressed,
                        })
                    } else {
                        DbusOverlayAction::ShowGameMenu
                    };
                }
                InputEventKind::Repeated => return DbusOverlayAction::Ignore,
            }
        }

        if self.guide_held && event.kind == InputEventKind::Pressed {
            match event.action {
                InputAction::Accept => {
                    self.guide_chorded = true;
                    return DbusOverlayAction::ShowQuickSettings;
                }
                InputAction::Context => {
                    self.guide_chorded = true;
                    return DbusOverlayAction::ShowGameMenu;
                }
                _ => {}
            }
        }

        if visible {
            DbusOverlayAction::Dispatch(event)
        } else {
            DbusOverlayAction::Ignore
        }
    }
}

pub(super) fn spawn_gilrs(handler: impl Fn(InputEvent) + Send + 'static) -> io::Result<()> {
    thread::Builder::new()
        .name("marina-overlay-controller".to_owned())
        .spawn(
            move || match InputLoop::spawn(InputConfig::default(), handler) {
                Ok(_input) => loop {
                    thread::park();
                },
                Err(error) => error!(%error, "window overlay controller input unavailable"),
            },
        )?;
    Ok(())
}

pub(super) fn spawn_inputplumber(
    runtime: &Arc<tokio::runtime::Runtime>,
    modes: tokio::sync::watch::Receiver<inputplumber::InterceptMode>,
    activations: tokio::sync::watch::Receiver<inputplumber::InterceptActivation>,
    handler: impl Fn(InputEvent) + Send + 'static,
) {
    runtime.spawn(inputplumber::monitor_input_events(
        modes,
        activations,
        handler,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(action: InputAction, kind: InputEventKind) -> InputEvent {
        InputEvent { action, kind }
    }

    #[test]
    fn intercepted_guide_north_signal_opens_the_game_menu_on_release() {
        let mut router = DbusOverlayRouter::default();

        assert_eq!(
            router.route(event(InputAction::Menu, InputEventKind::Pressed), false),
            DbusOverlayAction::Ignore
        );
        assert_eq!(
            router.route(event(InputAction::Menu, InputEventKind::Released), false),
            DbusOverlayAction::ShowGameMenu
        );
    }

    #[test]
    fn guide_south_opens_quick_settings_without_dispatching_accept() {
        let mut router = DbusOverlayRouter::default();
        assert_eq!(
            router.route(event(InputAction::Menu, InputEventKind::Pressed), false),
            DbusOverlayAction::Ignore
        );
        assert_eq!(
            router.route(event(InputAction::Accept, InputEventKind::Pressed), true),
            DbusOverlayAction::ShowQuickSettings
        );
    }

    #[test]
    fn guide_north_selects_the_game_menu() {
        let mut router = DbusOverlayRouter::default();
        assert_eq!(
            router.route(event(InputAction::Menu, InputEventKind::Pressed), true),
            DbusOverlayAction::Ignore
        );
        assert_eq!(
            router.route(event(InputAction::Context, InputEventKind::Pressed), true),
            DbusOverlayAction::ShowGameMenu
        );
        assert_eq!(
            router.route(event(InputAction::Menu, InputEventKind::Released), true),
            DbusOverlayAction::Ignore
        );
    }

    #[test]
    fn plain_guide_tap_closes_an_existing_overlay_on_release() {
        let mut router = DbusOverlayRouter::default();
        assert_eq!(
            router.route(event(InputAction::Menu, InputEventKind::Pressed), true),
            DbusOverlayAction::Ignore
        );
        assert_eq!(
            router.route(event(InputAction::Menu, InputEventKind::Released), true),
            DbusOverlayAction::Dispatch(event(InputAction::Menu, InputEventKind::Pressed))
        );
    }
}
