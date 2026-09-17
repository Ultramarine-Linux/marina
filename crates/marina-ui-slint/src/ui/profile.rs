//! Asynchronous shell profile hydration.

use slint::{ComponentHandle, Image, SharedString};

use crate::{MainWindow, ShellState, image};

pub(crate) fn profile_display_name(username: &str, passwd: &str) -> Option<String> {
    passwd.lines().find_map(|line| {
        let fields: Vec<_> = line.split(':').collect();
        if fields.first().copied() != Some(username) {
            return None;
        }
        fields
            .get(4)
            .and_then(|gecos| gecos.split(',').next())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
    })
}

pub(crate) fn profile_initials(name: &str) -> String {
    let initials: String = name
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect();
    if initials.is_empty() {
        name.chars()
            .next()
            .unwrap_or('U')
            .to_uppercase()
            .to_string()
    } else {
        initials.to_uppercase()
    }
}

pub(crate) fn initialize(window: &MainWindow, username: String) {
    window
        .global::<ShellState>()
        .set_profile_name(SharedString::from(username.clone()));
    window
        .global::<ShellState>()
        .set_profile_username(SharedString::from(username.clone()));
    window
        .global::<ShellState>()
        .set_profile_initials(SharedString::from(profile_initials(&username)));
    window
        .global::<ShellState>()
        .set_profile_image(Image::default());

    let profile_window = window.as_weak();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        let profile_window = profile_window.clone();
        let profile_username = username.clone();
        tokio::spawn(async move {
            let passwd = tokio::fs::read_to_string("/etc/passwd").await.ok();
            let display_name = passwd
                .as_deref()
                .and_then(|passwd| profile_display_name(&profile_username, passwd))
                .unwrap_or_else(|| profile_username.clone());
            let initials = profile_initials(&display_name);
            let icon_path = format!("/var/lib/AccountsService/icons/{profile_username}");
            let icon = image::load_path(icon_path, "profile-icon").await;
            let _ = profile_window.upgrade_in_event_loop(move |window| {
                window
                    .global::<ShellState>()
                    .set_profile_name(SharedString::from(display_name));
                window
                    .global::<ShellState>()
                    .set_profile_initials(SharedString::from(initials));
                if let Some(icon) = icon {
                    let (image, _) = image::into_slint_image(icon);
                    window.global::<ShellState>().set_profile_image(image);
                }
            });
        });
    });
}
