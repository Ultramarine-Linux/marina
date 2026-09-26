use super::*;

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// Serializes the tests that mutate process environment; Rust runs
    /// tests on threads sharing one environment.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn config_handle_replaces_snapshots() {
        let handle = ConfigHandle(Arc::new(RwLock::new(
            Config::from_figment(Figment::new()).unwrap(),
        )));
        assert!(!handle.snapshot().clock_twelve_hour);

        handle.replace(
            Config::from_figment(Figment::new().merge(("general.time_date.twelve_hour", true)))
                .unwrap(),
        );

        assert!(handle.snapshot().clock_twelve_hour);
    }

    #[test]
    fn parses_library_sections() {
        let config: FileConfig = toml::from_str(
            r#"
[library.romm]
enable = true
url = "https://romm.example.com"
token = "secret"
import_on_startup = true

[library.local]
scan_on_startup = false
"#,
        )
        .unwrap();
        assert_eq!(
            config.library.romm.url.as_deref(),
            Some("https://romm.example.com")
        );
        assert_eq!(config.library.romm.token.as_deref(), Some("secret"));
        assert_eq!(config.library.romm.enable, Some(true));
        assert_eq!(config.library.local.scan_on_startup, Some(false));
    }

    #[test]
    fn portmaster_store_values_are_optional_and_blank_paths_use_defaults() {
        let default_config = Config::from_figment(
            Figment::new().merge(Toml::string("[library.portmaster]\nenable = true\n")),
        )
        .unwrap();
        assert_eq!(
            default_config
                .portmaster_store
                .as_ref()
                .and_then(|config| config.release.as_deref()),
            None
        );

        let blank_override = Config::from_figment(Figment::new().merge(Toml::string(
            "[library.portmaster]\nenable = true\nrelease = \"   \"\n",
        )))
        .unwrap();
        assert_eq!(
            blank_override
                .portmaster_store
                .as_ref()
                .and_then(|config| config.release.as_deref()),
            None
        );

        let blank_path = Config::from_figment(Figment::new().merge(Toml::string(
            "[library.portmaster]\nenable = true\nports_dir = \"\"\n",
        )))
        .unwrap();
        assert_eq!(
            blank_path
                .portmaster_store
                .as_ref()
                .map(|config| config.ports_dir.as_path()),
            Some(Path::new(DEFAULT_PORTS_DIR))
        );
    }

    #[test]
    fn parses_clock_section() {
        let config: FileConfig = toml::from_str(
            r#"
[general.time_date]
twelve_hour = true
"#,
        )
        .unwrap();
        assert_eq!(config.general.time_date.twelve_hour, Some(true));
        // The shorthand `12hr` key is accepted as an alias.
        let config: FileConfig = toml::from_str(
            r#"
[general.time_date]
12hr = true
"#,
        )
        .unwrap();
        assert_eq!(config.general.time_date.twelve_hour, Some(true));

        let figment = Figment::new().merge(Toml::string("[general.time_date]\n12hr = true\n"));
        assert!(Config::from_figment(figment).unwrap().clock_twelve_hour);
        assert!(
            !Config::from_figment(Figment::new())
                .unwrap()
                .clock_twelve_hour
        );
    }

    #[test]
    fn figment_merges_toml_base_with_key_path_overrides() {
        let figment = Figment::new()
            .merge(Toml::string(
                "[library.romm]\nenable = true\nurl = \"https://file.example.com\"\n",
            ))
            .merge((
                "library.romm.url",
                "https://override.example.com".to_string(),
            ));
        let config = Config::from_figment(figment).unwrap();
        assert_eq!(
            config.romm_url.as_deref(),
            Some("https://override.example.com")
        );
    }

    #[test]
    fn obsolete_store_section_is_rejected_instead_of_silently_disabling_backends() {
        let error = Config::from_figment(
            Figment::new().merge(Toml::string("[store.romm]\nenable = true\n")),
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown field `store`"));
    }

    #[test]
    fn config_file_layers_are_ordered_and_primary_wins() {
        let root = std::env::temp_dir().join(format!(
            "marina-config-layers-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let shared_dropins = root.join("usr/share/marina/config.toml.d");
        let shared_config = root.join("usr/share/marina/config.toml");
        let etc_dropins = root.join("etc/marina/config.toml.d");
        let etc_config = root.join("etc/marina/config.toml");
        let user_dropins = root.join("config/marina/config.toml.d");
        let primary = root.join("config/marina/config.toml");

        for directory in [&shared_dropins, &etc_dropins, &user_dropins] {
            std::fs::create_dir_all(directory).unwrap();
        }
        std::fs::create_dir_all(primary.parent().unwrap()).unwrap();
        std::fs::write(
            shared_dropins.join("20-later.toml"),
            "[runtime.retroarch]\nbinary = \"/shared-20\"\n",
        )
        .unwrap();
        std::fs::write(
            shared_dropins.join("10-earlier.toml"),
            "[runtime.retroarch]\nbinary = \"/shared-10\"\ncores_dir = [\"/shared/cores\"]\n",
        )
        .unwrap();
        std::fs::write(shared_dropins.join("README"), "not TOML").unwrap();
        std::fs::write(
            &shared_config,
            "[runtime.retroarch]\nbinary = \"/shared-main\"\n",
        )
        .unwrap();
        std::fs::write(
            etc_dropins.join("10-admin.toml"),
            "[runtime.retroarch]\nbinary = \"/etc-dropin\"\n",
        )
        .unwrap();
        std::fs::write(&etc_config, "[runtime.retroarch]\nbinary = \"/etc-main\"\n").unwrap();
        std::fs::write(
            user_dropins.join("10-user.toml"),
            "[runtime.retroarch]\nbinary = \"/user-dropin\"\n",
        )
        .unwrap();
        std::fs::write(&primary, "[runtime.retroarch]\nbinary = \"/primary\"\n").unwrap();

        let sources = config_sources_from(
            &shared_dropins,
            &shared_config,
            &etc_dropins,
            &etc_config,
            Some(&user_dropins),
            Some(&primary),
        );
        assert_eq!(
            sources,
            vec![
                shared_dropins.join("10-earlier.toml"),
                shared_dropins.join("20-later.toml"),
                shared_config.clone(),
                etc_dropins.join("10-admin.toml"),
                etc_config.clone(),
                user_dropins.join("10-user.toml"),
                primary.clone(),
            ]
        );

        let config = Config::from_figment(merge_config_files(Figment::new(), sources)).unwrap();
        assert_eq!(config.retroarch.binary, PathBuf::from("/primary"));
        assert_eq!(
            config.retroarch.cores_dir,
            vec![PathBuf::from("/shared/cores")]
        );

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn toml_edit_upsert_preserves_comments() {
        let updated = upsert_toml_value(
            "# my romm server\n[library.romm]\nenable = false\n",
            &["library", "romm"],
            "url",
            toml_edit::value("https://romm.example.com"),
        )
        .unwrap();
        assert!(updated.contains("# my romm server"));
        assert!(updated.contains("enable = false"));
        assert!(updated.contains(r#"url = "https://romm.example.com""#));
    }

    #[test]
    fn generated_template_parses_and_covers_every_key() {
        let template = default_config_template();
        let value: toml::Value = toml::from_str(&template).expect("template must parse as TOML");
        for table in ["general", "library", "runtime"] {
            assert!(
                value.get(table).is_some(),
                "template is missing [{table}]:\n{template}"
            );
        }
        assert!(value["general"].get("time_date").is_some());
        assert!(value["library"].get("local").is_some());
        assert!(value["library"].get("romm").is_some());
        assert!(value["runtime"]["retroarch"].get("platforms").is_some());
        // Non-optional leaves render their real defaults, active.
        assert_eq!(
            value["runtime"]["retroarch"]
                .get("binary")
                .and_then(|v| v.as_str()),
            Some("retroarch")
        );
        assert_eq!(
            value["runtime"]["retroarch"]
                .get("cores_dir")
                .and_then(|v| v.as_array()),
            Some(&vec![
                toml::Value::String("/var/games/retroarch/cores".to_owned()),
                toml::Value::String("/usr/lib64/libretro/".to_owned()),
            ])
        );
        // Optional leaves render as commented placeholders with docs + env.
        for key in [
            "enable =",
            "url =",
            "token =",
            "import_on_startup =",
            "root =",
            "storage_uri =",
            "store_cache_dir =",
            "scan_on_startup =",
            "twelve_hour =",
            "backend =",
            "core =",
        ] {
            assert!(
                template.contains(key),
                "template is missing key `{key}`:\n{template}"
            );
        }
        for marker in [
            "MARINA_ENABLE_ROMM",
            "ROMM_URL",
            "ROMM_TOKEN",
            "MARINA_IMPORT_ROMM_ON_STARTUP",
            "MARINA_LIBRARY_ROOT",
            "MARINA_STORAGE_URI",
            "MARINA_STORE_CACHE_DIR",
            "MARINA_SCAN_ON_STARTUP",
            "MARINA_CLOCK_12HR",
            "MARINA_RETROARCH_BINARY",
        ] {
            assert!(
                template.contains(marker),
                "template is missing env `{marker}`:\n{template}"
            );
        }
        // Doc comments made it into the file; the skipped alias did not.
        assert!(template.contains("libretro cores"));
        assert!(
            !template.contains("platform = "),
            "deprecated `platform` alias must stay out of the template"
        );
    }

    #[test]
    fn ensure_writes_template_once_and_never_overwrites() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "marina-config-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("nested").join("config.toml");
        let previous = env::var_os("MARINA_CONFIG");
        // SAFETY: this is the only test that touches MARINA_CONFIG, and no
        // other test reads it, so no other thread can observe the mutation.
        unsafe {
            env::set_var("MARINA_CONFIG", &path);
        }

        let written = ensure_config_file().expect("ensure must create a missing file");
        assert_eq!(written, path);
        assert_eq!(
            std::fs::read_to_string(&path).expect("written file must be readable"),
            default_config_template()
        );
        // A second call is a no-op so user edits survive restarts.
        std::fs::write(&path, "# user edits\n").unwrap();
        let kept = ensure_config_file().expect("ensure must keep an existing file");
        assert_eq!(kept, path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# user edits\n");

        std::fs::remove_dir_all(&dir).ok();
        // SAFETY: same as above; restores the pre-test environment.
        unsafe {
            match previous {
                Some(value) => env::set_var("MARINA_CONFIG", value),
                None => env::remove_var("MARINA_CONFIG"),
            }
        }
    }

    #[test]
    fn env_vars_merge_over_file_through_generated_bindings() {
        let _guard = ENV_LOCK.lock().unwrap();
        // The generated bindings must address the documented key paths.
        let bindings = FileConfig::default().__marina_env_bindings("");
        for (path, var, is_bool) in [
            ("library.romm.enable", "MARINA_ENABLE_ROMM", true),
            ("library.romm.url", "ROMM_URL", false),
            (
                "library.local.scan_on_startup",
                "MARINA_SCAN_ON_STARTUP",
                true,
            ),
            ("general.time_date.twelve_hour", "MARINA_CLOCK_12HR", true),
            ("library.local.root", "MARINA_LIBRARY_ROOT", false),
            ("runtime.retroarch.binary", "MARINA_RETROARCH_BINARY", false),
        ] {
            let expected = (path.to_owned(), var, is_bool);
            assert!(
                bindings.contains(&expected),
                "missing env binding {expected:?} in {bindings:?}"
            );
        }

        const TOUCHED: &[&str] = &[
            "MARINA_CONFIG",
            "MARINA_ENABLE_ROMM",
            "ROMM_URL",
            "ROMM_TOKEN",
            "MARINA_IMPORT_ROMM_ON_STARTUP",
            "MARINA_SCAN_ON_STARTUP",
            "MARINA_CLOCK_12HR",
            "MARINA_STORAGE_URI",
            "MARINA_STORE_CACHE_DIR",
            "MARINA_LIBRARY_ROOT",
            "MARINA_RETROARCH_BINARY",
        ];
        let stashed: Vec<(String, Option<std::ffi::OsString>)> = TOUCHED
            .iter()
            .map(|var| ((*var).to_owned(), env::var_os(var)))
            .collect();
        // SAFETY: this is the only test that touches these variables, and
        // no other test reads them, so no other thread can observe this.
        unsafe {
            for var in TOUCHED {
                env::remove_var(var);
            }
        }
        let dir = std::env::temp_dir().join(format!(
            "marina-config-env-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("config.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &path,
            r#"
[library.romm]
enable = true
url = "https://file.example.com"
token = "file-token"
import_on_startup = false

[library.local]
scan_on_startup = false

[runtime.retroarch]
binary = "/file/retroarch"
cores_dir = ["/file/cores"]
"#,
        )
        .unwrap();
        unsafe {
            env::set_var("MARINA_CONFIG", &path);
            env::set_var("ROMM_URL", "https://env.example.com");
            env::set_var("MARINA_IMPORT_ROMM_ON_STARTUP", "true");
            env::set_var("MARINA_SCAN_ON_STARTUP", "yes");
            env::set_var("MARINA_LIBRARY_ROOT", "/env/library");
            env::set_var("MARINA_RETROARCH_BINARY", "/env/retroarch");
        }

        let config = Config::from_env().unwrap();
        assert_eq!(config.romm_url.as_deref(), Some("https://env.example.com"));
        assert!(config.import_romm_on_startup);
        assert!(config.scan_on_startup);
        assert_eq!(config.library_root, Some(PathBuf::from("/env/library")));
        assert_eq!(config.retroarch.binary, PathBuf::from("/env/retroarch"));
        // Nothing set in the environment: the file value survives.
        assert_eq!(
            config.retroarch.cores_dir,
            vec![PathBuf::from("/file/cores")]
        );
        assert_eq!(config.romm_token.as_deref(), Some("file-token"));

        std::fs::remove_dir_all(&dir).ok();
        unsafe {
            for (var, value) in stashed {
                match value {
                    Some(value) => env::set_var(var, value),
                    None => env::remove_var(var),
                }
            }
        }
    }

    #[test]
    fn default_database_path_uses_xdg_state_dir() {
        let _guard = ENV_LOCK.lock().unwrap();
        let state = std::env::temp_dir().join(format!(
            "marina-state-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let previous_state = env::var_os("XDG_STATE_HOME");
        let previous_cache_uri = env::var_os("MARINA_STORE_CACHE_URI");
        // SAFETY: only this test touches these variables, guarded by ENV_LOCK.
        unsafe {
            env::set_var("XDG_STATE_HOME", &state);
            env::remove_var("MARINA_STORE_CACHE_URI");
        }

        let config = Config::from_figment(Figment::new()).unwrap();
        assert_eq!(
            config.storage_uri,
            format!("sqlite://{}/marina/library.db", state.display())
        );
        assert_eq!(
            config.store_cache_dir,
            state.join("marina").join("store-cache")
        );

        unsafe {
            match previous_state {
                Some(value) => env::set_var("XDG_STATE_HOME", value),
                None => env::remove_var("XDG_STATE_HOME"),
            }
            match previous_cache_uri {
                Some(value) => env::set_var("MARINA_STORE_CACHE_URI", value),
                None => env::remove_var("MARINA_STORE_CACHE_URI"),
            }
        }
    }

    #[test]
    fn parses_retroarch_and_platform_sections() {
        let config: FileConfig = toml::from_str(
            r#"
[runtime.retroarch]
binary = "/usr/bin/retroarch"
cores_dir = ["/var/games/retroarch/cores"]
extra_args = ["-f"]

[runtime.retroarch.platforms."gba"]
backend = "retroarch"
[runtime.retroarch.platforms."gba".retroarch]
core = "mgba_libretro.so"

[runtime.retroarch.platforms."snes"]
backend = "native"
"#,
        )
        .unwrap();
        assert_eq!(
            config.runtime.retroarch.binary,
            PathBuf::from("/usr/bin/retroarch")
        );
        assert_eq!(
            config.runtime.retroarch.cores_dir,
            vec![PathBuf::from("/var/games/retroarch/cores")]
        );
        assert_eq!(config.runtime.retroarch.extra_args, vec!["-f"]);
        let gba = &config.runtime.retroarch.platforms["gba"];
        assert_eq!(
            gba.backend_kind("gba"),
            marina_runtime::PlatformBackendKind::RetroArch
        );
        assert_eq!(
            gba.retroarch.core.as_deref(),
            Some(std::path::Path::new("mgba_libretro.so"))
        );
        assert_eq!(
            config.runtime.retroarch.platforms["snes"].backend_kind("snes"),
            marina_runtime::PlatformBackendKind::Native
        );
    }

    #[test]
    fn platform_tables_accept_absolute_core_paths() {
        let figment = Figment::new().merge(Toml::string(
            "[runtime.retroarch.platforms.\"gba\"]\nbackend = \"retroarch\"\n[runtime.retroarch.platforms.\"gba\".retroarch]\ncore = \"/opt/cores/mgba_libretro.so\"\n",
        ));
        let config = Config::from_figment(figment).unwrap();
        assert_eq!(
            config.platforms["gba"].retroarch.core.as_deref(),
            Some(std::path::Path::new("/opt/cores/mgba_libretro.so"))
        );
    }

    #[test]
    fn runtime_config_reload_observes_core_file_edits() {
        let _guard = ENV_LOCK.lock().unwrap();
        const TOUCHED: &[&str] = &["MARINA_CONFIG", "MARINA_RETROARCH_BINARY"];
        let stashed = TOUCHED
            .iter()
            .map(|var| ((*var).to_owned(), env::var_os(var)))
            .collect::<Vec<_>>();
        let dir = std::env::temp_dir().join(format!(
            "marina-config-reload-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("config.toml");
        std::fs::create_dir_all(&dir).unwrap();
        unsafe {
            for var in TOUCHED {
                env::remove_var(var);
            }
            env::set_var("MARINA_CONFIG", &path);
        }

        std::fs::write(
            &path,
            "[runtime.retroarch.platforms.\"snes\".retroarch]\ncore = \"snes9x_libretro.so\"\n",
        )
        .unwrap();
        let initial = Config::from_env().unwrap();
        assert_eq!(
            initial.platforms["snes"].retroarch.core.as_deref(),
            Some(std::path::Path::new("snes9x_libretro.so"))
        );

        std::fs::write(
            &path,
            "[runtime.retroarch.platforms.\"snes\".retroarch]\ncore = \"bsnes_libretro.so\"\n",
        )
        .unwrap();
        let reloaded = Config::from_env().unwrap();
        assert_eq!(
            reloaded.platforms["snes"].retroarch.core.as_deref(),
            Some(std::path::Path::new("bsnes_libretro.so"))
        );

        std::fs::remove_dir_all(&dir).ok();
        unsafe {
            for (var, value) in stashed {
                match value {
                    Some(value) => env::set_var(var, value),
                    None => env::remove_var(var),
                }
            }
        }
    }

    #[test]
    fn legacy_single_retroarch_core_directory_is_accepted() {
        let config = Config::from_figment(Figment::new().merge(Toml::string(
            "[runtime.retroarch]\ncores_dir = \"/var/games/retroarch/cores\"\n",
        )))
        .unwrap();

        assert_eq!(
            config.retroarch.cores_dir,
            vec![PathBuf::from("/var/games/retroarch/cores")]
        );
    }

    #[test]
    fn retroarch_env_overrides_merge_over_file() {
        let figment = Figment::new()
            .merge(Toml::string(
                "[runtime.retroarch]\nbinary = \"/file/retroarch\"\ncores_dir = [\"/file/cores\"]\n",
            ))
            .merge(("runtime.retroarch.binary", "/env/retroarch".to_string()));
        let config = Config::from_figment(figment).unwrap();
        assert_eq!(config.retroarch.binary, PathBuf::from("/env/retroarch"));
        assert_eq!(
            config.retroarch.cores_dir,
            vec![PathBuf::from("/file/cores")]
        );
    }
}
