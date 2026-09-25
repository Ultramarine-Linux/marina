//! Slint window construction and application-lifetime wiring.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use marina_input::{InputConfig, InputLoop};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use tracing::{error, info, warn};

use crate::{
    GameState, HomeState, LibraryState, MainWindow, app, covers, empty_game_card,
    empty_preview_details, startup, ui,
};

const CONTROLLER_STARTUP_DELAY: Duration = Duration::from_secs(1);

pub(crate) async fn run() -> Result<(), slint::PlatformError> {
    // Keep the first window construction independent of filesystem-backed profile
    // discovery. The shell starts with a useful fallback and hydrates the profile
    // asynchronously once the event loop is running.
    let username = std::env::var("USER").unwrap_or_else(|_| "user".into());
    let window = MainWindow::new()?;
    let library_state: Arc<Mutex<Option<app::AppStateHandle>>> = Arc::new(Mutex::new(None));
    ui::settings::configure(&window, &library_state);
    // Controller events are allowed only while Marina owns the native window,
    // but the selected Slint focus target belongs to the UI regardless of
    // whether a controller is connected.
    let window_active = Arc::new(AtomicBool::new(true));
    let focus_state = window_active.clone();
    let focus_window = window.as_weak();
    i_slint_core::context::set_window_event_hook(Some(Box::new(
        move |_adapter, event, _result| {
            if let i_slint_core::platform::WindowEvent::WindowActiveChanged(active) = event {
                focus_state.store(*active, Ordering::Release);
                if *active {
                    // A controller hot-unplug can make the compositor rebuild
                    // its input focus. Reassert the mounted page's focus scope
                    // after the native window becomes active again, including
                    // for keyboard- and pointer-only use.
                    let focus_window = focus_window.clone();
                    let _ = focus_window.upgrade_in_event_loop(move |window| {
                        ui::controls::restore_content_focus(&window);
                    });
                }
            }
        },
    )))?;
    slint::set_xdg_app_id("org.ultramarinelinux.MarinaShell")?;
    info!("main window constructed; completing lightweight UI setup");
    ui::controls::configure_toasts(&window);
    ui::controls::configure_navigation(&window);
    ui::controls::configure_profile_menu(&window);

    // Controller discovery can block inside gilrs while probing input devices.
    // Never make window creation wait for it; initialize it after the event loop
    // has started and keep the returned loop alive in its background task.
    let controller_window = window.as_weak();
    let window_active = window_active.clone();
    tokio::spawn(async move {
        // gilrs probes every input device during construction. Let the first
        // frame and initial library query get CPU priority before doing that
        // work, especially on handhelds with slow input device enumeration.
        tokio::time::sleep(CONTROLLER_STARTUP_DELAY).await;
        let started = std::time::Instant::now();
        info!("starting controller input discovery");
        let result = tokio::task::spawn_blocking(move || {
            InputLoop::spawn(InputConfig::default(), move |event| {
                if !window_active.load(Ordering::Acquire) {
                    return;
                }
                let controller_window = controller_window.clone();
                let _ = controller_window.upgrade_in_event_loop(move |window| {
                    // gilrs remains alive while another application owns focus,
                    // but controller actions must only reach Marina when its
                    // native window is active.
                    ui::controls::dispatch_controller_action(&window, event);
                });
            })
        })
        .await;

        match result {
            Ok(Ok(input)) => {
                info!(
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "controller input ready"
                );
                // InputLoop owns its polling thread and must stay alive for the
                // lifetime of the application.
                std::future::pending::<()>().await;
                drop(input);
            }
            Ok(Err(error)) => {
                warn!(
                    %error,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "controller input unavailable; keyboard and pointer input remain active"
                );
            }
            Err(error) => {
                warn!(
                    %error,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "controller input task failed; keyboard and pointer input remain active"
                );
            }
        }
    });
    ui::profile::initialize(&window, username.clone());
    ui::battery::initialize(&window);
    ui::networking::initialize(&window);
    ui::clock::initialize(&window);
    window
        .global::<HomeState>()
        .set_games(ModelRc::from(std::rc::Rc::new(VecModel::from(Vec::new()))));
    window
        .global::<HomeState>()
        .set_played_games(ModelRc::from(std::rc::Rc::new(VecModel::from(Vec::new()))));
    window
        .global::<LibraryState>()
        .set_games(ModelRc::from(std::rc::Rc::new(VecModel::from(Vec::new()))));
    window
        .global::<GameState>()
        .set_selected_game(empty_game_card());
    window
        .global::<GameState>()
        .set_details(empty_preview_details());
    window
        .global::<GameState>()
        .set_tags(ModelRc::from(std::rc::Rc::new(VecModel::from(Vec::<
            SharedString,
        >::new(
        )))));

    window.global::<HomeState>().set_loading(true);
    window.global::<LibraryState>().set_loading(false);

    let cover_loader = covers::ViewportLoader::new(&window);
    let loader_sources = cover_loader.borrow().added_sources();
    let played_sources = cover_loader.borrow().played_sources();
    let home_source_store = Arc::new(Mutex::new(Vec::new()));
    let scroll_loader = cover_loader.clone();
    window.global::<HomeState>().on_viewport_changed(
        move |shelf, scroll_x, width, cover_height, visible| {
            scroll_loader.borrow_mut().update(
                covers::Page::Home,
                covers::Shelf::from_index(shelf),
                scroll_x,
                width,
                cover_height,
                visible,
            );
        },
    );
    let context_loader = cover_loader.clone();
    let context_loader_sources = loader_sources.clone();
    let context_home_sources = home_source_store.clone();
    window
        .global::<HomeState>()
        .on_cover_context_changed(move |active_tab| {
            let mut loader = context_loader.borrow_mut();
            if active_tab != 0 {
                loader.suspend(covers::Page::Home);
                return;
            }
            // Refresh is idempotent: it preserves resident and in-flight
            // covers, avoiding duplicate decode waves when model and route
            // notifications arrive during the same Home entry.
            *context_loader_sources
                .lock()
                .expect("cover source state poisoned") = context_home_sources
                .lock()
                .expect("home cover source state poisoned")
                .clone();
            loader.refresh(covers::Page::Home);
        });

    let played_store = ui::pages::home::new_played_store();
    ui::launch::install(&window, &library_state, &played_store, &played_sources);
    ui::pages::home::install(&window, &library_state, &home_source_store, &played_store);
    ui::pages::library::install(&window, &library_state);

    let weak_window = window.as_weak();
    let state_store = library_state.clone();
    let played_hydration = played_store.clone();
    let played_source_hydration = played_sources.clone();
    let played_window = weak_window.clone();
    let startup_sources = loader_sources.clone();
    let startup_home_sources = home_source_store.clone();
    let cached_home_ready = Arc::new(tokio::sync::Notify::new());
    slint::Timer::single_shot(Duration::ZERO, move || {
        tokio::spawn(async move {
            let state = match app::AppState::initialize().await {
                Ok(state) => state,
                Err(error) => {
                    error!(%error, "application initialization failed");
                    return;
                }
            };
            *state_store.lock().expect("library state lock poisoned") = Some(state.clone());

            // Restore persisted play activity into the recently-played shelf
            // alongside the other independent hydration phases.
            tokio::spawn(ui::pages::home::hydrate_played(
                state.clone(),
                played_window.clone(),
                played_hydration.clone(),
                played_source_hydration.clone(),
            ));

            // Each phase owns its own work and can publish as soon as it is ready.
            tokio::spawn(startup::hydrate_cached_home(
                state.clone(),
                weak_window.clone(),
                startup_sources.clone(),
                startup_home_sources.clone(),
                cached_home_ready.clone(),
            ));
            tokio::spawn(startup::reconcile_local(
                state.clone(),
                weak_window.clone(),
                startup_sources,
                startup_home_sources,
                cached_home_ready,
            ));
            tokio::spawn(startup::hydrate_startup_platforms(
                state.clone(),
                weak_window.clone(),
            ));
            tokio::spawn(startup::reconcile_stores(state));
        });
    });

    ui::pages::store::install(&window, &library_state);

    info!("starting Slint event loop; deferred startup work will run in background");
    window.run()
}
