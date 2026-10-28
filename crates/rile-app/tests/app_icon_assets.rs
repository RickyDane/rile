// Run the icon tests independently of GPUI's headless UI integration tests.
#[path = "../src/app_icon.rs"]
#[expect(dead_code, reason = "this harness tests embedded assets, not the UI and Dock entry points")]
mod app_icon;
