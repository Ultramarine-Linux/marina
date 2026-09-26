//! Application-wide transient notifications backed by the shared toast queue.

use std::{
    cell::Cell,
    rc::Rc,
    sync::{Mutex, OnceLock},
    time::Duration,
};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::{MainWindow, ToastItem, ToastQueue, ToastVariant};

const TOAST_DURATION: Duration = Duration::from_secs(4);
const TOAST_DISMISS_ANIMATION: Duration = Duration::from_millis(250);

/// Process-wide notification entry point for UI and background tasks.
pub(crate) static NOTIFICATION: NotificationService = NotificationService::new();

pub(crate) struct NotificationService {
    window: OnceLock<Mutex<slint::Weak<MainWindow>>>,
}

impl NotificationService {
    const fn new() -> Self {
        Self {
            window: OnceLock::new(),
        }
    }

    pub(crate) fn configure(&self, window: &MainWindow) {
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

        let weak = window.as_weak();
        if let Some(registered) = self.window.get() {
            *registered
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = weak;
        } else {
            let _ = self.window.set(Mutex::new(weak));
        }
    }

    pub(crate) fn show(&self, text: impl Into<String>, variant: ToastVariant) {
        let Some(window) = self.window.get().map(|window| {
            window
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }) else {
            tracing::warn!(message = %text.into(), "notification dropped before UI initialization");
            return;
        };
        let text = text.into();
        let _ = window.upgrade_in_event_loop(move |window| {
            window
                .global::<ToastQueue>()
                .invoke_show(SharedString::from(text), variant);
        });
    }

    pub(crate) fn error(&self, text: impl Into<String>) {
        self.show(text, ToastVariant::Error);
    }

    pub(crate) fn success(&self, text: impl Into<String>) {
        self.show(text, ToastVariant::Success);
    }
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
