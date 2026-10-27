//! "Open With" support: which applications can open a file, the default one,
//! launching files with a chosen application and listing every installed
//! application for a "choose other app" picker.

use std::path::{Path, PathBuf};

use anyhow::Result;

/// An installed application.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AppInfo {
    /// Display name, e.g. "TextEdit" (no `.app` suffix).
    pub name: String,
    /// Path to the application bundle.
    pub path: PathBuf,
    /// `CFBundleIdentifier`, when the bundle has one.
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

#[cfg(not(target_os = "macos"))]
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

#[cfg(not(target_os = "macos"))]
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

#[cfg(not(target_os = "macos"))]
pub fn open_with(_paths: &[PathBuf], _app: &Path) -> Result<()> {
    anyhow::bail!("Open With is only available on macOS")
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

#[cfg(not(target_os = "macos"))]
pub fn all_applications() -> Vec<AppInfo> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn is_app_bundle(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")) && path.is_dir()
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
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
