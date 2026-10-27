//! Unmounting and ejecting volumes via the platform's tools.

use std::path::Path;
#[cfg(not(windows))]
use std::process::Command;

use anyhow::{Result, bail};
use rile_core::places::Volume;

/// Mount roots under which user-mountable volumes live.
const USER_MOUNT_ROOTS: &[&str] = &["/Volumes", "/media", "/run/media", "/mnt"];
/// Mount points that must never be offered for unmounting.
const SYSTEM_MOUNTS: &[&str] =
    &["/", "/System", "/private", "/dev", "/boot", "/usr", "/var", "/home", "/nix", "/etc", "/opt", "/proc", "/sys"];

/// Whether the UI should offer "Eject"/"Unmount" for this volume.
pub fn can_unmount(v: &Volume) -> bool {
    if cfg!(windows) {
        return false;
    }
    let mp = v.mount_point.as_path();
    if SYSTEM_MOUNTS.iter().any(|s| mp == Path::new(s)) || mp.starts_with("/System") || mp.starts_with("/boot") {
        return false;
    }
    let under_user_root = USER_MOUNT_ROOTS.iter().any(|r| mp.starts_with(r) && mp != Path::new(r));
    v.removable || under_user_root
}

/// Unmounts (or with `eject`, unmounts and ejects/powers off) the volume at
/// `mount_point`. On failure the error carries the tool's stderr.
pub fn unmount(mount_point: &Path, eject: bool) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let verb = if eject { "eject" } else { "unmount" };
        run(Command::new("/usr/sbin/diskutil").arg(verb).arg(mount_point))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        unmount_linux(mount_point, eject)
    }
    #[cfg(windows)]
    {
        let _ = (mount_point, eject);
        bail!("Unmounting volumes is not supported on Windows")
    }
}

#[cfg(not(windows))]
fn run(cmd: &mut Command) -> Result<()> {
    let out = match cmd.output() {
        Ok(o) => o,
        Err(e) => bail!("Failed to run {:?}: {e}", cmd.get_program()),
    };
    if out.status.success() {
        return Ok(());
    }
    let mut msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if msg.is_empty() {
        msg = String::from_utf8_lossy(&out.stdout).trim().to_string();
    }
    if msg.is_empty() {
        msg = format!("{:?} exited with {}", cmd.get_program(), out.status);
    }
    bail!(msg)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn unmount_linux(mount_point: &Path, eject: bool) -> Result<()> {
    let device = std::fs::read_to_string("/proc/self/mounts")
        .ok()
        .and_then(|m| device_for_mount(&m, mount_point));
    let device = device.filter(|d| d.starts_with("/dev/"));
    if let Some(dev) = &device {
        match run(Command::new("udisksctl").args(["unmount", "--no-user-interaction", "-b", dev])) {
            Ok(()) => {
                if eject {
                    // Best effort: not every device can be powered off.
                    let _ = run(Command::new("udisksctl").args(["power-off", "--no-user-interaction", "-b", dev]));
                }
                return Ok(());
            }
            Err(e) if which("udisksctl") => {
                // udisks is present but refused; umount will most likely
                // fail the same way, but try anyway and report udisks' error.
                return run(Command::new("umount").arg(mount_point)).map_err(|_| e);
            }
            Err(_) => {}
        }
    }
    run(Command::new("umount").arg(mount_point))?;
    if eject && let Some(dev) = &device {
        let _ = run(Command::new("eject").arg(dev));
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn which(prog: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(prog).is_file()))
}

/// Finds the device mounted at `mount_point` in `/proc/mounts` content
/// (handles the octal escapes used for spaces).
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
fn device_for_mount(mounts: &str, mount_point: &Path) -> Option<String> {
    let unescape = |s: &str| {
        s.replace("\\040", " ").replace("\\011", "\t").replace("\\012", "\n").replace("\\134", "\\")
    };
    mounts.lines().rev().find_map(|l| {
        let mut it = l.split_whitespace();
        let dev = it.next()?;
        let mp = it.next()?;
        (Path::new(&unescape(mp)) == mount_point).then(|| unescape(dev))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(windows))]
    use std::path::PathBuf;

    #[cfg(not(windows))]
    fn vol(mp: &str, removable: bool) -> Volume {
        Volume { name: "v".into(), mount_point: PathBuf::from(mp), total: 1, available: 1, removable }
    }

    #[cfg(not(windows))]
    #[test]
    fn unmountable_volumes() {
        assert!(!can_unmount(&vol("/", true)));
        assert!(!can_unmount(&vol("/System/Volumes/Data", true)));
        assert!(!can_unmount(&vol("/boot/efi", true)));
        assert!(!can_unmount(&vol("/Volumes", false)));
        assert!(can_unmount(&vol("/Volumes/USB Stick", false)));
        assert!(can_unmount(&vol("/run/media/me/disk", false)));
        assert!(can_unmount(&vol("/media/disk", false)));
        assert!(can_unmount(&vol("/srv/removable", true)));
        assert!(!can_unmount(&vol("/srv/data", false)));
    }

    #[test]
    fn parses_proc_mounts() {
        let m = "/dev/sda1 / ext4 rw 0 0\n/dev/sdb1 /media/me/My\\040Disk vfat rw 0 0\n";
        assert_eq!(device_for_mount(m, Path::new("/media/me/My Disk")).as_deref(), Some("/dev/sdb1"));
        assert_eq!(device_for_mount(m, Path::new("/nope")), None);
    }

    #[cfg(not(windows))]
    #[test]
    fn unmount_nonexistent_reports_error() {
        let err = unmount(Path::new("/definitely/not/a/mount/point"), false).unwrap_err();
        assert!(!err.to_string().is_empty());
    }
}
