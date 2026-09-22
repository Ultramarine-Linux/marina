//! Shared input and notification event handling.

use std::{cell::Cell, rc::Rc, time::Duration};

use marina_input::{InputAction, InputEvent, InputEventKind};
use slint::{
    ComponentHandle, Model, ModelRc, SharedString, VecModel,
    platform::{Key, WindowEvent},
};
use tracing::error;

use crate::ui::nav;
use crate::{
    MainWindow, ProfileAction, ShellPage, ShellState, StoreState, ToastItem, ToastQueue,
    ToastVariant,
};

const TOAST_DURATION: Duration = Duration::from_secs(4);
const TOAST_DISMISS_ANIMATION: Duration = Duration::from_millis(250);

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
                    let weak = window.as_weak();
                    tokio::spawn(async move {
                        if let Err(error) = marina_power::power_off().await {
                            error!(%error, "shutdown request failed");
                            let _ = weak.upgrade_in_event_loop(move |window| {
                                window.global::<ToastQueue>().invoke_show(
                                    SharedString::from("Shutdown request failed"),
                                    ToastVariant::Error,
                                );
                            });
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
        InputAction::Menu => return,
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

pub(crate) fn configure_toasts(window: &MainWindow) {
    let items = Rc::new(VecModel::from(Vec::<ToastItem>::new()));
    let toast_queue = window.global::<ToastQueue>();
    toast_queue.set_items(ModelRc::from(items.clone()));

    let dismiss_items = items.clone();
    toast_queue.on_dismiss(move |id| {
        dismiss_toast(dismiss_items.clone(), id);
    });

    let next_id = Rc::new(Cell::new(0_i32));
    toast_queue.on_show(move |text, variant| {
        let id = next_id.get();
        next_id.set(id.saturating_add(1));
        items.push(ToastItem {
            id,
            text,
            variant,
            dismissed: false,
        });

        let auto_dismiss_items = items.clone();
        slint::Timer::single_shot(TOAST_DURATION, move || {
            dismiss_toast(auto_dismiss_items, id);
        });
    });
}

fn dismiss_toast(items: Rc<VecModel<ToastItem>>, id: i32) {
    let Some(index) = (0..items.row_count())
        .find(|&index| items.row_data(index).is_some_and(|item| item.id == id))
    else {
        return;
    };
    let Some(mut item) = items.row_data(index) else {
        return;
    };
    if item.dismissed {
        return;
    }

    item.dismissed = true;
    items.set_row_data(index, item);

    slint::Timer::single_shot(TOAST_DISMISS_ANIMATION, move || {
        if let Some(index) = (0..items.row_count())
            .find(|&index| items.row_data(index).is_some_and(|item| item.id == id))
        {
            items.remove(index);
        }
    });
}
