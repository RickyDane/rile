//! "Open in Terminal": starts a terminal window in a folder.
//!
//! `preferred` is the user's choice from settings: an app name on macOS
//! ("iTerm", "Ghostty"…), a command elsewhere ("kitty", "wt"); empty picks
//! the platform default (Terminal on macOS, Windows Terminal or PowerShell
//! on Windows, `$TERMINAL` or the first known terminal on Linux).

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// Terminals found on this computer: (label, value for settings).
pub fn installed() -> Vec<(String, String)> {
    #[cfg(target_os = "macos")]
    {
        const APPS: &[&str] = &["iTerm", "Ghostty", "Warp", "WezTerm", "Alacritty", "kitty", "Hyper", "Tabby"];
        let home_apps = dirs::home_dir().map(|h| h.join("Applications"));
        let mut out = vec![("Terminal".to_string(), "Terminal".to_string())];
        for app in APPS {
            let bundle = format!("{app}.app");
            let found = Path::new("/Applications").join(&bundle).exists() || home_apps.as_ref().is_some_and(|h| h.join(&bundle).exists());
            if found {
                out.push((if *app == "iTerm" { "iTerm2".into() } else { app.to_string() }, app.to_string()));
            }
        }
        out
    }
    #[cfg(windows)]
    {
        let mut out = Vec::new();
        if which("wt.exe").is_some() {
            out.push(("Windows Terminal".into(), "wt".into()));
        }
        out.push(("PowerShell".into(), "powershell".into()));
        if which("pwsh.exe").is_some() {
            out.push(("PowerShell 7".into(), "pwsh".into()));
        }
        out.push(("Command Prompt".into(), "cmd".into()));
        out
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        LINUX.iter().filter(|(cmd, _)| which(cmd).is_some()).map(|(cmd, _)| (cmd.to_string(), cmd.to_string())).collect()
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
const LINUX: &[(&str, &[&str])] = &[
    ("x-terminal-emulator", &[]),
    ("gnome-terminal", &["--working-directory"]),
    ("kgx", &["--working-directory"]),
    ("konsole", &["--workdir"]),
    ("xfce4-terminal", &["--working-directory"]),
    ("tilix", &["--working-directory"]),
    ("kitty", &["--directory"]),
    ("alacritty", &["--working-directory"]),
    ("wezterm", &["start", "--cwd"]),
    ("foot", &["--working-directory"]),
    ("ghostty", &[]),
    ("xterm", &[]),
];

#[cfg(not(target_os = "macos"))]
fn which(cmd: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(cmd)).find(|p| p.is_file())
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn spawn(mut cmd: Command) -> Result<()> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let program = cmd.get_program().to_string_lossy().into_owned();
    cmd.spawn().with_context(|| format!("Couldn't start {program}"))?;
    Ok(())
}

/// Opens a terminal window in `dir`.
pub fn open(dir: &Path, preferred: &str) -> Result<()> {
    if !dir.is_dir() {
        bail!("\"{}\" isn't a folder", dir.display());
    }
    let preferred = preferred.trim();
    #[cfg(target_os = "macos")]
    {
        let app = if preferred.is_empty() { "Terminal" } else { preferred };
        let mut cmd = Command::new("/usr/bin/open");
        cmd.arg("-a").arg(app).arg(dir);
        let status = cmd.status().with_context(|| format!("Couldn't start {app}"))?;
        if !status.success() {
            bail!("Couldn't open {app}. Is it installed?");
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        let choice = if preferred.is_empty() { if which("wt.exe").is_some() { "wt" } else { "powershell" } } else { preferred };
        let mut cmd = match choice.to_ascii_lowercase().as_str() {
            "wt" | "wt.exe" => {
                let mut c = Command::new("wt.exe");
                c.arg("-d").arg(dir);
                c
            }
            "cmd" | "cmd.exe" => {
                let mut c = Command::new("cmd.exe");
                c.args(["/C", "start", "cmd.exe"]);
                c
            }
            "powershell" | "pwsh" => {
                let mut c = Command::new("cmd.exe");
                c.args(["/C", "start", "", if choice == "pwsh" { "pwsh.exe" } else { "powershell.exe" }, "-NoExit"]);
                c
            }
            other => Command::new(other),
        };
        cmd.current_dir(dir);
        spawn(cmd)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let env = std::env::var("TERMINAL").unwrap_or_default();
        let choice = [preferred, env.as_str()]
            .into_iter()
            .find(|c| !c.is_empty())
            .map(str::to_string)
            .or_else(|| LINUX.iter().find(|(c, _)| which(c).is_some()).map(|(c, _)| c.to_string()))
            .context("No terminal found. Choose one in Settings → General.")?;
        let mut parts = choice.split_whitespace();
        let program = parts.next().unwrap_or_default().to_string();
        let mut cmd = Command::new(&program);
        cmd.args(parts);
        let name = Path::new(&program).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some((_, flag)) = LINUX.iter().find(|(c, _)| *c == name)
            && !flag.is_empty()
        {
            cmd.args(*flag).arg(dir);
        }
        cmd.current_dir(dir);
        spawn(cmd)
    }
}
