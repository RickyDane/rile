//! macOS launch agents for backups:
//! - the scheduler, which launchd starts every few minutes as
//!   `rile --run-backups` to run due background jobs while Rile is closed;
//! - the menu bar helper (`rile --backup-menu`), started at login, which
//!   shows running backups and their state.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Command-line flag that makes the app binary run due jobs and exit.
pub const RUN_FLAG: &str = "--run-backups";
/// Command-line flag that makes the app binary run the menu bar helper.
pub const MENU_FLAG: &str = "--backup-menu";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Scheduler,
    MenuBar,
}

impl Agent {
    pub fn label(self) -> &'static str {
        match self {
            Agent::Scheduler => "dev.rile.backup",
            Agent::MenuBar => "dev.rile.backup-menu",
        }
    }

    fn flag(self) -> &'static str {
        match self {
            Agent::Scheduler => RUN_FLAG,
            Agent::MenuBar => MENU_FLAG,
        }
    }

    fn log_name(self) -> &'static str {
        match self {
            Agent::Scheduler => "agent.log",
            Agent::MenuBar => "menu.log",
        }
    }
}

pub fn plist_path(agent: Agent) -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join("Library/LaunchAgents").join(format!("{}.plist", agent.label())))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn plist(agent: Agent, exe: &Path, log: &Path) -> String {
    let exe = xml_escape(&exe.to_string_lossy());
    let log = xml_escape(&log.to_string_lossy());
    let (label, flag) = (agent.label(), agent.flag());
    let lifetime = match agent {
        // Every 15 minutes, at low priority.
        Agent::Scheduler => "    <key>StartInterval</key>\n    <integer>900</integer>\n    <key>RunAtLoad</key>\n    <true/>\n    <key>ProcessType</key>\n    <string>Background</string>\n    <key>LowPriorityIO</key>\n    <true/>\n",
        // From login; restarted after a crash but not after "Hide".
        Agent::MenuBar => "    <key>RunAtLoad</key>\n    <true/>\n    <key>KeepAlive</key>\n    <dict>\n        <key>SuccessfulExit</key>\n        <false/>\n    </dict>\n    <key>LimitLoadToSessionType</key>\n    <string>Aqua</string>\n    <key>ProcessType</key>\n    <string>Interactive</string>\n",
    };
    // Agents must read the same config as the app that installed them.
    let env = std::env::var_os("RILE_CONFIG_DIR").map_or(String::new(), |d| {
        format!(
            "    <key>EnvironmentVariables</key>\n    <dict>\n        <key>RILE_CONFIG_DIR</key>\n        <string>{}</string>\n    </dict>\n",
            xml_escape(&d.to_string_lossy())
        )
    });
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>{flag}</string>
    </array>
{lifetime}{env}    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#
    )
}

pub fn is_installed(agent: Agent) -> bool {
    plist_path(agent).is_some_and(|p| p.exists())
}

/// Installs (or refreshes) `agent` for `exe`. Does nothing when it is
/// already installed for the same program.
#[cfg(target_os = "macos")]
pub fn install(agent: Agent, exe: &Path) -> Result<()> {
    let path = plist_path(agent).context("no home folder")?;
    let log = crate::state::job_dir("x").context("no config dir")?.with_file_name(agent.log_name());
    let content = plist(agent, exe, &log);
    if std::fs::read_to_string(&path).is_ok_and(|c| c == content) {
        return Ok(());
    }
    if path.exists() {
        launchctl(&["bootout", &target(agent)]);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
    if !launchctl(&["bootstrap", &domain(), &path.to_string_lossy()]) {
        // Older systems: fall back to the legacy interface.
        launchctl(&["load", "-w", &path.to_string_lossy()]);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn uninstall(agent: Agent) -> Result<()> {
    let Some(path) = plist_path(agent) else { return Ok(()) };
    if !path.exists() {
        return Ok(());
    }
    if !launchctl(&["bootout", &target(agent)]) {
        launchctl(&["unload", &path.to_string_lossy()]);
    }
    std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))
}

/// Starts an installed agent that isn't running (e.g. the menu bar helper
/// after the user quit it); no effect when it already runs.
#[cfg(target_os = "macos")]
pub fn start(agent: Agent) {
    if is_installed(agent) {
        launchctl(&["kickstart", &target(agent)]);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn install(_agent: Agent, _exe: &Path) -> Result<()> {
    anyhow::bail!("Background backups are only available on macOS")
}

#[cfg(not(target_os = "macos"))]
pub fn uninstall(_agent: Agent) -> Result<()> {
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn start(_agent: Agent) {}

#[cfg(target_os = "macos")]
fn domain() -> String {
    format!("gui/{}", unsafe { libc::getuid() })
}

#[cfg(target_os = "macos")]
fn target(agent: Agent) -> String {
    format!("{}/{}", domain(), agent.label())
}

#[cfg(target_os = "macos")]
fn launchctl(args: &[&str]) -> bool {
    std::process::Command::new("/bin/launchctl")
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Installs or removes both agents to match the configuration.
pub fn sync(config: &crate::BackupConfig) -> Result<()> {
    // Development runs can opt out of touching the user's launch agents.
    if std::env::var_os("RILE_NO_LAUNCH_AGENTS").is_some() {
        return Ok(());
    }
    let exe = std::env::current_exe().context("locating the Rile program")?;
    let mut result = Ok(());
    for (agent, wanted) in [(Agent::Scheduler, config.wants_agent()), (Agent::MenuBar, cfg!(target_os = "macos") && config.wants_menu_bar())] {
        let r = if wanted { install(agent, &exe) } else { uninstall(agent) };
        if r.is_err() && result.is_ok() {
            result = r;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plists_escape_paths_and_differ_per_agent() {
        let exe: &Path = "/Apps/R&D/Rile.app/Contents/MacOS/rile".as_ref();
        let p = plist(Agent::Scheduler, exe, "/tmp/a.log".as_ref());
        assert!(p.contains("R&amp;D"));
        assert!(p.contains("<string>--run-backups</string>") && p.contains("StartInterval"));
        let m = plist(Agent::MenuBar, exe, "/tmp/m.log".as_ref());
        assert!(m.contains("<string>--backup-menu</string>") && m.contains("SuccessfulExit") && !m.contains("StartInterval"));
    }
}
