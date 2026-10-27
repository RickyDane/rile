//! Remote file system access for Rile: FTP, FTPS (explicit TLS), SFTP and SMB2/3.
//!
//! The API is synchronous and blocking; callers are expected to run it on
//! background threads. SFTP (`russh`/`russh-sftp`) and SMB use private tokio
//! runtimes that never leak out of this crate. Do not call into
//! this crate from inside a tokio runtime (it uses `block_on` internally).
//!
//! Remote paths are always `/`-separated strings; see [`paths`].

mod backend;
pub mod credentials;
pub mod discovery;
mod ftp;
pub mod known_hosts;
pub mod paths;
mod pool;
mod sftp;
mod smb;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

pub use anyhow::Result;
use serde::{Deserialize, Serialize};

pub use pool::Pool;

/// Timeout for establishing TCP connections.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Timeout for individual reads/writes on an established connection.
pub(crate) const IO_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Protocol {
    Sftp,
    Ftp,
    Ftps,
    Smb,
}

impl Protocol {
    pub fn default_port(self) -> u16 {
        match self {
            Protocol::Sftp => 22,
            Protocol::Ftp | Protocol::Ftps => 21,
            Protocol::Smb => 445,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Protocol::Sftp => "SFTP",
            Protocol::Ftp => "FTP",
            Protocol::Ftps => "FTPS",
            Protocol::Smb => "SMB",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum Auth {
    /// Password authentication; the password is the `secret` passed to [`connect`].
    Password,
    /// SFTP only: private key file; `secret` is the optional passphrase.
    KeyFile { path: PathBuf },
    /// SFTP only: keys held by the running ssh-agent (`SSH_AUTH_SOCK`).
    Agent,
    /// FTP: `anonymous` login. SFTP: the `none` auth method. SMB: guest login.
    Anonymous,
}

impl Auth {
    /// Whether this auth method uses a secret (password or key passphrase).
    pub fn uses_secret(&self) -> bool {
        matches!(self, Auth::Password | Auth::KeyFile { .. })
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ConnectionInfo {
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: Auth,
    /// Directory returned by [`RemoteFs::home_dir`] instead of the login directory.
    /// Relative paths are resolved against the login directory.
    pub initial_path: Option<String>,
}

impl ConnectionInfo {
    /// Accept an SMB URL or UNC address in the server field. Credentials must
    /// stay in their dedicated fields so they never enter saved addresses.
    pub fn normalize_smb_address(&mut self) -> Result<()> {
        if self.protocol != Protocol::Smb { return Ok(()); }
        let address = self.host.trim().replace('\\', "/");
        let address = if address.starts_with("//") { format!("smb:{address}") } else { address };
        if !address.contains('/') { return Ok(()); }
        let url = url::Url::parse(&address).map_err(|_| anyhow::anyhow!("Enter a server name or smb://server/share"))?;
        if url.scheme() != "smb" || url.query().is_some() || url.fragment().is_some() {
            anyhow::bail!("Enter an SMB address without a query or fragment");
        }
        if !url.username().is_empty() || url.password().is_some() {
            anyhow::bail!("Enter SMB credentials in the Username and Password fields, not the server address");
        }
        let host = url.host_str().ok_or_else(|| anyhow::anyhow!("Enter an SMB server name"))?;
        let path = percent_encoding::percent_decode_str(url.path()).decode_utf8()
            .map_err(|_| anyhow::anyhow!("Invalid UTF-8 in SMB address"))?;
        if path != "/" && !path.is_empty() {
            if self.initial_path.as_ref().is_some_and(|p| !p.is_empty() && paths::normalize(p) != paths::normalize(&path)) {
                anyhow::bail!("The SMB address and Start folder specify different folders");
            }
            self.initial_path = Some(paths::normalize(&path));
        }
        self.host = host.trim_matches(['[', ']']).to_string();
        if let Some(port) = url.port() { self.port = port; }
        Ok(())
    }

    /// A new unique id (time + counter + pid based, hex).
    pub fn new_id() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{:x}-{:x}-{:x}", nanos, std::process::id(), n)
    }

    /// `user@host:port` (user omitted when empty; IPv6 hosts are bracketed).
    pub fn display(&self) -> String {
        let host = if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.username.is_empty() {
            format!("{}:{}", host, self.port)
        } else {
            format!("{}@{}:{}", self.username, host, self.port)
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RemoteEntry {
    pub name: String,
    /// Absolute remote path.
    pub path: String,
    /// For symlinks: whether the link target is a directory (when known).
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Unix permission bits (`0o7777` mask) when the server reports them.
    pub permissions: Option<u32>,
}

/// A connected remote file system. All paths are absolute remote paths.
pub trait RemoteFs: Send {
    /// Initial directory: `initial_path` if configured, else the cwd after login.
    fn home_dir(&mut self) -> Result<String>;
    /// Directory listing, excluding `.` and `..`.
    fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>>;
    fn stat(&mut self, path: &str) -> Result<RemoteEntry>;
    fn mkdir(&mut self, path: &str) -> Result<()>;
    /// Create (or truncate to) an empty file.
    fn create_file(&mut self, path: &str) -> Result<()>;
    /// Remove a file, symlink, or directory (recursively).
    fn remove(&mut self, path: &str) -> Result<()>;
    fn rename(&mut self, from: &str, to: &str) -> Result<()>;
    /// Download `remote` (file or directory, recursively) to the local path `local`
    /// (the destination itself, not its parent). `progress(done, total)` reports bytes;
    /// the total is computed before the transfer starts. Setting `cancel` aborts with
    /// a [`Cancelled`] error and removes the partially written file.
    fn download(
        &mut self,
        remote: &str,
        local: &Path,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<()>;
    /// Upload `local` (file or directory, recursively) to the remote path `remote`
    /// (the destination itself). Same progress/cancel semantics as [`RemoteFs::download`].
    fn upload(
        &mut self,
        local: &Path,
        remote: &str,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<()>;
    /// Read up to `max` bytes from the start of a file (for previews).
    fn read_head(&mut self, path: &str, max: usize) -> Result<Vec<u8>>;
}

/// Connect and log in. `secret` is the password (Password auth / FTP) or the key
/// passphrase (KeyFile auth). The returned connection transparently reconnects once
/// when an operation fails because the connection dropped.
pub fn connect(info: &ConnectionInfo, secret: Option<&str>) -> Result<Box<dyn RemoteFs>> {
    Ok(Box::new(pool::Reconnecting::connect(info, secret)?))
}

/// Returned (inside `anyhow::Error`) when a transfer was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("transfer cancelled")
    }
}
impl std::error::Error for Cancelled {}

/// Whether `err` (or anything in its chain) is a [`Cancelled`] error.
pub fn is_cancelled(err: &anyhow::Error) -> bool {
    err.downcast_ref::<Cancelled>().is_some()
        || err.chain().any(|e| e.is::<Cancelled>())
        || err.chain().any(|e| {
            e.downcast_ref::<std::io::Error>()
                .and_then(|io| io.get_ref())
                .is_some_and(|inner| inner.is::<Cancelled>())
        })
}

/// The connection to the server was lost (the operation may be retried after reconnecting).
#[derive(Debug, Clone)]
pub struct ConnectionLost(pub String);

impl fmt::Display for ConnectionLost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "connection lost: {}", self.0)
    }
}
impl std::error::Error for ConnectionLost {}

pub(crate) fn is_connection_lost(err: &anyhow::Error) -> bool {
    err.downcast_ref::<ConnectionLost>().is_some()
        || err.chain().any(|e| e.is::<ConnectionLost>())
        || smb::is_connection_lost(err)
}

/// The server's host key (SFTP) or certificate (FTPS) differs from the recorded one.
#[derive(Debug, Clone)]
pub struct HostKeyMismatch {
    pub host: String,
    pub expected: String,
    pub actual: String,
    pub known_hosts: PathBuf,
}

impl fmt::Display for HostKeyMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "WARNING: the identity of {} has changed (expected {}, got {}). \
             This could mean someone is intercepting the connection. If the change is \
             expected, remove the entry for this host from {}.",
            self.host,
            self.expected,
            self.actual,
            self.known_hosts.display()
        )
    }
}
impl std::error::Error for HostKeyMismatch {}
