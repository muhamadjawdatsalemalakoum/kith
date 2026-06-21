// Keep the console window from flashing on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `kith serve` runs the unified MCP server over stdio (for Claude Desktop / Cursor);
    // with no args it launches the desktop app.
    if std::env::args().nth(1).as_deref() == Some("serve") {
        kith_lib::serve();
    } else {
        kith_lib::run();
    }
}
