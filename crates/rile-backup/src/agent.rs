//! Background scheduler for backups, started every 15 minutes as
//! `rile --run-backups` to run due jobs while Rile is closed (a launchd agent
//! on macOS, a Task Scheduler task on Windows).
//! The former `--backup-menu` agent is retained here only for migration and
//! removal; its status/actions now belong to the main app's single tray item.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Command-line flag that makes the app binary run due jobs and exit.
pub const RUN_FLAG: &str = "--run-backups";
/// Legacy login flag, now treated as a background launch of the main app.
pub const MENU_FLAG: &str = "--backup-menu";
/// `--config-dir <dir>`: sets `RILE_CONFIG_DIR` for agents started by
/// Windows, which can't pass environment variables to scheduled tasks.
pub const CONFIG_DIR_FLAG: &str = "--config-dir";

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

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
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

#[cfg(not(windows))]
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
    // A legacy --backup-menu login entry can now be running the unified app.
    // Remove its future registration without booting out this very process.
    let current = std::process::Command::new("/bin/launchctl").args(["print", &target(agent)]).output()
        .is_ok_and(|out| launchctl_pid(&String::from_utf8_lossy(&out.stdout)) == Some(std::process::id()));
    if !current && !launchctl(&["bootout", &target(agent)]) {
        launchctl(&["unload", &path.to_string_lossy()]);
    }
    std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))
}

#[cfg(any(target_os = "macos", test))]
fn launchctl_pid(output: &str) -> Option<u32> {
    output.lines().find_map(|line| line.trim().strip_prefix("pid = ").and_then(|pid| pid.parse().ok()))
}

/// Starts an installed agent that isn't running (e.g. the menu bar helper
/// after the user quit it); no effect when it already runs.
#[cfg(target_os = "macos")]
pub fn start(agent: Agent) {
    if is_installed(agent) {
        launchctl(&["kickstart", &target(agent)]);
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn install(_agent: Agent, _exe: &Path) -> Result<()> {
    anyhow::bail!("Background backups are only available on macOS and Windows")
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn uninstall(_agent: Agent) -> Result<()> {
    Ok(())
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn start(_agent: Agent) {}

#[cfg(windows)]
pub use win::{install, is_installed, start, uninstall};

/// The scheduler is a per-user Task Scheduler task (no elevation needed);
/// the status helper starts at login from the `Run` key.
#[cfg(windows)]
mod win {
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::{Command, Stdio};

    use anyhow::{Context, Result, bail};
    use windows_registry::CURRENT_USER;

    use super::Agent;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    fn task_name(agent: Agent) -> String {
        format!(r"Rile\{}", agent.label())
    }

    /// The command line the agent runs, quoted for Task Scheduler and Run.
    fn command_line(agent: Agent, exe: &Path) -> String {
        let mut cmd = format!("\"{}\" {}", exe.display(), agent.flag());
        // Agents must read the same config as the app that installed them.
        if let Some(dir) = std::env::var_os("RILE_CONFIG_DIR") {
            cmd.push_str(&format!(" {} \"{}\"", super::CONFIG_DIR_FLAG, dir.to_string_lossy()));
        }
        cmd
    }

    fn schtasks(args: &[&str]) -> std::io::Result<std::process::Output> {
        Command::new("schtasks.exe").args(args).stdin(Stdio::null()).creation_flags(CREATE_NO_WINDOW).output()
    }

    /// The command an installed scheduler task runs, if any.
    fn installed_task_command(agent: Agent) -> Option<String> {
        let out = schtasks(&["/Query", "/TN", &task_name(agent), "/XML"]).ok().filter(|o| o.status.success())?;
        // schtasks /XML writes UTF-16 when redirected on some systems.
        let xml = if out.stdout.starts_with(&[0xff, 0xfe]) {
            String::from_utf16_lossy(&out.stdout[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
        } else {
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let tag = |name: &str| {
            let start = xml.find(&format!("<{name}>"))? + name.len() + 2;
            let end = xml[start..].find(&format!("</{name}>"))? + start;
            Some(xml[start..end].replace("&quot;", "\"").replace("&amp;", "&"))
        };
        Some(format!("\"{}\" {}", tag("Command")?.trim_matches('"'), tag("Arguments").unwrap_or_default()).trim_end().to_string())
    }

    pub fn is_installed(agent: Agent) -> bool {
        match agent {
            Agent::Scheduler => installed_task_command(agent).is_some(),
            Agent::MenuBar => CURRENT_USER.open(RUN_KEY).and_then(|k| k.get_string(agent.label())).is_ok(),
        }
    }

    pub fn install(agent: Agent, exe: &Path) -> Result<()> {
        let cmd = command_line(agent, exe);
        match agent {
            Agent::Scheduler => {
                if installed_task_command(agent).is_some_and(|c| c == cmd) {
                    return Ok(());
                }
                let out = schtasks(&["/Create", "/F", "/SC", "MINUTE", "/MO", "15", "/TN", &task_name(agent), "/TR", &cmd])
                    .context("running schtasks")?;
                if !out.status.success() {
                    bail!("Couldn't schedule background backups: {}", String::from_utf8_lossy(&out.stderr).trim());
                }
            }
            Agent::MenuBar => CURRENT_USER.create(RUN_KEY)?.set_string(agent.label(), &cmd).context("adding Rile to startup apps")?,
        }
        Ok(())
    }

    pub fn uninstall(agent: Agent) -> Result<()> {
        match agent {
            Agent::Scheduler => {
                if is_installed(agent) {
                    schtasks(&["/Delete", "/F", "/TN", &task_name(agent)]).context("running schtasks")?;
                }
            }
            Agent::MenuBar => {
                if let Ok(key) = CURRENT_USER.create(RUN_KEY) {
                    key.remove_value(agent.label()).ok();
                }
            }
        }
        Ok(())
    }

    /// Launches the status helper now (it exits at once when one already runs).
    pub fn start(agent: Agent) {
        if agent == Agent::MenuBar && is_installed(agent) {
            if let Ok(exe) = std::env::current_exe() {
                let mut cmd = Command::new(exe);
                cmd.arg(agent.flag()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(CREATE_NO_WINDOW);
                if let Some(dir) = std::env::var_os("RILE_CONFIG_DIR") {
                    cmd.arg(super::CONFIG_DIR_FLAG).arg(dir);
                }
                cmd.spawn().ok();
            }
        }
    }
}

/// Removes both agents (used by the Windows uninstaller).
pub fn uninstall_all() {
    for agent in [Agent::Scheduler, Agent::MenuBar] {
        uninstall(agent).ok();
    }
}

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

/// Keeps the headless scheduler in line with the configuration. The former
/// menu helper is retired by the app only after its active backups finish.
pub fn sync(config: &crate::BackupConfig) -> Result<()> {
    // Development runs can opt out of touching the user's launch agents.
    if std::env::var_os("RILE_NO_LAUNCH_AGENTS").is_some() {
        return Ok(());
    }
    let exe = std::env::current_exe().context("locating the Rile program")?;
    if config.wants_agent() {
        install(Agent::Scheduler, &exe)
    } else {
        uninstall(Agent::Scheduler)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_self_before_retiring_a_legacy_login_job() {
        assert_eq!(launchctl_pid("gui/501/dev.rile.backup-menu = {\n\tpid = 1234\n}"), Some(1234));
        assert_eq!(launchctl_pid("state = waiting\nlast exit code = 0"), None);
    }

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
