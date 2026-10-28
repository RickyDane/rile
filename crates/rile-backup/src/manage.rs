//! Managing an existing backup: restoring it (the latest state, or the
//! files a run replaced or removed), deleting runs and older versions, and
//! deleting the backup itself.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};

use crate::config::BackupJob;
use crate::engine::{MARKER, VERSIONS_DIR, source_names};
use crate::state::{self, RunRecord};
use crate::target::{DestFile, PARTIAL, Target};

/// Error messages kept per restore (the count is always exact).
const MAX_ERRORS: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreFrom {
    /// The backup as of its last run.
    Latest,
    /// The files a run replaced or removed (its version folder).
    Run(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreTo {
    /// Back where they were backed up from.
    Original,
    /// Into this folder, one subfolder per source.
    Folder(PathBuf),
}

#[derive(Debug, Clone, Copy)]
pub struct RestoreOptions {
    /// Move local files that get replaced to the Trash (else overwrite).
    pub trash_replaced: bool,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RestoreProgress {
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Default)]
pub struct RestoreSummary {
    pub restored: u64,
    /// Already identical locally.
    pub skipped: u64,
    /// Local files replaced (moved to the Trash when asked to).
    pub replaced: u64,
    pub bytes: u64,
    pub error_count: usize,
    pub errors: Vec<String>,
}

impl RestoreSummary {
    fn error(&mut self, msg: String) {
        self.error_count += 1;
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(msg);
        }
    }
}

/// Refuses destinations whose marker names another job.
fn check_owner(job: &BackupJob, target: &mut dyn Target) -> Result<bool> {
    #[derive(serde::Deserialize)]
    struct Marker {
        job_id: String,
    }
    match target.read_small(MARKER)?.and_then(|b| serde_json::from_slice::<Marker>(&b).ok()) {
        Some(m) if m.job_id != job.id => bail!("{} holds a different backup", target.describe()),
        Some(_) => Ok(true),
        None => Ok(false),
    }
}

/// `base` joined with a `/`-separated backup path, refusing anything that
/// could leave `base`.
fn local_path(base: &Path, rel: &str) -> Result<PathBuf> {
    let mut out = base.to_path_buf();
    for part in rel.split('/').filter(|p| !p.is_empty()) {
        if matches!(Path::new(part).components().next(), Some(Component::ParentDir | Component::RootDir | Component::Prefix(_))) || part == "." {
            bail!("unsafe path in backup: {rel}");
        }
        out.push(part);
    }
    Ok(out)
}

/// Copies backed up files to this computer. Files that are already
/// identical are skipped; files that differ are replaced.
pub fn restore(
    job: &BackupJob,
    target: &mut dyn Target,
    from: &RestoreFrom,
    to: &RestoreTo,
    opts: RestoreOptions,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(RestoreProgress),
) -> Result<RestoreSummary> {
    check_owner(job, target)?;
    if let RestoreFrom::Run(v) = from
        && !crate::engine::is_version_name(v)
    {
        bail!("invalid version folder {v}");
    }
    let base = match from {
        RestoreFrom::Latest => String::new(),
        RestoreFrom::Run(v) => format!("{VERSIONS_DIR}/{v}/"),
    };
    // (backed up file, local destination)
    let mut plan: Vec<(DestFile, PathBuf)> = Vec::new();
    for (source, name) in job.sources.iter().zip(source_names(&job.sources)) {
        let mut found = Vec::new();
        target.walk(&format!("{base}{name}"), &mut found).with_context(|| format!("Can't read {}", target.describe()))?;
        for f in found {
            let key = f.path.strip_prefix(&base).unwrap_or(&f.path).to_string();
            let local = match to {
                RestoreTo::Original if key == name => source.clone(),
                RestoreTo::Original => local_path(source, key.strip_prefix(&format!("{name}/")).unwrap_or(&key))?,
                RestoreTo::Folder(dir) => local_path(dir, &key)?,
            };
            plan.push((f, local));
        }
    }
    if plan.is_empty() {
        bail!("There is nothing to restore");
    }

    let mut p = RestoreProgress { files_total: plan.len() as u64, bytes_total: plan.iter().map(|(f, _)| f.size).sum(), ..Default::default() };
    progress(p);
    let keeps_mtime = target.keeps_mtime();
    let mut summary = RestoreSummary::default();
    for (file, local) in plan {
        if cancel.load(Ordering::Relaxed) {
            return Err(rile_remote::Cancelled.into());
        }
        let existing = fs::metadata(&local).ok().filter(|m| m.is_file());
        let identical = existing.as_ref().is_some_and(|m| {
            m.len() == file.size
                && (!keeps_mtime || file.mtime.zip(m.modified().ok()).is_some_and(|(a, b)| a.duration_since(b).unwrap_or_else(|e| e.duration()).as_secs() < 2))
        });
        if identical {
            summary.skipped += 1;
        } else {
            let result = (|| -> Result<()> {
                let dir = local.parent().context("no parent folder")?;
                fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
                let mut tmp_name = local.file_name().unwrap_or_default().to_os_string();
                tmp_name.push(PARTIAL);
                let tmp = local.with_file_name(tmp_name);
                let got = target.get(&file.path, &tmp, cancel, &mut |n| {
                    p.bytes_done += n;
                    progress(p);
                });
                if let Err(e) = got {
                    fs::remove_file(&tmp).ok();
                    return Err(e);
                }
                if let Some(t) = file.mtime.filter(|_| keeps_mtime)
                    && let Ok(f) = fs::OpenOptions::new().write(true).open(&tmp)
                {
                    f.set_modified(t).ok();
                }
                if existing.is_some() {
                    if opts.trash_replaced {
                        trash::delete(&local).with_context(|| format!("moving {} to the Trash", local.display()))?;
                    }
                    summary.replaced += 1;
                }
                fs::rename(&tmp, &local).with_context(|| format!("replacing {}", local.display()))
            })();
            match result {
                Ok(()) => {
                    summary.restored += 1;
                    summary.bytes += file.size;
                }
                Err(e) if rile_remote::is_cancelled(&e) => return Err(e),
                Err(e) => summary.error(format!("{}: {e:#}", local.display())),
            }
        }
        p.files_done += 1;
        progress(p);
    }
    Ok(summary)
}

/// Deletes a run: its entry in the history and the older file versions it
/// kept. Returns the remaining history.
pub fn delete_run(job: &BackupJob, target: &mut dyn Target, dir: &Path, run: &RunRecord) -> Result<Vec<RunRecord>> {
    if let Some(v) = run.versions.as_ref().filter(|v| crate::engine::is_version_name(v)) {
        check_owner(job, target)?;
        target.remove_all(&format!("{VERSIONS_DIR}/{v}")).context("Deleting the run's older versions")?;
    }
    state::remove_history(dir, run)
}

/// Deletes every kept older version; the backup's current state stays.
pub fn delete_all_versions(job: &BackupJob, target: &mut dyn Target, dir: &Path) -> Result<()> {
    check_owner(job, target)?;
    target.remove_all(VERSIONS_DIR).context("Deleting older versions")?;
    let mut history = state::load_history(dir);
    for r in &mut history {
        r.versions = None;
    }
    state::save_history(dir, &history)
}

/// Deletes the backed up files, their older versions and the marker. Only
/// a folder this job demonstrably wrote (its marker, or this computer's
/// record of a run that never finished) is touched.
pub fn delete_backup_data(job: &BackupJob, target: &mut dyn Target, dir: &Path) -> Result<()> {
    let marked = check_owner(job, target)?;
    if !marked && !dir.join("manifest.json").exists() {
        bail!("{} doesn't look like this backup's folder, so nothing was deleted there", target.describe());
    }
    for name in source_names(&job.sources) {
        target.remove_all(&name).with_context(|| format!("Deleting {name}"))?;
    }
    target.remove_all(VERSIONS_DIR).context("Deleting older versions")?;
    target.remove_all(MARKER)?;
    target.remove_dir_if_empty("");
    Ok(())
}
