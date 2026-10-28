//! Backup jobs as configured by the user, persisted in `backups.toml` next to
//! the settings so the app and the background agent share them.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use rile_core::settings::Settings;
use serde::{Deserialize, Serialize};

/// Where a job's backup is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Destination {
    /// A folder on this computer or a mounted drive.
    Local { path: PathBuf },
    /// A folder on a saved SFTP/FTP/SMB connection.
    Remote { conn: String, path: String },
}

impl Default for Destination {
    fn default() -> Self {
        Destination::Local { path: PathBuf::new() }
    }
}

impl Destination {
    pub fn is_set(&self) -> bool {
        match self {
            Destination::Local { path } => !path.as_os_str().is_empty(),
            Destination::Remote { conn, path } => !conn.is_empty() && !path.trim().is_empty(),
        }
    }
}

/// How often a job runs on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Schedule {
    /// Only when started by hand (or by file changes, see [`BackupJob::watch`]).
    #[default]
    Manual,
    Hourly,
    Daily,
    Weekly,
}

impl Schedule {
    pub const ALL: [Schedule; 4] = [Schedule::Manual, Schedule::Hourly, Schedule::Daily, Schedule::Weekly];

    pub fn interval(self) -> Option<Duration> {
        match self {
            Schedule::Manual => None,
            Schedule::Hourly => Some(Duration::from_secs(60 * 60)),
            Schedule::Daily => Some(Duration::from_secs(24 * 60 * 60)),
            Schedule::Weekly => Some(Duration::from_secs(7 * 24 * 60 * 60)),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Schedule::Manual => "Manual",
            Schedule::Hourly => "Hourly",
            Schedule::Daily => "Daily",
            Schedule::Weekly => "Weekly",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackupJob {
    pub id: String,
    pub name: String,
    /// Folders (or single files) to back up. Each one becomes a top-level
    /// folder of the backup, named after it.
    pub sources: Vec<PathBuf>,
    pub destination: Destination,
    /// Glob patterns of paths to skip, matched against the file name and
    /// against the path relative to its source (e.g. `node_modules`, `*.tmp`).
    pub excludes: Vec<String>,
    pub schedule: Schedule,
    /// Back up shortly after files in a source change (while Rile runs).
    pub watch: bool,
    /// Keep running the schedule when Rile is closed (macOS launch agent).
    pub background: bool,
    /// Days to keep replaced and deleted files in `.rile-versions` (0 = forever).
    pub keep_versions_days: u32,
    /// Paused jobs never run on their own.
    pub enabled: bool,
}

impl Default for BackupJob {
    fn default() -> Self {
        Self {
            id: new_id(),
            name: "Backup".into(),
            sources: Vec::new(),
            destination: Destination::default(),
            excludes: [".DS_Store", ".Trash", "node_modules", "Thumbs.db", "desktop.ini", "$RECYCLE.BIN", "System Volume Information"]
                .map(String::from)
                .to_vec(),
            schedule: Schedule::Daily,
            watch: false,
            background: false,
            keep_versions_days: 30,
            enabled: true,
        }
    }
}

impl BackupJob {
    /// Why the job can't run yet, if anything is missing.
    pub fn problem(&self) -> Option<&'static str> {
        if self.sources.is_empty() {
            Some("Add at least one folder to back up")
        } else if !self.destination.is_set() {
            Some("Choose where to store the backup")
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupConfig {
    /// Show backups in the menu bar (a helper that runs from login).
    #[serde(default = "yes")]
    pub menu_bar: bool,
    #[serde(default)]
    pub job: Vec<BackupJob>,
}

fn yes() -> bool {
    true
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self { menu_bar: true, job: Vec::new() }
    }
}

impl BackupConfig {
    pub fn path() -> Option<PathBuf> {
        Some(Settings::config_dir()?.join("backups.toml"))
    }

    /// Loads the jobs; a missing or unreadable file yields no jobs.
    pub fn load() -> Self {
        Self::path()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path().context("no config dir")?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&BackupJob> {
        self.job.iter().find(|j| j.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut BackupJob> {
        self.job.iter_mut().find(|j| j.id == id)
    }

    /// Whether the menu bar helper should run.
    pub fn wants_menu_bar(&self) -> bool {
        self.menu_bar && !self.job.is_empty()
    }

    /// Whether any job wants to run while Rile is closed.
    pub fn wants_agent(&self) -> bool {
        self.job.iter().any(|j| j.enabled && j.background && j.schedule != Schedule::Manual)
    }
}

/// A new unique job id (time + counter based, hex).
pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:x}", nanos, COUNTER.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut cfg = BackupConfig::default();
        cfg.job.push(BackupJob {
            sources: vec!["/a".into()],
            destination: Destination::Remote { conn: "c1".into(), path: "/backups".into() },
            ..Default::default()
        });
        cfg.job.push(BackupJob { destination: Destination::Local { path: "/Volumes/X".into() }, ..Default::default() });
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: BackupConfig = toml::from_str(&text).unwrap();
        assert_eq!(back, cfg);
        // Unknown/missing fields fall back to defaults.
        let partial: BackupConfig = toml::from_str("[[job]]\nid = \"x\"\nname = \"N\"\n").unwrap();
        assert_eq!(partial.job[0].schedule, Schedule::Daily);
        assert!(partial.job[0].problem().is_some());
    }
}
