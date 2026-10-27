use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardMode {
    Copy,
    Cut,
}

/// App-internal file clipboard (copy/cut/paste of paths).
#[derive(Debug, Clone, Default)]
pub struct FileClipboard {
    pub items: Option<(Vec<PathBuf>, ClipboardMode)>,
}

impl FileClipboard {
    pub fn set(&mut self, paths: Vec<PathBuf>, mode: ClipboardMode) {
        self.items = (!paths.is_empty()).then_some((paths, mode));
    }

    /// Returns the clipboard contents; cut contents are consumed.
    pub fn take_for_paste(&mut self) -> Option<(Vec<PathBuf>, ClipboardMode)> {
        match &self.items {
            Some((_, ClipboardMode::Cut)) => self.items.take(),
            other => other.clone(),
        }
    }

    pub fn is_cut(&self, path: &std::path::Path) -> bool {
        matches!(&self.items, Some((p, ClipboardMode::Cut)) if p.iter().any(|p| p == path))
    }
}
