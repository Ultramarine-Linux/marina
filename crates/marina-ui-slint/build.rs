fn main() {
    minify_platform_catalog();

    if std::env::var_os("CARGO_FEATURE_DEBUG_MCP").is_some()
        && std::env::var("PROFILE").as_deref() == Ok("debug")
    {
        // Slint's compiler reads this while generating the UI metadata used by
        // the embedded MCP server. This is intentionally enabled only for the
        // opt-in debug-mcp feature.
        unsafe {
            std::env::set_var("SLINT_EMIT_DEBUG_INFO", "1");
        }
    }

    if std::env::var_os("CARGO_FEATURE_LIVE_PREVIEW").is_some() {
        unsafe {
            std::env::set_var("SLINT_LIVE_PREVIEW", "1");
        }
    }

    let lucide_library = std::path::PathBuf::from(lucide_slint::lib());
    let libraries = std::collections::HashMap::from([(
        "lucide".to_owned(),
        lucide_library
            .parent()
            .expect("lucide-slint library file must have a parent directory")
            .to_path_buf(),
    )]);
    let configuration = slint_build::CompilerConfiguration::new().with_library_paths(libraries);
    slint_build::compile_with_config("ui/pages/main.slint", configuration).unwrap();
}

fn minify_platform_catalog() {
    const SOURCE: &str = "src/ui/platform-names.json";
    println!("cargo:rerun-if-changed={SOURCE}");

    let source = std::fs::read_to_string(SOURCE).expect("read RomM platform catalog");
    let catalog: serde_json::Value =
        serde_json::from_str(&source).expect("parse RomM platform catalog");
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"))
        .join("platform-names.json");
    std::fs::write(
        output,
        serde_json::to_vec(&catalog).expect("serialize RomM platform catalog"),
    )
    .expect("write minified RomM platform catalog");
}
