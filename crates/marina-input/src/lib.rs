//! Semantic game-controller input for Marina.
//!
//! This crate owns the platform-specific controller backend and exposes only
//! application actions. UI crates should not depend on `gilrs` button or axis
//! names directly.

use std::{
    collections::{HashMap, HashSet},
    sync::mpsc::{self, SyncSender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use gilrs::{Axis, Button, EventType, GamepadId, GilrsBuilder};
use thiserror::Error;

/// A controller action understood by Marina's user interfaces.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InputAction {
    Up,
    Down,
    Left,
    Right,
    Accept,
    Back,
    PreviousTab,
    NextTab,
    Menu,
}

impl InputAction {
    fn repeats(self) -> bool {
        matches!(self, Self::Up | Self::Down | Self::Left | Self::Right)
    }
}

/// The lifecycle stage of a semantic action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputEventKind {
    Pressed,
    Repeated,
    Released,
}

/// A semantic controller event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputEvent {
    pub action: InputAction,
    pub kind: InputEventKind,
}

/// Tuning parameters for analog navigation and held-direction repeat.
#[derive(Clone, Copy, Debug)]
pub struct InputConfig {
    pub stick_press_threshold: f32,
    pub stick_release_threshold: f32,
    pub initial_repeat_delay: Duration,
    pub repeat_interval: Duration,
    pub poll_interval: Duration,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            stick_press_threshold: 0.65,
            stick_release_threshold: 0.35,
            initial_repeat_delay: Duration::from_millis(350),
            repeat_interval: Duration::from_millis(90),
            poll_interval: Duration::from_millis(8),
        }
    }
}

/// Failure to initialize or start the controller input thread.
#[derive(Debug, Error)]
pub enum InputError {
    #[error("failed to spawn controller input thread: {0}")]
    ThreadSpawn(#[source] std::io::Error),
    #[error("failed to initialize controller input: {0}")]
    Initialization(String),
    #[error("controller input thread stopped during initialization")]
    InitializationChannelClosed,
}

/// Owns the controller polling thread. Dropping it requests a clean shutdown.
pub struct InputLoop {
    shutdown: SyncSender<()>,
    thread: Option<JoinHandle<()>>,
}

impl InputLoop {
    /// Starts controller discovery and event polling on a dedicated thread.
    ///
    /// The handler runs on that thread. UI integrations must forward events to
    /// their UI event loop instead of updating UI state directly.
    pub fn spawn(
        config: InputConfig,
        mut handler: impl FnMut(InputEvent) + Send + 'static,
    ) -> Result<Self, InputError> {
        let (shutdown_tx, shutdown_rx) = mpsc::sync_channel(1);
        let (initialized_tx, initialized_rx) = mpsc::sync_channel(1);

        let input_thread = thread::Builder::new()
            .name("marina-controller-input".into())
            .spawn(move || {
                let mut gilrs = match GilrsBuilder::new().build() {
                    Ok(gilrs) => {
                        let _ = initialized_tx.send(Ok(()));
                        gilrs
                    }
                    Err(error) => {
                        let _ = initialized_tx.send(Err(error.to_string()));
                        return;
                    }
                };

                let mut state = SemanticState::new(config);
                loop {
                    if shutdown_rx.try_recv().is_ok() {
                        break;
                    }

                    while let Some(event) = gilrs.next_event() {
                        state.handle_event(event.id, event.event, &mut handler);
                    }

                    state.emit_repeats(Instant::now(), &mut handler);
                    thread::sleep(config.poll_interval);
                }
            })
            .map_err(InputError::ThreadSpawn)?;

        match initialized_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                shutdown: shutdown_tx,
                thread: Some(input_thread),
            }),
            Ok(Err(error)) => {
                let _ = input_thread.join();
                Err(InputError::Initialization(error))
            }
            Err(_) => {
                let _ = input_thread.join();
                Err(InputError::InitializationChannelClosed)
            }
        }
    }
}

impl Drop for InputLoop {
    fn drop(&mut self) {
        let _ = self.shutdown.try_send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum PhysicalInput {
    Button(Button),
    AxisNegative(Axis),
    AxisPositive(Axis),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct InputSource {
    gamepad: usize,
    input: PhysicalInput,
}

struct SemanticState {
    config: InputConfig,
    sources: HashMap<InputAction, HashSet<InputSource>>,
    next_repeat: HashMap<InputAction, Instant>,
}

impl SemanticState {
    fn new(config: InputConfig) -> Self {
        Self {
            config,
            sources: HashMap::new(),
            next_repeat: HashMap::new(),
        }
    }

    fn handle_event(
        &mut self,
        gamepad: GamepadId,
        event: EventType,
        handler: &mut impl FnMut(InputEvent),
    ) {
        let gamepad = usize::from(gamepad);
        match event {
            EventType::ButtonPressed(button, _) => {
                if let Some(action) = button_action(button) {
                    self.set_source(
                        action,
                        InputSource {
                            gamepad,
                            input: PhysicalInput::Button(button),
                        },
                        true,
                        Instant::now(),
                        handler,
                    );
                }
            }
            EventType::ButtonReleased(button, _) => {
                if let Some(action) = button_action(button) {
                    self.set_source(
                        action,
                        InputSource {
                            gamepad,
                            input: PhysicalInput::Button(button),
                        },
                        false,
                        Instant::now(),
                        handler,
                    );
                }
            }
            EventType::AxisChanged(axis, value, _) => {
                self.update_axis(gamepad, axis, value, Instant::now(), handler);
            }
            EventType::Disconnected => self.release_gamepad(gamepad, handler),
            _ => {}
        }
    }

    fn update_axis(
        &mut self,
        gamepad: usize,
        axis: Axis,
        value: f32,
        now: Instant,
        handler: &mut impl FnMut(InputEvent),
    ) {
        let Some((negative, positive)) = axis_actions(axis) else {
            return;
        };
        let negative_source = InputSource {
            gamepad,
            input: PhysicalInput::AxisNegative(axis),
        };
        let positive_source = InputSource {
            gamepad,
            input: PhysicalInput::AxisPositive(axis),
        };
        let negative_active = self.source_is_active(negative, negative_source);
        let positive_active = self.source_is_active(positive, positive_source);

        self.set_source(
            negative,
            negative_source,
            value
                <= if negative_active {
                    -self.config.stick_release_threshold
                } else {
                    -self.config.stick_press_threshold
                },
            now,
            handler,
        );
        self.set_source(
            positive,
            positive_source,
            value
                >= if positive_active {
                    self.config.stick_release_threshold
                } else {
                    self.config.stick_press_threshold
                },
            now,
            handler,
        );
    }

    fn source_is_active(&self, action: InputAction, source: InputSource) -> bool {
        self.sources
            .get(&action)
            .is_some_and(|sources| sources.contains(&source))
    }

    fn set_source(
        &mut self,
        action: InputAction,
        source: InputSource,
        active: bool,
        now: Instant,
        handler: &mut impl FnMut(InputEvent),
    ) {
        let sources = self.sources.entry(action).or_default();
        let was_active = !sources.is_empty();
        if active {
            sources.insert(source);
        } else {
            sources.remove(&source);
        }
        let is_active = !sources.is_empty();

        if !was_active && is_active {
            handler(InputEvent {
                action,
                kind: InputEventKind::Pressed,
            });
            if action.repeats() {
                self.next_repeat
                    .insert(action, now + self.config.initial_repeat_delay);
            }
        } else if was_active && !is_active {
            handler(InputEvent {
                action,
                kind: InputEventKind::Released,
            });
            self.next_repeat.remove(&action);
        }
    }

    fn emit_repeats(&mut self, now: Instant, handler: &mut impl FnMut(InputEvent)) {
        for (action, next_repeat) in &mut self.next_repeat {
            if now >= *next_repeat {
                handler(InputEvent {
                    action: *action,
                    kind: InputEventKind::Repeated,
                });
                *next_repeat = now + self.config.repeat_interval;
            }
        }
    }

    fn release_gamepad(&mut self, gamepad: usize, handler: &mut impl FnMut(InputEvent)) {
        let mut released = Vec::new();
        for (action, sources) in &mut self.sources {
            let was_active = !sources.is_empty();
            sources.retain(|source| source.gamepad != gamepad);
            if was_active && sources.is_empty() {
                released.push(*action);
            }
        }
        for action in released {
            self.next_repeat.remove(&action);
            handler(InputEvent {
                action,
                kind: InputEventKind::Released,
            });
        }
    }
}

fn button_action(button: Button) -> Option<InputAction> {
    match button {
        Button::DPadUp => Some(InputAction::Up),
        Button::DPadDown => Some(InputAction::Down),
        Button::DPadLeft => Some(InputAction::Left),
        Button::DPadRight => Some(InputAction::Right),
        Button::South => Some(InputAction::Accept),
        Button::East => Some(InputAction::Back),
        Button::LeftTrigger => Some(InputAction::PreviousTab),
        Button::RightTrigger => Some(InputAction::NextTab),
        Button::Start => Some(InputAction::Menu),
        _ => None,
    }
}

fn axis_actions(axis: Axis) -> Option<(InputAction, InputAction)> {
    match axis {
        Axis::LeftStickX | Axis::DPadX => Some((InputAction::Left, InputAction::Right)),
        Axis::LeftStickY | Axis::DPadY => Some((InputAction::Down, InputAction::Up)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(input: PhysicalInput) -> InputSource {
        InputSource { gamepad: 0, input }
    }

    #[test]
    fn multiple_sources_keep_an_action_pressed() {
        let mut state = SemanticState::new(InputConfig::default());
        let mut events = Vec::new();
        let now = Instant::now();
        let dpad = source(PhysicalInput::Button(Button::DPadDown));
        let stick = source(PhysicalInput::AxisNegative(Axis::LeftStickY));

        state.set_source(InputAction::Down, dpad, true, now, &mut |event| {
            events.push(event)
        });
        state.set_source(InputAction::Down, stick, true, now, &mut |event| {
            events.push(event)
        });
        state.set_source(InputAction::Down, dpad, false, now, &mut |event| {
            events.push(event)
        });
        assert_eq!(events.len(), 1);

        state.set_source(InputAction::Down, stick, false, now, &mut |event| {
            events.push(event)
        });
        assert_eq!(events[1].kind, InputEventKind::Released);
    }

    #[test]
    fn stick_uses_hysteresis() {
        let mut state = SemanticState::new(InputConfig::default());
        let mut events = Vec::new();
        let now = Instant::now();

        state.update_axis(0, Axis::LeftStickX, 0.7, now, &mut |event| {
            events.push(event)
        });
        state.update_axis(0, Axis::LeftStickX, 0.5, now, &mut |event| {
            events.push(event)
        });
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, InputAction::Right);
        assert_eq!(events[0].kind, InputEventKind::Pressed);

        state.update_axis(0, Axis::LeftStickX, 0.2, now, &mut |event| {
            events.push(event)
        });
        assert_eq!(events[1].kind, InputEventKind::Released);
    }

    #[test]
    fn held_directions_repeat() {
        let config = InputConfig {
            initial_repeat_delay: Duration::from_millis(10),
            repeat_interval: Duration::from_millis(5),
            ..InputConfig::default()
        };
        let mut state = SemanticState::new(config);
        let mut events = Vec::new();
        let now = Instant::now();

        state.set_source(
            InputAction::Right,
            source(PhysicalInput::Button(Button::DPadRight)),
            true,
            now,
            &mut |event| events.push(event),
        );
        state.emit_repeats(now + Duration::from_millis(9), &mut |event| {
            events.push(event)
        });
        state.emit_repeats(now + Duration::from_millis(10), &mut |event| {
            events.push(event)
        });

        assert_eq!(events.len(), 2);
        assert_eq!(events[1].kind, InputEventKind::Repeated);
    }
}
