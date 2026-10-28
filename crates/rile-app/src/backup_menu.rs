//! The backups menu bar item (`rile --backup-menu`): a small helper process,
//! started at login by its launch agent, that shows running backups from any
//! process (the app, the scheduler agent or itself), starts and stops them,
//! posts a notification when one fails, and opens Rile on the Backups page.
//!
//! Everything it shows comes from the files the backup engine shares in the
//! config folder (jobs, history, live status), so it never talks to the app.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

use rile_backup::state::{self, RunRecord, RunStatus};
use rile_backup::{BackupConfig, Outcome, Progress, RunOptions, Trigger};

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

impl Icon {
    fn symbol(self) -> &'static str {
        match self {
            Icon::Idle => "externaldrive.badge.timemachine",
            Icon::Running => "arrow.triangle.2.circlepath",
            Icon::Problem => "externaldrive.badge.exclamationmark",
        }
    }
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
            let rates = rile_backup::format_rates(s.bytes_per_sec, s.items_per_sec);
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

// ---------------------------------------------------------------- state

#[derive(Default)]
struct Tray {
    config: BackupConfig,
    history: HashMap<String, Vec<RunRecord>>,
    live: HashMap<String, RunStatus>,
    seen: HashMap<String, i64>,
    shown: Option<MenuModel>,
    /// Job ids in menu order, for item tags.
    order: Vec<String>,
}

impl Tray {
    fn refresh(&mut self) {
        self.config = BackupConfig::load();
        self.history.clear();
        self.live.clear();
        for job in &self.config.job {
            let Some(dir) = state::job_dir(&job.id) else { continue };
            self.history.insert(job.id.clone(), state::load_history(&dir));
            if let Some(status) = state::read_status(&dir) {
                self.live.insert(job.id.clone(), status);
            }
        }
        for (title, body) in new_problems(&self.config, &self.history, &mut self.seen) {
            notify(&title, &body);
        }
    }
}

/// Posts a macOS notification (via AppleScript, which works without an app
/// bundle or notification entitlement).
fn notify(title: &str, body: &str) {
    let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("display notification \"{}\" with title \"{}\"", quote(body), quote(title));
    std::process::Command::new("/usr/bin/osascript").args(["-e", &script]).spawn().ok();
}

/// Runs a job on a thread of this process; its progress reaches the menu
/// (and the app) through the shared status file.
/// Backups this process runs: their cancel flags and threads.
type OwnRun = (std::sync::Arc<AtomicBool>, std::thread::JoinHandle<()>);

fn own_runs() -> &'static std::sync::Mutex<Vec<OwnRun>> {
    static RUNS: std::sync::OnceLock<std::sync::Mutex<Vec<OwnRun>>> = std::sync::OnceLock::new();
    RUNS.get_or_init(Default::default)
}

fn start_job(id: &str) {
    let Some(job) = BackupConfig::load().get(id).cloned() else { return };
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let thread = std::thread::spawn(move || {
        let progress = Progress::default();
        let connect = |conn: &str| crate::remote::connect_dedicated(conn);
        if let Err(e) = rile_backup::run_job(&job, Trigger::Manual, RunOptions::default(), &connect, &flag, &progress) {
            notify(&format!("Backup “{}”", job.name), &format!("{e:#}"));
        }
    });
    if let Ok(mut runs) = own_runs().lock() {
        runs.retain(|(_, t)| !t.is_finished());
        runs.push((cancel, thread));
    }
}

/// "Quit Backup Menu", noting backups that quitting would stop.
fn quit_title() -> String {
    let running = own_runs().lock().map_or(0, |r| r.iter().filter(|(_, t)| !t.is_finished()).count());
    match running {
        0 => "Quit Backup Menu".into(),
        1 => "Quit Backup Menu (stops 1 backup)".into(),
        n => format!("Quit Backup Menu (stops {n} backups)"),
    }
}

/// Exits after stopping the backups this process runs, so they end as
/// "Cancelled" (with what they copied kept) instead of being killed.
/// Backups run by the app or the scheduler are not affected.
fn exit_cleanly() -> ! {
    let runs = own_runs().lock().map(|mut r| std::mem::take(&mut *r)).unwrap_or_default();
    for (cancel, _) in &runs {
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while runs.iter().any(|(_, t)| !t.is_finished()) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // A successful exit: launchd doesn't restart it (see the agent's KeepAlive).
    std::process::exit(0)
}

fn stop_job(id: &str) {
    if let Some(dir) = state::job_dir(id) {
        state::request_cancel(&dir).ok();
    }
}

/// Opens Rile at `rile://<page>`: through the URL scheme when running from
/// the app bundle, else by starting this binary on the Backups page.
fn open_rile(page: &str) {
    let exe = std::env::current_exe().unwrap_or_default();
    if exe.to_string_lossy().contains(".app/Contents/MacOS/") {
        std::process::Command::new("/usr/bin/open").arg(format!("rile://{page}")).spawn().ok();
    } else {
        std::process::Command::new(exe).arg("--backups").spawn().ok();
    }
}

// ---------------------------------------------------------------- AppKit

mod mac {
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Sel};
    use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem};
    use objc2_foundation::{MainThreadMarker, NSObject, NSRunLoop, NSRunLoopCommonModes, NSString, NSTimer};

    use super::*;

    pub struct Ivars {
        tray: RefCell<Tray>,
        item: Retained<NSStatusItem>,
        menu: Retained<NSMenu>,
        items: RefCell<Option<Items>>,
    }

    /// A job's entry, its detail lines and its action.
    type JobItems = (Retained<NSMenuItem>, Vec<Retained<NSMenuItem>>, Retained<NSMenuItem>);

    /// The menu's items that change while it is shown.
    struct Items {
        shape: Vec<(String, bool, usize)>,
        header: Retained<NSMenuItem>,
        jobs: Vec<JobItems>,
        all: Retained<NSMenuItem>,
        quit: Retained<NSMenuItem>,
    }

    define_class!(
        // SAFETY: NSObject has no subclass requirements. Access is main-thread only.
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[name = "RileBackupMenu"]
        #[ivars = Ivars]
        pub struct Menu;

        impl Menu {
            #[unsafe(method(tick:))]
            fn tick(&self, _timer: &NSTimer) {
                self.update();
            }

            #[unsafe(method(runJob:))]
            fn run_job(&self, item: &NSMenuItem) {
                if let Some(id) = self.job_id(item) {
                    start_job(&id);
                }
                self.update();
            }

            #[unsafe(method(stopJob:))]
            fn stop_job(&self, item: &NSMenuItem) {
                if let Some(id) = self.job_id(item) {
                    stop_job(&id);
                }
            }

            #[unsafe(method(runAll:))]
            fn run_all(&self, _item: &NSMenuItem) {
                let ids: Vec<String> = self.ivars().tray.borrow().shown.iter().flat_map(|m| m.jobs.iter()).filter(|j| j.can_run).map(|j| j.id.clone()).collect();
                for id in ids {
                    start_job(&id);
                }
                self.update();
            }

            #[unsafe(method(openBackups:))]
            fn open_backups(&self, _item: &NSMenuItem) {
                open_rile("backups");
            }

            #[unsafe(method(openSettings:))]
            fn open_settings(&self, _item: &NSMenuItem) {
                open_rile("backup-settings");
            }

            #[unsafe(method(hide:))]
            fn hide(&self, _item: &NSMenuItem) {
                // Turning it off in the config also removes the login item.
                let mut config = BackupConfig::load();
                config.menu_bar = false;
                config.save().ok();
                rile_backup::agent::uninstall(rile_backup::agent::Agent::MenuBar).ok();
                exit_cleanly();
            }

            #[unsafe(method(quit:))]
            fn quit(&self, _item: &NSMenuItem) {
                // Stays a login item: back at the next login or Rile launch.
                exit_cleanly();
            }
        }
    );

    impl Menu {
        fn job_id(&self, item: &NSMenuItem) -> Option<String> {
            self.ivars().tray.borrow().order.get(usize::try_from(item.tag()).ok()?).cloned()
        }

        /// Reloads the shared state and rebuilds the menu when it changed.
        fn update(&self) {
            let mtm = self.mtm();
            let mut tray = self.ivars().tray.borrow_mut();
            tray.refresh();
            if !tray.config.wants_menu_bar() {
                // Turned off in Settings, or no jobs left.
                std::process::exit(0);
            }
            let model = model(&tray.config, &tray.history, &tray.live, chrono::Utc::now().timestamp());
            if tray.shown.as_ref() == Some(&model) {
                return;
            }
            tray.order = model.jobs.iter().map(|j| j.id.clone()).collect();
            let ivars = self.ivars();
            if let Some(button) = ivars.item.button(mtm) {
                let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(model.icon.symbol()), Some(&NSString::from_str("Backups")));
                if let Some(image) = &image {
                    image.setTemplate(true);
                }
                button.setImage(image.as_deref());
                button.setToolTip(Some(&NSString::from_str(if model.title.is_empty() { "Backups" } else { &model.title })));
            }
            // Rebuilding an open menu leaves AppKit with a stale height (an
            // empty row at the bottom); retitle the existing items instead.
            let mut items = ivars.items.borrow_mut();
            match items.as_ref() {
                Some(current) if current.shape == model.shape() => Self::retitle(current, &model),
                _ => *items = Some(self.build(&ivars.menu, &model, mtm)),
            }
            tray.shown = Some(model);
        }

        fn item(&self, title: &str, action: Option<Sel>, mtm: MainThreadMarker) -> Retained<NSMenuItem> {
            // SAFETY: The selectors are implemented by `self`, which outlives the menu.
            unsafe {
                let item = NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &NSString::from_str(title), action, &NSString::new());
                if action.is_some() {
                    item.setTarget(Some(self as &AnyObject));
                } else {
                    item.setEnabled(false);
                }
                item
            }
        }

        fn header_title(model: &MenuModel) -> &str {
            if model.title.is_empty() { "Backups" } else { &model.title }
        }

        fn retitle(items: &Items, model: &MenuModel) {
            items.header.setTitle(&NSString::from_str(Self::header_title(model)));
            for ((entry, lines, action), job) in items.jobs.iter().zip(&model.jobs) {
                entry.setTitle(&NSString::from_str(&job.title));
                for (line, text) in lines.iter().zip(&job.details) {
                    line.setTitle(&NSString::from_str(text));
                }
                if !job.running {
                    action.setEnabled(job.can_run);
                }
            }
            items.all.setEnabled(model.jobs.iter().any(|j| j.can_run));
            items.quit.setTitle(&NSString::from_str(&quit_title()));
        }

        fn build(&self, menu: &NSMenu, model: &MenuModel, mtm: MainThreadMarker) -> Items {
            menu.removeAllItems();
            let header = self.item(Self::header_title(model), None, mtm);
            menu.addItem(&header);
            let mut jobs = Vec::new();
            for (i, job) in model.jobs.iter().enumerate() {
                let entry = self.item(&job.title, Some(sel!(openBackups:)), mtm);
                let sub = NSMenu::new(mtm);
                sub.setAutoenablesItems(false);
                let lines: Vec<Retained<NSMenuItem>> = job.details.iter().map(|line| self.item(line, None, mtm)).collect();
                for line in &lines {
                    sub.addItem(line);
                }
                if !lines.is_empty() {
                    sub.addItem(&NSMenuItem::separatorItem(mtm));
                }
                let action = if job.running {
                    self.item("Stop", Some(sel!(stopJob:)), mtm)
                } else {
                    let run = self.item("Back Up Now", Some(sel!(runJob:)), mtm);
                    run.setEnabled(job.can_run);
                    run
                };
                action.setTag(i as isize);
                sub.addItem(&action);
                entry.setSubmenu(Some(&sub));
                menu.addItem(&entry);
                jobs.push((entry, lines, action));
            }
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            let all = self.item("Back Up All Now", Some(sel!(runAll:)), mtm);
            all.setEnabled(model.jobs.iter().any(|j| j.can_run));
            menu.addItem(&all);
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            menu.addItem(&self.item("Open Backups…", Some(sel!(openBackups:)), mtm));
            menu.addItem(&self.item("Backup Settings…", Some(sel!(openSettings:)), mtm));
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            menu.addItem(&self.item("Hide from Menu Bar", Some(sel!(hide:)), mtm));
            let quit = self.item(&quit_title(), Some(sel!(quit:)), mtm);
            quit.setKeyEquivalent(&NSString::from_str("q"));
            menu.addItem(&quit);
            Items { shape: model.shape(), header, jobs, all, quit }
        }
    }

    pub fn run() {
        let mtm = MainThreadMarker::new().expect("main thread");
        let app = NSApplication::sharedApplication(mtm);
        // A menu bar item only: no Dock icon, no menu bar of its own.
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(-2.0); // NSSquareStatusItemLength: a constant width
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        item.setMenu(Some(&menu));
        let this = Menu::alloc(mtm).set_ivars(Ivars { tray: RefCell::new(Tray::default()), item, menu, items: RefCell::new(None) });
        // SAFETY: Initialize the NSObject superclass.
        let this: Retained<Menu> = unsafe { msg_send![super(this), init] };
        this.update();
        // SAFETY: `tick:` has the timer-callback signature; common modes keep
        // the menu live while it is open.
        unsafe {
            let timer = NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(1.0, &this, sel!(tick:), None, true);
            NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
        }
        app.run();
    }
}

pub fn run() {
    // One menu bar item, however often it gets started.
    let Some(dir) = state::job_dir("menu") else { return };
    let Ok(_lock) = state::Lock::acquire(&dir) else { return };
    crate::remote::load_saved();
    if !BackupConfig::load().wants_menu_bar() {
        rile_backup::agent::uninstall(rile_backup::agent::Agent::MenuBar).ok();
        return;
    }
    mac::run();
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
        assert_eq!(busy.jobs[0].details, ["12.4 MB/s · 85 items/s", "1 of 4 changes", "Docs/a.txt"]);
        // Progress changes text, never the menu's items.
        let starting = RunStatus { bytes_per_sec: 0., items_per_sec: 0., files_total: 0, current: String::new(), ..status(0, 0) };
        let early = model(&config, &history, &HashMap::from([("a".to_string(), starting)]), now);
        assert_eq!(early.shape(), busy.shape());
        assert_eq!(early.jobs[0].details, ["Measuring speed…", "10 files found", "Copying…"]);
    }

    #[test]
    fn quit_names_the_backups_it_would_stop() {
        assert_eq!(quit_title(), "Quit Backup Menu");
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let thread = std::thread::spawn(move || {
            while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        });
        own_runs().lock().unwrap().push((cancel.clone(), thread));
        assert_eq!(quit_title(), "Quit Backup Menu (stops 1 backup)");
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        let (_, thread) = own_runs().lock().unwrap().pop().unwrap();
        thread.join().unwrap();
        assert_eq!(quit_title(), "Quit Backup Menu");
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
