//! Opt-in, per-user login startup for the main app, independent of backups.
//! Registration never launches a process or terminates the running app.

#[cfg(any(target_os = "macos", windows, test))]
use std::path::Path;

use anyhow::Result;

pub const BACKGROUND_FLAG: &str = "--background";
#[cfg(any(target_os = "macos", windows, test))]
const LABEL: &str = "dev.rile.app";

#[cfg(target_os = "macos")]
fn plist_path() -> Result<std::path::PathBuf> {
    use anyhow::Context;
    Ok(dirs::home_dir().context("no home folder")?.join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
}

pub fn is_enabled() -> bool {
    #[cfg(target_os = "macos")]
    return plist_path().is_ok_and(|path| path.is_file());
    #[cfg(windows)]
    return windows_registry::CURRENT_USER.open(RUN_KEY).and_then(|key| key.get_string(LABEL)).is_ok();
    #[cfg(not(any(target_os = "macos", windows)))]
    false
}

pub fn set_enabled(enabled: bool) -> Result<()> {
    anyhow::ensure!(std::env::var_os("RILE_NO_LAUNCH_AGENTS").is_none(), "Login startup is disabled by RILE_NO_LAUNCH_AGENTS");
    #[cfg(target_os = "macos")]
    {
        let content = if enabled {
            Some(plist(&std::env::current_exe()?, std::env::var_os("RILE_CONFIG_DIR").as_deref().map(Path::new)))
        } else {
            None
        };
        // launchd reads this at the next login. Do not bootstrap/bootout here:
        // those would start a second app or kill the current login-launched app.
        write_plist(&plist_path()?, content.as_deref())
    }
    #[cfg(windows)]
    {
        let key = windows_registry::CURRENT_USER.create(RUN_KEY)?;
        if enabled {
            let command = windows_command(&std::env::current_exe()?, std::env::var_os("RILE_CONFIG_DIR").as_deref().map(Path::new));
            key.set_string(LABEL, &command)?;
        } else if let Err(error) = key.remove_value(LABEL) {
            // Registry and Win32 bindings use different windows-result versions.
            if error.code().0 != windows::core::HRESULT::from_win32(2).0 {
                return Err(error.into());
            }
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = enabled;
        anyhow::bail!("Login startup is only available on macOS and Windows")
    }
}

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

#[cfg(any(target_os = "macos", test))]
fn write_plist(path: &Path, content: Option<&str>) -> Result<()> {
    use std::io::Write;
    if let Some(content) = content {
        let parent = path.parent().ok_or_else(|| anyhow::anyhow!("no login item folder"))?;
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(content.as_bytes())?;
        file.persist(path)?;
    } else if let Err(error) = std::fs::remove_file(path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error.into());
        }
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
fn plist(exe: &Path, config: Option<&Path>) -> String {
    // LaunchServices reuses an already-running app, even if login and a manual
    // launch race. Development executables outside a bundle are run directly.
    let bundle = exe.ancestors().nth(3).filter(|path| {
        path.extension().is_some_and(|ext| ext == "app")
            && exe.parent().is_some_and(|p| p.ends_with("Contents/MacOS"))
    });
    let mut args = match bundle {
        Some(bundle) => vec!["/usr/bin/open".into(), "-g".into(), "-j".into(), bundle.to_string_lossy().into_owned(), "--args".into()],
        None => vec![exe.to_string_lossy().into_owned()],
    };
    args.push(BACKGROUND_FLAG.into());
    if let Some(config) = config {
        args.extend(["--config-dir".into(), config.to_string_lossy().into_owned()]);
    }
    let args = args.iter().map(|arg| {
        let escaped = arg.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        format!("        <string>{escaped}</string>\n")
    }).collect::<String>();
    format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
{args}    </array>
    <key>RunAtLoad</key><true/>
    <key>LimitLoadToSessionType</key><string>Aqua</string>
</dict>
</plist>
"#)
}

#[cfg(any(windows, test))]
fn windows_command(exe: &Path, config: Option<&Path>) -> String {
    fn quote(value: &str) -> String {
        // Windows command-line rules: double backslashes before quotes and at
        // the end of quoted arguments (notably a config folder ending in \).
        let mut out = String::from("\"");
        let mut slashes = 0;
        for ch in value.chars() {
            if ch == '\\' {
                slashes += 1;
            } else {
                out.extend(std::iter::repeat_n('\\', slashes * if ch == '"' { 2 } else { 1 }));
                if ch == '"' { out.push('\\'); }
                out.push(ch);
                slashes = 0;
            }
        }
        out.extend(std::iter::repeat_n('\\', slashes * 2));
        out.push('"');
        out
    }
    let mut command = format!("{} {BACKGROUND_FLAG}", quote(&exe.to_string_lossy()));
    if let Some(config) = config {
        command.push_str(&format!(" --config-dir {}", quote(&config.to_string_lossy())));
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_plist_launches_bundle_hidden_without_restarting_after_quit() {
        let content = plist(Path::new("/Applications/Rile & Tools.app/Contents/MacOS/rile"), Some(Path::new("/tmp/a<b")));
        assert!(content.contains("<string>/usr/bin/open</string>"));
        assert!(content.contains("<string>/Applications/Rile &amp; Tools.app</string>"));
        for arg in ["-g", "-j", "--args", BACKGROUND_FLAG, "--config-dir", "/tmp/a&lt;b"] {
            assert!(content.contains(&format!("<string>{arg}</string>")));
        }
        assert!(content.contains("<key>RunAtLoad</key><true/>"));
        assert!(!content.contains("KeepAlive"));
        assert!(!content.contains("StartInterval"));
    }

    #[test]
    fn development_executable_is_not_treated_as_bundle() {
        let content = plist(Path::new("/tmp/rile/target/debug/rile"), None);
        assert!(content.contains("<string>/tmp/rile/target/debug/rile</string>"));
        assert!(!content.contains("/usr/bin/open"));
        assert!(!content.contains("--config-dir"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_accepts_the_generated_login_plist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dev.rile.app.plist");
        write_plist(&path, Some(&plist(Path::new("/Applications/Rile.app/Contents/MacOS/rile"), None))).unwrap();
        let result = std::process::Command::new("/usr/bin/plutil").arg("-lint").arg(&path).output().unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stdout));
    }

    #[test]
    fn registration_can_be_refreshed_and_removed_without_launching() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("LaunchAgents/dev.rile.app.plist");
        write_plist(&path, Some("first executable")).unwrap();
        write_plist(&path, Some("updated executable")).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "updated executable");
        write_plist(&path, None).unwrap();
        write_plist(&path, None).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn windows_startup_command_quotes_paths_and_trailing_slashes() {
        let command = windows_command(Path::new(r"C:\Program Files\Rile\rile.exe"), Some(Path::new("C:\\Rile Config\\")));
        assert_eq!(command, "\"C:\\Program Files\\Rile\\rile.exe\" --background --config-dir \"C:\\Rile Config\\\\\"");
    }
}
