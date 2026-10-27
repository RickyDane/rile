use std::path::PathBuf;

use sysinfo::Disks;

#[derive(Debug, Clone)]
pub struct Place {
    pub name: String,
    pub path: PathBuf,
    pub icon: PlaceIcon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceIcon {
    Home,
    Desktop,
    Documents,
    Downloads,
    Pictures,
    Music,
    Videos,
    Folder,
}

/// Standard user directories that exist on this machine.
pub fn standard_places() -> Vec<Place> {
    let candidates = [
        (dirs::home_dir(), "Home", PlaceIcon::Home),
        (dirs::desktop_dir(), "Desktop", PlaceIcon::Desktop),
        (dirs::document_dir(), "Documents", PlaceIcon::Documents),
        (dirs::download_dir(), "Downloads", PlaceIcon::Downloads),
        (dirs::picture_dir(), "Pictures", PlaceIcon::Pictures),
        (dirs::audio_dir(), "Music", PlaceIcon::Music),
        (dirs::video_dir(), "Movies", PlaceIcon::Videos),
    ];
    candidates
        .into_iter()
        .filter_map(|(p, name, icon)| {
            let path = p?;
            path.is_dir().then(|| Place { name: name.into(), path, icon })
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct Volume {
    pub name: String,
    pub mount_point: PathBuf,
    pub total: u64,
    pub available: u64,
    pub removable: bool,
}

impl Volume {
    pub fn used_fraction(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        1.0 - self.available as f32 / self.total as f32
    }
}

/// Mounted volumes the user cares about (skips macOS system sub-volumes).
pub fn volumes() -> Vec<Volume> {
    let disks = Disks::new_with_refreshed_list();
    let mut out: Vec<Volume> = disks
        .list()
        .iter()
        .filter(|d| {
            let mp = d.mount_point();
            mp == std::path::Path::new("/")
                || !(mp.starts_with("/System") || mp.starts_with("/private") || mp.starts_with("/dev"))
        })
        .map(|d| {
            let mut name = d.name().to_string_lossy().into_owned();
            if name.is_empty() || d.mount_point() == std::path::Path::new("/") {
                name = if cfg!(target_os = "macos") { "Macintosh HD".into() } else { d.mount_point().display().to_string() };
            }
            Volume {
                name,
                mount_point: d.mount_point().to_path_buf(),
                total: d.total_space(),
                available: d.available_space(),
                removable: d.is_removable(),
            }
        })
        .collect();
    out.dedup_by(|a, b| a.mount_point == b.mount_point);
    out
}
