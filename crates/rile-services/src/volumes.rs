//! Unmounting and ejecting volumes via the platform's tools.

use std::path::Path;
#[cfg(not(windows))]
use std::process::Command;

#[cfg(not(windows))]
use anyhow::bail;
use anyhow::Result;
use rile_core::places::Volume;

/// Mount roots under which user-mountable volumes live.
const USER_MOUNT_ROOTS: &[&str] = &["/Volumes", "/media", "/run/media", "/mnt"];
/// Mount points that must never be offered for unmounting.
const SYSTEM_MOUNTS: &[&str] =
    &["/", "/System", "/private", "/dev", "/boot", "/usr", "/var", "/home", "/nix", "/etc", "/opt", "/proc", "/sys"];

/// Whether the UI should offer "Eject"/"Unmount" for this volume.
pub fn can_unmount(v: &Volume) -> bool {
    if cfg!(windows) {
        #[cfg(windows)]
        return win::can_eject(v);
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
        // Windows has no unmount-only step for removable drives that users
        // would expect; both mean "Safely Remove".
        let _ = eject;
        win::eject(mount_point)
    }
}

#[cfg(windows)]
mod win {
    use std::path::Path;

    use anyhow::{Result, bail};
    use rile_core::places::Volume;
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_Parent, CM_Request_Device_EjectW, CR_SUCCESS, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, PNP_VETO_TYPE, SP_DEVICE_INTERFACE_DATA,
        SP_DEVICE_INTERFACE_DETAIL_DATA_W, SP_DEVINFO_DATA, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces,
        SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
    };
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, GetDriveTypeW, OPEN_EXISTING,
    };
    use windows::Win32::System::WindowsProgramming::{DRIVE_CDROM, DRIVE_REMOVABLE};
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::{GUID_DEVINTERFACE_DISK, IOCTL_STORAGE_GET_DEVICE_NUMBER, STORAGE_DEVICE_NUMBER};
    use windows::core::{HSTRING, PCWSTR};

    use crate::win::{Wide, from_wide};

    fn letter(mount_point: &Path) -> Option<char> {
        let s = mount_point.to_string_lossy();
        let mut chars = s.chars();
        let (l, colon) = (chars.next()?, chars.next()?);
        (l.is_ascii_alphabetic() && colon == ':').then_some(l.to_ascii_uppercase())
    }

    pub fn can_eject(v: &Volume) -> bool {
        let Some(l) = letter(&v.mount_point) else { return false };
        let system = std::env::var("SystemDrive").ok().and_then(|d| d.chars().next()).map(|c| c.to_ascii_uppercase());
        if system == Some(l) {
            return false;
        }
        let kind = unsafe { GetDriveTypeW(&HSTRING::from(format!("{l}:\\"))) };
        v.removable || kind == DRIVE_REMOVABLE || kind == DRIVE_CDROM
    }

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0).ok() };
        }
    }

    fn open(path: PCWSTR) -> Option<Handle> {
        // No access rights needed just to query the device number.
        unsafe { CreateFileW(path, 0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) }.ok().map(Handle)
    }

    fn device_number(h: &Handle) -> Option<(u32, u32)> {
        let mut n = STORAGE_DEVICE_NUMBER::default();
        let mut got = 0u32;
        unsafe {
            DeviceIoControl(h.0, IOCTL_STORAGE_GET_DEVICE_NUMBER, None, 0, Some(&mut n as *mut _ as *mut _), size_of::<STORAGE_DEVICE_NUMBER>() as u32, Some(&mut got), None)
        }
        .ok()?;
        Some((n.DeviceType, n.DeviceNumber))
    }

    /// Device instance of the disk that holds the volume.
    fn disk_devinst(target: (u32, u32)) -> Option<u32> {
        let set = unsafe { SetupDiGetClassDevsW(Some(&GUID_DEVINTERFACE_DISK), PCWSTR::null(), None, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE) }.ok()?;
        let mut found = None;
        for index in 0.. {
            let mut iface = SP_DEVICE_INTERFACE_DATA { cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32, ..Default::default() };
            if unsafe { SetupDiEnumDeviceInterfaces(set, None, &GUID_DEVINTERFACE_DISK, index, &mut iface) }.is_err() {
                break;
            }
            // Variable-size detail: a u32 cbSize followed by the device path.
            let mut buf = vec![0u64; 512];
            let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            unsafe { (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 };
            let mut info = SP_DEVINFO_DATA { cbSize: size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            let ok = unsafe { SetupDiGetDeviceInterfaceDetailW(set, &iface, Some(detail), (buf.len() * 8) as u32, None, Some(&mut info)) };
            if ok.is_err() {
                continue;
            }
            let path = unsafe { PCWSTR((*detail).DevicePath.as_ptr()) };
            if open(path).and_then(|h| device_number(&h)) == Some(target) {
                found = Some(info.DevInst);
                break;
            }
        }
        unsafe { SetupDiDestroyDeviceInfoList(set).ok() };
        found
    }

    fn request_eject(devinst: u32) -> Result<(), String> {
        let mut veto = PNP_VETO_TYPE::default();
        let mut name = [0u16; 260];
        let r = unsafe { CM_Request_Device_EjectW(devinst, Some(&mut veto), Some(&mut name), 0) };
        if r == CR_SUCCESS && veto.0 == 0 {
            Ok(())
        } else {
            Err(from_wide(&name))
        }
    }

    pub fn eject(mount_point: &Path) -> Result<()> {
        let Some(l) = letter(mount_point) else { bail!("{} isn't a drive", mount_point.display()) };
        let volume = Wide::new(format!(r"\\.\{l}:"));
        let Some(number) = open(volume.pcwstr()).and_then(|h| device_number(&h)) else {
            bail!("Couldn't find the device for drive {l}:")
        };
        let Some(disk) = disk_devinst(number) else { bail!("Couldn't find the device for drive {l}:") };
        // The disk itself, else its parent (the USB device).
        let first = request_eject(disk);
        if first.is_ok() {
            return Ok(());
        }
        let mut parent = 0u32;
        if unsafe { CM_Get_Parent(&mut parent, disk, 0) } == CR_SUCCESS && request_eject(parent).is_ok() {
            return Ok(());
        }
        let blocker = first.err().filter(|n| !n.is_empty());
        bail!(
            "Drive {l}: is in use{}. Close the files and windows that use it, then try again.",
            blocker.map(|b| format!(" ({b})")).unwrap_or_default()
        )
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
