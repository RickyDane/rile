//! Words that differ between macOS and Windows, so UI text reads native on
//! both ("Trash" vs "Recycle Bin", "Quick Look" vs "Preview").

const MAC: bool = cfg!(target_os = "macos");

/// Where deleted files go: "Trash" / "Recycle Bin".
pub fn trash() -> &'static str {
    if MAC { "Trash" } else { "Recycle Bin" }
}

/// The space-bar preview: "Quick Look" / "Preview".
pub fn preview() -> &'static str {
    if MAC { "Quick Look" } else { "Preview" }
}

/// This machine: "This Mac" / "This PC".
pub fn this_computer() -> &'static str {
    if MAC { "This Mac" } else { "This PC" }
}

/// This machine mid-sentence: "this Mac" / "this PC".
pub fn this_computer_lower() -> &'static str {
    if MAC { "this Mac" } else { "this PC" }
}

/// The OS file index: "Spotlight" / "Windows Search".
pub fn os_search() -> &'static str {
    if MAC { "Spotlight" } else { "Windows Search" }
}

/// Whether the OS search index can be queried here.
pub const HAS_OS_SEARCH: bool = cfg!(any(target_os = "macos", windows));
