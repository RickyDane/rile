//! Backup status and actions in the main app's single tray/menu bar item.
//! Shared status files also expose jobs run by the headless scheduler.
use std::collections::HashMap;

use rile_backup::state::{self, RunRecord, RunStatus};
use rile_backup::{BackupConfig, Outcome};
#[cfg(test)]
use rile_backup::Trigger;

use crate::workspace::backups::describe;

// ---------------------------------------------------------------- model

/// What the menu shows, derived from the shared files (no AppKit, testable).
#[derive(Debug, Clone, PartialEq)]
pub struct MenuModel {
    pub icon: Icon,
    /// Headline of the menu (overall progress while backing up). The menu
    /// bar item itself stays icon-only: a changing width pushes it into the
    /// overflow on crowded menu bars.
    pub title: String,
    pub jobs: Vec<JobEntry>,
}

impl MenuModel {
    /// What decides the menu's items (not their titles): while it stays the
    /// same, the open menu is updated in place instead of rebuilt.
    pub fn shape(&self) -> Vec<(String, bool, usize)> {
        self.jobs.iter().map(|j| (j.id.clone(), j.running, j.details.len())).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Idle,
    Running,
    Problem,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobEntry {
    pub id: String,
    pub title: String,
    /// Read-only lines in the job's submenu.
    pub details: Vec<String>,
    pub running: bool,
    /// Can be started now (configured and idle).
    pub can_run: bool,
}

fn ago(ts: i64, now: i64) -> String {
    match (now - ts).max(0) {
        0..60 => "just now".into(),
        d @ 60..3600 => format!("{} min ago", d / 60),
        d @ 3600..86_400 => format!("{} h ago", d / 3600),
        d => format!("{} days ago", d / 86_400),
    }
}

fn until(ts: i64, now: i64) -> String {
    match ts - now {
        ..60 => "soon".into(),
        d @ 60..3600 => format!("in {} min", d / 60),
        d @ 3600..86_400 => format!("in {} h", d / 3600),
        d => format!("in {} days", d / 86_400),
    }
}

fn percent(status: &RunStatus) -> Option<u32> {
    status.fraction().map(|f| (f * 100.).round() as u32)
}

pub fn model(config: &BackupConfig, history: &HashMap<String, Vec<RunRecord>>, live: &HashMap<String, RunStatus>, now: i64) -> MenuModel {
    let mut jobs = Vec::new();
    let mut problem = false;
    for job in &config.job {
        let runs = history.get(&job.id).map(Vec::as_slice).unwrap_or_default();
        let status = live.get(&job.id);
        let state = if let Some(s) = status {
            match percent(s) {
                Some(p) => format!("{} {p}%", s.phase),
                None => format!("{}…", s.phase),
            }
        } else if let Some(p) = job.problem() {
            p.to_string()
        } else if !job.enabled {
            "Paused".into()
        } else {
            match runs.first() {
                None => "Not backed up yet".into(),
                Some(r) => {
                    let what = match &r.outcome {
                        Outcome::Success => "Up to date",
                        Outcome::Partial(_) => "Some files skipped",
                        Outcome::Failed(_) => "Failed",
                        Outcome::Cancelled => "Cancelled",
                    };
                    format!("{what} · {}", ago(r.finished, now))
                }
            }
        };
        if status.is_none() && runs.first().is_some_and(|r| matches!(r.outcome, Outcome::Failed(_) | Outcome::Partial(_))) {
            problem = true;
        }
        let mut details = Vec::new();
        if let Some(s) = status {
            // Always the same three lines while running, so the open menu
            // only changes text (see `MenuModel::shape`).
            let mut rates = rile_backup::format_rates(s.bytes_per_sec, s.items_per_sec);
            if let Some(eta) = s.eta() {
                if !rates.is_empty() { rates.push_str(" · "); }
                rates.push_str(&format!("~{eta}"));
            }
            details.push(if rates.is_empty() { "Measuring speed…".into() } else { rates });
            details.push(if s.files_total > 0 {
                format!("{} of {} changes", s.files_done, s.files_total)
            } else {
                format!("{} files found", s.scanned)
            });
            details.push(if s.current.is_empty() { format!("{}…", s.phase) } else { s.current.clone() });
        } else if let Some(r) = runs.first() {
            details.push(describe(r));
        }
        if status.is_none()
            && let Some(next) = rile_backup::next_run(job, runs)
        {
            details.push(format!("Next backup {}", until(next, now)));
        }
        jobs.push(JobEntry {
            id: job.id.clone(),
            title: format!("{} — {state}", job.name),
            details,
            running: status.is_some(),
            can_run: status.is_none() && job.problem().is_none(),
        });
    }
    let running: Vec<&RunStatus> = config.job.iter().filter_map(|j| live.get(&j.id)).collect();
    let (icon, title) = if running.is_empty() {
        (if problem { Icon::Problem } else { Icon::Idle }, String::new())
    } else {
        // Overall progress of everything running.
        let (done, total) = running.iter().fold((0u64, 0u64), |(d, t), s| (d + s.bytes_done, t + s.bytes_total));
        let what = if running.len() == 1 { "Backing up".to_string() } else { format!("Backing up {} jobs", running.len()) };
        let mut title = if total > 0 { format!("{what} — {}%", (done as f64 / total as f64 * 100.).round() as u32) } else { format!("{what}…") };
        let rates = rile_backup::format_rates(running.iter().map(|s| s.bytes_per_sec).sum(), running.iter().map(|s| s.items_per_sec).sum());
        if !rates.is_empty() {
            title = format!("{title} · {rates}");
        }
        (Icon::Running, title)
    };
    MenuModel { icon, title, jobs }
}

/// Finished runs that deserve a notification: failures and partial runs
/// newer than `seen` (the newest finish time already handled per job).
pub fn new_problems(config: &BackupConfig, history: &HashMap<String, Vec<RunRecord>>, seen: &mut HashMap<String, i64>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for job in &config.job {
        let Some(latest) = history.get(&job.id).and_then(|h| h.first()) else { continue };
        let previous = seen.insert(job.id.clone(), latest.finished);
        // The first look only records where we are.
        if previous.is_some_and(|p| latest.finished > p) {
            match &latest.outcome {
                Outcome::Failed(e) => out.push((format!("Backup “{}” failed", job.name), e.clone())),
                Outcome::Partial(_) => out.push((format!("Backup “{}” skipped some files", job.name), describe(latest))),
                _ => {}
            }
        }
    }
    out
}

// ---------------------------------------------------------------- shared app menu

use tray_icon::menu::{MenuItem, PredefinedMenuItem, Submenu};

/// Native handles are retained so progress updates don't rebuild an open menu.
pub struct BackupMenu {
    pub menu: Submenu,
    shown: Option<MenuModel>,
    header: MenuItem,
    jobs: Vec<(Submenu, Vec<MenuItem>, MenuItem)>,
    all: MenuItem,
}

impl BackupMenu {
    pub fn new() -> Self {
        Self {
            menu: Submenu::new("Backups", true),
            shown: None,
            header: MenuItem::new("Backups", false, None),
            jobs: Vec::new(),
            all: MenuItem::with_id("rile.backup.all", "Back Up All Now", false, None),
        }
    }

    pub fn update(&mut self, model: &MenuModel) -> anyhow::Result<()> {
        if self.shown.as_ref() == Some(model) {
            return Ok(());
        }
        if self.shown.as_ref().is_none_or(|old| old.shape() != model.shape()) {
            while self.menu.remove_at(0).is_some() {}
            self.jobs.clear();
            self.menu.append(&self.header)?;
            self.menu.append(&PredefinedMenuItem::separator())?;
            for job in &model.jobs {
                let sub = Submenu::new(&job.title, true);
                let details: Vec<MenuItem> = job.details.iter().map(|line| MenuItem::new(line, false, None)).collect();
                for line in &details {
                    sub.append(line)?;
                }
                sub.append(&PredefinedMenuItem::separator())?;
                let action = if job.running {
                    MenuItem::with_id(format!("rile.backup.stop:{}", job.id), "Stop", true, None)
                } else {
                    MenuItem::with_id(format!("rile.backup.run:{}", job.id), "Back Up Now", job.can_run, None)
                };
                sub.append(&action)?;
                self.menu.append(&sub)?;
                self.jobs.push((sub, details, action));
            }
            self.menu.append_items(&[
                &PredefinedMenuItem::separator(),
                &self.all,
                &PredefinedMenuItem::separator(),
                &MenuItem::with_id("rile.backup.open", "Open Backups…", true, None),
                &MenuItem::with_id("rile.backup.settings", "Backup Settings…", true, None),
            ])?;
        }
        self.header.set_text(if model.title.is_empty() { "Backups" } else { &model.title });
        self.menu.set_text(match model.icon {
            Icon::Running => "Backups — Running",
            Icon::Problem => "Backups — Needs attention",
            Icon::Idle => "Backups",
        });
        for ((sub, lines, action), job) in self.jobs.iter().zip(&model.jobs) {
            sub.set_text(&job.title);
            for (item, title) in lines.iter().zip(&job.details) {
                item.set_text(title);
            }
            action.set_enabled(job.running || job.can_run);
        }
        self.all.set_enabled(model.jobs.iter().any(|job| job.can_run));
        self.shown = Some(model.clone());
        Ok(())
    }
}

#[derive(Default)]
pub struct Monitor {
    seen: HashMap<String, i64>,
    retired: bool,
    pub handoff_pending: bool,
}

impl Monitor {
    /// Reads shared status off the UI thread, including scheduler-owned runs.
    pub fn refresh(&mut self) -> MenuModel {
        let mut config = BackupConfig::load();
        if !self.retired {
            match retire_legacy_helper(&config) {
                Ok(retired) => {
                    self.retired = retired;
                    self.handoff_pending = !retired;
                }
                Err(error) => {
                    self.handoff_pending = false;
                    eprintln!("Retiring the old backup menu: {error:#}");
                }
            }
        }
        let mut history = HashMap::new();
        let mut live = HashMap::new();
        for job in &config.job {
            let Some(dir) = state::job_dir(&job.id) else { continue };
            history.insert(job.id.clone(), state::load_history(&dir));
            if let Some(status) = state::read_status(&dir) {
                live.insert(job.id.clone(), status);
            }
        }
        if config.menu_bar {
            for (title, body) in new_problems(&config, &history, &mut self.seen) {
                notify(&title, &body);
            }
        } else {
            config.job.clear();
        }
        model(&config, &history, &live, chrono::Utc::now().timestamp())
    }
}

/// Retire the old login registration/process only when its own backups are idle.
/// Scheduler jobs and their login-independent agent are deliberately untouched.
fn retire_legacy_helper(config: &BackupConfig) -> anyhow::Result<bool> {
    use rile_backup::agent::{self, Agent};
    if std::env::var_os("RILE_NO_LAUNCH_AGENTS").is_some() {
        return Ok(true);
    }
    let lock = state::job_dir("menu").map(|dir| dir.join("lock"));
    let pid = lock.as_ref().and_then(|path| std::fs::read_to_string(path).ok()).and_then(|s| s.trim().parse::<u32>().ok());
    let helper = pid.filter(|pid| *pid != std::process::id() && legacy_helper(*pid, |_| ()).is_some());
    let dirs: Vec<_> = config.job.iter().filter_map(|job| state::job_dir(&job.id)).collect();
    // Locks precede live status (for example while connecting to a target).
    let locked = dirs.iter().any(|dir| helper_owns_lock(helper, dir));
    let statuses = dirs.iter().filter_map(|dir| state::read_status(dir));
    if locked || helper_has_runs(helper, statuses) {
        return Ok(false);
    }
    // Preserve the old helper's login behavior when migrating an existing user.
    if agent::is_installed(Agent::MenuBar) && config.wants_menu_bar() && !rile_services::autostart::is_enabled() {
        rile_services::autostart::set_enabled(true)?;
    }
    agent::uninstall(Agent::MenuBar)?;
    // Windows removes the Run entry but doesn't stop its already-running helper.
    // Validate its command line before acting on a possibly stale/reused pid.
    if let Some(pid) = helper {
        anyhow::ensure!(legacy_helper(pid, |process| process.kill()) != Some(false), "couldn't stop the idle legacy backup helper");
    }
    Ok(true)
}

fn legacy_helper<T>(pid: u32, use_process: impl FnOnce(&sysinfo::Process) -> T) -> Option<T> {
    let system = sysinfo::System::new_with_specifics(
        sysinfo::RefreshKind::nothing().with_processes(sysinfo::ProcessRefreshKind::nothing().with_cmd(sysinfo::UpdateKind::Always)),
    );
    let process = system.process(sysinfo::Pid::from_u32(pid))?;
    let own_name = std::env::current_exe().ok()?.file_name()?.to_owned();
    let matches_exe = process.cmd().first().and_then(|arg| std::path::Path::new(arg).file_name()) == Some(own_name.as_os_str());
    (matches_exe && process.cmd().iter().any(|arg| arg == rile_backup::agent::MENU_FLAG)).then(|| use_process(process))
}

fn helper_has_runs(pid: Option<u32>, mut statuses: impl Iterator<Item = RunStatus>) -> bool {
    pid.is_some_and(|pid| statuses.any(|status| status.pid == pid))
}

fn helper_owns_lock(pid: Option<u32>, dir: &std::path::Path) -> bool {
    pid.is_some_and(|pid| std::fs::read_to_string(dir.join("lock")).ok()
        .and_then(|text| text.trim().parse::<u32>().ok()) == Some(pid))
}

pub fn quit_title(running: usize) -> String {
    match running {
        0 => "Quit Rile".into(),
        1 => "Quit Rile (stops 1 backup)".into(),
        n => format!("Quit Rile (stops {n} backups)"),
    }
}

/// Uses stable job ids, not menu indices: configuration can change while open.
pub fn dispatch(id: &str, cx: &mut gpui_kit::App) {
    match id {
        "rile.backup.open" => return crate::show_backups(false, cx),
        "rile.backup.settings" => return crate::show_backups(true, cx),
        _ => {},
    }
    let Some((handle, ws)) = cx.try_global::<crate::MainWindow>()
        .and_then(|main| main.1.upgrade().map(|ws| (main.0, ws))) else { return };
    handle.update(cx, |_, window, cx| {
        ws.update(cx, |ws, cx| {
            if id == "rile.backup.all" {
                let ids: Vec<String> = ws.backups.config.job.iter()
                    .filter(|job| job.problem().is_none() && ws.backups.live(&job.id).is_none() && !ws.backup_task_running(&job.id))
                    .map(|job| job.id.clone()).collect();
                for id in ids {
                    ws.run_backup(&id, rile_backup::Trigger::Manual, false, window, cx);
                }
            } else if let Some(id) = id.strip_prefix("rile.backup.run:") {
                ws.run_backup(id, rile_backup::Trigger::Manual, false, window, cx);
            } else if let Some(id) = id.strip_prefix("rile.backup.stop:") {
                ws.cancel_backup(id, cx);
                if let Some(dir) = state::job_dir(id) {
                    state::request_cancel(&dir).ok();
                }
            }
        });
    }).ok();
}

#[cfg(target_os = "macos")]
fn notify(title: &str, body: &str) {
    let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("display notification \"{}\" with title \"{}\"", quote(body), quote(title));
    std::process::Command::new("/usr/bin/osascript").args(["-e", &script]).spawn().ok();
}

#[cfg(windows)]
fn notify(title: &str, body: &str) {
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager, ToastTemplateType};
    use windows::core::HSTRING;
    let show = || -> windows::core::Result<()> {
        let xml = ToastNotificationManager::GetTemplateContent(ToastTemplateType::ToastText02)?;
        let texts = xml.GetElementsByTagName(&HSTRING::from("text"))?;
        texts.Item(0)?.AppendChild(&xml.CreateTextNode(&HSTRING::from(title))?)?;
        texts.Item(1)?.AppendChild(&xml.CreateTextNode(&HSTRING::from(body))?)?;
        let toast = ToastNotification::CreateToastNotification(&xml)?;
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from("dev.rile.Rile"))?.Show(&toast)
    };
    show().ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use rile_backup::{BackupJob, Destination, Schedule};

    fn job(id: &str, name: &str) -> BackupJob {
        BackupJob {
            id: id.into(),
            name: name.into(),
            sources: vec!["/src".into()],
            destination: Destination::Local { path: "/dest".into() },
            schedule: Schedule::Daily,
            ..Default::default()
        }
    }

    fn record(finished: i64, outcome: Outcome) -> RunRecord {
        RunRecord { id: String::new(), started: finished, finished, trigger: Trigger::Schedule, outcome, copied: 3, removed: 0, bytes: 3000, files: 10, versions: None }
    }

    fn status(done: u64, total: u64) -> RunStatus {
        RunStatus { pid: 1, started: 0, trigger: Trigger::Manual, phase: "Copying".into(), scanned: 10, files_done: 1, files_total: 4, bytes_done: done, bytes_total: total, current: "Docs/a.txt".into(), bytes_per_sec: 12_400_000., items_per_sec: 85. }
    }

    #[test]
    fn menu_shows_progress_results_and_problems() {
        let config = BackupConfig { menu_bar: true, job: vec![job("a", "Work"), job("b", "Photos"), BackupJob { sources: vec![], ..job("c", "New") }] };
        let now = 100_000;
        let mut history = HashMap::new();
        history.insert("a".to_string(), vec![record(now - 7200, Outcome::Success)]);
        history.insert("b".to_string(), vec![record(now - 60, Outcome::Failed("disk full".into()))]);

        let idle = model(&config, &history, &HashMap::new(), now);
        assert_eq!(idle.icon, Icon::Problem);
        assert_eq!(idle.jobs[0].title, "Work — Up to date · 2 h ago");
        assert!(idle.jobs[0].details.iter().any(|d| d == "Next backup in 22 h"));
        assert_eq!(idle.jobs[1].title, "Photos — Failed · 1 min ago");
        assert!(!idle.jobs[2].can_run, "unconfigured jobs can't run");

        let live = HashMap::from([("a".to_string(), status(250, 1000))]);
        let busy = model(&config, &history, &live, now);
        assert_eq!((busy.icon, busy.title.as_str()), (Icon::Running, "Backing up — 25% · 12.4 MB/s · 85 items/s"));
        assert_eq!(busy.jobs[0].title, "Work — Copying 25%");
        assert!(busy.jobs[0].running && !busy.jobs[0].can_run);
        assert_eq!(busy.jobs[0].details, ["12.4 MB/s · 85 items/s · ~1 s left", "1 of 4 changes", "Docs/a.txt"]);
        // Progress changes text, never the menu's items.
        let starting = RunStatus { bytes_per_sec: 0., items_per_sec: 0., files_total: 0, current: String::new(), ..status(0, 0) };
        let early = model(&config, &history, &HashMap::from([("a".to_string(), starting)]), now);
        assert_eq!(early.shape(), busy.shape());
        assert_eq!(early.jobs[0].details, ["Measuring speed…", "10 files found", "Copying…"]);
    }

    #[test]
    fn quit_names_the_backups_it_would_stop() {
        assert_eq!(quit_title(0), "Quit Rile");
        assert_eq!(quit_title(1), "Quit Rile (stops 1 backup)");
        assert_eq!(quit_title(3), "Quit Rile (stops 3 backups)");
    }

    #[test]
    fn handoff_waits_only_for_backups_owned_by_the_legacy_helper() {
        let live = status(1, 10);
        assert!(helper_has_runs(Some(live.pid), std::iter::once(live.clone())));
        assert!(!helper_has_runs(Some(live.pid + 1), std::iter::once(live.clone())));
        assert!(!helper_has_runs(None, std::iter::once(live)));
        assert!(!helper_has_runs(Some(42), std::iter::empty()));
    }

    #[test]
    fn handoff_waits_for_the_job_lock_before_progress_is_published() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lock"), "42").unwrap();
        assert!(helper_owns_lock(Some(42), dir.path()));
        assert!(!helper_owns_lock(Some(43), dir.path()));
        assert!(!helper_owns_lock(None, dir.path()));
        std::fs::remove_file(dir.path().join("lock")).unwrap();
        assert!(!helper_owns_lock(Some(42), dir.path()));
    }

    #[test]
    fn only_new_failures_are_notified() {
        let config = BackupConfig { menu_bar: true, job: vec![job("a", "Work")] };
        let mut seen = HashMap::new();
        let mut history = HashMap::from([("a".to_string(), vec![record(10, Outcome::Failed("old".into()))])]);
        assert!(new_problems(&config, &history, &mut seen).is_empty(), "failures from before start are not repeated");
        history.get_mut("a").unwrap().insert(0, record(20, Outcome::Success));
        assert!(new_problems(&config, &history, &mut seen).is_empty());
        history.get_mut("a").unwrap().insert(0, record(30, Outcome::Failed("disk full".into())));
        assert_eq!(new_problems(&config, &history, &mut seen), [("Backup “Work” failed".to_string(), "disk full".to_string())]);
        assert!(new_problems(&config, &history, &mut seen).is_empty());
    }
}
