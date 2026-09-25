//! Generated settings model, current scalar values, and persistence bindings.

use std::collections::HashMap;

use crate::{
    MainWindow, SettingsEntry, SettingsNavigationItem, SettingsPanel, SettingsPanelItem,
    SettingsSection, SettingsState,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use tracing::warn;

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::from(std::rc::Rc::new(VecModel::from(items)))
}

pub(crate) fn configure(window: &MainWindow) {
    let values = match crate::config::read_scalar_settings() {
        Ok(values) => values,
        Err(error) => {
            warn!(%error, "could not load editable settings values");
            HashMap::new()
        }
    };
    publish(window, values);

    let weak = window.as_weak();
    tokio::spawn(async move {
        let names = tokio::task::spawn_blocking(platform_names).await;
        match names {
            Ok(names) => {
                let _ = weak.upgrade_in_event_loop(move |window| {
                    publish_platform_names(&window, names);
                });
            }
            Err(error) => warn!(%error, "platform settings task failed"),
        }
    });

    let weak = window.as_weak();
    window
        .global::<SettingsState>()
        .on_setting_changed(move |path, control, value, bool_value| {
            let path = path.to_string();
            let value = match control.as_str() {
                "toggle" => crate::config::ScalarSettingValue::Bool(bool_value),
                "text" | "secret" | "path" => {
                    crate::config::ScalarSettingValue::String(value.to_string())
                }
                _ => return,
            };
            let weak = weak.clone();
            tokio::spawn(async move {
                let result = tokio::task::spawn_blocking(move || {
                    crate::config::write_scalar_setting(&path, value)?;
                    crate::config::reload();
                    crate::config::read_scalar_settings()
                })
                .await;
                match result {
                    Ok(Ok(values)) => {
                        let _ = weak.upgrade_in_event_loop(move |window| publish(&window, values));
                    }
                    Ok(Err(error)) => warn!(%error, "could not save setting"),
                    Err(error) => warn!(%error, "settings persistence task failed"),
                }
            });
        });
}

fn platform_names() -> Vec<String> {
    let mut names = crate::config::shared()
        .snapshot()
        .platforms
        .into_keys()
        .collect::<Vec<_>>();
    names.sort_unstable();
    names
}

fn publish_platform_names(window: &MainWindow, names: Vec<String>) {
    window
        .global::<SettingsState>()
        .set_platform_names(model(names.into_iter().map(SharedString::from).collect()));
}

fn publish(window: &MainWindow, values: HashMap<String, crate::config::ScalarSettingValue>) {
    let mut schema = crate::config::settings_schema();
    schema.sort_by(|left, right| {
        left.11
            .cmp(&right.11)
            .then_with(|| left.9.cmp(&right.9))
            .then_with(|| left.8.cmp(&right.8))
            .then_with(|| left.6.cmp(&right.6))
            .then_with(|| left.12.cmp(&right.12))
            .then_with(|| left.0.cmp(&right.0))
    });

    let mut grouped = Vec::<(String, String, Vec<(String, String, Vec<SettingsEntry>)>)>::new();
    for (
        path,
        title,
        description,
        control,
        environment,
        sensitive,
        panel_id,
        panel_title,
        _panel_order,
        section_id,
        section_title,
        _section_order,
        _order,
    ) in schema
    {
        let section_index = grouped
            .iter()
            .position(|(candidate, _, _)| candidate == &section_id)
            .unwrap_or_else(|| {
                grouped.push((section_id, section_title, Vec::new()));
                grouped.len() - 1
            });
        let panels = &mut grouped[section_index].2;
        let panel_index = panels
            .iter()
            .position(|(candidate, _, _)| candidate == &panel_id)
            .unwrap_or_else(|| {
                panels.push((panel_id, panel_title, Vec::new()));
                panels.len() - 1
            });
        let value = values.get(&path);
        let bool_value = matches!(value, Some(crate::config::ScalarSettingValue::Bool(true)));
        let value = match value {
            Some(crate::config::ScalarSettingValue::String(value)) => value.clone(),
            _ => String::new(),
        };
        let environment_overridden =
            environment.is_some_and(|name| std::env::var_os(name).is_some());
        panels[panel_index].2.push(SettingsEntry {
            path: SharedString::from(path),
            title: SharedString::from(title),
            summary: SharedString::from(description),
            control: SharedString::from(control),
            value: SharedString::from(value),
            bool_value,
            sensitive,
            environment_overridden,
        });
    }

    let mut panel_items = Vec::new();
    let mut navigation = Vec::new();
    let mut scroll_y = 0.0;
    for (section_index, (_, section_title, panels)) in grouped.iter().enumerate() {
        navigation.push(SettingsNavigationItem {
            title: SharedString::from(section_title.as_str()),
            is_section: true,
            panel_selection_index: -1,
            scroll_y,
            height: 34.0,
        });
        scroll_y += 34.0;
        for (panel_index, (_, panel_title, _)) in panels.iter().enumerate() {
            let panel_selection_index = panel_items.len() as i32;
            let navigation_index = navigation.len() as i32;
            panel_items.push(SettingsPanelItem {
                section_index: section_index as i32,
                panel_index: panel_index as i32,
                navigation_index,
            });
            navigation.push(SettingsNavigationItem {
                title: SharedString::from(panel_title.as_str()),
                is_section: false,
                panel_selection_index,
                scroll_y,
                height: 58.0,
            });
            scroll_y += 58.0;
        }
    }

    let sections = grouped
        .into_iter()
        .map(|(_, title, panels)| SettingsSection {
            title: SharedString::from(title),
            panels: model(
                panels
                    .into_iter()
                    .map(|(_, title, entries)| SettingsPanel {
                        title: SharedString::from(title),
                        entries: model(entries),
                    })
                    .collect(),
            ),
        })
        .collect::<Vec<_>>();

    let state = window.global::<SettingsState>();
    state.set_navigation_content_height(scroll_y);
    state.set_navigation(model(navigation));
    state.set_panels(model(panel_items));
    state.set_sections(model(sections));
}
