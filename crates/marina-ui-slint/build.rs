fn main() {
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

    slint_build::compile("ui/pages/main.slint").unwrap();
}
