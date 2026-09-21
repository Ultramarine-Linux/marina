//! Breadcrumb navigation trail.
//!
//! Back (header buttons, Escape, controller) traverses this trail instead of
//! hardcoding destinations, so returning from a game detail lands on the
//! game list it came from — even though route changes destroy and recreate
//! the page components. Drilled sub-page state (library/store `page` and
//! selected platform) lives in globals for the same reason; the trail only
//! records *where*, the globals preserve *what*.
//!
//! The trail is a path, not a history: tabs are roots (jumping rebase to
//! just the tab), drills append beneath them. Open games are not pushed:
//! detail views render the selected game title as the visual current crumb,
//! and back from a detail restores the trail top beneath it.

use std::sync::{Mutex, OnceLock};

use crate::{
    BreadcrumbItem, GameState, HomeState, LibraryState, MainWindow, ShellPage, ShellState,
    StoreState,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

const MAX_CRUMBS: usize = 25;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Crumb {
    Tab(Tab),
    LibraryPlatform { slug: String, name: String },
    StorePlatform { slug: String, name: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tab {
    Home,
    Library,
    Store,
}

impl Tab {
    fn index(self) -> i32 {
        match self {
            Self::Home => 0,
            Self::Library => 1,
            Self::Store => 2,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Library => "Library",
            Self::Store => "Store",
        }
    }
}

impl Crumb {
    fn label(&self) -> String {
        match self {
            Self::Tab(tab) => tab.label().to_owned(),
            Self::LibraryPlatform { name, .. } | Self::StorePlatform { name, .. } => name.clone(),
        }
    }
}

fn stack() -> &'static Mutex<Vec<Crumb>> {
    static STACK: OnceLock<Mutex<Vec<Crumb>>> = OnceLock::new();
    STACK.get_or_init(|| Mutex::new(Vec::new()))
}

/// Suppresses drill pushes while a restore re-invokes query callbacks.
/// Slint callbacks invoked from Rust run synchronously, so a plain bool is
/// sufficient (set around the invoke, cleared right after).
fn suppress_push() -> &'static Mutex<bool> {
    static SUPPRESS: OnceLock<Mutex<bool>> = OnceLock::new();
    SUPPRESS.get_or_init(|| Mutex::new(false))
}

fn with_stack(update: impl FnOnce(&mut Vec<Crumb>)) {
    update(&mut stack().lock().expect("breadcrumb stack poisoned"));
}

fn push(crumb: Crumb) {
    with_stack(|stack| {
        if stack.last().is_some_and(|top| *top == crumb) {
            return;
        }
        stack.push(crumb);
        while stack.len() > MAX_CRUMBS {
            stack.remove(0);
        }
    });
}

/// Records a tab jump. Tabs are trail roots: jumping rebase the trail to
/// just the tab, since a breadcrumb shows the path to the current location,
/// not the temporal history that led there. No ping-pong accumulation.
pub(crate) fn push_tab(window: &MainWindow, index: i32) {
    let tab = match index {
        1 => Tab::Library,
        2 => Tab::Store,
        _ => Tab::Home,
    };
    with_stack(|stack| {
        stack.clear();
        stack.push(Crumb::Tab(tab));
    });
    publish(window);
}

/// Records drilling into a library platform. No-op during restores.
pub(crate) fn drill_library(window: &MainWindow, slug: &str) {
    if *suppress_push().lock().expect("breadcrumb lock poisoned") {
        return;
    }
    let home = window.global::<LibraryState>();
    let platforms = home.get_platforms();
    let name = (0..platforms.row_count())
        .filter_map(|index| platforms.row_data(index))
        .find(|platform| platform.slug.as_str() == slug)
        .map(|platform| platform.name.to_string())
        .unwrap_or_else(|| slug.to_owned());
    push(Crumb::LibraryPlatform {
        slug: slug.to_owned(),
        name,
    });
    publish(window);
}

/// Records drilling into a store platform. No-op during restores.
pub(crate) fn drill_store(window: &MainWindow, slug: &str) {
    if *suppress_push().lock().expect("breadcrumb lock poisoned") {
        return;
    }
    let store = window.global::<StoreState>();
    let platforms = store.get_platforms();
    let name = (0..platforms.row_count())
        .filter_map(|index| platforms.row_data(index))
        .find(|platform| platform.slug.as_str() == slug)
        .map(|platform| platform.name.to_string())
        .unwrap_or_else(|| slug.to_owned());
    push(Crumb::StorePlatform {
        slug: slug.to_owned(),
        name,
    });
    publish(window);
}

/// Rebuilds the widget model: stack labels plus the open game title as the
/// visual current crumb on detail routes.
pub(crate) fn publish(window: &MainWindow) {
    let shell = window.global::<ShellState>();
    if stack()
        .lock()
        .expect("breadcrumb stack poisoned")
        .is_empty()
    {
        let tab = match shell.get_active_tab() {
            1 => Tab::Library,
            2 => Tab::Store,
            _ => Tab::Home,
        };
        push(Crumb::Tab(tab));
    }
    let mut labels: Vec<String> = stack()
        .lock()
        .expect("breadcrumb stack poisoned")
        .iter()
        .map(Crumb::label)
        .collect();
    if shell.get_page() == ShellPage::GameDetails {
        let title = window.global::<GameState>().get_selected_game().title;
        if !title.is_empty() {
            labels.push(title.to_string());
        }
    }
    shell.set_breadcrumbs(ModelRc::from(std::rc::Rc::new(VecModel::from(
        labels
            .into_iter()
            .map(|label| BreadcrumbItem {
                label: SharedString::from(label),
            })
            .collect::<Vec<_>>(),
    ))));
}

/// Jumps to crumb `index`, dropping everything above it.
pub(crate) fn jump(window: &MainWindow, index: i32) {
    let target = with_stack_result(|stack| {
        let index = index.max(0) as usize;
        if index >= stack.len() {
            return None;
        }
        stack.truncate(index + 1);
        stack.last().cloned()
    });
    let Some(target) = target else { return };
    restore(window, &target);
    let weak = window.as_weak();
    let _ = weak.upgrade_in_event_loop(move |window| {
        let shell = window.global::<ShellState>();
        shell.set_content_focus_request(shell.get_content_focus_request() + 1);
    });
    publish(window);
}

fn with_stack_result<T>(update: impl FnOnce(&mut Vec<Crumb>) -> T) -> T {
    update(&mut stack().lock().expect("breadcrumb stack poisoned"))
}

/// Pops the trail for back navigation. Returns the location to restore, or
/// `None` when already at a root (nothing to do).
fn pop() -> Option<Crumb> {
    with_stack_result(|stack| {
        if stack.len() <= 1 {
            return None;
        }
        stack.pop();
        stack.last().cloned()
    })
}

/// Dismisses the topmost open overlay on any tab, topmost-first, before
/// page navigation runs. Returns true when Back is consumed.
/// Overlay open-state stays page-local; this inventory only *reads* it, so
/// a missed close can never wedge navigation. New overlays (launch options,
/// entry editing, …) register an arm here instead of growing tab-specific
/// helpers.
pub(crate) fn dismiss_overlay(window: &MainWindow) -> bool {
    match window.global::<ShellState>().get_page() {
        ShellPage::Store => {
            let store = window.global::<StoreState>();
            if store.get_artifact_sheet_open() {
                store.set_artifact_sheet_open(false);
                return true;
            }
            if !store.get_install_status().is_empty() {
                store.set_install_status(SharedString::default());
                return true;
            }
            false
        }
        _ => false,
    }
}

/// Single entry point for every back action: header buttons, Escape, and the
/// controller back key all funnel through `ShellState.back-requested`.
pub(crate) fn back(window: &MainWindow) {
    let shell = window.global::<ShellState>();
    match shell.get_page() {
        // Details were never pushed: restore whatever lies beneath.
        ShellPage::GameDetails => {
            let fallback = active_tab_root(&shell);
            let target = with_stack_result(|stack| stack.last().cloned()).unwrap_or(fallback);
            unload_game_details(window);
            restore(window, &target);
        }
        ShellPage::Library if window.global::<LibraryState>().get_page() == 1 => {
            let target = pop().unwrap_or(Crumb::Tab(Tab::Library));
            restore(window, &target);
        }
        ShellPage::Store if window.global::<StoreState>().get_page() == 2 => {
            // Inline store details aren't a crumb: step to the games list.
            window.global::<StoreState>().set_page(1);
        }
        ShellPage::Store if window.global::<StoreState>().get_page() == 1 => {
            let target = pop().unwrap_or(Crumb::Tab(Tab::Store));
            restore(window, &target);
        }
        _ => {}
    }
    // Back destroys the focused view (e.g. the details page); hand focus to
    // the revealed content scope so controller input never falls into the
    // void. Deferred a tick: remounted pages only observe the bump after
    // they exist. Mounted pages listen on content-focus-request for this.
    let weak = window.as_weak();
    let _ = weak.upgrade_in_event_loop(move |window| {
        let shell = window.global::<ShellState>();
        shell.set_content_focus_request(shell.get_content_focus_request() + 1);
    });
    publish(window);
}

fn active_tab_root(shell: &ShellState) -> Crumb {
    match shell.get_active_tab() {
        1 => Crumb::Tab(Tab::Library),
        2 => Crumb::Tab(Tab::Store),
        _ => Crumb::Tab(Tab::Home),
    }
}

/// Releases the full game record and decoded artwork when details are hidden.
fn unload_game_details(window: &MainWindow) {
    let game = window.global::<GameState>();
    game.set_selected_game(crate::empty_game_card());
    game.set_details(crate::empty_preview_details());
    game.set_tags(crate::string_model(Vec::new()));
    game.set_details_loading(false);
}

/// Ends active Home work while preserving its bounded visible-cover cache.
pub(crate) fn leave_home(window: &MainWindow) {
    let home = window.global::<HomeState>();
    home.invoke_exited();
    // This callback owns the viewport loader in Rust. A non-Home context
    // suspends background work without rebuilding the visible-cover cache.
    home.invoke_cover_context_changed(-1);
}

/// Tab jumps shared by header navigation (records) and restores (replays).
/// Tabs always land on their root: sub-pages reset so the trail and the
/// visible view can never disagree.
pub(crate) fn goto_tab(window: &MainWindow, index: i32) {
    let shell = window.global::<ShellState>();
    if shell.get_page() == ShellPage::GameDetails {
        unload_game_details(window);
    }
    let home = window.global::<HomeState>();
    let left_home = shell.get_active_tab() == 0 && index != 0;
    if left_home {
        leave_home(window);
    }
    let store = window.global::<StoreState>();
    if shell.get_page() == ShellPage::Store && (index != 2 || store.get_page() != 0) {
        // Store catalogs and decoded previews are deliberately scoped to the
        // selected platform. Release them when returning to the Store root or
        // switching tabs instead of retaining them in the global StoreState.
        store.invoke_platform_exited();
    }
    shell.set_active_tab(index);
    window.global::<LibraryState>().set_page(0);
    store.set_page(0);
    match index {
        0 => {
            shell.set_page(ShellPage::Home);
            home.set_loading(home.get_games().row_count() == 0);
            home.invoke_entered();
        }
        1 => {
            shell.set_page(ShellPage::Library);
            window.global::<LibraryState>().invoke_entered();
        }
        _ => {
            shell.set_page(ShellPage::Store);
            window.global::<StoreState>().invoke_entered();
        }
    }
    if !left_home {
        home.invoke_cover_context_changed(index);
    }
    window.invoke_focus_navigation();
}

fn restore(window: &MainWindow, target: &Crumb) {
    match target {
        Crumb::Tab(tab) => goto_tab(window, tab.index()),
        Crumb::LibraryPlatform { slug, .. } => restore_library_games(window, slug),
        Crumb::StorePlatform { slug, .. } => restore_store_games(window, slug),
    }
}

fn restore_library_games(window: &MainWindow, slug: &str) {
    let shell = window.global::<ShellState>();
    shell.set_active_tab(1);
    shell.set_page(ShellPage::Library);
    let library = window.global::<LibraryState>();
    // Re-resolve the index: the platforms model may have refreshed since.
    let index = (0..library.get_platforms().row_count())
        .find(|&index| {
            library
                .get_platforms()
                .row_data(index)
                .is_some_and(|platform| platform.slug.as_str() == slug)
        })
        .unwrap_or(0) as i32;
    library.set_selected_platform_index(index);
    if let Some(platform) = library.get_platforms().row_data(index.max(0) as usize) {
        library.set_selected_platform(platform.slug);
    }
    library.set_page(1);
    // Reload through the normal query path (also re-pushes nothing: the
    // suppress flag covers the drill hook inside the handler).
    *suppress_push().lock().expect("breadcrumb lock poisoned") = true;
    library.invoke_platform_query(SharedString::from(slug));
    *suppress_push().lock().expect("breadcrumb lock poisoned") = false;
    window.invoke_focus_navigation();
}

fn restore_store_games(window: &MainWindow, slug: &str) {
    let shell = window.global::<ShellState>();
    shell.set_active_tab(2);
    shell.set_page(ShellPage::Store);
    let store = window.global::<StoreState>();
    let index = (0..store.get_platforms().row_count())
        .find(|&index| {
            store
                .get_platforms()
                .row_data(index)
                .is_some_and(|platform| platform.slug.as_str() == slug)
        })
        .unwrap_or(0) as i32;
    store.set_selected_platform_index(index);
    if let Some(platform) = store.get_platforms().row_data(index.max(0) as usize) {
        store.set_selected_platform(platform.slug);
    }
    store.set_page(1);
    *suppress_push().lock().expect("breadcrumb lock poisoned") = true;
    store.invoke_platform_query(SharedString::from(slug));
    *suppress_push().lock().expect("breadcrumb lock poisoned") = false;
    window.invoke_focus_navigation();
}
