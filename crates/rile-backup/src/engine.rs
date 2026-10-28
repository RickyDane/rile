//! One incremental backup run: scan the sources, compare them with what the
//! destination holds, copy new and changed files, and move replaced or
//! deleted files into a dated `.rile-versions` folder.
//!
//! Destination layout:
//! ```text
//! <dest>/<source name>/…                    plain mirror of each source
//! <dest>/.rile-versions/<run time>/<source name>/…   older copies
//! <dest>/.rile-backup.json                  marker: owning job and last run
//! ```

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::config::{BackupJob, Destination};
use crate::state::{Manifest, Stamp};
use crate::target::{DestFile, Target};

pub const VERSIONS_DIR: &str = ".rile-versions";
pub const MARKER: &str = ".rile-backup.json";
const VERSION_FORMAT: &str = "%Y-%m-%d_%H-%M-%S";
/// Error messages kept per run (the count is always exact).
const MAX_ERRORS: usize = 20;
/// Consecutive failed files after which the run gives up (e.g. the
/// destination went away).
const MAX_CONSECUTIVE_FAILURES: usize = 25;
/// Modification times closer than this count as equal (FAT/exFAT and some
/// network shares store them coarsely).
const MTIME_SLACK_NS: i128 = 2_000_000_000;

#[derive(Debug, Serialize, Deserialize)]
struct Marker {
    job_id: String,
    job_name: String,
    run_id: String,
    finished: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Preparing = 0,
    Scanning,
    Comparing,
    Copying,
    Cleaning,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Preparing => "Connecting",
            Phase::Scanning => "Scanning",
            Phase::Comparing => "Comparing",
            Phase::Copying => "Copying",
            Phase::Cleaning => "Cleaning up",
        }
    }
}

/// Live counters, shared with the UI.
#[derive(Default)]
pub struct Progress {
    phase: AtomicU8,
    pub scanned: AtomicU64,
    pub files_total: AtomicU64,
    pub files_done: AtomicU64,
    pub bytes_total: AtomicU64,
    pub bytes_done: AtomicU64,
    pub current: Mutex<String>,
}

impl Progress {
    pub fn phase(&self) -> Phase {
        match self.phase.load(Ordering::Relaxed) {
            1 => Phase::Scanning,
            2 => Phase::Comparing,
            3 => Phase::Copying,
            4 => Phase::Cleaning,
            _ => Phase::Preparing,
        }
    }

    pub fn set_phase(&self, phase: Phase) {
        self.phase.store(phase as u8, Ordering::Relaxed);
    }

    fn set_current(&self, s: &str) {
        if let Ok(mut c) = self.current.lock() {
            c.clear();
            c.push_str(s);
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RunOptions {
    /// Re-check the destination instead of trusting the last run's manifest.
    pub verify: bool,
}

#[derive(Debug, Default)]
pub struct Summary {
    pub copied: u64,
    pub removed: u64,
    pub bytes: u64,
    /// Files in the backup afterwards.
    pub files: u64,
    pub error_count: usize,
    pub errors: Vec<String>,
    /// Version folder this run filled (see [`crate::RunRecord::versions`]).
    pub versions: Option<String>,
}

impl Summary {
    fn error(&mut self, msg: String) {
        log::warn!("backup: {msg}");
        self.error_count += 1;
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(msg);
        }
    }
}

/// Folder name of each source inside the backup: its own name, with a
/// numeric suffix when two sources share one.
pub fn source_names(sources: &[PathBuf]) -> Vec<String> {
    let mut used = HashSet::new();
    sources
        .iter()
        .map(|s| {
            let base = s
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .filter(|n| !n.is_empty() && !n.starts_with('.'))
                .unwrap_or_else(|| "Root".into());
            let mut name = base.clone();
            let mut n = 2;
            while !used.insert(name.to_lowercase()) {
                name = format!("{base} {n}");
                n += 1;
            }
            name
        })
        .collect()
}

fn stamp_of(meta: &fs::Metadata) -> Stamp {
    Stamp { size: meta.len(), mtime: meta.modified().map(nanos).unwrap_or(0) }
}

fn nanos(t: SystemTime) -> i128 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i128,
        Err(e) => -(e.duration().as_nanos() as i128),
    }
}

fn from_nanos(n: i128) -> Option<SystemTime> {
    (n > 0).then(|| UNIX_EPOCH + Duration::from_nanos(n as u64))
}

fn excludes(patterns: &[String]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for p in patterns.iter().map(|p| p.trim().trim_end_matches('/')).filter(|p| !p.is_empty()) {
        b.add(Glob::new(p).with_context(|| format!("invalid exclude pattern \"{p}\""))?);
    }
    Ok(b.build()?)
}

/// A file to back up.
struct SourceFile {
    abs: PathBuf,
    stamp: Stamp,
}

struct Scan {
    files: BTreeMap<String, SourceFile>,
    /// Backup paths (prefixes) that could not be read: nothing below them
    /// may be treated as deleted.
    unreadable: Vec<String>,
}

impl Scan {
    fn protects(&self, key: &str) -> bool {
        self.unreadable.iter().any(|p| key == p || key.starts_with(&format!("{p}/")))
    }
}

#[allow(clippy::too_many_arguments)]
fn scan_dir(
    dir: &Path,
    key: &str,
    rel: &str,
    skip: &GlobSet,
    scan: &mut Scan,
    summary: &mut Summary,
    cancel: &AtomicBool,
    progress: &Progress,
) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(rile_remote::Cancelled.into());
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            summary.error(format!("{}: {e}", dir.display()));
            scan.unreadable.push(key.to_string());
            return Ok(());
        }
    };
    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let child_rel = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        if skip.is_match(&name) || skip.is_match(&child_rel) {
            continue;
        }
        let child_key = format!("{key}/{name}");
        let path = entry.path();
        let Ok(lmeta) = entry.metadata() else { continue };
        // Symlinked files are copied as their target; symlinked folders are
        // skipped (they may loop or point outside the source).
        let meta = if lmeta.file_type().is_symlink() {
            match fs::metadata(&path) {
                Ok(m) if m.is_file() => m,
                _ => continue,
            }
        } else {
            lmeta
        };
        if meta.is_dir() {
            scan_dir(&path, &child_key, &child_rel, skip, scan, summary, cancel, progress)?;
        } else if meta.is_file() {
            progress.scanned.fetch_add(1, Ordering::Relaxed);
            scan.files.insert(child_key, SourceFile { abs: path, stamp: stamp_of(&meta) });
        }
    }
    Ok(())
}

fn scan_sources(job: &BackupJob, names: &[String], summary: &mut Summary, cancel: &AtomicBool, progress: &Progress) -> Result<Scan> {
    let skip = excludes(&job.excludes)?;
    let mut scan = Scan { files: BTreeMap::new(), unreadable: Vec::new() };
    for (source, name) in job.sources.iter().zip(names) {
        match fs::metadata(source) {
            Ok(m) if m.is_dir() => scan_dir(source, name, "", &skip, &mut scan, summary, cancel, progress)?,
            Ok(m) if m.is_file() => {
                progress.scanned.fetch_add(1, Ordering::Relaxed);
                scan.files.insert(name.clone(), SourceFile { abs: source.clone(), stamp: stamp_of(&m) });
            }
            _ => {
                // An unplugged drive must not look like everything was deleted.
                summary.error(format!("{} is not available", source.display()));
                scan.unreadable.push(name.clone());
            }
        }
    }
    Ok(scan)
}

/// Refuses destinations that would back up into themselves.
fn check_overlap(job: &BackupJob) -> Result<()> {
    let Destination::Local { path } = &job.destination else { return Ok(()) };
    // Resolve through the deepest existing ancestor, so a destination that
    // doesn't exist yet still compares correctly (e.g. /var vs /private/var).
    let canon = |p: &Path| {
        let mut rest = Vec::new();
        let mut cur = p;
        loop {
            if let Ok(real) = dunce::canonicalize(cur) {
                return rest.iter().rev().fold(real, |acc: PathBuf, part| acc.join(part));
            }
            match (cur.file_name(), cur.parent()) {
                (Some(name), Some(parent)) => {
                    rest.push(name.to_os_string());
                    cur = parent;
                }
                _ => return p.to_path_buf(),
            }
        }
    };
    let dest = canon(path);
    for source in &job.sources {
        let source = canon(source);
        if dest.starts_with(&source) {
            bail!("The backup location is inside {}, which is being backed up", source.display());
        }
        if source.starts_with(&dest) {
            bail!("{} is inside the backup location", source.display());
        }
    }
    Ok(())
}

fn parent_key(key: &str) -> Option<&str> {
    key.rfind('/').map(|i| &key[..i])
}

/// Runs one backup of `job` into `target`. `state_dir` holds the job's
/// manifest. Returns what was done; per-file problems are collected in the
/// summary, while problems that stop the whole run are errors.
pub fn run(
    job: &BackupJob,
    target: &mut dyn Target,
    state_dir: &Path,
    opts: RunOptions,
    cancel: &AtomicBool,
    progress: &Progress,
) -> Result<Summary> {
    if job.sources.is_empty() {
        bail!("No folders to back up");
    }
    check_overlap(job)?;
    let mut summary = Summary::default();
    let names = source_names(&job.sources);
    let run_id = crate::config::new_id();
    let version_name = chrono::Local::now().format(VERSION_FORMAT).to_string();
    let version_dir = format!("{VERSIONS_DIR}/{version_name}");

    progress.set_phase(Phase::Preparing);
    target.prepare().with_context(|| format!("Can't open {}", target.describe()))?;
    let marker: Option<Marker> = target.read_small(MARKER)?.and_then(|b| serde_json::from_slice(&b).ok());
    if let Some(m) = &marker
        && m.job_id != job.id
    {
        bail!("{} already holds another backup (\"{}\"). Choose an empty folder.", target.describe(), m.job_name);
    }
    let manifest = Manifest::load(state_dir);
    let trusted = !opts.verify && !manifest.run_id.is_empty() && marker.as_ref().is_some_and(|m| m.run_id == manifest.run_id);

    progress.set_phase(Phase::Scanning);
    let scan = scan_sources(job, &names, &mut summary, cancel, progress)?;

    // What the destination holds now, as far as we can tell.
    progress.set_phase(Phase::Comparing);
    let mut present: BTreeMap<String, Stamp> = BTreeMap::new();
    if trusted {
        present = manifest.files.clone();
    } else {
        let keeps_mtime = target.keeps_mtime();
        let mut found: Vec<DestFile> = Vec::new();
        for name in &names {
            if cancel.load(Ordering::Relaxed) {
                return Err(rile_remote::Cancelled.into());
            }
            target.walk(name, &mut found).with_context(|| format!("Can't read {}", target.describe()))?;
        }
        for f in found {
            // Trust the destination copy when its size matches and either
            // its time matches the source (local) or the manifest recorded
            // this exact source version.
            let recorded = manifest.files.get(&f.path).copied();
            let mtime = match (keeps_mtime, f.mtime) {
                (true, Some(t)) => nanos(t),
                _ => recorded.filter(|r| r.size == f.size).map_or(0, |r| r.mtime),
            };
            present.insert(f.path, Stamp { size: f.size, mtime });
        }
    }
    let same = |have: &Stamp, want: &Stamp| have.size == want.size && have.mtime != 0 && (have.mtime - want.mtime).abs() <= if trusted { 0 } else { MTIME_SLACK_NS };

    let to_copy: Vec<(&String, &SourceFile)> = scan
        .files
        .iter()
        .filter(|(key, file)| !present.get(*key).is_some_and(|have| same(have, &file.stamp)))
        .collect();
    let to_remove: Vec<String> = present
        .keys()
        .filter(|k| !scan.files.contains_key(*k) && !scan.protects(k))
        .cloned()
        .collect();

    // The new manifest starts with every file that needs no work.
    let mut files: BTreeMap<String, Stamp> = BTreeMap::new();
    for (key, have) in &present {
        match scan.files.get(key) {
            Some(file) if same(have, &file.stamp) => {
                files.insert(key.clone(), file.stamp);
            }
            // Unreadable sources keep their last known state.
            None if scan.protects(key) => {
                if let Some(old) = manifest.files.get(key) {
                    files.insert(key.clone(), *old);
                }
            }
            _ => {}
        }
    }
    let mut new_manifest = Manifest { run_id: run_id.clone(), files };

    progress.files_total.store((to_copy.len() + to_remove.len()) as u64, Ordering::Relaxed);
    progress.bytes_total.store(to_copy.iter().map(|(_, f)| f.stamp.size).sum(), Ordering::Relaxed);
    progress.set_phase(Phase::Copying);

    let mut last_save = Instant::now();
    let kept_versions = std::cell::Cell::new(false);
    let mut failures = 0;
    // Folders where a file already failed: more failures there (e.g. a
    // folder the server refuses to create) are reported once and don't
    // count towards giving up, which is meant for a vanished destination.
    let mut failed_folders: HashSet<String> = HashSet::new();
    let finish_early = |manifest: &Manifest, e: anyhow::Error| -> Result<Summary> {
        // Keep what was copied so the next run doesn't start over.
        manifest.save(state_dir).ok();
        Err(e)
    };
    for (key, file) in to_copy {
        if cancel.load(Ordering::Relaxed) {
            return finish_early(&new_manifest, rile_remote::Cancelled.into());
        }
        progress.set_current(key);
        let result = (|| -> Result<()> {
            if present.contains_key(key) && target.move_file(key, &format!("{version_dir}/{key}"))? {
                kept_versions.set(true);
            }
            target.put(&file.abs, from_nanos(file.stamp.mtime), key, cancel, &mut |n| {
                progress.bytes_done.fetch_add(n, Ordering::Relaxed);
            })
        })();
        progress.files_done.fetch_add(1, Ordering::Relaxed);
        match result {
            Ok(()) => {
                failures = 0;
                summary.copied += 1;
                summary.bytes += file.stamp.size;
                new_manifest.files.insert(key.clone(), file.stamp);
            }
            Err(e) if rile_remote::is_cancelled(&e) => return finish_early(&new_manifest, e),
            Err(e) => {
                let folder = parent_key(key).unwrap_or_default().to_string();
                if failed_folders.iter().any(|f| folder == *f || folder.starts_with(&format!("{f}/"))) {
                    summary.error_count += 1;
                    continue;
                }
                summary.error(format!("{key}: {e:#}"));
                failed_folders.insert(folder);
                failures += 1;
                if failures >= MAX_CONSECUTIVE_FAILURES {
                    return finish_early(&new_manifest, e.context("Too many files failed in a row"));
                }
            }
        }
        if last_save.elapsed() > Duration::from_secs(20) {
            new_manifest.save(state_dir).ok();
            last_save = Instant::now();
        }
    }

    progress.set_phase(Phase::Cleaning);
    let mut emptied: HashSet<String> = HashSet::new();
    for key in &to_remove {
        if cancel.load(Ordering::Relaxed) {
            return finish_early(&new_manifest, rile_remote::Cancelled.into());
        }
        progress.set_current(key);
        match target.move_file(key, &format!("{version_dir}/{key}")) {
            Ok(moved) => {
                kept_versions.set(kept_versions.get() || moved);
                summary.removed += 1;
                new_manifest.files.remove(key);
                if let Some(p) = parent_key(key) {
                    emptied.insert(p.to_string());
                }
            }
            Err(e) => summary.error(format!("{key}: {e:#}")),
        }
        progress.files_done.fetch_add(1, Ordering::Relaxed);
    }
    // Deepest first, so whole emptied trees go away.
    let mut dirs: Vec<String> = emptied.into_iter().flat_map(|d| {
        let mut chain = vec![];
        let mut cur = Some(d.as_str());
        while let Some(c) = cur {
            chain.push(c.to_string());
            cur = parent_key(c);
        }
        chain
    }).collect::<HashSet<_>>().into_iter().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.matches('/').count()));
    for d in dirs.iter().filter(|d| d.contains('/')) {
        target.remove_dir_if_empty(d);
    }

    // Remember the new state, then mark the destination as matching it.
    summary.files = new_manifest.files.len() as u64;
    summary.versions = kept_versions.get().then_some(version_name);
    new_manifest.save(state_dir).context("Saving the backup manifest")?;
    let marker = Marker {
        job_id: job.id.clone(),
        job_name: job.name.clone(),
        run_id,
        finished: chrono::Utc::now().timestamp(),
    };
    target.write_small(MARKER, &serde_json::to_vec_pretty(&marker)?).context("Writing the backup marker")?;

    if job.keep_versions_days > 0 {
        prune_versions(target, job.keep_versions_days, &mut summary);
    }
    progress.set_current("");
    Ok(summary)
}

/// Deletes version folders older than `days`.
fn prune_versions(target: &mut dyn Target, days: u32, summary: &mut Summary) {
    let cutoff = chrono::Local::now().naive_local() - chrono::Duration::days(days as i64);
    let Ok(names) = target.list_names(VERSIONS_DIR) else { return };
    for name in names {
        if let Ok(t) = chrono::NaiveDateTime::parse_from_str(&name, VERSION_FORMAT)
            && t < cutoff
            && let Err(e) = target.remove_all(&format!("{VERSIONS_DIR}/{name}"))
        {
            summary.error(format!("removing old versions {name}: {e:#}"));
        }
    }
}

/// Whether `name` is a version folder name (never a path).
pub fn is_version_name(name: &str) -> bool {
    chrono::NaiveDateTime::parse_from_str(name, VERSION_FORMAT).is_ok()
}

/// Dated version folders in the destination, newest first.
pub fn list_versions(target: &mut dyn Target) -> Result<Vec<String>> {
    let mut names: Vec<String> = target
        .list_names(VERSIONS_DIR)?
        .into_iter()
        .filter(|n| chrono::NaiveDateTime::parse_from_str(n, VERSION_FORMAT).is_ok())
        .collect();
    names.sort_by(|a, b| b.cmp(a));
    Ok(names)
}
