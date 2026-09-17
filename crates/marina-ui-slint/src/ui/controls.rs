//! Shared input and notification event handling.

use std::{cell::Cell, rc::Rc, time::Duration};

use marina_input::{InputAction, InputEvent, InputEventKind};
use slint::{
    ComponentHandle, Model, ModelRc, VecModel,
    platform::{Key, WindowEvent},
};

use crate::{MainWindow, ToastItem, ToastQueue};

const TOAST_DURATION: Duration = Duration::from_secs(4);
const TOAST_DISMISS_ANIMATION: Duration = Duration::from_millis(250);

pub(crate) fn dispatch_controller_action(window: &MainWindow, event: InputEvent) {
    if !matches!(
        event.kind,
        InputEventKind::Pressed | InputEventKind::Repeated
    ) {
        return;
    }

    match event.action {
        InputAction::PreviousTab => {
            let active = window.get_active_tab();
            window.set_active_tab((active + 2) % 3);
            window.invoke_focus_navigation();
            return;
        }
        InputAction::NextTab => {
            let active = window.get_active_tab();
            window.set_active_tab((active + 1) % 3);
            window.invoke_focus_navigation();
            return;
        }
        InputAction::Menu => return,
        _ => {}
    }

    let key = match event.action {
        InputAction::Up => Key::UpArrow,
        InputAction::Down => Key::DownArrow,
        InputAction::Left => Key::LeftArrow,
        InputAction::Right => Key::RightArrow,
        InputAction::Accept => Key::Return,
        InputAction::Back => Key::Escape,
        InputAction::PreviousTab | InputAction::NextTab | InputAction::Menu => return,
    };
    window
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
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
