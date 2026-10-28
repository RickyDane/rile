//! Drive speed test: sequential and random (4 KiB) read/write throughput,
//! measured on a temporary file in the volume's root (or a writable folder).
//!
//! The OS page cache is bypassed where the platform allows it (`F_NOCACHE` on
//! macOS, `posix_fadvise(DONTNEED)` on Linux) so reads hit the device.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// One stage of the test, in run order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    SeqWrite,
    SeqRead,
    RandWrite,
    RandRead,
}

impl Phase {
    pub const ALL: [Phase; 4] = [Phase::SeqWrite, Phase::SeqRead, Phase::RandWrite, Phase::RandRead];

    pub fn label(self) -> &'static str {
        match self {
            Phase::SeqWrite => "Sequential write",
            Phase::SeqRead => "Sequential read",
            Phase::RandWrite => "Random 4K write",
            Phase::RandRead => "Random 4K read",
        }
    }

    pub fn index(self) -> usize {
        Phase::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }
}

/// Test sizes and limits.
#[derive(Clone, Debug)]
pub struct Config {
    /// Size of the sequential test file (shrunk to fit free space).
    pub file_size: u64,
    /// Block size of sequential I/O.
    pub seq_block: usize,
    /// Block size of random I/O.
    pub rand_block: usize,
    /// Time budget of each random phase.
    pub rand_duration: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self { file_size: 1 << 30, seq_block: 8 << 20, rand_block: 4096, rand_duration: Duration::from_secs(4) }
    }
}

/// Result of one phase.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Measure {
    pub bytes: u64,
    pub ops: u64,
    pub elapsed: Duration,
    /// Per-operation latency (min, avg, max).
    pub lat_min: Duration,
    pub lat_avg: Duration,
    pub lat_max: Duration,
}

impl Measure {
    /// Throughput in MB/s (decimal).
    pub fn mb_per_s(&self) -> f64 {
        let s = self.elapsed.as_secs_f64();
        if s <= 0. { 0. } else { self.bytes as f64 / 1e6 / s }
    }

    pub fn iops(&self) -> f64 {
        let s = self.elapsed.as_secs_f64();
        if s <= 0. { 0. } else { self.ops as f64 / s }
    }
}

/// Live progress of the running phase.
#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub phase: Phase,
    /// 0..=1 within the phase.
    pub fraction: f32,
    /// Throughput so far in this phase, MB/s.
    pub mb_per_s: f64,
}

/// Everything a finished run measured.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub dir: PathBuf,
    pub file_size: u64,
    /// Indexed like [`Phase::ALL`].
    pub results: [Option<Measure>; 4],
    pub uncached: bool,
}

/// Runs all phases in `dir`. `on_progress` is throttled to ~20 Hz;
/// `on_phase_done` fires once per finished phase.
pub fn run(
    dir: &Path,
    cfg: &Config,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(Progress),
    mut on_phase_done: impl FnMut(Phase, Measure),
) -> Result<Report> {
    let free = free_space(dir).unwrap_or(u64::MAX);
    let mut size = cfg.file_size.min(free.saturating_sub(256 << 20) / 2);
    size -= size % cfg.seq_block as u64;
    if size < (64 << 20) {
        bail!("Not enough free space on this drive to run a speed test");
    }

    remove_stale(dir);
    let path = dir.join(format!(".rile-speedtest-{}.tmp", std::process::id()));
    let _guard = Remove(path.clone());
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .with_context(|| format!("Can't write to {}", dir.display()))?;
    let uncached = no_cache(&file);

    let mut report = Report { dir: dir.to_path_buf(), file_size: size, uncached, ..Default::default() };
    let mut buf = vec![0u8; cfg.seq_block];
    let mut rng = Rng::new();
    rng.fill(&mut buf);

    let mut done = |phase: Phase, m: Measure, report: &mut Report| {
        report.results[phase.index()] = Some(m);
        on_phase_done(phase, m);
    };

    // Sequential write.
    let m = sequential(Phase::SeqWrite, size, &mut buf, cancel, &mut on_progress, |b| file.write_all(b))?;
    let t = Instant::now();
    file.sync_all()?;
    let m = Measure { elapsed: m.elapsed + t.elapsed(), ..m };
    done(Phase::SeqWrite, m, &mut report);
    drop_cache(&file, size);

    // Sequential read.
    file.seek(SeekFrom::Start(0))?;
    let m = sequential(Phase::SeqRead, size, &mut buf, cancel, &mut on_progress, |b| file.read_exact(b))?;
    done(Phase::SeqRead, m, &mut report);

    // Random 4K write / read.
    let blocks = size / cfg.rand_block as u64;
    let mut small = vec![0u8; cfg.rand_block];
    rng.fill(&mut small);
    let m = random(Phase::RandWrite, cfg, blocks, &mut rng, cancel, &mut on_progress, |off| {
        file.seek(SeekFrom::Start(off))?;
        file.write_all(&small)
    })?;
    let t = Instant::now();
    file.sync_all()?;
    let m = Measure { elapsed: m.elapsed + t.elapsed(), ..m };
    done(Phase::RandWrite, m, &mut report);
    drop_cache(&file, size);

    let m = random(Phase::RandRead, cfg, blocks, &mut rng, cancel, &mut on_progress, |off| {
        file.seek(SeekFrom::Start(off))?;
        file.read_exact(&mut small)
    })?;
    done(Phase::RandRead, m, &mut report);

    Ok(report)
}

fn sequential(
    phase: Phase,
    size: u64,
    buf: &mut [u8],
    cancel: &AtomicBool,
    on_progress: &mut impl FnMut(Progress),
    mut io: impl FnMut(&mut [u8]) -> std::io::Result<()>,
) -> Result<Measure> {
    let mut lat = Latency::default();
    let start = Instant::now();
    let mut last = start;
    let mut bytes = 0u64;
    while bytes < size {
        if cancel.load(Ordering::Relaxed) {
            bail!("Cancelled");
        }
        let t = Instant::now();
        io(buf)?;
        lat.add(t.elapsed());
        bytes += buf.len() as u64;
        if last.elapsed() >= Duration::from_millis(50) {
            last = Instant::now();
            on_progress(Progress { phase, fraction: bytes as f32 / size as f32, mb_per_s: bytes as f64 / 1e6 / start.elapsed().as_secs_f64() });
        }
    }
    Ok(lat.finish(bytes, start.elapsed()))
}

fn random(
    phase: Phase,
    cfg: &Config,
    blocks: u64,
    rng: &mut Rng,
    cancel: &AtomicBool,
    on_progress: &mut impl FnMut(Progress),
    mut io: impl FnMut(u64) -> std::io::Result<()>,
) -> Result<Measure> {
    let mut lat = Latency::default();
    let start = Instant::now();
    let mut last = start;
    let mut bytes = 0u64;
    while start.elapsed() < cfg.rand_duration {
        if cancel.load(Ordering::Relaxed) {
            bail!("Cancelled");
        }
        let off = (rng.next() % blocks) * cfg.rand_block as u64;
        let t = Instant::now();
        io(off)?;
        lat.add(t.elapsed());
        bytes += cfg.rand_block as u64;
        if last.elapsed() >= Duration::from_millis(50) {
            last = Instant::now();
            let el = start.elapsed();
            on_progress(Progress {
                phase,
                fraction: (el.as_secs_f32() / cfg.rand_duration.as_secs_f32()).min(1.),
                mb_per_s: bytes as f64 / 1e6 / el.as_secs_f64(),
            });
        }
    }
    Ok(lat.finish(bytes, start.elapsed()))
}

#[derive(Default)]
struct Latency {
    ops: u64,
    sum: Duration,
    min: Option<Duration>,
    max: Duration,
}

impl Latency {
    fn add(&mut self, d: Duration) {
        self.ops += 1;
        self.sum += d;
        self.min = Some(self.min.map_or(d, |m| m.min(d)));
        self.max = self.max.max(d);
    }

    fn finish(self, bytes: u64, elapsed: Duration) -> Measure {
        Measure {
            bytes,
            ops: self.ops,
            elapsed,
            lat_min: self.min.unwrap_or_default(),
            lat_avg: if self.ops == 0 { Duration::ZERO } else { self.sum / self.ops as u32 },
            lat_max: self.max,
        }
    }
}

/// xorshift64*: incompressible test data and random offsets.
struct Rng(u64);

impl Rng {
    fn new() -> Self {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
        Rng(seed | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            let v = self.next().to_le_bytes();
            chunk.copy_from_slice(&v[..chunk.len()]);
        }
    }
}

/// Deletes test files left behind by runs whose process died (crash,
/// force quit), so they don't keep eating space.
fn remove_stale(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.strip_prefix(".rile-speedtest-")?.strip_suffix(".tmp")?.parse::<u32>().ok()) else {
            continue;
        };
        if pid != std::process::id() && !process_alive(pid) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // Access denied still means the process exists (another user's).
    let handle = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
        Ok(h) => h,
        Err(e) => return e.code() == windows::Win32::Foundation::E_ACCESSDENIED,
    };
    let mut code = 0u32;
    let alive = unsafe { GetExitCodeProcess(handle, &mut code) }.is_ok() && code == STILL_ACTIVE.0 as u32;
    unsafe { CloseHandle(handle).ok() };
    alive
}

#[cfg(not(any(unix, windows)))]
fn process_alive(_: u32) -> bool {
    true
}

struct Remove(PathBuf);

impl Drop for Remove {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(target_os = "macos")]
fn no_cache(file: &File) -> bool {
    use std::os::fd::AsRawFd;
    unsafe { libc::fcntl(file.as_raw_fd(), libc::F_NOCACHE, 1) != -1 }
}

#[cfg(target_os = "linux")]
fn no_cache(_: &File) -> bool {
    true
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn no_cache(_: &File) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn drop_cache(file: &File, len: u64) {
    use std::os::fd::AsRawFd;
    unsafe {
        libc::posix_fadvise(file.as_raw_fd(), 0, len as libc::off_t, libc::POSIX_FADV_DONTNEED);
    }
}

#[cfg(not(target_os = "linux"))]
fn drop_cache(_: &File, _: u64) {}

#[cfg(unix)]
fn free_space(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some(st.f_bavail as u64 * st.f_frsize as u64)
}

#[cfg(windows)]
fn free_space(dir: &Path) -> Option<u64> {
    let path = crate::win::Wide::path(dir);
    let mut available = 0u64;
    unsafe { windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(path.pcwstr(), Some(&mut available), None, None) }.ok()?;
    Some(available)
}

#[cfg(not(any(unix, windows)))]
fn free_space(_: &Path) -> Option<u64> {
    None
}

/// Where to put the test file for a volume: its root if writable, else the
/// user's home when the volume is the startup disk (macOS' sealed `/`).
pub fn test_dir(mount_point: &Path) -> PathBuf {
    let probe = mount_point.join(format!(".rile-speedtest-probe-{}", std::process::id()));
    if File::create(&probe).is_ok() {
        let _ = std::fs::remove_file(&probe);
        return mount_point.to_path_buf();
    }
    // The startup disk's root is usually read-only for normal users (`/` on
    // Unix, `C:\` on Windows); measure inside the home folder on it instead.
    let is_root = mount_point.parent().is_none();
    if is_root && let Some(home) = dirs::home_dir().filter(|h| h.starts_with(mount_point)) {
        return home;
    }
    if is_root && cfg!(windows) {
        return std::env::temp_dir();
    }
    mount_point.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_all_phases_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config { file_size: 64 << 20, seq_block: 1 << 20, rand_block: 4096, rand_duration: Duration::from_millis(200) };
        let mut phases = Vec::new();
        let report = run(dir.path(), &cfg, &AtomicBool::new(false), |_| {}, |p, _| phases.push(p)).unwrap();
        assert_eq!(phases, Phase::ALL);
        assert!(report.results.iter().all(|m| m.is_some_and(|m| m.bytes > 0 && m.mb_per_s() > 0.)));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn removes_files_of_dead_runs() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join(".rile-speedtest-999999.tmp");
        std::fs::write(&stale, b"x").unwrap();
        remove_stale(dir.path());
        assert!(!stale.exists());
    }

    #[test]
    fn cancel_stops_early() {
        let dir = tempfile::tempdir().unwrap();
        assert!(run(dir.path(), &Config::default(), &AtomicBool::new(true), |_| {}, |_, _| {}).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
