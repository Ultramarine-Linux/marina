use marina_input::{InputAction, InputEvent, InputEventKind};
use marina_shell::{ManagedWindow, SwayWindowManager, WindowManager};
use tracing::info;

#[derive(Clone, Debug)]
pub(super) struct Presentation {
    pub(super) window_mode: bool,
    pub(super) active_title: String,
    pub(super) menu_selected: i32,
    pub(super) title: String,
    pub(super) app_id: String,
    pub(super) location: String,
    pub(super) position: String,
    pub(super) window_titles: Vec<String>,
    pub(super) selected_index: i32,
    pub(super) status: String,
    pub(super) quick_mode: bool,
    pub(super) profile_mode: bool,
    pub(super) quick_selected: i32,
    pub(super) brightness: i32,
    pub(super) volume: i32,
    pub(super) profile_names: Vec<String>,
    pub(super) active_profile: String,
    pub(super) active_profile_index: i32,
    pub(super) profile_selected: i32,
    pub(super) clock_text: String,
    pub(super) battery_available: bool,
    pub(super) battery_percentage: f64,
    pub(super) battery_charging: bool,
    pub(super) network_available: bool,
    pub(super) network_connected: bool,
    pub(super) network_signal: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OverlayMode {
    GameMenu,
    WindowList,
    QuickSettings,
    ProfileList,
}

const MENU_ITEM_COUNT: usize = 3;
const QUICK_ITEM_COUNT: usize = 3;

pub(super) enum OverlayMessage {
    Show(Presentation),
    Hide,
    Presentation(Presentation),
    Battery(marina_power::BatteryStatus),
    Network(marina_networking::NetworkStatus),
    Clock(String),
    SwitchProfile(String),
    SetBrightness(u8),
    SetVolume(u8),
}

pub(super) struct ControllerState {
    manager: SwayWindowManager,
    windows: Vec<ManagedWindow>,
    active: Option<usize>,
    selected: usize,
    menu_selected: usize,
    pub(super) mode: OverlayMode,
    profiles: Vec<marina_power::TunedProfile>,
    pub(super) active_profile: String,
    profile_selected: usize,
    pub(super) profile_switch_pending: bool,
    quick_selected: usize,
    brightness: u8,
    volume: u8,
    pub(super) clock_text: String,
    pub(super) battery: marina_power::BatteryStatus,
    pub(super) network: marina_networking::NetworkStatus,
}

impl ControllerState {
    pub(super) fn new(manager: SwayWindowManager) -> Self {
        Self {
            manager,
            windows: Vec::new(),
            active: None,
            selected: 0,
            menu_selected: 0,
            mode: OverlayMode::GameMenu,
            profiles: Vec::new(),
            active_profile: String::new(),
            profile_selected: 0,
            profile_switch_pending: false,
            quick_selected: 0,
            brightness: 0,
            volume: 0,
            clock_text: String::new(),
            battery: marina_power::BatteryStatus {
                available: false,
                percentage: 0.0,
                charging: false,
            },
            network: marina_networking::NetworkStatus::unavailable(),
        }
    }

    pub(super) fn refresh(&mut self) -> Result<Presentation, marina_shell::WindowManagerError> {
        self.windows = self.manager.windows()?;
        self.active = self.windows.iter().position(|window| window.focused);
        self.selected = self.active.unwrap_or(0);
        self.menu_selected = 0;
        self.mode = OverlayMode::GameMenu;
        Ok(self.presentation(""))
    }

    pub(super) fn begin_quick_settings(&mut self) -> Presentation {
        self.mode = OverlayMode::QuickSettings;
        self.quick_selected = 0;
        self.presentation("Loading system controls…")
    }

    pub(super) fn apply_quick_snapshot(
        &mut self,
        profiles: Result<Vec<marina_power::TunedProfile>, String>,
        active: Result<String, String>,
        brightness: u8,
        volume: u8,
        battery: marina_power::BatteryStatus,
        network: marina_networking::NetworkStatus,
    ) -> Presentation {
        let status = match (profiles, active) {
            (Ok(profiles), Ok(active)) => {
                self.profiles = profiles;
                self.active_profile = active;
                self.profile_selected = self
                    .profiles
                    .iter()
                    .position(|profile| profile.name == self.active_profile)
                    .unwrap_or(0);
                String::new()
            }
            (Err(error), _) | (_, Err(error)) => format!("TuneD unavailable: {error}"),
        };
        self.brightness = brightness;
        self.volume = volume;
        self.clock_text = marina_ui_slint::clock_format::current_time_string(
            marina_ui_slint::config::shared()
                .map(|config| config.snapshot().clock_twelve_hour)
                .unwrap_or(false),
        );
        self.battery = battery;
        self.network = network;
        info!(
            profile = %self.active_profile,
            brightness = self.brightness,
            volume = self.volume,
            battery = self.battery.percentage,
            network_connected = self.network.connected,
            "quick settings loaded"
        );
        self.presentation(status)
    }

    pub(super) fn presentation(&self, status: impl Into<String>) -> Presentation {
        let selected = self.windows.get(self.selected);
        let active_title = self
            .active
            .and_then(|index| self.windows.get(index))
            .map(|window| window.title.clone())
            .unwrap_or_else(|| "No active game".to_owned());
        let location = selected
            .map(|window| match (&window.workspace, &window.output) {
                (Some(workspace), Some(output)) => format!("Workspace {workspace} · {output}"),
                (Some(workspace), None) => format!("Workspace {workspace}"),
                (None, Some(output)) => output.clone(),
                (None, None) => String::new(),
            })
            .unwrap_or_default();
        Presentation {
            window_mode: self.mode == OverlayMode::WindowList,
            active_title,
            menu_selected: self.menu_selected as i32,
            title: selected
                .map(|window| window.title.clone())
                .unwrap_or_else(|| "No application windows".to_owned()),
            app_id: selected
                .and_then(|window| window.app_id.clone())
                .unwrap_or_default(),
            location,
            position: if self.windows.is_empty() {
                "0 of 0".to_owned()
            } else {
                format!("{} of {}", self.selected + 1, self.windows.len())
            },
            window_titles: self
                .windows
                .iter()
                .map(|candidate| candidate.title.clone())
                .collect(),
            selected_index: self.selected as i32,
            status: status.into(),
            quick_mode: matches!(
                self.mode,
                OverlayMode::QuickSettings | OverlayMode::ProfileList
            ),
            profile_mode: self.mode == OverlayMode::ProfileList,
            quick_selected: self.quick_selected as i32,
            brightness: i32::from(self.brightness),
            volume: i32::from(self.volume),
            profile_names: self
                .profiles
                .iter()
                .map(|profile| profile.name.clone())
                .collect(),
            active_profile: self.active_profile.clone(),
            active_profile_index: self
                .profiles
                .iter()
                .position(|profile| profile.name == self.active_profile)
                .map(|index| index as i32)
                .unwrap_or(-1),
            profile_selected: self.profile_selected as i32,
            clock_text: self.clock_text.clone(),
            battery_available: self.battery.available,
            battery_percentage: self.battery.percentage,
            battery_charging: self.battery.charging,
            network_available: self.network.available,
            network_connected: self.network.connected,
            network_signal: self
                .network
                .wifi_signal_strength
                .map(i32::from)
                .unwrap_or(-1),
        }
    }

    pub(super) fn handle(&mut self, event: InputEvent) -> Option<OverlayMessage> {
        if !matches!(
            event.kind,
            InputEventKind::Pressed | InputEventKind::Repeated
        ) {
            return None;
        }
        match self.mode {
            OverlayMode::GameMenu => self.handle_game_menu(event.action),
            OverlayMode::WindowList => self.handle_window_list(event.action),
            OverlayMode::QuickSettings => self.handle_quick_settings(event.action),
            OverlayMode::ProfileList => self.handle_profile_list(event.action),
        }
    }

    fn handle_game_menu(&mut self, action: InputAction) -> Option<OverlayMessage> {
        match action {
            InputAction::Up | InputAction::Left | InputAction::PreviousTab => {
                self.menu_selected = self
                    .menu_selected
                    .checked_sub(1)
                    .unwrap_or(MENU_ITEM_COUNT - 1);
                Some(OverlayMessage::Presentation(self.presentation("")))
            }
            InputAction::Down | InputAction::Right | InputAction::NextTab => {
                self.menu_selected = (self.menu_selected + 1) % MENU_ITEM_COUNT;
                Some(OverlayMessage::Presentation(self.presentation("")))
            }
            InputAction::Accept => match self.menu_selected {
                0 => Some(OverlayMessage::Hide),
                1 => {
                    self.mode = OverlayMode::WindowList;
                    Some(OverlayMessage::Presentation(self.presentation("")))
                }
                _ => {
                    let Some(window) = self.active.and_then(|index| self.windows.get(index)) else {
                        return Some(OverlayMessage::Presentation(
                            self.presentation("No active game window to exit"),
                        ));
                    };
                    Some(match self.manager.close(window.id) {
                        Ok(()) => OverlayMessage::Hide,
                        Err(error) => {
                            OverlayMessage::Presentation(self.presentation(error.to_string()))
                        }
                    })
                }
            },
            InputAction::Back | InputAction::Menu => Some(OverlayMessage::Hide),
            _ => None,
        }
    }

    fn handle_quick_settings(&mut self, action: InputAction) -> Option<OverlayMessage> {
        match action {
            InputAction::Up | InputAction::PreviousTab => {
                self.quick_selected = self
                    .quick_selected
                    .checked_sub(1)
                    .unwrap_or(QUICK_ITEM_COUNT - 1);
                Some(OverlayMessage::Presentation(self.presentation("")))
            }
            InputAction::Down | InputAction::NextTab => {
                self.quick_selected = (self.quick_selected + 1) % QUICK_ITEM_COUNT;
                Some(OverlayMessage::Presentation(self.presentation("")))
            }
            InputAction::Left | InputAction::Right if self.quick_selected == 1 => {
                let delta = if action == InputAction::Right { 5 } else { -5 };
                self.brightness = (i16::from(self.brightness) + delta).clamp(1, 100) as u8;
                Some(OverlayMessage::SetBrightness(self.brightness))
            }
            InputAction::Left | InputAction::Right if self.quick_selected == 2 => {
                let delta = if action == InputAction::Right { 5 } else { -5 };
                self.volume = (i16::from(self.volume) + delta).clamp(0, 100) as u8;
                Some(OverlayMessage::SetVolume(self.volume))
            }
            InputAction::Accept if self.quick_selected == 0 => {
                self.mode = OverlayMode::ProfileList;
                Some(OverlayMessage::Presentation(self.presentation("")))
            }
            InputAction::Back | InputAction::Menu => Some(OverlayMessage::Hide),
            _ => None,
        }
    }

    fn handle_profile_list(&mut self, action: InputAction) -> Option<OverlayMessage> {
        match action {
            InputAction::Up | InputAction::Left | InputAction::PreviousTab
                if !self.profiles.is_empty() =>
            {
                self.profile_selected = self
                    .profile_selected
                    .checked_sub(1)
                    .unwrap_or(self.profiles.len() - 1);
                Some(OverlayMessage::Presentation(self.presentation("")))
            }
            InputAction::Down | InputAction::Right | InputAction::NextTab
                if !self.profiles.is_empty() =>
            {
                self.profile_selected = (self.profile_selected + 1) % self.profiles.len();
                Some(OverlayMessage::Presentation(self.presentation("")))
            }
            InputAction::Accept if !self.profile_switch_pending => {
                let Some(profile) = self.profiles.get(self.profile_selected) else {
                    return Some(OverlayMessage::Presentation(
                        self.presentation("No TuneD profiles available"),
                    ));
                };
                self.profile_switch_pending = true;
                Some(OverlayMessage::SwitchProfile(profile.name.clone()))
            }
            InputAction::Back if !self.profile_switch_pending => {
                self.mode = OverlayMode::QuickSettings;
                Some(OverlayMessage::Presentation(self.presentation("")))
            }
            InputAction::Menu => Some(OverlayMessage::Hide),
            _ => None,
        }
    }

    fn handle_window_list(&mut self, action: InputAction) -> Option<OverlayMessage> {
        match action {
            InputAction::Left | InputAction::Up | InputAction::PreviousTab
                if !self.windows.is_empty() =>
            {
                self.selected = self
                    .selected
                    .checked_sub(1)
                    .unwrap_or(self.windows.len() - 1);
                Some(OverlayMessage::Presentation(self.presentation("")))
            }
            InputAction::Right | InputAction::Down | InputAction::NextTab
                if !self.windows.is_empty() =>
            {
                self.selected = (self.selected + 1) % self.windows.len();
                Some(OverlayMessage::Presentation(self.presentation("")))
            }
            InputAction::Accept => {
                let Some(window) = self.windows.get(self.selected) else {
                    return Some(OverlayMessage::Presentation(
                        self.presentation("No window to focus"),
                    ));
                };
                Some(match self.manager.focus(window.id) {
                    Ok(()) => OverlayMessage::Hide,
                    Err(error) => {
                        OverlayMessage::Presentation(self.presentation(error.to_string()))
                    }
                })
            }
            InputAction::Back => {
                self.mode = OverlayMode::GameMenu;
                Some(OverlayMessage::Presentation(self.presentation("")))
            }
            InputAction::Menu => Some(OverlayMessage::Hide),
            _ => None,
        }
    }
}
