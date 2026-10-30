//! Backup jobs in the app: configuration, runs (shown with the other
//! background jobs), the schedule, the file-change trigger and the launch
//! agent that keeps schedules running while Rile is closed.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::component::{WindowExt, notification::Notification};
use gpui_kit::{AppContext, Context, SharedString, Task, Window};
use notify::Watcher as _;
use rile_backup::{BackupConfig, BackupJob, Destination, Outcome, Progress, RunOptions, RunRecord, Trigger};
use rile_core::history::Location;
use rile_core::ops;

use super::{FinishedJob, RunningOp, Workspace};

/// How often schedules and pending changes are checked.
const TICK: Duration = Duration::from_secs(2);
/// Quiet time after the last file change before a change-triggered run.
const CHANGE_DEBOUNCE: Duration = Duration::from_secs(30);

pub struct RunningBackup {
    pub op_id: usize,
    pub progress: Arc<Progress>,
}

#[derive(Default)]
pub struct BackupState {
    pub config: BackupConfig,
    /// Run history per job, most recent first.
    pub history: HashMap<String, Vec<RunRecord>>,
    pub running: HashMap<String, RunningBackup>,
    /// Runs by other processes (the background agent, the menu bar helper).
    pub external: HashMap<String, rile_backup::state::RunStatus>,
    /// Jobs whose sources changed since their last run: time of the last change.
    changed: Arc<Mutex<HashMap<String, Instant>>>,
    watcher: Option<notify::RecommendedWatcher>,
    /// Jobs whose card lists all runs, not just the recent ones.
    pub expanded: std::collections::HashSet<String>,
    /// Restores and deletions in progress: job id → op id.
    tasks: HashMap<String, usize>,
    /// Why the launch agent couldn't be installed or removed.
    pub agent_error: Option<String>,
    _tick: Option<Task<()>>,
}

impl BackupState {
    pub fn load() -> Self {
        let config = BackupConfig::load();
        let history = config.job.iter().map(|j| (j.id.clone(), load_history(&j.id))).collect();
        Self { config, history, ..Default::default() }
    }

    pub fn history(&self, id: &str) -> &[RunRecord] {
        self.history.get(id).map(Vec::as_slice).unwrap_or_default()
    }

    /// The job's running backup in this or another process.
    pub fn live(&self, id: &str) -> Option<rile_backup::state::RunStatus> {
        if let Some(run) = self.running.get(id) {
            // Rates come from the status the run publishes for other processes.
            let published = rile_backup::state::job_dir(id).and_then(|d| rile_backup::state::read_status(&d));
            let p = &run.progress;
            let load = |a: &std::sync::atomic::AtomicU64| a.load(Ordering::Relaxed);
            return Some(rile_backup::state::RunStatus {
                pid: std::process::id(),
                started: 0,
                trigger: Trigger::Manual,
                phase: p.phase().label().to_string(),
                scanned: load(&p.scanned),
                files_done: load(&p.files_done),
                files_total: load(&p.files_total),
                bytes_done: load(&p.bytes_done),
                bytes_total: load(&p.bytes_total),
                current: p.current.lock().map(|c| c.clone()).unwrap_or_default(),
                bytes_per_sec: published.as_ref().map_or(0., |s| s.bytes_per_sec),
                items_per_sec: published.as_ref().map_or(0., |s| s.items_per_sec),
            });
        }
        self.external.get(id).cloned()
    }

    /// Whether files changed and a change-triggered run is waiting.
    pub fn has_pending_changes(&self, id: &str) -> bool {
        self.changed.lock().is_ok_and(|c| c.contains_key(id))
    }
}

fn load_history(id: &str) -> Vec<RunRecord> {
    rile_backup::state::job_dir(id).map(|d| rile_backup::state::load_history(&d)).unwrap_or_default()
}

/// Connects to a saved server for a backup run.
fn connect(conn: &str) -> anyhow::Result<(Box<dyn rile_remote::RemoteFs>, String)> {
    crate::remote::connect_dedicated(conn)
}

fn to_ops_progress(p: &Progress) -> ops::Progress {
    let load = |a: &std::sync::atomic::AtomicU64| a.load(Ordering::Relaxed);
    ops::Progress {
        bytes_done: load(&p.bytes_done),
        bytes_total: load(&p.bytes_total),
        files_done: load(&p.files_done) as usize,
        files_total: load(&p.files_total) as usize,
        ..Default::default()
    }
}

/// One line describing a finished run.
pub fn describe(record: &RunRecord) -> String {
    let size = humansize::format_size(record.bytes, humansize::DECIMAL);
    let changes = match (record.copied, record.removed) {
        (0, 0) => "no changes".to_string(),
        (c, 0) => format!("{c} {} copied ({size})", if c == 1 { "file" } else { "files" }),
        (0, r) => format!("{r} removed"),
        (c, r) => format!("{c} copied ({size}), {r} removed"),
    };
    match &record.outcome {
        Outcome::Success => changes,
        Outcome::Partial(e) => format!("{changes} · {e}"),
        Outcome::Failed(e) => e.clone(),
        Outcome::Cancelled => "Cancelled".into(),
    }
}

impl Workspace {
    /// Starts the schedule/change ticker and the file watcher.
    pub(super) fn start_backups(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_backup_watcher();
        if !cfg!(test) {
            // Only schedules need an agent; the main app owns the unified tray.
            let config = self.backups.config.clone();
            cx.background_spawn(async move {
                if let Err(e) = rile_backup::agent::sync(&config) {
                    eprintln!("backup agents: {e:#}");
                }
            })
            .detach();
        }
        let task = cx.spawn_in(window, async move |this, cx| {
            // Let the app settle before catching up on missed runs.
            cx.background_executor().timer(Duration::from_secs(8)).await;
            loop {
                if this.update_in(cx, |ws, window, cx| ws.backup_tick(window, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(TICK).await;
            }
        });
        self.backups._tick = Some(task);
    }

    fn backup_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Follow runs of other processes; reload history when one ends.
        let mut external = HashMap::new();
        for job in &self.backups.config.job {
            if let Some(status) = rile_backup::state::job_dir(&job.id).and_then(|d| rile_backup::state::read_status(&d))
                && status.pid != std::process::id()
            {
                external.insert(job.id.clone(), status);
            }
        }
        let ended: Vec<String> = self.backups.external.keys().filter(|id| !external.contains_key(*id)).cloned().collect();
        for id in ended {
            self.backups.history.insert(id.clone(), load_history(&id));
        }
        if external != self.backups.external {
            self.backups.external = external;
            cx.notify();
        }
        let now = chrono::Utc::now().timestamp();
        let jobs: Vec<BackupJob> = self.backups.config.job.iter().filter(|j| j.enabled).cloned().collect();
        for job in jobs {
            if self.backups.running.contains_key(&job.id) || self.backups.external.contains_key(&job.id) || self.backup_task_running(&job.id) {
                continue;
            }
            if rile_backup::is_due(&job, self.backups.history(&job.id), now) {
                // The background agent may have run it meanwhile.
                let history = load_history(&job.id);
                let due = rile_backup::is_due(&job, &history, now);
                self.backups.history.insert(job.id.clone(), history);
                if due {
                    self.run_backup(&job.id, Trigger::Schedule, false, window, cx);
                    continue;
                }
            }
            let settled = self.backups.changed.lock().ok().and_then(|c| c.get(&job.id).copied()).is_some_and(|t| t.elapsed() >= CHANGE_DEBOUNCE);
            if job.watch && settled && job.problem().is_none() {
                self.run_backup(&job.id, Trigger::Change, false, window, cx);
            }
        }
    }

    /// Runs a job now (no-op while it runs). It is listed with the background
    /// jobs; manual runs also report success.
    pub fn run_backup(&mut self, id: &str, trigger: Trigger, verify: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.backups.running.contains_key(id) {
            return;
        }
        if self.backups.external.contains_key(id) || self.backup_task_running(id) {
            if trigger == Trigger::Manual {
                window.push_notification(Notification::info("This backup is busy right now"), cx);
            }
            return;
        }
        let Some(job) = self.backups.config.get(id).cloned() else { return };
        if let Some(problem) = job.problem() {
            if trigger == Trigger::Manual {
                window.push_notification(Notification::warning(problem), cx);
            }
            return;
        }
        if let Ok(mut c) = self.backups.changed.lock() {
            c.remove(id);
        }
        let op_id = self.next_op_id;
        self.next_op_id += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Progress::default());
        let work = {
            let (job, cancel, progress) = (job.clone(), cancel.clone(), progress.clone());
            cx.background_spawn(async move { rile_backup::run_job(&job, trigger, RunOptions { verify }, &connect, &cancel, &progress) })
        };
        let job_id = job.id.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            this.update_in(cx, |ws, window, cx| ws.finish_backup(&job_id, op_id, trigger, result, window, cx)).ok();
        });
        // Live progress for the jobs panel and the Backups page, until the
        // run's op is gone.
        let poll = progress.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(200)).await;
                let p = to_ops_progress(&poll);
                let running = this.update(cx, |ws, cx| {
                    let Some(op) = ws.ops.iter_mut().find(|o| o.id == op_id) else { return false };
                    op.update_progress(p);
                    if matches!(ws.pane().read(cx).location, Location::Backups) || ws.jobs_open {
                        cx.notify();
                    }
                    true
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        self.ops.push(RunningOp {
            connecting_to: None,
            id: op_id,
            label: SharedString::from(format!("Backing up “{}”", job.name)),
            progress: ops::Progress::default(),
            cancel,
            started: Instant::now(),
            // Backups show their progress on the Backups page and in the
            // menu bar, never in the transfer window.
            backgrounded: true,
            speed: 0.,
            last_sample: None,
            _task: task,
        });
        self.backups.running.insert(job.id.clone(), RunningBackup { op_id, progress });
        cx.notify();
    }

    fn finish_backup(&mut self, id: &str, op_id: usize, trigger: Trigger, result: anyhow::Result<RunRecord>, window: &mut Window, cx: &mut Context<Self>) {
        self.backups.running.remove(id);
        let name = self.backups.config.get(id).map(|j| j.name.clone()).unwrap_or_default();
        let error = match &result {
            Ok(r) => match &r.outcome {
                Outcome::Success => None,
                Outcome::Cancelled => Some("Cancelled".to_string()),
                _ => Some(describe(r)),
            },
            Err(e) => Some(format!("{e:#}")),
        };
        if let Some(ix) = self.ops.iter().position(|o| o.id == op_id) {
            let op = self.ops.remove(ix);
            self.finished_jobs.insert(0, FinishedJob { label: op.label, error: error.clone().map(Into::into), finished: Instant::now() });
            self.finished_jobs.truncate(6);
        }
        self.backups.history.insert(id.to_string(), load_history(id));
        match result {
            Ok(r) => match &r.outcome {
                Outcome::Success if trigger == Trigger::Manual => {
                    window.push_notification(Notification::success(format!("Backup “{name}” finished: {}", describe(&r))), cx);
                }
                Outcome::Partial(_) => {
                    window.push_notification(Notification::warning(format!("Backup “{name}” finished with problems: {}", describe(&r))), cx);
                }
                Outcome::Failed(e) => window.push_notification(Notification::error(format!("Backup “{name}” failed: {e}")), cx),
                _ => {}
            },
            // Couldn't even record the run (e.g. it was already running elsewhere).
            Err(e) if trigger == Trigger::Manual => window.push_notification(Notification::error(format!("Backup “{name}”: {e:#}")), cx),
            Err(e) => eprintln!("backup {name}: {e:#}"),
        }
        cx.notify();
    }

    pub fn cancel_backup(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(run) = self.backups.running.get(id) {
            self.cancel_op(run.op_id);
        } else if self.backups.external.contains_key(id)
            && let Some(dir) = rile_backup::state::job_dir(id)
        {
            rile_backup::state::request_cancel(&dir).ok();
        }
        cx.notify();
    }

    /// Adds a new job (with `sources` preselected) and returns its id.
    pub fn add_backup_job(&mut self, sources: Vec<PathBuf>, cx: &mut Context<Self>) -> String {
        let n = self.backups.config.job.len() + 1;
        let name = match sources.first().and_then(|s| s.file_name()) {
            Some(f) if sources.len() == 1 => f.to_string_lossy().into_owned(),
            _ if n == 1 => "My Backup".into(),
            _ => format!("Backup {n}"),
        };
        let job = BackupJob { name, sources, ..Default::default() };
        let id = job.id.clone();
        self.backups.config.job.push(job);
        self.backup_config_changed(cx);
        id
    }

    pub fn update_backup_job(&mut self, id: &str, cx: &mut Context<Self>, f: impl FnOnce(&mut BackupJob)) {
        let Some(job) = self.backups.config.get_mut(id) else { return };
        f(job);
        self.backup_config_changed(cx);
    }

    /// Shows or hides backup status in the shared Rile tray menu.
    pub fn set_backup_menu_bar(&mut self, on: bool, cx: &mut Context<Self>) {
        self.backups.config.menu_bar = on;
        self.backup_config_changed(cx);
    }

    /// Deletes the job's settings and bookkeeping; the backup itself stays.
    pub fn remove_backup_job(&mut self, id: &str, cx: &mut Context<Self>) {
        self.cancel_backup(id, cx);
        self.backups.config.job.retain(|j| j.id != id);
        self.backups.history.remove(id);
        if let Some(dir) = rile_backup::state::job_dir(id) {
            std::fs::remove_dir_all(dir).ok();
        }
        self.backup_config_changed(cx);
    }

    /// Saves the jobs and brings the watcher and the launch agent in line.
    fn backup_config_changed(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.backups.config.save() {
            eprintln!("saving backups: {e:#}");
        }
        self.sync_backup_watcher();
        // Tests must never touch the user's real launch agents.
        if cfg!(test) {
            cx.notify();
            return;
        }
        let config = self.backups.config.clone();
        let task = cx.background_spawn(async move { rile_backup::agent::sync(&config) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |ws, cx| {
                ws.backups.agent_error = result.err().map(|e| format!("{e:#}"));
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Watches the sources of every job with "back up when files change".
    fn sync_backup_watcher(&mut self) {
        self.backups.watcher = None;
        let watched: Vec<(String, PathBuf, Option<PathBuf>)> = self
            .backups
            .config
            .job
            .iter()
            .filter(|j| j.enabled && j.watch)
            .flat_map(|j| {
                let dest = match &j.destination {
                    Destination::Local { path } => Some(path.clone()),
                    Destination::Remote { .. } => None,
                };
                j.sources.iter().map(move |s| (j.id.clone(), s.clone(), dest.clone()))
            })
            .collect();
        if watched.is_empty() {
            return;
        }
        let changed = self.backups.changed.clone();
        let filter = watched.clone();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            if matches!(event.kind, notify::EventKind::Access(_)) {
                return;
            }
            let Ok(mut changed) = changed.lock() else { return };
            for path in &event.paths {
                for (id, source, dest) in &filter {
                    let ours = dest.as_ref().is_some_and(|d| path.starts_with(d));
                    if path.starts_with(source) && !ours {
                        changed.insert(id.clone(), Instant::now());
                    }
                }
            }
        });
        match watcher {
            Ok(mut w) => {
                for (_, source, _) in &watched {
                    let mode = if source.is_dir() { notify::RecursiveMode::Recursive } else { notify::RecursiveMode::NonRecursive };
                    if let Err(e) = w.watch(source, mode) {
                        eprintln!("not watching {} for backups: {e}", source.display());
                    }
                }
                self.backups.watcher = Some(w);
            }
            Err(e) => eprintln!("backup watcher: {e}"),
        }
    }

    /// Opens the backup's folder (or its versions) in the current pane.
    pub fn open_backup_location(&mut self, id: &str, versions: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(job) = self.backups.config.get(id) else { return };
        let sub = if versions { rile_backup::engine::VERSIONS_DIR } else { "" };
        let location = match &job.destination {
            Destination::Local { path } => {
                let dir = if sub.is_empty() { path.clone() } else { path.join(sub) };
                if !dir.is_dir() {
                    let what = if versions { "No older versions have been kept yet" } else { "The backup folder isn't available" };
                    window.push_notification(Notification::info(what), cx);
                    return;
                }
                Location::Dir(dir)
            }
            Destination::Remote { conn, path } => {
                let path = if sub.is_empty() { path.clone() } else { crate::remote::join(path, sub) };
                Location::Remote { conn: conn.clone(), path }
            }
        };
        self.open_location(location, window, cx);
    }
}

impl Workspace {
    /// Whether a restore or deletion is running for the job.
    pub fn backup_task_running(&self, id: &str) -> bool {
        self.backups.tasks.get(id).is_some_and(|op| self.ops.iter().any(|o| o.id == *op))
    }

    /// Runs management work on a job's backup in the background (listed with
    /// the other jobs, holding the job's lock so no backup runs meanwhile).
    /// `work` returns the success message; `then` runs after success.
    fn backup_task(
        &mut self,
        id: &str,
        label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
        work: impl FnOnce(&BackupJob, &mut dyn rile_backup::target::Target, &std::path::Path, &AtomicBool, &mut dyn FnMut(ops::Progress)) -> anyhow::Result<String>
            + Send
            + 'static,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if self.backups.running.contains_key(id) || self.backups.external.contains_key(id) || self.backup_task_running(id) {
            window.push_notification(Notification::info("Wait until the current backup activity has finished"), cx);
            return;
        }
        let Some(job) = self.backups.config.get(id).cloned() else { return };
        let message = Arc::new(Mutex::new(String::new()));
        let result_message = message.clone();
        let job_id = job.id.clone();
        let op_id = self.next_op_id;
        self.start_op_then(
            label,
            window,
            cx,
            move |cancel, report| {
                let dir = rile_backup::state::job_dir(&job.id).ok_or_else(|| anyhow::anyhow!("no config dir"))?;
                let _lock = rile_backup::state::Lock::acquire(&dir)?;
                let mut target = rile_backup::open_target(&job.destination, &connect)?;
                let msg = work(&job, target.as_mut(), &dir, cancel, report).map_err(|e| if rile_remote::is_cancelled(&e) { anyhow::anyhow!("Cancelled") } else { e })?;
                *message.lock().unwrap() = msg;
                Ok(Vec::new())
            },
            move |ws, window, cx| {
                let msg = std::mem::take(&mut *result_message.lock().unwrap());
                if !msg.is_empty() {
                    window.push_notification(Notification::success(msg), cx);
                }
                then(ws, window, cx);
            },
        );
        // Like backups, these never open the transfer window.
        if let Some(op) = self.ops.iter_mut().find(|o| o.id == op_id) {
            op.backgrounded = true;
        }
        self.backups.tasks.insert(job_id, op_id);
        cx.notify();
    }

    /// Copies the backup (or the files a run kept) back to this computer.
    pub fn restore_backup(&mut self, id: &str, from: rile_backup::manage::RestoreFrom, to: rile_backup::manage::RestoreTo, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.backups.config.get(id).map(|j| j.name.clone()).unwrap_or_default();
        self.backup_task(id, format!("Restoring “{name}”"), window, cx, move |job, target, _, cancel, report| {
            let s = rile_backup::manage::restore(job, target, &from, &to, rile_backup::manage::RestoreOptions { trash_replaced: true }, cancel, &mut |p| {
                report(ops::Progress {
                    files_done: p.files_done as usize,
                    files_total: p.files_total as usize,
                    bytes_done: p.bytes_done,
                    bytes_total: p.bytes_total,
                    ..Default::default()
                })
            })?;
            let mut msg = format!("Restored {} {}", s.restored, if s.restored == 1 { "file" } else { "files" });
            if s.skipped > 0 {
                msg.push_str(&format!(", {} already up to date", s.skipped));
            }
            if s.replaced > 0 {
                msg.push_str(&format!("; {} replaced {} moved to the {}", s.replaced, if s.replaced == 1 { "file was" } else { "files were" }, crate::platform_text::trash()));
            }
            if let Some(first) = s.errors.first() {
                anyhow::bail!("{msg}, but {} failed: {first}", s.error_count);
            }
            Ok(msg)
        }, |_, _, _| {});
    }

    /// Deletes a run from the history together with the versions it kept.
    pub fn delete_backup_run(&mut self, id: &str, run: RunRecord, window: &mut Window, cx: &mut Context<Self>) {
        let job_id = id.to_string();
        self.backup_task(id, "Deleting a backup run".into(), window, cx, move |job, target, dir, _, _| {
            rile_backup::manage::delete_run(job, target, dir, &run)?;
            Ok(String::new())
        }, move |ws, _, cx| {
            ws.backups.history.insert(job_id.clone(), load_history(&job_id));
            cx.notify();
        });
    }

    /// Deletes all older versions kept by the job.
    pub fn delete_backup_versions(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let job_id = id.to_string();
        self.backup_task(id, "Deleting older versions".into(), window, cx, move |job, target, dir, _, _| {
            rile_backup::manage::delete_all_versions(job, target, dir)?;
            Ok("Older versions deleted".into())
        }, move |ws, _, cx| {
            ws.backups.history.insert(job_id.clone(), load_history(&job_id));
            cx.notify();
        });
    }

    /// Removes the job; with `files`, deletes its backup at the destination first.
    pub fn delete_backup(&mut self, id: &str, files: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !files {
            self.remove_backup_job(id, cx);
            return;
        }
        let job_id = id.to_string();
        let name = self.backups.config.get(id).map(|j| j.name.clone()).unwrap_or_default();
        self.backup_task(id, format!("Deleting backup “{name}”"), window, cx, move |job, target, dir, _, _| {
            rile_backup::manage::delete_backup_data(job, target, dir)?;
            Ok(format!("Backup “{}” deleted", job.name))
        }, move |ws, _, cx| ws.remove_backup_job(&job_id, cx));
    }
}

/// `rile --run-backups`: run the due background jobs and exit (started by
/// the launch agent while the app may be closed).
pub fn run_agent() {
    crate::remote::load_saved();
    let config = BackupConfig::load();
    if !config.wants_agent() {
        // Nothing left to do in the background: remove ourselves.
        rile_backup::agent::uninstall(rile_backup::agent::Agent::Scheduler).ok();
        return;
    }
    let now = chrono::Utc::now().timestamp();
    for job in config.job.iter().filter(|j| j.background) {
        if !rile_backup::is_due(job, &load_history(&job.id), now) {
            continue;
        }
        let progress = Progress::default();
        match rile_backup::run_job(job, Trigger::Background, RunOptions::default(), &connect, &AtomicBool::new(false), &progress) {
            Ok(r) => eprintln!("{} backup \"{}\": {}", chrono::Local::now().format("%F %T"), job.name, describe(&r)),
            Err(e) => eprintln!("{} backup \"{}\": {e:#}", chrono::Local::now().format("%F %T"), job.name),
        }
    }
}
