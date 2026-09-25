use std::collections::HashMap;

use marina_config_derive::ConfigSettings;

type SettingsRow = (
    String,
    String,
    String,
    &'static str,
    Option<&'static str>,
    bool,
    String,
    String,
    i32,
    String,
    String,
    i32,
    i32,
);

fn assert_row(actual: &SettingsRow, expected: SettingsRow) {
    assert_eq!(actual.0, expected.0);
    assert_eq!(actual.1, expected.1);
    assert_eq!(actual.2, expected.2);
    assert_eq!(actual.3, expected.3);
    assert_eq!(actual.4, expected.4);
    assert_eq!(actual.5, expected.5);
    assert_eq!(actual.6, expected.6);
    assert_eq!(actual.7, expected.7);
    assert_eq!(actual.8, expected.8);
    assert_eq!(actual.9, expected.9);
    assert_eq!(actual.10, expected.10);
    assert_eq!(actual.11, expected.11);
    assert_eq!(actual.12, expected.12);
}

#[allow(dead_code)]
#[derive(ConfigSettings)]
struct NestedSettings {
    /// Directory containing the library.
    library_root: Option<String>,

    #[setting(skip)]
    skipped: bool,
}

#[allow(dead_code)]
#[derive(ConfigSettings)]
struct Settings {
    /// Enable automatic scanning.
    #[template(env = "MARINA_SCAN")]
    #[setting(
        title = "Scan on startup",
        control = "toggle",
        sensitive,
        panel = "library",
        panel_title = "Library",
        panel_order = "1"
    )]
    scan: bool,

    /// Runtime configuration.
    #[setting(
        section = "library",
        section_title = "Library",
        section_order = "2",
        panel = "nested",
        panel_title = "Nested settings",
        panel_order = "5"
    )]
    nested: NestedSettings,

    #[setting(section = "library", panel = "nested")]
    library_enabled: bool,

    #[setting(
        control = "custom",
        section = "advanced",
        section_title = "Advanced settings",
        section_order = "4",
        order = "3"
    )]
    custom_value: String,

    /// User-defined platform settings.
    platforms: HashMap<String, String>,

    #[template(skip)]
    omitted: Option<bool>,
}

#[test]
fn settings_are_flattened_with_metadata() {
    let settings = Settings {
        scan: true,
        nested: NestedSettings {
            library_root: None,
            skipped: false,
        },
        library_enabled: true,
        custom_value: String::new(),
        platforms: HashMap::new(),
        omitted: None,
    };

    let rows = settings.__marina_settings("");
    assert_eq!(rows.len(), 5);
    assert_row(
        &rows[0],
        (
            "scan".to_owned(),
            "Scan on startup".to_owned(),
            "Enable automatic scanning.".to_owned(),
            "toggle",
            Some("MARINA_SCAN"),
            true,
            "library".to_owned(),
            "Library".to_owned(),
            1,
            "".to_owned(),
            "".to_owned(),
            0,
            0,
        ),
    );
    assert_row(
        &rows[1],
        (
            "nested.library_root".to_owned(),
            "Library root".to_owned(),
            "Directory containing the library.".to_owned(),
            "text",
            None,
            false,
            "nested".to_owned(),
            "Nested settings".to_owned(),
            5,
            "library".to_owned(),
            "Library".to_owned(),
            2,
            0,
        ),
    );
    assert_row(
        &rows[2],
        (
            "library_enabled".to_owned(),
            "Library enabled".to_owned(),
            "".to_owned(),
            "toggle",
            None,
            false,
            "nested".to_owned(),
            "Nested settings".to_owned(),
            5,
            "library".to_owned(),
            "Library".to_owned(),
            2,
            0,
        ),
    );
    assert_row(
        &rows[3],
        (
            "custom_value".to_owned(),
            "Custom value".to_owned(),
            "".to_owned(),
            "custom",
            None,
            false,
            "".to_owned(),
            "".to_owned(),
            0,
            "advanced".to_owned(),
            "Advanced settings".to_owned(),
            4,
            3,
        ),
    );
    assert_row(
        &rows[4],
        (
            "platforms".to_owned(),
            "Platforms".to_owned(),
            "User-defined platform settings.".to_owned(),
            "map",
            None,
            false,
            "".to_owned(),
            "".to_owned(),
            0,
            "".to_owned(),
            "".to_owned(),
            0,
            0,
        ),
    );
}
