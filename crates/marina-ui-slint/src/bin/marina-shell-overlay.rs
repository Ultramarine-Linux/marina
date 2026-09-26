use std::{
    cell::RefCell,
    path::PathBuf,
    process,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

use layer_shika::{
    calloop::TimeoutAction,
    prelude::{KeyboardInteractivity, Shell, ShellControl},
    slint::{Image, ModelRc, SharedString, VecModel},
    slint_interpreter::{ComponentInstance, Struct, Value},
};
use marina_input::InputEvent;
use marina_shell::{
    OverlayRequest, SwayWindowManager, build_overlay_shell, layer_shell::Surface,
    spawn_overlay_server, validate_overlay_ui,
};
use tracing::{debug, error, info, warn};

#[path = "marina-shell-overlay/controller.rs"]
mod controller;
#[path = "marina-shell-overlay/input.rs"]
mod input;

use controller::{ControllerState, OverlayMessage, OverlayMode, Presentation};

const INSTALLED_UI_ROOT: &str = "/usr/share/marina/ui";
const SURFACE_NAME: &str = "WindowOverlay";
const ANIMATION_FRAME_INTERVAL: Duration = Duration::from_millis(16);
const RESUME_CLOCK_GAP: Duration = Duration::from_millis(500);
const RESUME_EXIT_CODE: i32 = 75;

thread_local! {
    static SWITCH_ICON_CACHE: RefCell<Vec<(String, Image)>> = RefCell::new(Vec::new());
}

fn load_switch_icon(name: &str) -> Image {
    SWITCH_ICON_CACHE.with(|cache| {
        if let Some((_, image)) = cache.borrow().iter().find(|(cached, _)| cached == name) {
            return image.clone();
        }

        let Some(path) = marina_apps::resolve_icon_in_theme(name, "marina-assets") else {
            warn!(icon = name, "switch icon was not found");
            return Image::default();
        };
        let Ok(image) = Image::load_from_path(std::path::Path::new(&path)) else {
            warn!(icon = name, %path, "switch icon could not be loaded");
            return Image::default();
        };
        cache.borrow_mut().push((name.to_owned(), image.clone()));
        image
    })
}

fn spawn_control_operation(
    state: Arc<Mutex<ControllerState>>,
    sender: layer_shika::calloop::channel::Sender<OverlayMessage>,
    runtime: Arc<tokio::runtime::Runtime>,
    operation: OverlayMessage,
) {
    match operation {
        OverlayMessage::SwitchProfile(profile_name) => {
            let presentation = state
                .lock()
                .expect("overlay state lock poisoned")
                .presentation("Applying power profile…");
            send_overlay_message(&sender, OverlayMessage::Presentation(presentation));

            runtime.spawn(async move {
                let result = tokio::time::timeout(
                    Duration::from_secs(2),
                    marina_power::switch_tuned_profile(&profile_name),
                )
                .await;
                let presentation = {
                    let mut state = state.lock().expect("overlay state lock poisoned");
                    state.profile_switch_pending = false;
                    match result {
                        Ok(Ok(true)) => {
                            state.active_profile = profile_name;
                            state.mode = OverlayMode::QuickSettings;
                            state.presentation("")
                        }
                        Ok(Ok(false)) => state.presentation("TuneD declined the selected profile"),
                        Ok(Err(error)) => state.presentation(error.to_string()),
                        Err(_) => state.presentation("TuneD timed out while switching profile"),
                    }
                };
                send_overlay_message(&sender, OverlayMessage::Presentation(presentation));
            });
        }
        OverlayMessage::SetBrightness(value) => {
            let presentation = state
                .lock()
                .expect("overlay state lock poisoned")
                .presentation("Applying brightness…");
            send_overlay_message(&sender, OverlayMessage::Presentation(presentation));
            runtime.spawn(async move {
                let result = tokio::task::spawn_blocking(move || {
                    marina_power::set_display_brightness(value)
                })
                .await;
                let status = match result {
                    Ok(Ok(())) => String::new(),
                    Ok(Err(error)) => error.to_string(),
                    Err(error) => format!("brightness task failed: {error}"),
                };
                let presentation = state
                    .lock()
                    .expect("overlay state lock poisoned")
                    .presentation(status);
                send_overlay_message(&sender, OverlayMessage::Presentation(presentation));
            });
        }
        OverlayMessage::SetVolume(value) => {
            let presentation = state
                .lock()
                .expect("overlay state lock poisoned")
                .presentation("Applying volume…");
            send_overlay_message(&sender, OverlayMessage::Presentation(presentation));
            runtime.spawn(async move {
                let result =
                    tokio::task::spawn_blocking(move || marina_audio::set_volume(value)).await;
                let status = match result {
                    Ok(Ok(())) => String::new(),
                    Ok(Err(error)) => error.to_string(),
                    Err(error) => format!("volume task failed: {error}"),
                };
                let presentation = state
                    .lock()
                    .expect("overlay state lock poisoned")
                    .presentation(status);
                send_overlay_message(&sender, OverlayMessage::Presentation(presentation));
            });
        }
        _ => {}
    }
}

fn spawn_quick_settings_hydration(
    state: Arc<Mutex<ControllerState>>,
    sender: layer_shika::calloop::channel::Sender<OverlayMessage>,
    runtime: Arc<tokio::runtime::Runtime>,
) {
    runtime.spawn(async move {
        tracing::debug!("loading TuneD profiles for quick settings");
        let (profiles, active) = tokio::join!(
            tokio::time::timeout(Duration::from_secs(2), marina_power::tuned_profiles()),
            tokio::time::timeout(Duration::from_secs(2), marina_power::active_tuned_profile()),
        );
        let profiles = profiles
            .map_err(|_| "profile query timed out".to_owned())
            .and_then(|result| result.map_err(|error| error.to_string()));
        let active = active
            .map_err(|_| "active-profile query timed out".to_owned())
            .and_then(|result| result.map_err(|error| error.to_string()));

        let brightness = tokio::task::spawn_blocking(marina_power::display_brightness)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(0);
        let volume = tokio::task::spawn_blocking(marina_audio::volume)
            .await
            .ok()
            .and_then(Result::ok)
            .map(|state| state.percent)
            .unwrap_or(0);
        let battery = marina_power::read_battery_status().await;
        let network = marina_networking::read_network_status().await;

        let presentation = state
            .lock()
            .expect("overlay state lock poisoned")
            .apply_quick_snapshot(profiles, active, brightness, volume, battery, network);
        let _ = sender.send(OverlayMessage::Presentation(presentation));
    });
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    marina_logging::init();

    let ui_path = overlay_ui_path();
    if std::env::args().any(|argument| argument == "--check-ui") {
        validate_overlay_ui(ui_path).map_err(std::io::Error::other)?;
        return Ok(());
    }

    monitor_suspend_resume();
    let manager = SwayWindowManager::new()?;
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?,
    );
    let state = Arc::new(Mutex::new(ControllerState::new(manager)));
    let visible = Arc::new(AtomicBool::new(false));
    let (inputplumber_control, inputplumber_modes) = marina_input::inputplumber::intercept_control(
        marina_input::inputplumber::InterceptMode::Pass,
    );

    let mut shell = build_overlay_shell(&ui_path)?;
    let control = capture_shell_control(&shell)?;
    control.surface(SURFACE_NAME).set_input_region(0, 0, 0, 0)?;

    let handle = shell.event_loop_handle();
    let event_control = control.clone();
    let event_visible = visible.clone();
    let event_inputplumber = inputplumber_control.clone();
    let (_token, sender) =
        handle.add_channel(move |message: OverlayMessage, app_state| match message {
            OverlayMessage::Show(presentation) => {
                event_inputplumber.set_mode(marina_input::inputplumber::InterceptMode::All);
                for surface in app_state.all_outputs() {
                    apply_presentation_to_component(surface.component_instance(), &presentation);
                    set_open_on_component(surface.component_instance(), true);
                }
                let surface = event_control.surface(SURFACE_NAME);
                if let Err(error) = surface.set_input_region(0, 0, i32::MAX, i32::MAX) {
                    error!(%error, "failed to enable overlay input region");
                }
                if let Err(error) =
                    surface.set_keyboard_interactivity(KeyboardInteractivity::Exclusive)
                {
                    error!(%error, "failed to acquire exclusive overlay keyboard focus");
                }
            }
            OverlayMessage::Hide => {
                event_inputplumber.set_mode(marina_input::inputplumber::InterceptMode::Pass);
                event_visible.store(false, Ordering::Release);
                for surface in app_state.all_outputs() {
                    set_open_on_component(surface.component_instance(), false);
                }
                let surface = event_control.surface(SURFACE_NAME);
                if let Err(error) = surface.set_keyboard_interactivity(KeyboardInteractivity::None)
                {
                    error!(%error, "failed to release overlay keyboard focus");
                }
                if let Err(error) = surface.set_input_region(0, 0, 0, 0) {
                    error!(%error, "failed to clear overlay input region");
                }
            }
            OverlayMessage::Presentation(presentation) => {
                for surface in app_state.all_outputs() {
                    apply_presentation_to_component(surface.component_instance(), &presentation);
                }
            }
            OverlayMessage::Battery(status) => {
                for surface in app_state.all_outputs() {
                    apply_battery_status(surface.component_instance(), status);
                }
            }
            OverlayMessage::Network(status) => {
                for surface in app_state.all_outputs() {
                    apply_network_status(surface.component_instance(), status);
                }
            }
            OverlayMessage::Clock(clock) => {
                for surface in app_state.all_outputs() {
                    if let Err(error) = surface.component_instance().set_global_property(
                        "ClockState",
                        "time",
                        Value::String(clock.clone().into()),
                    ) {
                        warn!(%error, "failed to update overlay clock state");
                    }
                }
            }
            OverlayMessage::SwitchProfile(_)
            | OverlayMessage::SetBrightness(_)
            | OverlayMessage::SetVolume(_) => {
                warn!("control operation reached the UI channel without being scheduled");
            }
        })?;

    // layer-shika currently updates Slint timers only when calloop wakes for
    // another source. A real timer source guarantees one render iteration per
    // animation frame instead of relying on coalesced channel wakeups.
    let _animation_timer = handle
        .add_timer(ANIMATION_FRAME_INTERVAL, |_deadline, _app_state| {
            TimeoutAction::ToDuration(ANIMATION_FRAME_INTERVAL)
        })?;

    let dismiss_sender = sender.clone();
    shell
        .select(Surface::named(SURFACE_NAME))
        .on_callback("dismissed", move |_context| {
            send_overlay_message(&dismiss_sender, OverlayMessage::Hide);
        });

    let battery_sender = sender.clone();
    let battery_state = state.clone();
    runtime.spawn(marina_power::monitor_battery_status(move |status| {
        battery_state
            .lock()
            .expect("overlay state lock poisoned")
            .battery = status;
        let _ = battery_sender.send(OverlayMessage::Battery(status));
    }));

    let network_sender = sender.clone();
    let network_state = state.clone();
    runtime.spawn(marina_networking::monitor_network_status(move |status| {
        network_state
            .lock()
            .expect("overlay state lock poisoned")
            .network = status;
        let _ = network_sender.send(OverlayMessage::Network(status));
    }));

    let clock_sender = sender.clone();
    let clock_state = state.clone();
    runtime.spawn(async move {
        loop {
            let clock = marina_ui_slint::clock_format::current_time_string(
                marina_ui_slint::config::shared()
                    .map(|config| config.snapshot().clock_twelve_hour)
                    .unwrap_or(false),
            );
            clock_state
                .lock()
                .expect("overlay state lock poisoned")
                .clock_text
                .clone_from(&clock);
            if clock_sender.send(OverlayMessage::Clock(clock)).is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    let request_sender = sender.clone();
    let request_state = state.clone();
    let request_runtime = runtime.clone();
    let request_visible = visible.clone();
    let request_inputplumber = inputplumber_control.clone();
    let request_handler: Arc<dyn Fn(OverlayRequest) + Send + Sync> = Arc::new(move |request| {
        let should_show = match request {
            OverlayRequest::Show | OverlayRequest::QuickSettings => true,
            OverlayRequest::Hide => false,
            OverlayRequest::Toggle => !request_visible.load(Ordering::Acquire),
        };
        if !should_show {
            request_visible.store(false, Ordering::Release);
            request_inputplumber.set_mode(marina_input::inputplumber::InterceptMode::Pass);
            send_overlay_message(&request_sender, OverlayMessage::Hide);
            return;
        }

        if request == OverlayRequest::QuickSettings {
            let presentation = request_state
                .lock()
                .expect("overlay state lock poisoned")
                .begin_quick_settings();
            request_visible.store(true, Ordering::Release);
            send_overlay_message(&request_sender, OverlayMessage::Show(presentation));
            spawn_quick_settings_hydration(
                request_state.clone(),
                request_sender.clone(),
                request_runtime.clone(),
            );
            return;
        }

        let presentation = request_state
            .lock()
            .expect("overlay state lock poisoned")
            .refresh();
        match presentation {
            Ok(presentation) => {
                request_visible.store(true, Ordering::Release);
                send_overlay_message(&request_sender, OverlayMessage::Show(presentation));
            }
            Err(error) => {
                request_visible.store(false, Ordering::Release);
                request_inputplumber.set_mode(marina_input::inputplumber::InterceptMode::Pass);
                send_overlay_message(&request_sender, OverlayMessage::Hide);
                warn!(%error, "failed to refresh Sway windows");
            }
        }
    });

    let server_handler = request_handler.clone();
    let _server = spawn_overlay_server(move |request| server_handler(request))?;

    let input_sender = sender.clone();
    let input_state = state.clone();
    let input_visible = visible.clone();
    let input_runtime = runtime.clone();
    let dispatch_inputplumber = inputplumber_control.clone();
    let dispatch_visible: Arc<dyn Fn(InputEvent) + Send + Sync> = Arc::new(move |event| {
        if !input_visible.load(Ordering::Acquire) {
            return;
        }
        let message = input_state
            .lock()
            .expect("overlay state lock poisoned")
            .handle(event);
        if let Some(message) = message {
            if matches!(message, OverlayMessage::Hide) {
                input_visible.store(false, Ordering::Release);
                dispatch_inputplumber.set_mode(marina_input::inputplumber::InterceptMode::Pass);
            }
            match message {
                OverlayMessage::SwitchProfile(_)
                | OverlayMessage::SetBrightness(_)
                | OverlayMessage::SetVolume(_) => {
                    spawn_control_operation(
                        input_state.clone(),
                        input_sender.clone(),
                        input_runtime.clone(),
                        message,
                    );
                }
                message => {
                    let _ = input_sender.send(message);
                }
            }
        }
    });

    let gilrs_dispatch = dispatch_visible.clone();
    input::spawn_gilrs(move |event| gilrs_dispatch(event))?;

    let dbus_dispatch = dispatch_visible.clone();
    let dbus_visible = visible.clone();
    let dbus_request = request_handler.clone();
    let dbus_inputplumber = inputplumber_control.clone();
    let dbus_router = Arc::new(Mutex::new(input::DbusOverlayRouter::default()));
    input::spawn_inputplumber(&runtime, inputplumber_modes, move |event| {
        let action = dbus_router
            .lock()
            .expect("InputPlumber router lock poisoned")
            .route(event, dbus_visible.load(Ordering::Acquire));
        match action {
            input::DbusOverlayAction::ShowGameMenu => {
                dbus_inputplumber.observe_mode(marina_input::inputplumber::InterceptMode::All);
                dbus_request(OverlayRequest::Show);
            }
            input::DbusOverlayAction::ShowQuickSettings => {
                dbus_inputplumber.observe_mode(marina_input::inputplumber::InterceptMode::All);
                dbus_request(OverlayRequest::QuickSettings);
            }
            input::DbusOverlayAction::Dispatch(event) => dbus_dispatch(event),
            input::DbusOverlayAction::Ignore => {}
        }
    });

    info!("persistent window overlay ready");
    let result = shell.run();
    inputplumber_control.set_mode(marina_input::inputplumber::InterceptMode::Pass);
    runtime.block_on(async {
        match marina_input::inputplumber::Client::connect().await {
            Ok(client) => {
                if let Err(error) = client
                    .set_intercept_mode(marina_input::inputplumber::InterceptMode::Pass)
                    .await
                {
                    warn!(%error, "failed to restore InputPlumber pass-through during shutdown");
                }
            }
            Err(error) => debug!(%error, "InputPlumber unavailable during overlay shutdown"),
        }
    });
    result?;
    Ok(())
}

fn send_overlay_message(
    sender: &layer_shika::calloop::channel::Sender<OverlayMessage>,
    message: OverlayMessage,
) {
    let _ = sender.send(message);
}

fn monitor_suspend_resume() {
    thread::Builder::new()
        .name("marina-overlay-resume-monitor".to_owned())
        .spawn(|| {
            let mut wall = SystemTime::now();
            let mut monotonic = Instant::now();
            loop {
                thread::sleep(Duration::from_secs(1));
                let next_wall = SystemTime::now();
                let next_monotonic = Instant::now();
                let wall_elapsed = next_wall.duration_since(wall).unwrap_or_default();
                let monotonic_elapsed = next_monotonic.duration_since(monotonic);
                if wall_elapsed.saturating_sub(monotonic_elapsed) > RESUME_CLOCK_GAP {
                    warn!(
                        wall_elapsed_ms = wall_elapsed.as_millis() as u64,
                        monotonic_elapsed_ms = monotonic_elapsed.as_millis() as u64,
                        "resume detected; restarting overlay to recreate its Wayland surface"
                    );
                    process::exit(RESUME_EXIT_CODE);
                }
                wall = next_wall;
                monotonic = next_monotonic;
            }
        })
        .expect("failed to start overlay resume monitor");
}

fn capture_shell_control(shell: &Shell) -> Result<ShellControl, Box<dyn std::error::Error>> {
    let slot = Rc::new(RefCell::new(None));
    let callback_slot = slot.clone();
    let selection = shell.select(Surface::named(SURFACE_NAME));
    selection.on_callback("control-requested", move |context| {
        *callback_slot.borrow_mut() = Some(context.control().clone());
    });
    selection.with_component(|component| {
        if let Err(error) = component.invoke("control-requested", &[]) {
            warn!(%error, "failed to obtain layer-shell control handle");
        }
    });
    slot.borrow_mut().take().ok_or_else(|| {
        std::io::Error::other("layer-shika did not provide a shell control handle").into()
    })
}

fn apply_presentation_to_component(component: &ComponentInstance, presentation: &Presentation) {
    for (name, value) in [
        ("active-title", presentation.active_title.as_str()),
        ("selected-title", presentation.title.as_str()),
        ("selected-app-id", presentation.app_id.as_str()),
        ("selected-location", presentation.location.as_str()),
        ("position-label", presentation.position.as_str()),
        ("status-text", presentation.status.as_str()),
        ("active-profile", presentation.active_profile.as_str()),
    ] {
        set_component_value(component, name, Value::String(value.into()));
    }
    set_component_value(
        component,
        "window-mode",
        Value::Bool(presentation.window_mode),
    );
    set_component_value(
        component,
        "quick-mode",
        Value::Bool(presentation.quick_mode),
    );
    set_component_value(
        component,
        "profile-mode",
        Value::Bool(presentation.profile_mode),
    );
    set_component_value(
        component,
        "menu-selected",
        Value::Number(f64::from(presentation.menu_selected)),
    );
    for (name, image) in [
        ("dpad-icon", load_switch_icon("dpad-default")),
        ("accept-icon", load_switch_icon("button-south")),
        ("back-icon", load_switch_icon("button-east")),
        ("menu-icon", load_switch_icon("button-home")),
    ] {
        set_component_value(component, name, Value::Image(image));
    }
    for (name, value) in [
        ("quick-selected", presentation.quick_selected),
        ("brightness", presentation.brightness),
        ("volume", presentation.volume),
        ("active-profile-index", presentation.active_profile_index),
        ("profile-selected", presentation.profile_selected),
    ] {
        set_component_value(component, name, Value::Number(f64::from(value)));
    }
    let profiles = presentation
        .profile_names
        .iter()
        .map(|name| {
            Value::Struct(Struct::from_iter([
                (
                    "value".to_owned(),
                    Value::String(SharedString::from(name.as_str())),
                ),
                (
                    "label".to_owned(),
                    Value::String(SharedString::from(name.as_str())),
                ),
            ]))
        })
        .collect::<Vec<_>>();
    set_component_value(
        component,
        "profile-items",
        Value::Model(ModelRc::from(Rc::new(VecModel::from(profiles)))),
    );
    let titles = presentation
        .window_titles
        .iter()
        .map(|title| Value::String(SharedString::from(title.as_str())))
        .collect::<Vec<_>>();
    set_component_value(
        component,
        "window-titles",
        Value::Model(ModelRc::from(Rc::new(VecModel::from(titles)))),
    );
    set_component_value(
        component,
        "selected-index",
        Value::Number(f64::from(presentation.selected_index)),
    );
    apply_battery_status(
        component,
        marina_power::BatteryStatus {
            available: presentation.battery_available,
            percentage: presentation.battery_percentage,
            charging: presentation.battery_charging,
        },
    );
    if let Err(error) = component.set_global_property(
        "ClockState",
        "time",
        Value::String(presentation.clock_text.clone().into()),
    ) {
        warn!(%error, "failed to update overlay clock state");
    }
    apply_network_status(
        component,
        marina_networking::NetworkStatus {
            available: presentation.network_available,
            connected: presentation.network_connected,
            wifi_signal_strength: (presentation.network_signal >= 0)
                .then_some(presentation.network_signal as u8),
        },
    );
}

fn apply_battery_status(component: &ComponentInstance, status: marina_power::BatteryStatus) {
    for (property, value) in [
        ("available", Value::Bool(status.available)),
        ("percentage", Value::Number(status.percentage)),
        ("charging", Value::Bool(status.charging)),
    ] {
        if let Err(error) = component.set_global_property("BatteryState", property, value) {
            warn!(%error, property, "failed to update overlay battery state");
        }
    }
}

fn apply_network_status(component: &ComponentInstance, status: marina_networking::NetworkStatus) {
    for (property, value) in [
        ("available", Value::Bool(status.available)),
        ("connected", Value::Bool(status.connected)),
        (
            "signal-strength",
            Value::Number(f64::from(
                status.wifi_signal_strength.map(i32::from).unwrap_or(-1),
            )),
        ),
    ] {
        if let Err(error) = component.set_global_property("NetworkState", property, value) {
            warn!(%error, property, "failed to update overlay network state");
        }
    }
}

fn set_open_on_component(component: &ComponentInstance, open: bool) {
    set_component_value(component, "overlay-open", Value::Bool(open));
}

fn set_component_value(component: &ComponentInstance, name: &str, value: Value) {
    if let Err(error) = component.set_property(name, value) {
        warn!(%error, property = name, "failed to update window overlay");
    }
}

fn overlay_ui_path() -> PathBuf {
    let installed = PathBuf::from(INSTALLED_UI_ROOT).join("pages/window-overlay.slint");
    if installed.is_file() {
        installed
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/pages/window-overlay.slint")
    }
}
