//! `rilectl`: sets up Rile Server and changes what it shares (Unix only).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use inquire::autocompletion::{Autocomplete, Replacement};
use inquire::{Confirm, CustomUserError, InquireError, MultiSelect, Select, Text};
use crate::config::{Access, Config, ShareConfig, check_name, default_name};
use crate::ipc::{Client, Request, Response, Status};
use crate::{Paths, SERVICE, facts};

const UNIT_PATH: &str = "/etc/systemd/system/rile-server.service";
const UNIT_TEMPLATE: &str = include_str!("../../../packaging/linux/rile-server.service");
const DEFAULT_INBOX: &str = "/srv/rile/inbox";

#[derive(Parser)]
#[command(name = "rilectl", version, about = "Set up Rile Server and change what it shares with Rile apps nearby")]
struct Cli {
    #[command(flatten)]
    paths: PathArgs,
    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct PathArgs {
    /// Configuration file [default: /etc/rile-server/config.toml]
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,
    /// Daemon state directory [default: /var/lib/rile-server]
    #[arg(long, global = true, value_name = "DIR")]
    state: Option<PathBuf>,
    /// Daemon control socket [default: /run/rile-server/control.sock]
    #[arg(long, global = true, value_name = "FILE")]
    socket: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Interactive setup: name, shared folders, inbox, background service.
    Setup,
    /// What the server shares and who is paired.
    Status,
    /// List, add, remove or change shared folders.
    #[command(subcommand)]
    Share(ShareCmd),
    /// Let a Rile app pair with this server (asks you to confirm the code).
    Pair {
        /// How long to wait for pair requests, e.g. 90s, 5m, 1h.
        #[arg(long, default_value = "5m", value_parser = parse_duration)]
        timeout: Duration,
        /// Keep waiting after a device paired.
        #[arg(long)]
        keep_open: bool,
    },
    /// Paired devices and Rile devices seen on the network.
    Devices,
    /// Unpair a device (asks which when no id is given).
    Forget { id: Option<String> },
    /// Change the name Rile apps show for this server.
    Name { name: Option<String> },
    /// Where files sent to this server are saved.
    Inbox {
        path: Option<PathBuf>,
        /// Decline files sent to this server.
        #[arg(long, conflicts_with = "path")]
        off: bool,
    },
    /// Change the TCP port (restarts the service).
    Port { port: u16 },
    /// Apply changes made to the configuration file by hand.
    Reload,
    /// Follow the service log.
    Logs,
    /// Stop the service and remove Rile Server.
    Uninstall {
        /// Also delete the configuration and the pairing identity.
        #[arg(long)]
        purge: bool,
        #[arg(long, short)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum ShareCmd {
    /// Show the shared folders.
    List,
    /// Share a folder (asks for anything left out).
    Add {
        path: Option<PathBuf>,
        #[arg(long)]
        name: Option<String>,
        /// Let paired devices change files in it.
        #[arg(long, conflicts_with = "read_only")]
        read_write: bool,
        #[arg(long)]
        read_only: bool,
    },
    /// Stop sharing folders (pick them when no names are given).
    Remove { names: Vec<String> },
    /// Rename a share or change its access.
    Edit { name: Option<String> },
    /// Choose again: the whole machine, one folder or several.
    SetMode,
}

fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: u64 = num.parse().map_err(|_| format!("invalid duration “{s}”"))?;
    let secs = match unit {
        "" | "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        _ => return Err(format!("invalid duration “{s}” (use e.g. 90s, 5m, 1h)")),
    };
    Ok(Duration::from_secs(secs))
}

pub fn main() {
    let cli = Cli::parse();
    let paths = Paths::resolve(cli.paths.config, cli.paths.state, cli.paths.socket);
    if let Err(e) = run(&paths, cli.command) {
        if let Some(InquireError::OperationCanceled | InquireError::OperationInterrupted) = e.downcast_ref::<InquireError>() {
            eprintln!("Cancelled.");
            std::process::exit(130);
        }
        if let Some(InquireError::NotTTY) = e.downcast_ref::<InquireError>() {
            eprintln!("error: this needs an interactive terminal (pass the values as arguments, see --help)");
            std::process::exit(2);
        }
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run(paths: &Paths, command: Command) -> Result<()> {
    match command {
        Command::Setup => setup(paths),
        Command::Status => status(paths),
        Command::Share(cmd) => share(paths, cmd),
        Command::Pair { timeout, keep_open } => pair(paths, timeout, keep_open),
        Command::Devices => devices(paths),
        Command::Forget { id } => forget(paths, id),
        Command::Name { name } => {
            require_root(paths)?;
            let mut config = Config::load(&paths.config)?;
            let current = facts::read().device_name(&config);
            let name = match name {
                Some(n) => n,
                None => Text::new("Name shown in Rile:").with_default(&current).prompt()?,
            };
            config.name = name.trim().to_string();
            apply(paths, &config)?;
            println!("Rile apps now see this server as “{}”.", if config.name.is_empty() { facts::read().host } else { config.name.clone() });
            Ok(())
        }
        Command::Inbox { path, off } => {
            require_root(paths)?;
            let mut config = Config::load(&paths.config)?;
            config.inbox = if off {
                None
            } else {
                match path {
                    Some(p) => Some(absolute(&p)?),
                    None => ask_inbox(config.inbox.as_deref(), true)?,
                }
            };
            if let Some(inbox) = &config.inbox {
                prepare_inbox(inbox, service_user().as_deref())?;
            }
            apply(paths, &config)?;
            match &config.inbox {
                Some(p) => println!("Files sent to this server are saved in {}.", p.display()),
                None => println!("Files sent to this server are declined."),
            }
            Ok(())
        }
        Command::Port { port } => {
            require_root(paths)?;
            let mut config = Config::load(&paths.config)?;
            config.port = port;
            config.save(&paths.config)?;
            if paths.is_system() && service_installed() {
                systemctl(&["restart", SERVICE])?;
                println!("Port changed to {port}; the service was restarted.");
            } else {
                println!("Port changed to {port}; restart rile-server to use it.");
            }
            Ok(())
        }
        Command::Reload => {
            Client::call(&paths.socket, &Request::Reload)?;
            println!("Reloaded {}.", paths.config.display());
            Ok(())
        }
        Command::Logs => {
            let err = exec("journalctl", &["-u", SERVICE, "-f", "-n", "100", "-o", "cat"]);
            bail!("can't run journalctl: {err}")
        }
        Command::Uninstall { purge, yes } => uninstall(paths, purge, yes),
    }
}

// ---------------------------------------------------------------- helpers --

fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// Changes to the system install need root.
fn require_root(paths: &Paths) -> Result<()> {
    if paths.is_system() && !is_root() {
        bail!("this changes the system configuration: run it with sudo (sudo rilectl …)");
    }
    Ok(())
}

fn absolute(p: &Path) -> Result<PathBuf> {
    let p = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir()?.join(p) };
    Ok(fs::canonicalize(&p).unwrap_or(p))
}

fn has_systemd() -> bool {
    Path::new("/run/systemd/system").is_dir()
}

fn service_installed() -> bool {
    Path::new(UNIT_PATH).exists()
}

fn systemctl(args: &[&str]) -> Result<()> {
    let status = Process::new("systemctl").args(args).status().context("running systemctl")?;
    if !status.success() {
        bail!("systemctl {} failed", args.join(" "));
    }
    Ok(())
}

#[cfg(unix)]
fn exec(cmd: &str, args: &[&str]) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    Process::new(cmd).args(args).exec()
}

/// Saves the configuration and tells the running service.
fn apply(paths: &Paths, config: &Config) -> Result<()> {
    config.save(&paths.config)?;
    if paths.is_system() && service_installed() {
        sync_unit(config, None)?;
    }
    match Client::call(&paths.socket, &Request::Reload) {
        Ok(_) => {}
        Err(_) if paths.is_system() && !service_installed() => {
            eprintln!("Saved. The service isn't installed yet: run `sudo rilectl setup` to start it.");
        }
        Err(e) => eprintln!("Saved, but the running server wasn't updated: {e:#}"),
    }
    Ok(())
}

/// The user the service runs as (from the installed unit).
fn service_user() -> Option<String> {
    let unit = fs::read_to_string(UNIT_PATH).ok()?;
    unit.lines().find_map(|l| l.strip_prefix("User=")).map(|u| u.trim().to_string())
}

fn unit_text(config: &Config, user: Option<&str>) -> String {
    // Writing to /usr, /boot or /etc needs ProtectSystem off.
    let writes_system = config.shares.iter().any(|s| {
        s.access == Access::ReadWrite && ["/usr", "/boot", "/etc"].iter().any(|sys| Path::new(sys).starts_with(&s.path) || s.path.starts_with(sys))
    });
    UNIT_TEMPLATE
        .replace("@USER@", &user.filter(|u| *u != "root").map(|u| format!("User={u}\n")).unwrap_or_default())
        .replace("@PROTECT@", if writes_system { "" } else { "ProtectSystem=full\n" })
}

/// Writes the systemd unit; restarts the service when it changed.
fn sync_unit(config: &Config, user: Option<&str>) -> Result<bool> {
    let user = user.map(str::to_string).or_else(service_user);
    let text = unit_text(config, user.as_deref());
    if fs::read_to_string(UNIT_PATH).ok().as_deref() == Some(text.as_str()) {
        return Ok(false);
    }
    let existed = service_installed();
    fs::write(UNIT_PATH, text).with_context(|| format!("writing {UNIT_PATH}"))?;
    systemctl(&["daemon-reload"])?;
    if existed {
        systemctl(&["restart", SERVICE])?;
    }
    Ok(true)
}

fn user_ids(user: &str) -> Option<(u32, u32)> {
    let name = std::ffi::CString::new(user).ok()?;
    let pw = unsafe { libc::getpwnam(name.as_ptr()) };
    if pw.is_null() {
        return None;
    }
    Some(unsafe { ((*pw).pw_uid, (*pw).pw_gid) })
}

/// Creates the inbox so the service user can write to it.
fn prepare_inbox(inbox: &Path, user: Option<&str>) -> Result<()> {
    let created = !inbox.exists();
    fs::create_dir_all(inbox).with_context(|| format!("creating {}", inbox.display()))?;
    if created && let Some((uid, gid)) = user.filter(|u| *u != "root").and_then(user_ids) {
        std::os::unix::fs::chown(inbox, Some(uid), Some(gid))?;
    }
    Ok(())
}

// ------------------------------------------------------------- choosing ---

/// Completes folder paths on Tab.
#[derive(Clone, Default)]
struct DirCompleter;

impl DirCompleter {
    fn matches(input: &str) -> Vec<String> {
        let input = if input.is_empty() { "/" } else { input };
        let (dir, prefix) = match input.rfind('/') {
            Some(i) => (&input[..=i], &input[i + 1..]),
            None => ("./", input),
        };
        let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
        let mut out: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(prefix) && (prefix.starts_with('.') || !n.starts_with('.')))
            .map(|n| format!("{dir}{n}/"))
            .collect();
        out.sort();
        out
    }
}

impl Autocomplete for DirCompleter {
    fn get_suggestions(&mut self, input: &str) -> Result<Vec<String>, CustomUserError> {
        Ok(Self::matches(input).into_iter().take(30).collect())
    }

    fn get_completion(&mut self, input: &str, highlighted: Option<String>) -> Result<Replacement, CustomUserError> {
        if highlighted.is_some() {
            return Ok(highlighted);
        }
        let all = Self::matches(input);
        let Some(first) = all.first() else { return Ok(None) };
        // The longest prefix all matches share.
        let common = all.iter().fold(first.clone(), |acc, m| {
            acc.chars().zip(m.chars()).take_while(|(a, b)| a == b).map(|(a, _)| a).collect()
        });
        Ok((common.len() > input.len()).then_some(common))
    }
}

/// Walks the folder tree with the arrow keys.
fn browse(start: &Path) -> Result<Option<PathBuf>> {
    let mut dir = if start.is_dir() { start.to_path_buf() } else { PathBuf::from("/") };
    loop {
        let mut subdirs: Vec<String> = fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'))
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        subdirs.sort_by_key(|s| s.to_lowercase());
        let pick = format!("✓ Choose {}", dir.display());
        let mut options = vec![pick.clone()];
        if dir.parent().is_some() {
            options.push("↑ ..".into());
        }
        options.extend(subdirs.iter().map(|s| format!("  {s}/")));
        options.push("✗ Cancel".into());
        let choice = Select::new(&format!("Browse {}", dir.display()), options)
            .with_page_size(15)
            .with_help_message("↑↓ move · Enter open · type to filter")
            .prompt()?;
        if choice == pick {
            return Ok(Some(dir));
        } else if choice == "↑ .." {
            dir = dir.parent().map(Path::to_path_buf).unwrap_or(dir);
        } else if choice == "✗ Cancel" {
            return Ok(None);
        } else {
            dir.push(choice.trim().trim_end_matches('/'));
        }
    }
}

/// Asks for one folder: typed (Tab completes) or browsed (empty input).
fn ask_dir(prompt: &str) -> Result<Option<PathBuf>> {
    loop {
        let input = Text::new(prompt)
            .with_autocomplete(DirCompleter)
            .with_help_message("Tab completes · leave empty to browse · Esc to go back")
            .prompt_skippable()?;
        let Some(input) = input else { return Ok(None) };
        if input.trim().is_empty() {
            match browse(Path::new("/"))? {
                Some(p) => return Ok(Some(p)),
                None => continue,
            }
        }
        let path = absolute(Path::new(input.trim()))?;
        if path.is_dir() {
            return Ok(Some(path));
        }
        eprintln!("  {} is not a folder.", path.display());
    }
}

/// Folders a server is likely to share.
fn suggestions() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let children = |dir: &str| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.is_dir() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    };
    for dir in ["/home", "/srv", "/mnt"] {
        out.extend(children(dir).into_iter().filter(|p| !p.ends_with("lost+found")));
    }
    for media in children("/media") {
        out.extend(children(&media.to_string_lossy()));
    }
    for p in ["/data", "/var/www", "/opt", "/root"] {
        if Path::new(p).is_dir() {
            out.push(PathBuf::from(p));
        }
    }
    out.retain(|p| p.as_path() != Path::new(DEFAULT_INBOX) && !p.starts_with("/srv/rile"));
    out.dedup();
    out
}

/// Asks for several folders: suggestions to tick, plus typed or browsed ones.
fn ask_dirs(already: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut chosen: Vec<PathBuf> = Vec::new();
    let mut options: Vec<String> = suggestions().iter().map(|p| p.display().to_string()).collect();
    for p in already {
        let s = p.display().to_string();
        if !options.contains(&s) {
            options.push(s);
        }
    }
    if !options.is_empty() {
        let defaults: Vec<usize> =
            options.iter().enumerate().filter(|(_, o)| already.iter().any(|a| a.display().to_string() == **o)).map(|(i, _)| i).collect();
        let picked = MultiSelect::new("Folders to share:", options)
            .with_default(&defaults)
            .with_page_size(15)
            .with_help_message("Space toggles · → all · ← none · Enter continues (you can add other folders next)")
            .prompt()?;
        chosen.extend(picked.into_iter().map(PathBuf::from));
    }
    loop {
        if !chosen.is_empty() {
            println!("  Sharing so far:");
            for p in &chosen {
                println!("    • {}", p.display());
            }
        }
        const TYPE: &str = "Add a folder by path";
        const BROWSE: &str = "Browse for a folder";
        const DONE: &str = "Done";
        let mut actions = vec![DONE, TYPE, BROWSE];
        if chosen.is_empty() {
            actions.remove(0);
        }
        let next = Select::new("Add more?", actions).prompt()?;
        let added = match next {
            TYPE => ask_dir("Folder:")?,
            BROWSE => browse(Path::new("/"))?,
            _ => break,
        };
        if let Some(p) = added
            && !chosen.contains(&p)
        {
            chosen.push(p);
        }
    }
    Ok(chosen)
}

fn ask_access(name: &str, current: Access) -> Result<Access> {
    let options = vec!["Read-only — browse and download", "Read & write — also upload, rename and delete"];
    let start = if current == Access::ReadWrite { 1 } else { 0 };
    let pick = Select::new(&format!("Access to “{name}”:"), options.clone()).with_starting_cursor(start).prompt()?;
    Ok(if pick == options[1] { Access::ReadWrite } else { Access::ReadOnly })
}

fn ask_share_name(config: &Config, path: &Path, host: &str, current: Option<&str>) -> Result<String> {
    let suggested = current.map(str::to_string).unwrap_or_else(|| config.unique_name(&default_name(path, host)));
    loop {
        let name = Text::new(&format!("Name for {}:", path.display())).with_default(&suggested).prompt()?;
        let name = name.trim().to_string();
        if let Err(e) = check_name(&name) {
            eprintln!("  {e}");
            continue;
        }
        if current.is_none_or(|c| !c.eq_ignore_ascii_case(&name)) && config.share(&name).is_some() {
            eprintln!("  Another shared folder is already called “{name}”.");
            continue;
        }
        return Ok(name);
    }
}

/// Step 2–4 of the setup: what to share and how.
fn ask_shares(config: &mut Config, host: &str) -> Result<()> {
    const WHOLE: &str = "The whole machine (/)";
    const ONE: &str = "One folder";
    const MANY: &str = "Several folders";
    let current: Vec<PathBuf> = config.shares.iter().map(|s| s.path.clone()).collect();
    let start = match current.as_slice() {
        [p] if p.as_path() == Path::new("/") => 0,
        [_] => 1,
        [] => 1,
        _ => 2,
    };
    let mode = Select::new("What should Rile apps see on this server?", vec![WHOLE, ONE, MANY]).with_starting_cursor(start).prompt()?;
    let paths = match mode {
        WHOLE => vec![PathBuf::from("/")],
        ONE => match ask_dir("Folder to share:")? {
            Some(p) => vec![p],
            None => return ask_shares(config, host),
        },
        _ => ask_dirs(&current)?,
    };
    let previous = std::mem::take(&mut config.shares);
    for path in paths {
        let old = previous.iter().find(|s| s.path == path);
        let name = ask_share_name(config, &path, host, old.map(|s| s.name.as_str()))?;
        let default = old.map(|s| s.access).unwrap_or_default();
        let access = ask_access(&name, default)?;
        config.shares.push(ShareConfig { name, path, access });
    }
    if mode == WHOLE && config.shares[0].access == Access::ReadWrite {
        eprintln!("  Note: paired devices can now change any file the service user may change.");
    }
    Ok(())
}

fn ask_inbox(current: Option<&Path>, default_on: bool) -> Result<Option<PathBuf>> {
    let receive = Confirm::new("Accept files sent to this server with “Send to”?")
        .with_default(default_on)
        .with_help_message("They are saved in an inbox folder; only paired devices can send")
        .prompt()?;
    if !receive {
        return Ok(None);
    }
    let default = current.map(|p| p.display().to_string()).unwrap_or_else(|| DEFAULT_INBOX.to_string());
    let path = Text::new("Inbox folder:").with_default(&default).with_autocomplete(DirCompleter).prompt()?;
    Ok(Some(absolute(Path::new(path.trim()))?))
}

// ---------------------------------------------------------------- setup ---

fn setup(paths: &Paths) -> Result<()> {
    require_root(paths)?;
    let facts = facts::read();
    let mut config = Config::load(&paths.config)?;
    let fresh = config.shares.is_empty() && !paths.config.exists();
    println!("\n  Rile Server setup — {} · {}\n", facts.host, facts.os);

    let name = Text::new("Name shown in Rile:").with_default(&facts.device_name(&config)).prompt()?;
    config.name = if name.trim() == facts.host { String::new() } else { name.trim().to_string() };

    let keep = !config.shares.is_empty()
        && Confirm::new(&format!(
            "Keep the current shared folders ({})?",
            config.shares.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
        ))
        .with_default(true)
        .prompt()?;
    if !keep {
        ask_shares(&mut config, &facts.host)?;
    }
    config.inbox = ask_inbox(config.inbox.as_deref(), fresh || config.inbox.is_some())?;

    // Which account the service runs as decides what it can read and change.
    let sudo_user = std::env::var("SUDO_USER").ok().filter(|u| !u.is_empty() && u != "root");
    let whole = config.shares.iter().any(|s| s.path.as_path() == Path::new("/"));
    let user = match (&sudo_user, paths.is_system()) {
        (Some(u), true) => {
            let as_root = "root — can read (and, for read & write shares, change) every file".to_string();
            let as_user = format!("{u} — only what {u} can access");
            let current = service_user();
            let start = if current.as_deref() == Some(u.as_str()) || (current.is_none() && !whole && !fresh) { 1 } else if whole { 0 } else { 1 };
            let pick = Select::new("Run the service as:", vec![as_root.clone(), as_user]).with_starting_cursor(start).prompt()?;
            if pick == as_root { None } else { Some(u.clone()) }
        }
        _ => service_user(),
    };

    if let Some(inbox) = &config.inbox {
        prepare_inbox(inbox, user.as_deref())?;
    }
    config.save(&paths.config)?;
    println!("✓ Saved {}", paths.config.display());

    if paths.is_system() && has_systemd() {
        sync_unit(&config, Some(user.as_deref().unwrap_or("root")))?;
        systemctl(&["enable", "--now", SERVICE])?;
        let _ = Client::call(&paths.socket, &Request::Reload);
        println!("✓ The rile-server service is running and starts with the system");
    } else if paths.is_system() {
        println!("! systemd wasn't found. Start the server yourself with: rile-server run");
    } else {
        println!("  Test setup: start the server with `rile-server run --config {}`", paths.config.display());
    }
    open_firewall(config.port)?;

    println!();
    print_shares(&config);
    if paths.is_system() && has_systemd() && Confirm::new("Pair a Rile app with this server now?").with_default(true).prompt()? {
        std::thread::sleep(Duration::from_millis(500)); // let the service come up
        pair(paths, Duration::from_secs(300), false)?;
    } else {
        println!("\nPair a Rile app later with: sudo rilectl pair");
    }
    Ok(())
}

fn command_ok(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Process::new(cmd).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Offers to let Rile through an active ufw / firewalld.
fn open_firewall(port: u16) -> Result<()> {
    let tcp = format!("{port}/tcp");
    if command_ok("ufw", &["status"]).is_some_and(|s| s.contains("Status: active")) {
        if Confirm::new(&format!("ufw is active. Allow Rile ({tcp} and mDNS 5353/udp)?")).with_default(true).prompt()? {
            for rule in [tcp.as_str(), "5353/udp"] {
                Process::new("ufw").args(["allow", rule]).status()?;
            }
        }
    } else if command_ok("firewall-cmd", &["--state"]).is_some_and(|s| s.trim() == "running")
        && Confirm::new(&format!("firewalld is running. Allow Rile ({tcp} and mDNS)?")).with_default(true).prompt()?
    {
        Process::new("firewall-cmd").args(["--permanent", &format!("--add-port={tcp}"), "--add-service=mdns"]).status()?;
        Process::new("firewall-cmd").arg("--reload").status()?;
    }
    Ok(())
}

// ------------------------------------------------------------- commands ---

fn print_shares(config: &Config) {
    if config.shares.is_empty() {
        println!("Nothing is shared yet. Add a folder with: sudo rilectl share add");
        return;
    }
    println!("Shared folders:");
    let width = config.shares.iter().map(|s| s.name.chars().count()).max().unwrap_or(0);
    for s in &config.shares {
        let missing = if s.path.is_dir() { "" } else { "  (missing — not shared)" };
        println!("  {:width$}  {:10}  {}{missing}", s.name, s.access.label(), s.path.display());
    }
}

fn fetch_status(paths: &Paths) -> Result<Status> {
    match Client::call(&paths.socket, &Request::Status)? {
        Response::Status(s) => Ok(s),
        other => bail!("unexpected answer: {other:?}"),
    }
}

fn status(paths: &Paths) -> Result<()> {
    let config = Config::load(&paths.config)?;
    match fetch_status(paths) {
        Ok(s) => {
            println!("● {} — running", s.name);
            println!("  {} · {} · Rile Server {}", s.model, s.os, s.version);
            println!("  Device id {}   port {}", rile_remote::peer::short_id(&s.id), s.port);
            match &s.inbox {
                Some(p) => println!("  Inbox     {}", p.display()),
                None => println!("  Inbox     off (files sent here are declined)"),
            }
            println!();
            print_shares(&config);
            let paired: Vec<_> = s.devices.iter().filter(|d| d.paired).collect();
            println!();
            if paired.is_empty() {
                println!("No paired devices. Pair one with: sudo rilectl pair");
            } else {
                println!("Paired devices:");
                for d in paired {
                    println!("  {}  {}{}", d.name, rile_remote::peer::short_id(&d.id), if d.address.is_some() { "  · online" } else { "" });
                }
            }
        }
        Err(e) => {
            println!("○ rile-server is not reachable: {e:#}");
            println!();
            print_shares(&config);
        }
    }
    Ok(())
}

fn share(paths: &Paths, cmd: ShareCmd) -> Result<()> {
    if matches!(cmd, ShareCmd::List) {
        print_shares(&Config::load(&paths.config)?);
        return Ok(());
    }
    require_root(paths)?;
    let mut config = Config::load(&paths.config)?;
    let host = facts::read().host;
    match cmd {
        ShareCmd::List => unreachable!(),
        ShareCmd::Add { path, name, read_write, read_only } => {
            let path = match path {
                Some(p) => absolute(&p)?,
                None => match ask_dir("Folder to share:")? {
                    Some(p) => p,
                    None => return Ok(()),
                },
            };
            if !path.is_dir() {
                bail!("{} is not a folder", path.display());
            }
            if let Some(s) = config.shares.iter().find(|s| s.path == path) {
                bail!("{} is already shared as “{}”", path.display(), s.name);
            }
            let name = match name {
                Some(n) => {
                    check_name(&n)?;
                    if config.share(&n).is_some() {
                        bail!("another shared folder is already called “{n}”");
                    }
                    n
                }
                None if read_write || read_only => config.unique_name(&default_name(&path, &host)),
                None => ask_share_name(&config, &path, &host, None)?,
            };
            let access = if read_write {
                Access::ReadWrite
            } else if read_only {
                Access::ReadOnly
            } else {
                ask_access(&name, Access::ReadOnly)?
            };
            println!("Sharing {} as “{name}” ({}).", path.display(), access.label());
            config.shares.push(ShareConfig { name, path, access });
        }
        ShareCmd::Remove { names } => {
            if config.shares.is_empty() {
                bail!("nothing is shared");
            }
            let names = if names.is_empty() {
                let options: Vec<String> = config.shares.iter().map(|s| format!("{} — {}", s.name, s.path.display())).collect();
                let picked = MultiSelect::new("Stop sharing:", options.clone()).prompt()?;
                picked.iter().filter_map(|p| options.iter().position(|o| o == p)).map(|i| config.shares[i].name.clone()).collect()
            } else {
                for n in &names {
                    if config.share(n).is_none() {
                        bail!("no shared folder named “{n}”");
                    }
                }
                names
            };
            config.shares.retain(|s| !names.iter().any(|n| n.eq_ignore_ascii_case(&s.name)));
            println!("Stopped sharing {}.", names.join(", "));
        }
        ShareCmd::Edit { name } => {
            let idx = match name {
                Some(n) => config.shares.iter().position(|s| s.name.eq_ignore_ascii_case(&n)).with_context(|| format!("no shared folder named “{n}”"))?,
                None => {
                    if config.shares.is_empty() {
                        bail!("nothing is shared");
                    }
                    let options: Vec<String> =
                        config.shares.iter().map(|s| format!("{} — {} ({})", s.name, s.path.display(), s.access.label())).collect();
                    let pick = Select::new("Change which share?", options.clone()).prompt()?;
                    options.iter().position(|o| *o == pick).unwrap_or(0)
                }
            };
            let current = config.shares[idx].clone();
            let new_name = ask_share_name(&config, &current.path, &host, Some(&current.name))?;
            let access = ask_access(&new_name, current.access)?;
            config.shares[idx] = ShareConfig { name: new_name, access, ..current };
        }
        ShareCmd::SetMode => ask_shares(&mut config, &host)?,
    }
    apply(paths, &config)?;
    print_shares(&config);
    Ok(())
}

fn pair(paths: &Paths, timeout: Duration, keep_open: bool) -> Result<()> {
    let mut client = Client::connect(&paths.socket)?;
    client.send(&Request::Pair { secs: timeout.as_secs().max(10) })?;
    client.set_timeout(None)?;
    let name = fetch_status(paths).map(|s| s.name).unwrap_or_else(|_| "this server".into());
    loop {
        match client.recv()? {
            Some(Response::PairOpen { secs }) => {
                println!("\nWaiting {} for a Rile app to pair (Ctrl-C to stop).", human(secs));
                println!("In Rile, open Nearby, select “{name}” and click Pair.\n");
            }
            Some(Response::PairRequest { name, model, os, code, .. }) => {
                let about = [model, os].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
                println!("→ “{name}”{} wants to pair.", if about.is_empty() { String::new() } else { format!(" ({about})") });
                println!("  Code: {code}   — Rile must show the same code.");
                let accept = Confirm::new("Pair with it?").with_default(false).prompt().unwrap_or(false);
                client.send(&Request::Answer { accept })?;
                if accept {
                    println!("  Waiting for “{name}” to confirm…");
                }
            }
            Some(Response::PairResult { name, paired }) => {
                if paired {
                    println!("✓ Paired with {name}. It can now browse this server's shared folders.");
                    if !keep_open {
                        return Ok(());
                    }
                } else {
                    println!("✗ Not paired with {name}.");
                }
            }
            Some(Response::PairClosed) | None => {
                println!("Pairing window closed.");
                return Ok(());
            }
            Some(Response::Error { message }) => bail!(message),
            Some(_) => {}
        }
    }
}

fn human(secs: u64) -> String {
    match secs {
        s if s >= 3600 && s % 3600 == 0 => format!("{} h", s / 3600),
        s if s >= 60 && s % 60 == 0 => format!("{} min", s / 60),
        s => format!("{s} s"),
    }
}

fn devices(paths: &Paths) -> Result<()> {
    let status = fetch_status(paths)?;
    if status.devices.is_empty() {
        println!("No devices yet. Pair one with: sudo rilectl pair");
    }
    for d in &status.devices {
        let state = match (d.paired, &d.address) {
            (true, Some(a)) => format!("paired · online at {a}"),
            (true, None) => "paired · offline".into(),
            (false, Some(a)) => format!("not paired · at {a}"),
            (false, None) => "not paired".into(),
        };
        let about = [d.model.as_str(), d.os.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
        println!("{}  {}  {}{}", rile_remote::peer::short_id(&d.id), d.name, state, if about.is_empty() { String::new() } else { format!("  ({about})") });
    }
    Ok(())
}

fn forget(paths: &Paths, id: Option<String>) -> Result<()> {
    require_root(paths)?;
    let id = match id {
        Some(id) => id,
        None => {
            let paired: Vec<_> = fetch_status(paths)?.devices.into_iter().filter(|d| d.paired).collect();
            if paired.is_empty() {
                bail!("no device is paired");
            }
            let options: Vec<String> = paired.iter().map(|d| format!("{} ({})", d.name, rile_remote::peer::short_id(&d.id))).collect();
            let pick = Select::new("Unpair which device?", options.clone()).prompt()?;
            paired[options.iter().position(|o| *o == pick).unwrap_or(0)].id.clone()
        }
    };
    Client::call(&paths.socket, &Request::Forget { id })?;
    println!("Unpaired. It has to pair again to browse this server.");
    Ok(())
}

fn uninstall(paths: &Paths, purge: bool, yes: bool) -> Result<()> {
    require_root(paths)?;
    if !yes
        && !Confirm::new(if purge { "Remove Rile Server, its configuration and pairings?" } else { "Remove Rile Server (keeping its configuration)?" })
            .with_default(false)
            .prompt()?
    {
        return Ok(());
    }
    if service_installed() {
        let _ = systemctl(&["disable", "--now", SERVICE]);
        fs::remove_file(UNIT_PATH)?;
        let _ = systemctl(&["daemon-reload"]);
        println!("✓ Removed the service");
    }
    let bin_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(|| "/usr/local/bin".into());
    for bin in ["rile-server", "rilectl"] {
        let p = bin_dir.join(bin);
        if p.exists() {
            fs::remove_file(&p).with_context(|| format!("removing {}", p.display()))?;
        }
    }
    println!("✓ Removed rile-server and rilectl from {}", bin_dir.display());
    if purge {
        for dir in [paths.config.parent().map(Path::to_path_buf), Some(paths.state.clone())].into_iter().flatten() {
            if dir.exists() {
                fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
                println!("✓ Removed {}", dir.display());
            }
        }
    } else {
        println!("  Kept {} and {} (pairings). Remove them with --purge.", paths.config.display(), paths.state.display());
    }
    println!("  Firewall rules (if you added any) were left in place.");
    Ok(())
}
