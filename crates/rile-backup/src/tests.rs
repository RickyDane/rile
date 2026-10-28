use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use super::*;

struct Fixture {
    _tmp: tempfile::TempDir,
    src: PathBuf,
    dest: PathBuf,
    state: PathBuf,
    job: BackupJob,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("Projects");
    let dest = tmp.path().join("Backup");
    let state = tmp.path().join("state");
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::write(src.join("a.txt"), "alpha").unwrap();
    fs::write(src.join("sub/b.txt"), "bravo").unwrap();
    fs::write(src.join("skip.tmp"), "x").unwrap();
    let job = BackupJob {
        name: "Test".into(),
        sources: vec![src.clone()],
        destination: Destination::Local { path: dest.clone() },
        excludes: vec!["*.tmp".into()],
        ..Default::default()
    };
    Fixture { _tmp: tmp, src, dest, state, job }
}

fn no_remote(_: &str) -> Result<(Box<dyn RemoteFs>, String)> {
    anyhow::bail!("no remote in tests")
}

fn run(f: &Fixture, verify: bool) -> RunRecord {
    run_job_in(&f.job, &f.state, Trigger::Manual, RunOptions { verify }, &no_remote, &AtomicBool::new(false), &Progress::default()).unwrap()
}

/// Bumps a file's mtime so the change is visible even within one second.
fn touch_later(p: &Path) {
    let f = fs::OpenOptions::new().write(true).open(p).unwrap();
    f.set_modified(SystemTime::now() + Duration::from_secs(5)).unwrap();
}

fn versions(dest: &Path) -> Vec<PathBuf> {
    fs::read_dir(dest.join(engine::VERSIONS_DIR)).map(|r| r.map(|e| e.unwrap().path()).collect()).unwrap_or_default()
}

#[test]
fn first_run_copies_everything_but_excludes() {
    let f = fixture();
    let r = run(&f, false);
    assert_eq!(r.outcome, Outcome::Success);
    assert_eq!((r.copied, r.files), (2, 2));
    assert_eq!(fs::read_to_string(f.dest.join("Projects/a.txt")).unwrap(), "alpha");
    assert_eq!(fs::read_to_string(f.dest.join("Projects/sub/b.txt")).unwrap(), "bravo");
    assert!(!f.dest.join("Projects/skip.tmp").exists());
    assert!(f.dest.join(engine::MARKER).exists());
    // mtime is preserved.
    let m = |p: PathBuf| fs::metadata(p).unwrap().modified().unwrap();
    assert_eq!(m(f.src.join("a.txt")), m(f.dest.join("Projects/a.txt")));
}

#[test]
fn second_run_is_incremental_and_keeps_versions() {
    let f = fixture();
    run(&f, false);
    let r = run(&f, false);
    assert_eq!((r.copied, r.removed), (0, 0), "nothing changed");

    fs::write(f.src.join("a.txt"), "alpha v2").unwrap();
    touch_later(&f.src.join("a.txt"));
    fs::remove_file(f.src.join("sub/b.txt")).unwrap();
    fs::write(f.src.join("c.txt"), "charlie").unwrap();
    let r = run(&f, false);
    assert_eq!((r.copied, r.removed, r.files), (2, 1, 2));
    assert_eq!(fs::read_to_string(f.dest.join("Projects/a.txt")).unwrap(), "alpha v2");
    assert!(!f.dest.join("Projects/sub").exists(), "emptied folders are removed");
    let v = versions(&f.dest);
    assert_eq!(v.len(), 1);
    assert_eq!(fs::read_to_string(v[0].join("Projects/a.txt")).unwrap(), "alpha");
    assert_eq!(fs::read_to_string(v[0].join("Projects/sub/b.txt")).unwrap(), "bravo");
}

#[test]
fn lost_manifest_rechecks_destination_without_recopying() {
    let f = fixture();
    run(&f, false);
    fs::remove_dir_all(&f.state).unwrap();
    let r = run(&f, false);
    assert_eq!((r.copied, r.files), (0, 2));
    assert!(versions(&f.dest).is_empty());
}

#[test]
fn verify_repairs_files_removed_from_the_destination() {
    let f = fixture();
    run(&f, false);
    fs::remove_file(f.dest.join("Projects/a.txt")).unwrap();
    assert_eq!(run(&f, false).copied, 0, "trusted manifest doesn't look");
    assert_eq!(run(&f, true).copied, 1);
    assert!(f.dest.join("Projects/a.txt").exists());
}

#[test]
fn missing_source_is_not_treated_as_deleted() {
    let mut f = fixture();
    let other = f.src.with_file_name("Volume");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("keep.txt"), "k").unwrap();
    f.job.sources.push(other.clone());
    run(&f, false);
    fs::remove_dir_all(&other).unwrap();
    let r = run(&f, false);
    assert!(matches!(r.outcome, Outcome::Partial(_)));
    assert_eq!(r.removed, 0);
    assert!(f.dest.join("Volume/keep.txt").exists());
    // Next successful run still knows the file.
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("keep.txt"), "k").unwrap();
    assert_eq!(run(&f, false).outcome, Outcome::Success);
}

#[test]
fn refuses_foreign_or_nested_destinations() {
    let mut f = fixture();
    run(&f, false);
    let mut other = f.job.clone();
    other.id = "someone-else".into();
    let r = run_job_in(&other, &f.state.join("x"), Trigger::Manual, RunOptions::default(), &no_remote, &AtomicBool::new(false), &Progress::default()).unwrap();
    assert!(matches!(r.outcome, Outcome::Failed(ref m) if m.contains("another backup")), "{r:?}");

    f.job.destination = Destination::Local { path: f.src.join("inner") };
    assert!(matches!(run(&f, false).outcome, Outcome::Failed(_)));
}

#[test]
fn old_versions_are_pruned() {
    let mut f = fixture();
    f.job.keep_versions_days = 1;
    run(&f, false);
    let old = f.dest.join(engine::VERSIONS_DIR).join("2000-01-01_00-00-00");
    fs::create_dir_all(&old).unwrap();
    fs::write(old.join("x"), "x").unwrap();
    run(&f, false);
    assert!(!old.exists());
}

#[test]
fn cancelled_run_is_recorded() {
    let f = fixture();
    let r = run_job_in(&f.job, &f.state, Trigger::Manual, RunOptions::default(), &no_remote, &AtomicBool::new(true), &Progress::default()).unwrap();
    assert_eq!(r.outcome, Outcome::Cancelled);
    assert_eq!(state::load_history(&f.state).len(), 1);
}

#[test]
fn duplicate_source_names_get_suffixes() {
    let names = engine::source_names(&["/a/Docs".into(), "/b/Docs".into(), "/".into()]);
    assert_eq!(names, ["Docs", "Docs 2", "Root"]);
}

#[test]
fn due_follows_schedule_and_failures() {
    let job = BackupJob { sources: vec!["/x".into()], destination: Destination::Local { path: "/y".into() }, schedule: Schedule::Hourly, ..Default::default() };
    let rec = |finished, outcome| RunRecord { id: String::new(), started: finished, finished, trigger: Trigger::Schedule, outcome, copied: 0, removed: 0, bytes: 0, files: 0, versions: None };
    assert!(is_due(&job, &[], 10_000));
    assert!(!is_due(&job, &[rec(9_000, Outcome::Success)], 10_000));
    assert!(is_due(&job, &[rec(6_000, Outcome::Success)], 10_000));
    assert!(!is_due(&job, &[rec(9_500, Outcome::Failed("x".into()))], 10_000));
    assert!(!is_due(&BackupJob { schedule: Schedule::Manual, ..job.clone() }, &[], 10_000));
}

#[test]
fn status_is_published_and_cancel_requests_are_honoured() {
    let f = fixture();
    // Many files keep the run busy long enough to observe it.
    fs::create_dir_all(f.src.join("many")).unwrap();
    for i in 0..400 {
        fs::write(f.src.join(format!("many/{i}.txt")), "x").unwrap();
    }
    let state = f.state.clone();
    let observer = std::thread::spawn(move || {
        for _ in 0..400 {
            if let Some(status) = state::read_status(&state) {
                assert_eq!(status.pid, std::process::id());
                state::request_cancel(&state).unwrap();
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    });
    let r = run(&f, false);
    assert!(observer.join().unwrap(), "status was never published");
    assert_eq!(r.outcome, Outcome::Cancelled);
    assert!(state::read_status(&f.state).is_none(), "status is cleared afterwards");
    // The next run isn't affected by the old request.
    assert_eq!(run(&f, false).outcome, Outcome::Success);
}

#[test]
fn rates_are_smoothed_and_formatted() {
    use std::time::Instant;
    let mut r = state::Rate::default();
    let t0 = Instant::now();
    assert_eq!(r.sample(t0, 0), 0.);
    assert_eq!(r.sample(t0 + Duration::from_secs(1), 10_000_000), 10_000_000.);
    // Too soon: unchanged.
    assert_eq!(r.sample(t0 + Duration::from_millis(1100), 50_000_000), 10_000_000.);
    // Blends towards the new rate (20 MB/s).
    assert_eq!(r.sample(t0 + Duration::from_secs(2), 30_000_000), 14_000_000.);
    assert_eq!(format_rates(12_400_000., 85.), "12.4 MB/s · 85 items/s");
    assert_eq!(format_rates(0., 2.5), "2.5 items/s");
    assert_eq!(format_rates(1_040., 213.), "1.0 kB/s · 213 items/s");
    assert_eq!(format_rates(1.2e9, 0.), "1.2 GB/s");
    assert_eq!(format_rates(0., 0.), "");
}

/// A folder the destination refuses (here: read-only) is reported once and
/// doesn't stop the rest of the backup.
#[test]
fn an_unwritable_folder_does_not_abort_the_run() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture();
    fs::create_dir_all(f.src.join("locked")).unwrap();
    for i in 0..40 {
        fs::write(f.src.join(format!("locked/{i}.txt")), "x").unwrap();
    }
    fs::write(f.src.join("zz-after.txt"), "after").unwrap();
    let locked = f.dest.join("Projects/locked");
    fs::create_dir_all(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
    let r = run(&f, false);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    match &r.outcome {
        Outcome::Partial(msg) => assert!(msg.contains("locked/") && msg.contains("and 39 more"), "{msg}"),
        other => panic!("expected a partial run, got {other:?}"),
    }
    assert_eq!(r.copied, 3);
    assert_eq!(fs::read_to_string(f.dest.join("Projects/zz-after.txt")).unwrap(), "after");
}

mod manage {
    use super::*;
    use crate::manage::{self, RestoreFrom, RestoreOptions, RestoreTo};
    use crate::target::LocalTarget;

    const OVERWRITE: RestoreOptions = RestoreOptions { trash_replaced: false };

    fn restore(f: &Fixture, from: RestoreFrom, to: RestoreTo) -> manage::RestoreSummary {
        let mut t = LocalTarget::new(&f.dest);
        manage::restore(&f.job, &mut t, &from, &to, OVERWRITE, &AtomicBool::new(false), &mut |_| {}).unwrap()
    }

    #[test]
    fn restores_latest_to_original_and_to_a_folder() {
        let f = fixture();
        run(&f, false);
        fs::remove_file(f.src.join("a.txt")).unwrap();
        fs::write(f.src.join("sub/b.txt"), "changed locally").unwrap();
        let s = restore(&f, RestoreFrom::Latest, RestoreTo::Original);
        assert_eq!((s.restored, s.replaced, s.skipped), (2, 1, 0));
        assert_eq!(fs::read_to_string(f.src.join("a.txt")).unwrap(), "alpha");
        assert_eq!(fs::read_to_string(f.src.join("sub/b.txt")).unwrap(), "bravo");
        // Identical files are left alone.
        assert_eq!(restore(&f, RestoreFrom::Latest, RestoreTo::Original).skipped, 2);

        let elsewhere = f.dest.with_file_name("Restored");
        let s = restore(&f, RestoreFrom::Latest, RestoreTo::Folder(elsewhere.clone()));
        assert_eq!(s.restored, 2);
        assert_eq!(fs::read_to_string(elsewhere.join("Projects/sub/b.txt")).unwrap(), "bravo");
    }

    #[test]
    fn restores_and_deletes_a_runs_versions() {
        let f = fixture();
        run(&f, false);
        fs::write(f.src.join("a.txt"), "alpha v2").unwrap();
        touch_later(&f.src.join("a.txt"));
        let r = run(&f, false);
        let v = r.versions.clone().expect("the replaced file was kept");
        let out = f.dest.with_file_name("Old");
        assert_eq!(restore(&f, RestoreFrom::Run(v.clone()), RestoreTo::Folder(out.clone())).restored, 1);
        assert_eq!(fs::read_to_string(out.join("Projects/a.txt")).unwrap(), "alpha");

        let mut t = LocalTarget::new(&f.dest);
        let left = manage::delete_run(&f.job, &mut t, &f.state, &r).unwrap();
        assert_eq!(left.len(), 1);
        assert!(!f.dest.join(engine::VERSIONS_DIR).join(&v).exists());
        // A crafted version name can't reach outside the versions.
        assert!(manage::restore(&f.job, &mut t, &RestoreFrom::Run("../Projects".into()), &RestoreTo::Folder(out), OVERWRITE, &AtomicBool::new(false), &mut |_| {}).is_err());
    }

    #[test]
    fn deletes_versions_and_the_whole_backup() {
        let f = fixture();
        run(&f, false);
        fs::remove_file(f.src.join("a.txt")).unwrap();
        run(&f, false);
        let mut t = LocalTarget::new(&f.dest);
        manage::delete_all_versions(&f.job, &mut t, &f.state).unwrap();
        assert!(!f.dest.join(engine::VERSIONS_DIR).exists());
        assert!(state::load_history(&f.state).iter().all(|r| r.versions.is_none()));

        // Unrelated files next to the backup survive.
        fs::write(f.dest.join("mine.txt"), "keep").unwrap();
        manage::delete_backup_data(&f.job, &mut t, &f.state).unwrap();
        assert!(!f.dest.join("Projects").exists() && !f.dest.join(engine::MARKER).exists());
        assert!(f.dest.join("mine.txt").exists());

        // Without a marker or a local record, nothing is deleted.
        fs::create_dir_all(f.dest.join("Projects")).unwrap();
        let other = BackupJob { id: "x".into(), ..f.job.clone() };
        assert!(manage::delete_backup_data(&other, &mut t, &f.state.join("none")).is_err());
        assert!(f.dest.join("Projects").exists());
    }
}
