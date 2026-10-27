//! Trophy progress uses high-water marks, not a disk-wide crawl or usage history.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::collections::HashSet;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Progress {
    pub files: usize,
    pub folders: usize,
    pub images: usize,
    pub documents: usize,
    pub searches: usize,
    pub ai_found: usize,
    pub servers: usize,
    pub organized: usize,
    pub archives: usize,
    pub photo_percent: usize,
    pub dual_transfers: usize,
    pub personalized: usize,
}

impl Progress {
    pub fn merge(&mut self, current: Self) {
        self.files = self.files.max(current.files);
        self.folders = self.folders.max(current.folders);
        self.images = self.images.max(current.images);
        self.documents = self.documents.max(current.documents);
        self.searches = self.searches.max(current.searches);
        self.ai_found = self.ai_found.max(current.ai_found);
        self.servers = self.servers.max(current.servers);
        self.organized = self.organized.max(current.organized);
        self.archives = self.archives.max(current.archives);
        self.photo_percent = self.photo_percent.max(current.photo_percent);
        self.dual_transfers = self.dual_transfers.max(current.dual_transfers);
        self.personalized = self.personalized.max(current.personalized);
    }

    #[cfg(test)]
    fn load(path: &Path) -> Self {
        fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    #[cfg(test)]
    fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec(self)?)?;
        fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn tracks(self) -> [Track; 14] {
        [
            Track::new("files", "File explorer", "files discovered", self.files, &[100, 1_000, 10_000, 100_000, 400_000]),
            Track::new("folders", "Trailblazer", "folders explored", self.folders, &[1, 25, 100, 500, 1_000]),
            Track::new("images", "Picture perfect", "images smart-tagged", self.images, &[1, 1_000, 10_000, 100_000, 200_000, 400_000]),
            Track::new("documents", "Knowledge keeper", "documents smart-tagged", self.documents, &[1, 100, 1_000, 10_000, 100_000]),
            Track::new("searches", "Search detective", "searches completed", self.searches, &[10, 100, 1_000]),
            Track::new("ai-found", "Needle in a haystack", "successful AI searches", self.ai_found, &[1]),
            Track::new("servers", "Around the world", "distinct servers connected", self.servers, &[1, 5, 10]),
            Track { unavailable: Some("Coming soon · duplicate detection and cleanup aren't available yet."), unit: " GB", ..Track::new("space", "Space saver", "freed by duplicate cleanup", 0, &[1, 10, 100]) },
            Track { unavailable: Some("Coming soon · verified backups aren't available yet."), ..Track::new("backup", "Safety first", "verified backups completed", 0, &[1]) },
            Track::new("organized", "Everything in its place", "files moved into folders", self.organized, &[100, 1_000, 10_000]),
            Track::new("archives", "Archive architect", "archives created or extracted", self.archives, &[10, 100]),
            Track { unit: "%", ..Track::new("photo-percent", "Photo librarian", "of discovered photos smart-tagged", self.photo_percent, &[25, 50, 75, 100]) },
            Track::new("dual-transfers", "Well connected", "dual-pane transfers completed", self.dual_transfers, &[1]),
            Track::new("personalized", "Personal touch", "workspace personalizations", self.personalized, &[1]),
        ]
    }
}

pub struct Track {
    pub id: &'static str,
    pub name: &'static str,
    pub action: &'static str,
    pub count: usize,
    pub targets: &'static [usize],
    pub unavailable: Option<&'static str>,
    pub unit: &'static str,
}

impl Track {
    fn new(id: &'static str, name: &'static str, action: &'static str, count: usize, targets: &'static [usize]) -> Self {
        Self { id, name, action, count, targets, unavailable: None, unit: "" }
    }

    pub fn earned(&self) -> usize {
        if self.unavailable.is_some() { return 0; }
        self.targets.iter().filter(|&&target| self.count >= target).count()
    }

    pub fn next(&self) -> Option<usize> {
        self.targets.iter().copied().find(|&target| self.count < target)
    }

    pub fn fraction(&self) -> f32 {
        if self.unavailable.is_some() { return 0.; }
        self.next().map(|target| self.count as f32 / target as f32).unwrap_or(1.)
    }

    pub fn label(&self, value: usize) -> String {
        let number = if value >= 1_000 && value % 1_000 == 0 { format!("{}k", value / 1_000) } else { value.to_string() };
        format!("{number}{}", self.unit)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unlock {
    pub id: String,
    pub name: String,
    pub detail: String,
}

/// Events are recorded only after successful operations, never at submission.
pub enum Event {
    Search,
    AiFound,
    Server(String),
    Organized(usize),
    Archive,
    DualTransfer,
    Personalized,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Store {
    #[serde(flatten)]
    progress: Progress,
    server_ids: HashSet<String>,
    celebrated: HashSet<String>,
    version: usize,
}

impl Store {
    fn load(path: &Path) -> Self {
        let mut store: Self = fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        // Existing trophies are not new unlocks when upgrading from v1.
        if store.version == 0 {
            store.take_unlocks();
            store.version = 1;
        }
        store
    }

    fn event(&mut self, event: Event) {
        let p = &mut self.progress;
        match event {
            Event::Search => p.searches = p.searches.saturating_add(1),
            Event::AiFound => p.ai_found = p.ai_found.saturating_add(1),
            Event::Server(id) => { self.server_ids.insert(id); p.servers = p.servers.max(self.server_ids.len()); }
            Event::Organized(count) => p.organized = p.organized.saturating_add(count),
            Event::Archive => p.archives = p.archives.saturating_add(1),
            Event::DualTransfer => p.dual_transfers = p.dual_transfers.saturating_add(1),
            Event::Personalized => p.personalized = 1,
        }
    }

    fn take_unlocks(&mut self) -> Vec<Unlock> {
        let mut unlocks = Vec::new();
        for track in self.progress.tracks() {
            if track.unavailable.is_some() { continue; }
            for &target in track.targets.iter().take(track.earned()) {
                let id = format!("{}:{target}", track.id);
                if self.celebrated.insert(id.clone()) {
                    unlocks.push(Unlock { id, name: track.name.into(), detail: format!("{} {}", track.label(target), track.action) });
                }
            }
        }
        unlocks
    }

    fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec(self)?)?;
        fs::rename(tmp, path)?;
        Ok(())
    }
}

#[derive(Default)]
struct Global {
    path: Option<PathBuf>,
    store: Store,
    dirty: bool,
}

fn update<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    static GLOBAL: OnceLock<Mutex<Global>> = OnceLock::new();
    let path = rile_core::settings::Settings::config_dir().map(|d| d.join("rewards.json"));
    let mut global = GLOBAL.get_or_init(|| Mutex::new(Global::default())).lock().unwrap_or_else(|e| e.into_inner());
    if global.path != path {
        global.store = path.as_deref().map(Store::load).unwrap_or_default();
        global.path = path;
    }
    let before = serde_json::to_vec(&global.store).ok();
    let result = f(&mut global.store);
    global.dirty |= before != serde_json::to_vec(&global.store).ok();
    if global.dirty && let Some(path) = &global.path && global.store.save(path).is_ok() {
        global.dirty = false;
    }
    result
}

/// All disk access in this module must be called from background threads.
pub fn record(current: Progress) -> Progress {
    update(|store| { store.progress.merge(current); store.progress })
}

pub fn record_event(event: Event) {
    update(|store| store.event(event));
}

/// The window polls off-thread; persisted IDs prevent replay after restart.
pub fn poll(current: Progress) -> (Progress, Vec<Unlock>) {
    update(|store| {
        store.progress.merge(current);
        (store.progress, store.take_unlocks())
    })
}

/// Current discovery/tagging counts. Cache the derived counts until their
/// sources change, so idle windows don't repeatedly walk a 400k-image store.
/// No crawling or filesystem metadata reads.
pub fn snapshot() -> Progress {
    type Cached = Option<((u64, u64), Progress)>;
    static CACHE: OnceLock<Mutex<Cached>> = OnceLock::new();
    let revision = (crate::index::revision(), crate::tags::revision());
    let mut cache = CACHE.get_or_init(|| Mutex::new(None)).lock().unwrap_or_else(|e| e.into_inner());
    if let Some((seen, progress)) = *cache && seen == revision { return progress; }
    let (files, _) = crate::index::counts();
    let (folders, _) = crate::index::stats();
    let (images, documents) = crate::tags::reward_counts();
    let paths = crate::index::image_paths();
    let photo_percent = crate::tags::photo_coverage(&paths);
    let progress = Progress { files, folders, images, documents, photo_percent, ..Default::default() };
    *cache = Some((revision, progress));
    progress
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn milestones_include_boundaries_and_completion() {
        for mut track in Progress::default().tracks() {
            if track.unavailable.is_some() { continue; }
            assert_eq!(track.earned(), 0);
            assert_eq!(track.fraction(), 0.);
            for (index, &target) in track.targets.iter().enumerate() {
                track.count = target - 1;
                assert_eq!(track.earned(), index);
                assert_eq!(track.next(), Some(target));
                track.count = target;
                assert_eq!(track.earned(), index + 1);
                assert_eq!(track.next(), track.targets.get(index + 1).copied());
                assert!((0.0..=1.0).contains(&track.fraction()));
            }
            track.count = usize::MAX;
            assert_eq!(track.next(), None);
            assert_eq!(track.fraction(), 1.);
        }
        let tracks = Progress { images: 200_000, documents: 100_000, ..Default::default() }.tracks();
        assert_eq!(tracks[2].earned(), 5);
        assert_eq!(tracks[2].next(), Some(400_000));
        assert_eq!(tracks[2].fraction(), 0.5);
        assert_eq!(tracks[3].next(), None);
        assert_eq!(tracks[3].fraction(), 1.);
    }

    #[test]
    fn earned_progress_survives_cache_clear_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rewards.json");
        let mut progress = Progress::load(&path);
        progress.merge(Progress { files: 1_000, folders: 25, images: 100_000, documents: 100, ..Default::default() });
        progress.save(&path).unwrap();
        let mut loaded = Progress::load(&path);
        loaded.merge(Progress::default());
        assert_eq!(loaded, progress);
        loaded.merge(Progress { images: 200_000, ..Default::default() });
        assert_eq!(loaded.images, 200_000);
        assert_eq!(loaded.files, 1_000);
        fs::write(&path, "invalid").unwrap();
        assert_eq!(Progress::load(&path), Progress::default());
    }

    #[test]
    fn events_persist_and_unlock_each_badge_only_once() {
        let mut store = Store { version: 1, ..Default::default() };
        for _ in 0..10 { store.event(Event::Search); }
        store.event(Event::Server("sftp://example.test:22".into()));
        store.event(Event::Server("sftp://example.test:22".into()));
        store.event(Event::AiFound);
        store.event(Event::Organized(100));
        for _ in 0..10 { store.event(Event::Archive); }
        store.event(Event::DualTransfer);
        store.event(Event::Personalized);
        store.progress.merge(Progress { photo_percent: 75, ..Default::default() });
        assert_eq!(store.progress.servers, 1);
        let unlocks = store.take_unlocks();
        assert_eq!(unlocks.len(), 10);
        assert!(unlocks.iter().any(|u| u.id == "photo-percent:75"));
        assert!(store.take_unlocks().is_empty());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rewards.json");
        store.save(&path).unwrap();
        let mut loaded = Store::load(&path);
        assert_eq!(loaded.progress, store.progress);
        assert!(loaded.take_unlocks().is_empty());
        loaded.event(Event::Server("sftp://example.test:22".into()));
        assert_eq!(loaded.progress.servers, 1);
        loaded.progress.photo_percent = 100;
        assert_eq!(loaded.take_unlocks()[0].id, "photo-percent:100");
    }

    #[test]
    fn upgrades_keep_existing_badges_without_replaying_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rewards.json");
        fs::write(&path, r#"{"files":1000,"folders":25,"images":100000,"documents":100}"#).unwrap();
        let mut store = Store::load(&path);
        assert_eq!(store.progress.images, 100_000);
        assert!(store.take_unlocks().is_empty());
        store.progress.images = 200_000;
        assert_eq!(store.take_unlocks()[0].id, "images:200000");
    }

    #[test]
    fn unavailable_badges_never_unlock_or_show_progress() {
        for mut track in Progress::default().tracks().into_iter().filter(|t| t.unavailable.is_some()) {
            track.count = usize::MAX;
            assert_eq!(track.earned(), 0);
            assert_eq!(track.fraction(), 0.);
        }
    }
}
