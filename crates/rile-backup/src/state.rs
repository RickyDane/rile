//! Per-job bookkeeping kept on this computer: what the last run copied
//! (the manifest), the run history and a lock against concurrent runs.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rile_core::settings::Settings;
use serde::{Deserialize, Serialize};

/// Runs kept in the history file.
const HISTORY_LEN: usize = 50;

/// `<config>/backups/<job id>/`
pub fn job_dir(id: &str) -> Option<PathBuf> {
    Some(Settings::config_dir()?.join("backups").join(id))
}

/// Size and modification time of a source file when it was backed up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub size: u64,
    /// Nanoseconds since the Unix epoch (0 when unknown).
    pub mtime: i128,
}

/// Every file in the backup as of the last run, by backup-relative path
/// (`<source name>/<path inside source>`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    /// Matches the destination's marker when the backup is exactly what
    /// this manifest describes; otherwise the destination is re-checked.
    pub run_id: String,
    pub files: BTreeMap<String, Stamp>,
}

impl Manifest {
    pub fn load(dir: &Path) -> Self {
        fs::read(dir.join("manifest.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        write_atomic(&dir.join("manifest.json"), &serde_json::to_vec(self)?)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    Manual,
    Schedule,
    Change,
    Background,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "message", rename_all = "lowercase")]
pub enum Outcome {
    Success,
    /// Finished, but some files could not be backed up.
    Partial(String),
    Failed(String),
    Cancelled,
}

impl Outcome {
    pub fn is_success(&self) -> bool {
        matches!(self, Outcome::Success | Outcome::Partial(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    /// Unique per run (empty for runs recorded before ids existed).
    #[serde(default)]
    pub id: String,
    /// Unix seconds.
    pub started: i64,
    pub finished: i64,
    pub trigger: Trigger,
    pub outcome: Outcome,
    /// Files copied (new or changed).
    pub copied: u64,
    /// Files removed from the mirror (kept in versions).
    pub removed: u64,
    pub bytes: u64,
    /// Files in the backup afterwards.
    pub files: u64,
    /// Name of the folder in `.rile-versions` holding the files this run
    /// replaced or removed (None when it kept none).
    #[serde(default)]
    pub versions: Option<String>,
}

pub fn load_history(dir: &Path) -> Vec<RunRecord> {
    fs::read(dir.join("history.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Prepends `record` to the job's history (most recent first).
pub fn push_history(dir: &Path, record: RunRecord) -> Result<Vec<RunRecord>> {
    let mut history = load_history(dir);
    history.insert(0, record);
    history.truncate(HISTORY_LEN);
    write_atomic(&dir.join("history.json"), &serde_json::to_vec_pretty(&history)?)?;
    Ok(history)
}

/// Removes `run` from the history.
pub fn remove_history(dir: &Path, run: &RunRecord) -> Result<Vec<RunRecord>> {
    let mut history = load_history(dir);
    history.retain(|r| if run.id.is_empty() { r != run } else { r.id != run.id });
    write_atomic(&dir.join("history.json"), &serde_json::to_vec_pretty(&history)?)?;
    Ok(history)
}

/// Rewrites the history (e.g. after its version folders were deleted).
pub fn save_history(dir: &Path, history: &[RunRecord]) -> Result<()> {
    write_atomic(&dir.join("history.json"), &serde_json::to_vec_pretty(history)?)
}

/// Unix seconds of the last run that finished (fully or partly).
pub fn last_success(history: &[RunRecord]) -> Option<i64> {
    history.iter().find(|r| r.outcome.is_success()).map(|r| r.finished)
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = fs::File::create(&tmp).with_context(|| format!("writing {}", tmp.display()))?;
    f.write_all(bytes)?;
    f.sync_all().ok();
    fs::rename(&tmp, path)?;
    Ok(())
}

/// A run in progress, published by whichever process runs it (the app, the
/// background agent or the menu bar item) so the others can show it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunStatus {
    pub pid: u32,
    /// Unix seconds.
    pub started: i64,
    pub trigger: Trigger,
    pub phase: String,
    pub scanned: u64,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub current: String,
    /// Smoothed transfer rate while copying.
    #[serde(default)]
    pub bytes_per_sec: f64,
    /// Smoothed rate of changes (copies and removals) handled.
    #[serde(default)]
    pub items_per_sec: f64,
}

impl RunStatus {
    /// Overall progress (0..1) once copying has started.
    pub fn fraction(&self) -> Option<f32> {
        if self.bytes_total > 0 {
            Some((self.bytes_done as f64 / self.bytes_total as f64).clamp(0., 1.) as f32)
        } else if self.files_total > 0 {
            Some((self.files_done as f64 / self.files_total as f64).clamp(0., 1.) as f32)
        } else {
            None
        }
    }
}

/// A smoothed per-second rate of a growing counter.
#[derive(Debug, Default, Clone, Copy)]
pub struct Rate {
    last: Option<(std::time::Instant, u64)>,
    value: f64,
}

impl Rate {
    /// Feeds the counter's current total; returns the smoothed rate.
    pub fn sample(&mut self, now: std::time::Instant, total: u64) -> f64 {
        match self.last {
            Some((t, prev)) => {
                let secs = now.duration_since(t).as_secs_f64();
                if secs >= 0.25 {
                    let rate = total.saturating_sub(prev) as f64 / secs;
                    // First real sample sets the value; later ones blend in.
                    self.value = if self.value == 0. { rate } else { self.value * 0.6 + rate * 0.4 };
                    self.last = Some((now, total));
                }
            }
            None => self.last = Some((now, total)),
        }
        self.value
    }
}

pub(crate) fn write_status(dir: &Path, status: &RunStatus) {
    if let Ok(bytes) = serde_json::to_vec(status) {
        write_atomic(&dir.join("status.json"), &bytes).ok();
    }
}

pub(crate) fn clear_status(dir: &Path) {
    fs::remove_file(dir.join("status.json")).ok();
    fs::remove_file(dir.join("cancel")).ok();
}

/// The job's running backup, from any process (None when idle or when the
/// process that ran it is gone).
pub fn read_status(dir: &Path) -> Option<RunStatus> {
    let status: RunStatus = serde_json::from_slice(&fs::read(dir.join("status.json")).ok()?).ok()?;
    process_alive(status.pid).then_some(status)
}

/// Asks the process running the job to stop.
pub fn request_cancel(dir: &Path) -> Result<()> {
    fs::write(dir.join("cancel"), b"").context("requesting cancel")
}

pub(crate) fn cancel_requested(dir: &Path) -> bool {
    dir.join("cancel").exists()
}

/// Held while a job runs, so the app and the background agent never back
/// up the same job at once. Released on drop.
pub struct Lock(PathBuf);

impl Lock {
    pub fn acquire(dir: &Path) -> Result<Lock> {
        fs::create_dir_all(dir)?;
        let path = dir.join("lock");
        for _ in 0..2 {
            match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    write!(f, "{}", std::process::id()).ok();
                    return Ok(Lock(path));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let holder = fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u32>().ok());
                    if holder.is_some_and(|pid| pid != std::process::id() && process_alive(pid)) {
                        bail!("this backup is already running");
                    }
                    // Left behind by a process that died (or by this one).
                    fs::remove_file(&path).ok();
                }
                Err(e) => return Err(e).context("creating lock file"),
            }
        }
        bail!("this backup is already running")
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        fs::remove_file(&self.0).ok();
    }
}

#[cfg(unix)]
pub fn process_alive(pid: u32) -> bool {
    // Signal 0 only checks that the process exists (EPERM: it does, but
    // belongs to someone else).
    let ok = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
    ok || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
pub fn process_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_is_exclusive_and_released() {
        let dir = tempfile::tempdir().unwrap();
        let lock = Lock::acquire(dir.path()).unwrap();
        // Same process may not hold it twice through a stale-looking file...
        fs::write(dir.path().join("lock"), "1").unwrap(); // pid 1 (launchd/init) is alive
        assert!(Lock::acquire(dir.path()).is_err());
        drop(lock);
        assert!(!dir.path().join("lock").exists());
        // ...but a dead holder's lock is taken over.
        fs::write(dir.path().join("lock"), "999999999").unwrap();
        let _again = Lock::acquire(dir.path()).unwrap();
    }

    #[test]
    fn history_is_capped_and_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..60 {
            let outcome = if i % 2 == 0 { Outcome::Success } else { Outcome::Failed("x".into()) };
            push_history(dir.path(), RunRecord { id: String::new(), started: i, finished: i, trigger: Trigger::Manual, outcome, copied: 0, removed: 0, bytes: 0, files: 0, versions: None }).unwrap();
        }
        let h = load_history(dir.path());
        assert_eq!(h.len(), HISTORY_LEN);
        assert_eq!(h[0].started, 59);
        assert_eq!(last_success(&h), Some(58));
    }
}
