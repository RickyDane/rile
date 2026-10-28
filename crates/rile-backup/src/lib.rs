//! Incremental backups for Rile.
//!
//! A job mirrors selected folders into a local folder or a folder on a saved
//! SFTP/FTP/SMB connection. Each run copies only files that are new or
//! changed since the last run and moves replaced or deleted files into
//! dated version folders, which are pruned after a configurable time. The
//! backup stays a plain folder tree that can be browsed and restored from
//! with any file manager.
//!
//! The app runs jobs on demand, on a schedule and after file changes; the
//! macOS launch agent ([`agent`]) keeps schedules running while it's closed.

pub mod agent;
pub mod config;
pub mod engine;
pub mod manage;
pub mod state;
pub mod target;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use rile_remote::RemoteFs;

pub use config::{BackupConfig, BackupJob, Destination, Schedule};
pub use engine::{Phase, Progress, RunOptions};
pub use state::{Outcome, RunRecord, Trigger};

use target::{LocalTarget, RemoteTarget, Target};

/// Opens a dedicated connection for a saved remote connection id, with a
/// label for messages.
pub type Connector<'a> = &'a dyn Fn(&str) -> Result<(Box<dyn RemoteFs>, String)>;

pub fn open_target(dest: &Destination, connect: Connector) -> Result<Box<dyn Target>> {
    Ok(match dest {
        Destination::Local { path } => Box::new(LocalTarget::new(path.clone())),
        Destination::Remote { conn, path } => {
            let (fs, label) = connect(conn)?;
            Box::new(RemoteTarget::new(fs, path, label))
        }
    })
}

/// Runs `job` once and records the outcome in its history (also when it
/// fails or is cancelled). Never runs the same job twice at a time, across
/// processes.
pub fn run_job(
    job: &BackupJob,
    trigger: Trigger,
    opts: RunOptions,
    connect: Connector,
    cancel: &AtomicBool,
    progress: &Progress,
) -> Result<RunRecord> {
    let dir = state::job_dir(&job.id).ok_or_else(|| anyhow::anyhow!("no config dir"))?;
    run_job_in(job, &dir, trigger, opts, connect, cancel, progress)
}

pub fn run_job_in(
    job: &BackupJob,
    dir: &Path,
    trigger: Trigger,
    opts: RunOptions,
    connect: Connector,
    cancel: &AtomicBool,
    progress: &Progress,
) -> Result<RunRecord> {
    let _lock = state::Lock::acquire(dir)?;
    state::clear_status(dir);
    let started = chrono::Utc::now().timestamp();
    // Publish progress for other processes and honour their cancel requests
    // while the run goes on.
    let stop = AtomicBool::new(false);
    let finished = AtomicBool::new(false);
    let result = std::thread::scope(|s| {
        s.spawn(|| {
            let load = |a: &std::sync::atomic::AtomicU64| a.load(Ordering::Relaxed);
            let (mut bytes_rate, mut items_rate) = (state::Rate::default(), state::Rate::default());
            while !finished.load(Ordering::Relaxed) {
                let now = std::time::Instant::now();
                let copying = matches!(progress.phase(), Phase::Copying | Phase::Cleaning);
                let (bytes_per_sec, items_per_sec) = if copying {
                    (bytes_rate.sample(now, load(&progress.bytes_done)), items_rate.sample(now, load(&progress.files_done)))
                } else {
                    (0., 0.)
                };
                state::write_status(dir, &state::RunStatus {
                    pid: std::process::id(),
                    started,
                    trigger,
                    phase: progress.phase().label().to_string(),
                    scanned: load(&progress.scanned),
                    files_done: load(&progress.files_done),
                    files_total: load(&progress.files_total),
                    bytes_done: load(&progress.bytes_done),
                    bytes_total: load(&progress.bytes_total),
                    current: progress.current.lock().map(|c| c.clone()).unwrap_or_default(),
                    bytes_per_sec,
                    items_per_sec,
                });
                for _ in 0..10 {
                    if finished.load(Ordering::Relaxed) {
                        break;
                    }
                    if cancel.load(Ordering::Relaxed) || state::cancel_requested(dir) {
                        stop.store(true, Ordering::Relaxed);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        });
        if cancel.load(Ordering::Relaxed) {
            stop.store(true, Ordering::Relaxed);
        }
        let result = open_target(&job.destination, connect)
            .and_then(|mut target| engine::run(job, target.as_mut(), dir, opts, &stop, progress));
        finished.store(true, Ordering::Relaxed);
        result
    });
    state::clear_status(dir);
    let previous = state::load_history(dir);
    let files_before = previous.first().map_or(0, |r| r.files);
    let record = match result {
        Ok(s) => RunRecord {
            id: config::new_id(),
            started,
            finished: chrono::Utc::now().timestamp(),
            trigger,
            outcome: if s.error_count == 0 {
                Outcome::Success
            } else {
                let more = s.error_count.saturating_sub(1);
                let first = s.errors.first().cloned().unwrap_or_default();
                Outcome::Partial(if more > 0 { format!("{first} (and {more} more)") } else { first })
            },
            copied: s.copied,
            removed: s.removed,
            bytes: s.bytes,
            files: s.files,
            versions: s.versions,
        },
        Err(e) => RunRecord {
            id: config::new_id(),
            started,
            finished: chrono::Utc::now().timestamp(),
            trigger,
            outcome: if rile_remote::is_cancelled(&e) { Outcome::Cancelled } else { Outcome::Failed(format!("{e:#}")) },
            copied: progress.files_done.load(Ordering::Relaxed),
            removed: 0,
            bytes: progress.bytes_done.load(Ordering::Relaxed),
            files: files_before,
            versions: None,
        },
    };
    state::push_history(dir, record.clone())?;
    Ok(record)
}

/// Whether a scheduled job should run now, given its history.
pub fn is_due(job: &BackupJob, history: &[RunRecord], now: i64) -> bool {
    if !job.enabled || job.problem().is_some() {
        return false;
    }
    let Some(interval) = job.schedule.interval() else { return false };
    // After a failure, retry no sooner than every 15 minutes.
    if let Some(last) = history.first()
        && matches!(last.outcome, Outcome::Failed(_))
        && now - last.finished < 15 * 60
    {
        return false;
    }
    state::last_success(history).is_none_or(|t| now - t >= interval.as_secs() as i64)
}

/// When a scheduled job will next run (Unix seconds), if it has a schedule.
pub fn next_run(job: &BackupJob, history: &[RunRecord]) -> Option<i64> {
    let interval = job.schedule.interval()?;
    if !job.enabled {
        return None;
    }
    Some(state::last_success(history).map_or_else(|| chrono::Utc::now().timestamp(), |t| t + interval.as_secs() as i64))
}

/// "12.4 MB/s · 85 items/s" (empty until a rate is known).
pub fn format_rates(bytes_per_sec: f64, items_per_sec: f64) -> String {
    let mut parts = Vec::new();
    if bytes_per_sec >= 1. {
        // Decimal units, like the rest of Rile; MB/s for typical transfers.
        let (value, unit) = match bytes_per_sec {
            b if b >= 1e9 => (b / 1e9, "GB/s"),
            b if b >= 1e6 => (b / 1e6, "MB/s"),
            b if b >= 1e3 => (b / 1e3, "kB/s"),
            b => (b, "B/s"),
        };
        parts.push(if unit == "B/s" { format!("{value:.0} {unit}") } else { format!("{value:.1} {unit}") });
    }
    if items_per_sec >= 0.05 {
        parts.push(if items_per_sec < 10. { format!("{items_per_sec:.1} items/s") } else { format!("{items_per_sec:.0} items/s") });
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests;
