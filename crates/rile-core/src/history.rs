use std::path::PathBuf;

/// Browser-style back/forward navigation stack.
#[derive(Debug, Clone, Default)]
pub struct History {
    back: Vec<Location>,
    forward: Vec<Location>,
}

/// A place the pane can show: the home page, the photo library, a local
/// directory, or a directory on a remote connection (FTP/SFTP), identified
/// by connection id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Home,
    /// Every image on this Mac, as a photo library.
    Gallery,
    Dir(PathBuf),
    Remote { conn: String, path: String },
    /// The Space Analyzer, showing the analyzed folder at this path (or its
    /// start page). Drilling down is navigation, so back/forward work.
    SpaceAnalyzer(Option<PathBuf>),
    /// Incremental backup jobs and their status.
    Backups,
    /// Nearby Rile devices: paired and discovered ones, as cards.
    Devices,
    /// A custom page contributed by a plugin.
    PluginPage { plugin: String, page: String, title: String },
}

/// A [`Location`] as stored in saved workspaces.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SavedLocation {
    Overview,
    Gallery,
    Folder { path: PathBuf },
    Remote { conn: String, path: String },
    SpaceAnalyzer {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<PathBuf>,
    },
    Backups,
    Devices,
    PluginPage { plugin: String, page: String, title: String },
}

impl From<&Location> for SavedLocation {
    fn from(l: &Location) -> Self {
        match l {
            Location::Home => SavedLocation::Overview,
            Location::Gallery => SavedLocation::Gallery,
            Location::Dir(p) => SavedLocation::Folder { path: p.clone() },
            Location::Remote { conn, path } => SavedLocation::Remote { conn: conn.clone(), path: path.clone() },
            Location::SpaceAnalyzer(p) => SavedLocation::SpaceAnalyzer { path: p.clone() },
            Location::Backups => SavedLocation::Backups,
            Location::Devices => SavedLocation::Devices,
            Location::PluginPage { plugin, page, title } => {
                SavedLocation::PluginPage { plugin: plugin.clone(), page: page.clone(), title: title.clone() }
            }
        }
    }
}

impl From<SavedLocation> for Location {
    fn from(l: SavedLocation) -> Self {
        match l {
            SavedLocation::Overview => Location::Home,
            SavedLocation::Gallery => Location::Gallery,
            SavedLocation::Folder { path } => Location::Dir(path),
            SavedLocation::Remote { conn, path } => Location::Remote { conn, path },
            SavedLocation::SpaceAnalyzer { path } => Location::SpaceAnalyzer(path),
            SavedLocation::Backups => Location::Backups,
            SavedLocation::Devices => Location::Devices,
            SavedLocation::PluginPage { plugin, page, title } => Location::PluginPage { plugin, page, title },
        }
    }
}

impl History {
    /// Records `from` before navigating somewhere new; clears forward stack.
    pub fn push(&mut self, from: Location) {
        if self.back.last() != Some(&from) {
            self.back.push(from);
        }
        self.forward.clear();
    }

    pub fn go_back(&mut self, current: Location) -> Option<Location> {
        let prev = self.back.pop()?;
        self.forward.push(current);
        Some(prev)
    }

    pub fn go_forward(&mut self, current: Location) -> Option<Location> {
        let next = self.forward.pop()?;
        self.back.push(current);
        Some(next)
    }

    /// Where [`Self::go_back`] would go, without going.
    pub fn back_target(&self) -> Option<&Location> {
        self.back.last()
    }

    /// Where [`Self::go_forward`] would go, without going.
    pub fn forward_target(&self) -> Option<&Location> {
        self.forward.last()
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_and_forward() {
        let a = Location::Dir("/a".into());
        let b = Location::Dir("/b".into());
        let c = Location::Dir("/c".into());
        let mut h = History::default();
        h.push(a.clone());
        h.push(b.clone());
        assert_eq!(h.go_back(c.clone()), Some(b.clone()));
        assert_eq!(h.go_back(b.clone()), Some(a.clone()));
        assert!(!h.can_go_back());
        assert_eq!(h.go_forward(a.clone()), Some(b.clone()));
        h.push(b);
        assert!(!h.can_go_forward());
    }
}
