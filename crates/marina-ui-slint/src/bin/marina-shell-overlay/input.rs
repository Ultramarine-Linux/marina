use std::{io, sync::Arc, thread};

use marina_input::{InputAction, InputConfig, InputEvent, InputEventKind, InputLoop, inputplumber};
use tracing::{debug, error};

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
    back_held: bool,
}

impl DbusOverlayRouter {
    pub(super) fn route(&mut self, event: InputEvent, visible: bool) -> DbusOverlayAction {
        let guide_held_before = self.guide_held;
        let guide_chorded_before = self.guide_chorded;
        let back_held_before = self.back_held;
        let action = self.route_inner(event, visible);
        debug!(
            ?event,
            visible,
            guide_held_before,
            guide_chorded_before,
            back_held_before,
            guide_held_after = self.guide_held,
            guide_chorded_after = self.guide_chorded,
            back_held_after = self.back_held,
            ?action,
            "routed InputPlumber overlay event"
        );
        action
    }

    fn route_inner(&mut self, event: InputEvent, visible: bool) -> DbusOverlayAction {
        if !visible {
            self.back_held = false;
        }

        // During RetroArch pass-through, InputPlumber emits a synthetic South
        // event only after the complete Guide + South activation chord. Normal
        // South presses stay on RetroArch's virtual controller and never reach
        // this D-Bus router while the overlay is hidden.
        if !visible
            && !self.guide_held
            && event.action == InputAction::Accept
            && event.kind == InputEventKind::Pressed
        {
            return DbusOverlayAction::ShowQuickSettings;
        }

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

        if event.action == InputAction::Back {
            match event.kind {
                InputEventKind::Pressed => self.back_held = true,
                InputEventKind::Released if self.back_held => self.back_held = false,
                InputEventKind::Released => {
                    return if visible {
                        DbusOverlayAction::Dispatch(InputEvent {
                            action: InputAction::Back,
                            kind: InputEventKind::Pressed,
                        })
                    } else {
                        DbusOverlayAction::Ignore
                    };
                }
                InputEventKind::Repeated => {}
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

/// Keeps InputPlumber interception until the button that dismissed the overlay
/// is released.
///
/// InputPlumber's D-Bus target only reports value changes. Returning to
/// pass-through on the dismissing press sends that button's release to the
/// virtual gamepad instead, so the D-Bus target still considers it held and
/// swallows its next press in a later overlay: only the release arrives.
#[derive(Debug, Default)]
pub(super) struct ReleaseGate {
    pending: Option<InputAction>,
}

impl ReleaseGate {
    pub(super) fn hold(&mut self, action: InputAction) {
        self.pending = Some(action);
    }

    pub(super) fn is_holding(&self) -> bool {
        self.pending.is_some()
    }

    /// Returns true when `event` is the awaited release; the caller then
    /// restores pass-through and drops the event.
    pub(super) fn release(&mut self, event: InputEvent) -> bool {
        if event.kind == InputEventKind::Released && self.pending == Some(event.action) {
            self.pending = None;
            return true;
        }
        false
    }

    /// Abandons the hold, returning whether one was pending.
    pub(super) fn clear(&mut self) -> bool {
        self.pending.take().is_some()
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
    fn plain_guide_opens_the_game_menu_outside_retroarch() {
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
    fn guide_south_activation_opens_quick_settings() {
        let mut router = DbusOverlayRouter::default();
        assert_eq!(
            router.route(event(InputAction::Accept, InputEventKind::Pressed), false),
            DbusOverlayAction::ShowQuickSettings
        );
        assert_eq!(
            router.route(event(InputAction::Accept, InputEventKind::Released), true),
            DbusOverlayAction::Dispatch(event(InputAction::Accept, InputEventKind::Released))
        );
        assert_eq!(
            router.route(event(InputAction::Back, InputEventKind::Released), true),
            DbusOverlayAction::Dispatch(event(InputAction::Back, InputEventKind::Pressed))
        );
    }

    #[test]
    fn normal_back_release_is_not_dispatched_as_a_second_press() {
        let mut router = DbusOverlayRouter::default();
        assert_eq!(
            router.route(event(InputAction::Back, InputEventKind::Pressed), true),
            DbusOverlayAction::Dispatch(event(InputAction::Back, InputEventKind::Pressed))
        );
        assert_eq!(
            router.route(event(InputAction::Back, InputEventKind::Released), true),
            DbusOverlayAction::Dispatch(event(InputAction::Back, InputEventKind::Released))
        );
    }

    #[test]
    fn a_new_overlay_clears_back_state_left_by_the_previous_overlay() {
        let mut router = DbusOverlayRouter::default();

        assert_eq!(
            router.route(event(InputAction::Back, InputEventKind::Pressed), true),
            DbusOverlayAction::Dispatch(event(InputAction::Back, InputEventKind::Pressed))
        );
        assert_eq!(
            router.route(event(InputAction::Menu, InputEventKind::Pressed), false),
            DbusOverlayAction::Ignore
        );
        assert_eq!(
            router.route(event(InputAction::Accept, InputEventKind::Pressed), false),
            DbusOverlayAction::ShowQuickSettings
        );
        assert_eq!(
            router.route(event(InputAction::Accept, InputEventKind::Released), true),
            DbusOverlayAction::Dispatch(event(InputAction::Accept, InputEventKind::Released))
        );
        assert_eq!(
            router.route(event(InputAction::Menu, InputEventKind::Released), true),
            DbusOverlayAction::Ignore
        );
        assert_eq!(
            router.route(event(InputAction::Back, InputEventKind::Released), true),
            DbusOverlayAction::Dispatch(event(InputAction::Back, InputEventKind::Pressed))
        );
    }

    #[test]
    fn guide_context_opens_the_game_menu_without_reopening_after_back() {
        let mut router = DbusOverlayRouter::default();
        assert_eq!(
            router.route(event(InputAction::Menu, InputEventKind::Pressed), false),
            DbusOverlayAction::Ignore
        );
        assert_eq!(
            router.route(event(InputAction::Context, InputEventKind::Pressed), false),
            DbusOverlayAction::ShowGameMenu
        );
        assert_eq!(
            router.route(event(InputAction::Menu, InputEventKind::Released), true),
            DbusOverlayAction::Ignore
        );
        assert_eq!(
            router.route(event(InputAction::Back, InputEventKind::Pressed), true),
            DbusOverlayAction::Dispatch(event(InputAction::Back, InputEventKind::Pressed))
        );
    }

    #[test]
    fn release_gate_waits_for_the_dismissing_button() {
        let mut gate = ReleaseGate::default();
        gate.hold(InputAction::Accept);
        assert!(gate.is_holding());

        assert!(!gate.release(event(InputAction::Down, InputEventKind::Released)));
        assert!(!gate.release(event(InputAction::Accept, InputEventKind::Pressed)));
        assert!(gate.is_holding());

        assert!(gate.release(event(InputAction::Accept, InputEventKind::Released)));
        assert!(!gate.is_holding());
        assert!(!gate.release(event(InputAction::Accept, InputEventKind::Released)));
    }

    #[test]
    fn release_gate_clear_reports_an_abandoned_hold() {
        let mut gate = ReleaseGate::default();
        assert!(!gate.clear());
        gate.hold(InputAction::Back);
        assert!(gate.clear());
        assert!(!gate.is_holding());
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
