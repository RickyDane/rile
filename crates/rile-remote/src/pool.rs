//! Reconnecting wrapper and a process-wide connection pool.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::Result;

use crate::backend::{self, Backend};
use crate::{ConnectionInfo, Protocol, RemoteEntry, RemoteFs, is_connection_lost, paths};

fn open_backend(info: &ConnectionInfo, secret: Option<&str>) -> Result<Box<dyn Backend>> {
    Ok(match info.protocol {
        Protocol::Ftp | Protocol::Ftps => Box::new(crate::ftp::FtpBackend::connect(info, secret)?),
        Protocol::Sftp => Box::new(crate::sftp::SftpBackend::connect(info, secret)?),
        Protocol::Smb => Box::new(crate::smb::SmbBackend::connect(info, secret)?),
        Protocol::Peer => Box::new(crate::peer::PeerBackend::connect(info)?),
        Protocol::GoogleDrive => Box::new(crate::gdrive::DriveBackend::connect(info, secret)?),
        Protocol::OneDrive => Box::new(crate::onedrive::OneDriveBackend::connect(info, secret)?),
    })
}

/// A [`RemoteFs`] that reconnects once when an operation fails because the
/// connection dropped.
pub(crate) struct Reconnecting {
    info: ConnectionInfo,
    secret: Option<String>,
    inner: Option<Box<dyn Backend>>,
}

impl Reconnecting {
    pub(crate) fn connect(info: &ConnectionInfo, secret: Option<&str>) -> Result<Self> {
        let inner = open_backend(info, secret)?;
        Ok(Self {
            info: info.clone(),
            secret: secret.map(str::to_string),
            inner: Some(inner),
        })
    }

    fn backend(&mut self) -> Result<&mut dyn Backend> {
        if self.inner.as_ref().is_none_or(|b| !b.is_alive()) {
            self.inner = None;
            log::info!("reconnecting to {}", self.info.display());
            self.inner = Some(open_backend(&self.info, self.secret.as_deref())?);
        }
        Ok(self.inner.as_deref_mut().expect("connected above"))
    }

    /// Run `op`; if it fails with a lost connection and `retry_ok()` allows it,
    /// reconnect and run it once more.
    fn run<R>(
        &mut self,
        mut op: impl FnMut(&mut dyn Backend) -> Result<R>,
        retry_ok: impl Fn() -> bool,
    ) -> Result<R> {
        let first = op(self.backend()?);
        match first {
            Err(e) if is_connection_lost(&e) => {
                self.inner = None;
                if !retry_ok() {
                    return Err(e);
                }
                log::info!("{e:#}; retrying after reconnect");
                let res = op(self.backend()?);
                if res.as_ref().is_err_and(is_connection_lost) {
                    self.inner = None;
                }
                res
            }
            other => {
                if self.inner.as_ref().is_some_and(|b| !b.is_alive()) {
                    self.inner = None;
                }
                other
            }
        }
    }

    fn simple<R>(&mut self, op: impl FnMut(&mut dyn Backend) -> Result<R>) -> Result<R> {
        self.run(op, || true)
    }
}

impl RemoteFs for Reconnecting {
    fn home_dir(&mut self) -> Result<String> {
        let login = self.simple(|b| b.login_dir())?;
        Ok(match self.info.initial_path.as_deref().map(str::trim) {
            Some(p) if !p.is_empty() => paths::join(&login, p),
            _ => login,
        })
    }

    fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>> {
        let path = paths::normalize(path);
        self.simple(|b| b.list(&path))
    }

    fn stat(&mut self, path: &str) -> Result<RemoteEntry> {
        let path = paths::normalize(path);
        self.simple(|b| b.stat(&path))
    }

    fn mkdir(&mut self, path: &str) -> Result<()> {
        let path = paths::normalize(path);
        self.simple(|b| b.mkdir(&path))
    }

    fn create_file(&mut self, path: &str) -> Result<()> {
        let path = paths::normalize(path);
        self.simple(|b| b.write_file(&path, &mut std::io::empty(), 0))
    }

    fn remove(&mut self, path: &str) -> Result<()> {
        // Protect share roots before recursive removal visits any children.
        let path = paths::normalize(path);
        if self.info.protocol == Protocol::Smb && !path.trim_matches('/').contains('/') {
            anyhow::bail!("refusing to remove an SMB server or share root");
        }
        self.simple(|b| backend::remove_recursive(b, &path))
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        let (from, to) = (paths::normalize(from), paths::normalize(to));
        self.simple(|b| b.rename(&from, &to))
    }

    fn download(
        &mut self,
        remote: &str,
        local: &Path,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<()> {
        let remote = paths::normalize(remote);
        let moved = std::cell::Cell::new(0u64);
        self.run(
            |b| {
                let mut n = 0;
                let r = backend::download(b, &remote, local, cancel, progress, &mut n);
                moved.set(moved.get() + n);
                r
            },
            || moved.get() == 0,
        )
    }

    fn upload(
        &mut self,
        local: &Path,
        remote: &str,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<()> {
        let moved = std::cell::Cell::new(0u64);
        self.run(
            |b| {
                let mut n = 0;
                let r = backend::upload(b, local, remote, cancel, progress, &mut n);
                moved.set(moved.get() + n);
                r
            },
            || moved.get() == 0,
        )
    }

    fn read_head(&mut self, path: &str, max: usize) -> Result<Vec<u8>> {
        let path = paths::normalize(path);
        self.simple(|b| backend::read_head(b, &path, max))
    }
}

type Shared = Arc<Mutex<Box<dyn RemoteFs>>>;

/// Shares one live connection per [`ConnectionInfo::id`] across threads.
pub struct Pool {
    conns: Mutex<HashMap<String, (ConnectionInfo, Shared)>>,
}

impl Pool {
    pub fn new() -> Self {
        Self {
            conns: Mutex::new(HashMap::new()),
        }
    }

    pub fn global() -> &'static Pool {
        static POOL: OnceLock<Pool> = OnceLock::new();
        POOL.get_or_init(Pool::new)
    }

    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, (ConnectionInfo, Shared)>> {
        self.conns.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Return the pooled connection for `info.id`, connecting if needed. If the
    /// stored settings for that id differ from `info`, the old connection is replaced.
    /// The connect itself happens without holding the pool lock.
    pub fn get_or_connect(&self, info: &ConnectionInfo, secret: Option<&str>) -> Result<Shared> {
        if let Some((existing, conn)) = self.map().get(&info.id)
            && existing == info
        {
            return Ok(conn.clone());
        }
        let conn: Shared = Arc::new(Mutex::new(connect_boxed(info, secret)?));
        let mut map = self.map();
        match map.get(&info.id) {
            // Another thread won the race with identical settings: use theirs.
            Some((existing, other)) if existing == info => Ok(other.clone()),
            _ => {
                map.insert(info.id.clone(), (info.clone(), conn.clone()));
                Ok(conn)
            }
        }
    }

    /// Drop the pooled connection (it closes once no other thread holds it).
    pub fn disconnect(&self, id: &str) {
        let removed = self.map().remove(id);
        drop(removed);
    }

    pub fn is_connected(&self, id: &str) -> bool {
        self.map().contains_key(id)
    }
}

impl Default for Pool {
    fn default() -> Self {
        Self::new()
    }
}

fn connect_boxed(info: &ConnectionInfo, secret: Option<&str>) -> Result<Box<dyn RemoteFs>> {
    crate::connect(info, secret)
}
