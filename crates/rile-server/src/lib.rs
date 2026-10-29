//! Rile Server: a headless Nearby device for Linux servers.
//!
//! `rile-server` (the systemd service) shares folders with Rile apps on the
//! LAN; `rilectl` sets it up and changes what it shares.

pub mod config;
pub mod daemon;
pub mod facts;
pub mod ipc;

use std::path::PathBuf;

pub const CONFIG_PATH: &str = "/etc/rile-server/config.toml";
pub const STATE_DIR: &str = "/var/lib/rile-server";
pub const SOCKET_PATH: &str = "/run/rile-server/control.sock";
pub const SERVICE: &str = "rile-server";

/// Where the server keeps its files. Each can be overridden (flags or
/// `RILE_SERVER_CONFIG` / `RILE_SERVER_STATE` / `RILE_SERVER_SOCKET`), e.g. to
/// run it unprivileged for testing.
#[derive(Clone, Debug, PartialEq)]
pub struct Paths {
    pub config: PathBuf,
    pub state: PathBuf,
    pub socket: PathBuf,
}

impl Paths {
    pub fn resolve(config: Option<PathBuf>, state: Option<PathBuf>, socket: Option<PathBuf>) -> Self {
        let pick = |flag: Option<PathBuf>, env: &str, default: &str| {
            flag.or_else(|| std::env::var_os(env).filter(|v| !v.is_empty()).map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from(default))
        };
        Self {
            config: pick(config, "RILE_SERVER_CONFIG", CONFIG_PATH),
            state: pick(state, "RILE_SERVER_STATE", STATE_DIR),
            socket: pick(socket, "RILE_SERVER_SOCKET", SOCKET_PATH),
        }
    }

    /// The system-wide install (as opposed to a test setup).
    pub fn is_system(&self) -> bool {
        self.config == std::path::Path::new(CONFIG_PATH)
    }
}
