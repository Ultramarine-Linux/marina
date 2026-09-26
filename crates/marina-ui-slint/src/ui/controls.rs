//! Shared navigation and controller input handling.

use marina_input::{InputAction, InputEvent, InputEventKind};
use slint::{
    ComponentHandle,
    platform::{Key, WindowEvent},
};
use tracing::error;

use crate::ui::{nav, notifications::NOTIFICATION};
use crate::{MainWindow, ProfileAction, ShellPage, ShellState, StoreState};

pub(crate) fn configure_navigation(window: &MainWindow) {
    let weak = window.as_weak();
    window.global::<ShellState>().on_navigate(move |index| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let index = index.clamp(0, 2);
        let shell = window.global::<ShellState>();
        let target = match index {
            0 => ShellPage::Home,
            1 => ShellPage::Library,
            _ => ShellPage::Store,
        };
        if shell.get_active_tab() == index && shell.get_page() == target {
            window.invoke_focus_content();
            return;
        }
        nav::push_tab(&window, index);
        nav::goto_tab(&window, index);
    });

    let weak = window.as_weak();
    window.global::<ShellState>().on_back_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        nav::back(&window);
    });

    let weak = window.as_weak();
    window
        .global::<ShellState>()
        .on_crumb_navigate(move |index| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            nav::jump(&window, index);
        });
}

/// Returns focus to the mounted page after the native window regains input.
///
/// This is deliberately independent of controller discovery: keyboard and
/// pointer users need the same focus guarantee after a compositor focus reset.
pub(crate) fn restore_content_focus(window: &MainWindow) {
    let shell = window.global::<ShellState>();
    shell.set_content_focus_request(shell.get_content_focus_request() + 1);
}

pub(crate) fn configure_profile_menu(window: &MainWindow) {
    let weak = window.as_weak();
    window
        .global::<ShellState>()
        .on_profile_action(move |action| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            match action {
                ProfileAction::Settings => nav::open_settings(&window),
                ProfileAction::Exit => {
                    if let Err(error) = slint::quit_event_loop() {
                        error!(%error, "failed to quit the event loop");
                    }
                }
                ProfileAction::Shutdown => {
                    tokio::spawn(async move {
                        if let Err(error) = marina_power::power_off().await {
                            error!(%error, "shutdown request failed");
                            NOTIFICATION.error(format!("Shutdown request failed: {error}"));
                        }
                    });
                }
            }
        });
}

pub(crate) fn dispatch_controller_action(window: &MainWindow, event: InputEvent) {
    if !matches!(
        event.kind,
        InputEventKind::Pressed | InputEventKind::Repeated
    ) {
        return;
    }

    if event.kind == InputEventKind::Repeated
        && matches!(
            event.action,
            InputAction::PreviousTab | InputAction::NextTab
        )
    {
        return;
    }

    match event.action {
        InputAction::PreviousTab => {
            let shell = window.global::<ShellState>();
            if shell.get_page() != ShellPage::Settings {
                shell.invoke_navigate((shell.get_active_tab() + 2) % 3);
            }
            return;
        }
        InputAction::NextTab => {
            let shell = window.global::<ShellState>();
            if shell.get_page() != ShellPage::Settings {
                shell.invoke_navigate((shell.get_active_tab() + 1) % 3);
            }
            return;
        }
        InputAction::Menu => {
            crate::window_overlay::toggle();
            return;
        }
        InputAction::Back if nav::dismiss_overlay(&window) => return,
        InputAction::ScrollUp
        | InputAction::ScrollDown
        | InputAction::ScrollLeft
        | InputAction::ScrollRight => {}
        InputAction::Back if is_back_route(&window) => {
            window.global::<ShellState>().invoke_back_requested();
            return;
        }
        _ => {}
    }

    let scroll_text = match event.action {
        InputAction::ScrollUp => Some("ScrollUp"),
        InputAction::ScrollDown => Some("ScrollDown"),
        InputAction::ScrollLeft => Some("ScrollLeft"),
        InputAction::ScrollRight => Some("ScrollRight"),
        _ => None,
    };
    if let Some(text) = scroll_text {
        window
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.into() });
        return;
    }

    let key = match event.action {
        InputAction::Up => Key::UpArrow,
        InputAction::Down => Key::DownArrow,
        InputAction::Left => Key::LeftArrow,
        InputAction::Right => Key::RightArrow,
        InputAction::Accept => Key::Return,
        InputAction::Back => Key::Escape,
        InputAction::PageUp => Key::PageUp,
        InputAction::PageDown => Key::PageDown,
        InputAction::ScrollUp
        | InputAction::ScrollDown
        | InputAction::ScrollLeft
        | InputAction::ScrollRight => return,
        InputAction::PreviousTab | InputAction::NextTab | InputAction::Menu => return,
    };
    window
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
}

/// Back-worthy locations for the semantic Back action: the game-details
/// route plus inline detail views (e.g. store page 2) whose focus never
/// reaches a Slint Esc branch. Everything else falls through to the Escape
/// key event, which list pages handle themselves.
fn is_back_route(window: &MainWindow) -> bool {
    let shell = window.global::<ShellState>();
    if shell.get_page() == ShellPage::GameDetails {
        return true;
    }
    shell.get_page() == ShellPage::Store && window.global::<StoreState>().get_page() == 2
}
