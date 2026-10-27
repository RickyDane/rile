//! Platform-independent core logic for Rile: directory listing, file
//! operations, navigation history, places, settings and the file clipboard.
//! Nothing in here depends on the UI toolkit so it can be unit tested.

pub mod clipboard;
pub mod extension;
pub mod fs_entry;
pub mod history;
pub mod ops;
pub mod places;
pub mod settings;
pub mod watcher;

pub use clipboard::{ClipboardMode, FileClipboard};
pub use fs_entry::{Entry, EntryKind, SortKey, SortOrder, list_dir, sort_entries};
pub use history::History;
