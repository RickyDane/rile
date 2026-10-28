//! Non-UI services for Rile: streaming search, file properties, quick-look
//! previews, the self-updater and volume unmounting.
//!
//! Everything here is synchronous and index-free. Long-running operations take
//! an `&AtomicBool` cancel flag and report through callbacks; callers are
//! expected to run them on a background thread.

pub mod app_icons;
pub mod archive;
pub mod file_manager;
pub mod gallery;
pub mod highlight;
pub mod index;
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
pub mod updater;
pub mod vision;
pub mod volumes;
