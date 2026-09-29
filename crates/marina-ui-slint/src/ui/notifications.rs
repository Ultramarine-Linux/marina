//! Application-wide transient notifications backed by the shared toast queue.

use std::{
    rc::Rc,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicI32, Ordering},
    },
    time::Duration,
};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::{ErrorSheetState, MainWindow, ToastItem, ToastQueue, ToastVariant};

const TOAST_DURATION: Duration = Duration::from_secs(4);
const TOAST_DISMISS_ANIMATION: Duration = Duration::from_millis(250);

/// Process-wide notification entry point for UI and background tasks.
pub(crate) static NOTIFICATION: NotificationService = NotificationService::new();

pub(crate) struct NotificationService {
    window: OnceLock<Mutex<slint::Weak<MainWindow>>>,
    next_id: AtomicI32,
}

#[must_use = "loading notifications remain visible until they are completed"]
pub(crate) struct LoadingNotification {
    id: i32,
}

impl NotificationService {
    const fn new() -> Self {
        Self {
            window: OnceLock::new(),
            next_id: AtomicI32::new(0),
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

        let push_items = items.clone();
        toast_queue.on_push(move |id, text, variant, loading| {
            push_toast(push_items.clone(), id, text, variant, loading);
        });

        let update_items = items.clone();
        toast_queue.on_update(move |id, text, variant| {
            update_toast(update_items.clone(), id, text, variant);
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
        self.push(text.into(), variant, false);
    }

    pub(crate) fn loading(&self, text: impl Into<String>) -> LoadingNotification {
        let id = self.push(text.into(), ToastVariant::Default, true);
        LoadingNotification { id }
    }

    fn push(&self, text: String, variant: ToastVariant, loading: bool) -> i32 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let Some(window) = self.window() else {
            tracing::warn!(message = %text, "notification dropped before UI initialization");
            return id;
        };
        let _ = window.upgrade_in_event_loop(move |window| {
            window.global::<ToastQueue>().invoke_push(
                id,
                SharedString::from(text),
                variant,
                loading,
            );
        });
        id
    }

    fn update(&self, id: i32, text: String, variant: ToastVariant) {
        let Some(window) = self.window() else {
            tracing::warn!(message = %text, "notification update dropped before UI initialization");
            return;
        };
        let _ = window.upgrade_in_event_loop(move |window| {
            window
                .global::<ToastQueue>()
                .invoke_update(id, SharedString::from(text), variant);
        });
    }

    fn window(&self) -> Option<slint::Weak<MainWindow>> {
        self.window.get().map(|window| {
            window
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        })
    }

    pub(crate) fn error(&self, text: impl Into<String>) {
        self.error_with_title("Something went wrong", text);
    }

    pub(crate) fn error_with_title(&self, title: impl Into<String>, message: impl Into<String>) {
        let Some(window) = self.window.get().map(|window| {
            window
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }) else {
            tracing::warn!(message = %message.into(), "error notification dropped before UI initialization");
            return;
        };
        let title = title.into();
        let message = message.into();
        let _ = window.upgrade_in_event_loop(move |window| {
            let error = window.global::<ErrorSheetState>();
            error.set_title(SharedString::from(title));
            error.set_message(SharedString::from(message));
            error.set_open(true);
        });
    }

    pub(crate) fn success(&self, text: impl Into<String>) {
        self.show(text, ToastVariant::Success);
    }
}

impl LoadingNotification {
    pub(crate) fn info(self, text: impl Into<String>) {
        NOTIFICATION.update(self.id, text.into(), ToastVariant::Default);
    }

    pub(crate) fn success(self, text: impl Into<String>) {
        NOTIFICATION.update(self.id, text.into(), ToastVariant::Success);
    }

    pub(crate) fn error(self, text: impl Into<String>) {
        NOTIFICATION.update(self.id, text.into(), ToastVariant::Error);
    }
}

fn push_toast(
    items: Rc<VecModel<ToastItem>>,
    id: i32,
    text: SharedString,
    variant: ToastVariant,
    loading: bool,
) {
    items.push(ToastItem {
        id,
        text,
        variant,
        loading,
        dismissed: false,
    });
    if !loading {
        schedule_dismiss(items, id);
    }
}

fn update_toast(
    items: Rc<VecModel<ToastItem>>,
    id: i32,
    text: SharedString,
    variant: ToastVariant,
) {
    let Some(index) = (0..items.row_count())
        .find(|&index| items.row_data(index).is_some_and(|item| item.id == id))
    else {
        push_toast(items, id, text, variant, false);
        return;
    };
    let Some(mut item) = items.row_data(index) else {
        return;
    };
    item.text = text;
    item.variant = variant;
    item.loading = false;
    item.dismissed = false;
    items.set_row_data(index, item);
    schedule_dismiss(items, id);
}

fn schedule_dismiss(items: Rc<VecModel<ToastItem>>, id: i32) {
    slint::Timer::single_shot(TOAST_DURATION, move || {
        dismiss_toast(items, id);
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
    if item.loading || item.dismissed {
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
