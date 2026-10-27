use std::path::Path;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// Watches a single directory (non-recursive) and calls `on_change` for
/// every filesystem event. Consumers are expected to debounce. Dropping the
/// watcher stops it.
pub struct DirWatcher {
    _watcher: RecommendedWatcher,
}

impl DirWatcher {
    pub fn new(dir: &Path, on_change: impl Fn() + Send + 'static) -> notify::Result<Self> {
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if res.is_ok() {
                on_change();
            }
        })?;
        watcher.watch(dir, RecursiveMode::NonRecursive)?;
        Ok(Self { _watcher: watcher })
    }
}
