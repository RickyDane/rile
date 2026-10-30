//! Non-UI services for Rile: streaming search, file properties, quick-look
//! previews, the self-updater and volume unmounting.
//!
//! Everything here is synchronous and index-free. Long-running operations take
//! an `&AtomicBool` cancel flag and report through callbacks; callers are
//! expected to run them on a background thread.

pub mod app_icons;
pub mod autostart;
pub mod batch_rename;
pub mod compare;
pub mod local_model;
pub mod model_runtime;
pub mod mlx_runtime;
pub mod archive;
pub mod file_manager;
pub mod gallery;
pub mod highlight;
pub mod index;
pub mod journal;
pub mod open_with;
pub mod pdf;
pub mod preview;
pub mod properties;
pub mod rewards;
pub mod search;
pub mod semantic_search;
pub mod space;
pub mod speedtest;
pub mod tags;
pub mod terminal;
pub mod updater;
pub mod vision;
pub mod volumes;
#[cfg(windows)]
pub mod win;

/// A comparable form of `path` for maps and prefix checks: on Windows letter
/// case and a `\\?\` prefix don't make two paths different.
pub fn path_key(path: &std::path::Path) -> std::path::PathBuf {
    if cfg!(windows) {
        let s = path.to_string_lossy();
        std::path::PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_lowercase())
    } else {
        path.to_path_buf()
    }
}

/// Keeps a console program from flashing a console window on Windows (Rile
/// itself has none to share). No effect elsewhere.
pub fn no_window(command: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}
