//! "Open With" support: which applications can open a file, the default one,
//! launching files with a chosen application and listing every installed
//! application for a "choose other app" picker.

#[cfg(not(windows))]
use std::path::Path;
use std::path::PathBuf;

#[cfg(not(windows))]
use anyhow::Result;

/// An installed application.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AppInfo {
    /// Display name, e.g. "TextEdit" (no `.app` suffix).
    pub name: String,
    /// Path to the application bundle (the program's .exe on Windows).
    pub path: PathBuf,
    /// `CFBundleIdentifier`, when the bundle has one (unused on Windows).
    pub bundle_id: Option<String>,
}

/// Applications able to open `path`: the default app first, then the rest
/// sorted by name (case-insensitive), deduplicated by bundle path.
#[cfg(target_os = "macos")]
pub fn apps_for(path: &Path) -> Vec<AppInfo> {
    use objc2_app_kit::NSWorkspace;

    objc2::rc::autoreleasepool(|_| {
        let Some(url) = mac::file_url(path) else { return Vec::new() };
        let workspace = NSWorkspace::sharedWorkspace();
        let default = workspace.URLForApplicationToOpenURL(&url).and_then(|u| mac::url_path(&u));
        let mut seen = std::collections::HashSet::new();
        let mut apps: Vec<AppInfo> = workspace
            .URLsForApplicationsToOpenURL(&url)
            .iter()
            .filter_map(|u| mac::url_path(&u))
            .filter(|p| Some(p) != default.as_ref())
            .filter(|p| seen.insert(p.clone()))
            .filter_map(|p| mac::app_info(&p))
            .collect();
        sort_by_name(&mut apps);
        if let Some(info) = default.as_deref().and_then(mac::app_info) {
            apps.insert(0, info);
        }
        apps
    })
}

#[cfg(windows)]
pub use win::{all_applications, apps_for, default_app_for, open_with};

#[cfg(not(any(target_os = "macos", windows)))]
pub fn apps_for(_path: &Path) -> Vec<AppInfo> {
    Vec::new()
}

/// The application the system would use to open `path`.
#[cfg(target_os = "macos")]
pub fn default_app_for(path: &Path) -> Option<AppInfo> {
    use objc2_app_kit::NSWorkspace;

    objc2::rc::autoreleasepool(|_| {
        let url = mac::file_url(path)?;
        let app = NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&url)?;
        mac::app_info(&mac::url_path(&app)?)
    })
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn default_app_for(_path: &Path) -> Option<AppInfo> {
    None
}

/// Opens `paths` with the application bundle at `app` (via `open -a`, which
/// returns once LaunchServices has handed the files over).
#[cfg(target_os = "macos")]
pub fn open_with(paths: &[PathBuf], app: &Path) -> Result<()> {
    use anyhow::Context;

    if paths.is_empty() {
        return Ok(());
    }
    let output = std::process::Command::new("/usr/bin/open")
        .arg("-a")
        .arg(app)
        .arg("--")
        .args(paths)
        .output()
        .context("failed to run `open`")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("open -a {} failed: {}", app.display(), stderr.trim());
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn open_with(_paths: &[PathBuf], _app: &Path) -> Result<()> {
    anyhow::bail!("Open With is only available on macOS and Windows")
}

/// Every application in `/Applications`, `/System/Applications` and
/// `~/Applications` (plus one level of subfolders, e.g. `Utilities`),
/// deduplicated and sorted by name.
#[cfg(target_os = "macos")]
pub fn all_applications() -> Vec<AppInfo> {
    let mut roots = vec![PathBuf::from("/Applications"), PathBuf::from("/System/Applications")];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join("Applications"));
    }
    let mut bundles = Vec::new();
    for root in &roots {
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if is_app_bundle(&path) {
                bundles.push(path);
            } else if entry.file_type().is_ok_and(|t| t.is_dir()) {
                let Ok(sub) = std::fs::read_dir(&path) else { continue };
                bundles.extend(sub.flatten().map(|e| e.path()).filter(|p| is_app_bundle(p)));
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut apps: Vec<AppInfo> = objc2::rc::autoreleasepool(|_| {
        bundles
            .iter()
            .filter_map(|p| mac::app_info(p))
            // The same app can live in two roots (e.g. Safari via a firmlink);
            // prefer the first by bundle id, falling back to the path.
            .filter(|a| seen.insert(a.bundle_id.clone().unwrap_or_else(|| a.path.display().to_string())))
            .collect()
    });
    sort_by_name(&mut apps);
    apps
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn all_applications() -> Vec<AppInfo> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn is_app_bundle(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")) && path.is_dir()
}

#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
fn sort_by_name(apps: &mut [AppInfo]) {
    apps.sort_by_cached_key(|a| (a.name.to_lowercase(), a.path.clone()));
}

#[cfg(target_os = "macos")]
mod mac {
    use std::path::{Path, PathBuf};

    use objc2::rc::Retained;
    use objc2_foundation::{NSBundle, NSFileManager, NSString, NSURL};

    use super::AppInfo;

    pub fn file_url(path: &Path) -> Option<Retained<NSURL>> {
        Some(NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?)))
    }

    pub fn url_path(url: &NSURL) -> Option<PathBuf> {
        Some(PathBuf::from(url.path()?.to_string()))
    }

    pub fn app_info(path: &Path) -> Option<AppInfo> {
        let path_str = NSString::from_str(path.to_str()?);
        let bundle = NSBundle::bundleWithPath(&path_str);
        let bundle_id = bundle.as_ref().and_then(|b| b.bundleIdentifier()).map(|s| s.to_string());
        // displayNameAtPath gives the localized Finder name ("Calculator",
        // "Rechner", ...) and strips `.app` when the extension is hidden.
        let mut name = NSFileManager::defaultManager().displayNameAtPath(&path_str).to_string();
        if let Some(stripped) = name.strip_suffix(".app") {
            name = stripped.to_string();
        }
        if name.is_empty() {
            name = path.file_stem()?.to_string_lossy().into_owned();
        }
        Some(AppInfo { name, path: path.to_path_buf(), bundle_id })
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn text_file_has_apps_with_default_first() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.txt");
        std::fs::write(&file, "hello").unwrap();
        let apps = apps_for(&file);
        assert!(!apps.is_empty());
        let default = default_app_for(&file).expect("txt has a default app");
        assert_eq!(apps[0], default);
        let paths: std::collections::HashSet<_> = apps.iter().map(|a| &a.path).collect();
        assert_eq!(paths.len(), apps.len(), "duplicates in {apps:?}");
    }

    #[test]
    fn lists_installed_applications() {
        let apps = all_applications();
        let calc = apps.iter().find(|a| a.bundle_id.as_deref() == Some("com.apple.calculator"));
        assert!(calc.is_some(), "Calculator missing");
        assert!(!calc.unwrap().name.ends_with(".app"));
    }
}

/// Windows: the Shell's registered handlers for the file type, and the
/// programs in the Start menu for "Other…".
#[cfg(windows)]
mod win {
    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result, bail};
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IPersistFile, STGM_READ};
    use windows::Win32::UI::Shell::{
        ASSOC_FILTER_RECOMMENDED, ASSOCF_NONE, ASSOCSTR_EXECUTABLE, ASSOCSTR_FRIENDLYAPPNAME, AssocQueryStringW, IAssocHandler,
        IShellLinkW, SHAssocEnumHandlers, ShellLink,
    };
    use windows::core::{HSTRING, Interface, PCWSTR, PWSTR};

    use super::{AppInfo, sort_by_name};
    use crate::win::{ComScope, from_wide};

    fn ext_of(path: &Path) -> Option<String> {
        path.extension().map(|e| format!(".{}", e.to_string_lossy()))
    }

    fn take(p: PWSTR) -> String {
        let s = unsafe { p.to_string() }.unwrap_or_default();
        unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
        s
    }

    fn handler_info(h: &IAssocHandler) -> Option<AppInfo> {
        let exe = PathBuf::from(take(unsafe { h.GetName() }.ok()?));
        // Store apps report a package name, not a program we can start.
        if !exe.is_file() {
            return None;
        }
        let name = take(unsafe { h.GetUIName() }.ok()?);
        Some(AppInfo { name, path: exe, bundle_id: None })
    }

    fn query(flag: windows::Win32::UI::Shell::ASSOCSTR, ext: &str) -> Option<String> {
        let ext = HSTRING::from(ext);
        let mut len = 0u32;
        let _ = unsafe { AssocQueryStringW(ASSOCF_NONE, flag, &ext, PCWSTR::null(), None, &mut len) };
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        unsafe { AssocQueryStringW(ASSOCF_NONE, flag, &ext, PCWSTR::null(), Some(PWSTR(buf.as_mut_ptr())), &mut len) }.ok().ok()?;
        Some(from_wide(&buf)).filter(|s| !s.is_empty())
    }

    pub fn default_app_for(path: &Path) -> Option<AppInfo> {
        let _com = ComScope::new();
        let ext = ext_of(path)?;
        let exe = PathBuf::from(query(ASSOCSTR_EXECUTABLE, &ext)?);
        if !exe.is_file() {
            return None;
        }
        let name = query(ASSOCSTR_FRIENDLYAPPNAME, &ext).unwrap_or_else(|| exe.file_stem().unwrap_or_default().to_string_lossy().into_owned());
        Some(AppInfo { name, path: exe, bundle_id: None })
    }

    pub fn apps_for(path: &Path) -> Vec<AppInfo> {
        let _com = ComScope::new();
        let Some(ext) = ext_of(path) else { return Vec::new() };
        let default = default_app_for(path);
        let mut apps = Vec::new();
        if let Ok(handlers) = unsafe { SHAssocEnumHandlers(&HSTRING::from(&ext), ASSOC_FILTER_RECOMMENDED) } {
            loop {
                let mut batch = [None];
                let mut fetched = 0u32;
                if unsafe { handlers.Next(&mut batch, Some(&mut fetched)) }.is_err() || fetched == 0 {
                    break;
                }
                if let Some(info) = batch[0].as_ref().and_then(handler_info) {
                    apps.push(info);
                }
            }
        }
        let key = |p: &Path| crate::path_key(p);
        let mut seen = std::collections::HashSet::new();
        if let Some(d) = &default {
            seen.insert(key(&d.path));
        }
        apps.retain(|a| seen.insert(key(&a.path)));
        sort_by_name(&mut apps);
        if let Some(d) = default {
            apps.insert(0, d);
        }
        apps
    }

    /// Starts `app` with the files as arguments, like Explorer's "Open with".
    pub fn open_with(paths: &[PathBuf], app: &Path) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        if !app.is_file() {
            bail!("{} isn't a program", app.display());
        }
        std::process::Command::new(app)
            .args(paths)
            .current_dir(paths[0].parent().unwrap_or(Path::new(".")))
            .spawn()
            .with_context(|| format!("starting {}", app.display()))?;
        Ok(())
    }

    /// The program a Start menu shortcut starts.
    fn shortcut_target(link: &IShellLinkW, file: &IPersistFile, lnk: &Path) -> Option<PathBuf> {
        unsafe { file.Load(&HSTRING::from(lnk.as_os_str()), STGM_READ) }.ok()?;
        let mut buf = [0u16; 1024];
        unsafe { link.GetPath(&mut buf, std::ptr::null_mut(), 0) }.ok()?;
        let target = PathBuf::from(from_wide(&buf));
        let is_program = target.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"));
        (is_program && target.is_file()).then_some(target)
    }

    /// Programs with a Start menu shortcut (the user's and everyone's),
    /// without uninstallers and duplicates.
    pub fn all_applications() -> Vec<AppInfo> {
        let _com = ComScope::new();
        let Ok(link) = (unsafe { CoCreateInstance::<_, IShellLinkW>(&ShellLink, None, CLSCTX_INPROC_SERVER) }) else { return Vec::new() };
        let Ok(file) = link.cast::<IPersistFile>() else { return Vec::new() };
        let mut roots: Vec<PathBuf> = Vec::new();
        if let Some(appdata) = dirs::data_dir() {
            roots.push(appdata.join(r"Microsoft\Windows\Start Menu\Programs"));
        }
        if let Some(program_data) = std::env::var_os("ProgramData") {
            roots.push(PathBuf::from(program_data).join(r"Microsoft\Windows\Start Menu\Programs"));
        }
        let mut stack = roots;
        let mut seen = std::collections::HashSet::new();
        let mut apps = Vec::new();
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push(path);
                    continue;
                }
                if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk")) {
                    continue;
                }
                let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                let lower = name.to_lowercase();
                if lower.contains("uninstall") || lower.contains("deinstall") {
                    continue;
                }
                let Some(target) = shortcut_target(&link, &file, &path) else { continue };
                if seen.insert(crate::path_key(&target)) {
                    apps.push(AppInfo { name, path: target, bundle_id: None });
                }
            }
        }
        sort_by_name(&mut apps);
        apps
    }
}
