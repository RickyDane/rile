//! Connecting to another Rile device: browsing its shared folders (as a
//! [`Backend`]), pairing, and sending it files.

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rustls::pki_types::ServerName;
use rustls::{ClientConnection, StreamOwned};

use super::identity::{PeerStore, TrustedPeer, short_id};
use super::server::{LocalDevice, PROMPT_TIMEOUT};
use super::tls;
use super::wire::{self, DeviceInfo, ErrorKind, OfferItem, PROTOCOL_VERSION, PeerError, Request, Response};
use crate::backend::Backend;
use crate::{CONNECT_TIMEOUT, Cancelled, ConnectionInfo, ConnectionLost, IO_TIMEOUT, RemoteEntry, paths};

const CHUNK: usize = 256 * 1024;

static LOCAL: RwLock<Option<LocalDevice>> = RwLock::new(None);

/// What this device says about itself when it connects to others.
pub fn set_local_device(local: LocalDevice) {
    *LOCAL.write().unwrap_or_else(|e| e.into_inner()) = Some(local);
}

fn local_info() -> DeviceInfo {
    LOCAL.read().unwrap_or_else(|e| e.into_inner()).as_ref().map(|l| l.info.clone()).unwrap_or_default()
}

/// The identity of the device at an address differs from the one expected.
#[derive(Debug, Clone)]
pub struct IdentityMismatch;

impl std::fmt::Display for IdentityMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("A different device answered at this address. Wait for it to reappear under Nearby Devices.")
    }
}
impl std::error::Error for IdentityMismatch {}

pub(crate) struct PeerConn {
    tls: StreamOwned<ClientConnection, TcpStream>,
    /// The other device, as it describes itself (`id` from its certificate).
    pub info: DeviceInfo,
    /// Whether the other device has paired with this one.
    pub trusts_us: bool,
    alive: bool,
}

fn lost(e: impl std::fmt::Display) -> anyhow::Error {
    ConnectionLost(e.to_string()).into()
}

impl PeerConn {
    fn open(store: &PeerStore, host: &str, port: u16, expected: Option<&str>) -> Result<Self> {
        let addrs: Vec<_> = (host, port).to_socket_addrs().with_context(|| format!("resolving {host}"))?.collect();
        let mut last = None;
        let mut sock = None;
        for addr in addrs {
            match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                Ok(s) => {
                    sock = Some(s);
                    break;
                }
                Err(e) => last = Some(e),
            }
        }
        let sock = match sock {
            Some(s) => s,
            None => return Err(anyhow!(last.map(|e| e.to_string()).unwrap_or_else(|| "no address".into())).context(format!("connecting to {host}:{port}"))),
        };
        sock.set_nodelay(true)?;
        sock.set_read_timeout(Some(IO_TIMEOUT))?;
        sock.set_write_timeout(Some(IO_TIMEOUT))?;
        let name = ServerName::try_from(tls::SERVER_NAME)?;
        let mut tls = StreamOwned::new(ClientConnection::new(tls::client_config(&store.identity)?, name)?, sock);
        while tls.conn.is_handshaking() {
            tls.conn.complete_io(&mut tls.sock).map_err(lost)?;
        }
        let id = tls::remote_id(&tls.conn).ok_or_else(|| anyhow!("the other device sent no certificate"))?;
        if expected.is_some_and(|e| e != id) {
            bail!(IdentityMismatch);
        }
        wire::write_json(&mut tls, &Request::Hello { version: PROTOCOL_VERSION, info: local_info() }).map_err(lost)?;
        let Response::Hello { mut info, trusted, .. } = wire::read_json(&mut tls).map_err(lost)? else {
            bail!("unexpected answer from {host}");
        };
        info.id = id;
        let _ = store.refresh(&info);
        Ok(Self { tls, info, trusts_us: trusted, alive: true })
    }

    fn call(&mut self, req: &Request) -> Result<Response> {
        let result = wire::write_json(&mut self.tls, req).map_err(lost).and_then(|_| wire::read_json(&mut self.tls).map_err(lost));
        match result {
            Ok(Response::Error { kind, message }) => Err(PeerError { kind, message }.into()),
            Ok(r) => Ok(r),
            Err(e) => {
                self.alive = false;
                Err(e)
            }
        }
    }

    fn not_paired(&self) -> anyhow::Error {
        PeerError {
            kind: ErrorKind::NotPaired,
            message: format!("{} isn't paired with this device. Pair again to continue.", self.info.name),
        }
        .into()
    }
}

/// Connects to a device and returns what it says about itself, and whether it
/// has paired with this one.
pub fn probe(store: &PeerStore, host: &str, port: u16, expected: Option<&str>) -> Result<(DeviceInfo, bool)> {
    let conn = PeerConn::open(store, host, port, expected)?;
    Ok((conn.info.clone(), conn.trusts_us))
}

/// A pairing in progress: show [`Pairing::code`], then call [`Pairing::finish`].
pub struct Pairing {
    conn: PeerConn,
    store: Arc<PeerStore>,
    pub code: u32,
}

impl Pairing {
    /// Starts pairing with the device at `host:port`; it is asked to confirm too.
    pub fn start(store: Arc<PeerStore>, host: &str, port: u16, expected: Option<&str>) -> Result<Self> {
        let mut conn = PeerConn::open(&store, host, port, expected)?;
        let code = tls::pairing_code(&tls::export(&conn.tls.conn)?, store.id(), &conn.info.id);
        wire::write_json(&mut conn.tls, &Request::Pair).map_err(lost)?;
        Ok(Self { conn, store, code })
    }

    pub fn device(&self) -> &DeviceInfo {
        &self.conn.info
    }

    /// Tells the other device whether this user confirmed the code, then waits
    /// for its user. Returns whether both confirmed (and the devices are paired).
    pub fn finish(mut self, confirmed: bool) -> Result<bool> {
        wire::write_json(&mut self.conn.tls, &Request::PairConfirm { ok: confirmed }).map_err(lost)?;
        if !confirmed {
            return Ok(false);
        }
        self.conn.tls.sock.set_read_timeout(Some(PROMPT_TIMEOUT + Duration::from_secs(15)))?;
        let Response::Paired { ok } = wire::read_json(&mut self.conn.tls).map_err(lost)? else {
            bail!("unexpected answer while pairing");
        };
        if ok {
            self.store.trust(TrustedPeer::from_info(&self.conn.info))?;
        }
        Ok(ok)
    }
}

/// One file or folder to send, relative path first.
fn collect(path: &Path, rel: &str, out: &mut Vec<(OfferItem, Option<PathBuf>)>) -> Result<()> {
    let meta = fs::metadata(path).with_context(|| path.display().to_string())?;
    if meta.is_dir() {
        out.push((OfferItem { path: rel.to_string(), is_dir: true, size: 0 }, None));
        let mut children: Vec<_> = fs::read_dir(path)?.filter_map(|e| e.ok()).collect();
        children.sort_by_key(|e| e.file_name());
        for child in children {
            // Don't follow links to folders (cycles, surprises).
            if child.file_type().is_ok_and(|t| t.is_symlink()) && child.path().is_dir() {
                continue;
            }
            let name = child.file_name().to_string_lossy().into_owned();
            collect(&child.path(), &format!("{rel}/{name}"), out)?;
        }
    } else {
        out.push((OfferItem { path: rel.to_string(), is_dir: false, size: meta.len() }, Some(path.to_path_buf())));
    }
    Ok(())
}

/// Sends `paths` (files or folders) to a paired device, which asks its user
/// first. `progress(done, total)` reports bytes.
pub fn send(
    store: &PeerStore,
    host: &str,
    port: u16,
    expected: Option<&str>,
    paths: &[PathBuf],
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<()> {
    let mut items = Vec::new();
    for p in paths {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).context("no file name")?;
        collect(p, &name, &mut items)?;
    }
    let total: u64 = items.iter().map(|(i, _)| i.size).sum();
    let mut conn = PeerConn::open(store, host, port, expected)?;
    if !conn.trusts_us {
        return Err(conn.not_paired());
    }
    let offer: Vec<OfferItem> = items.iter().map(|(i, _)| i.clone()).collect();
    wire::write_json(&mut conn.tls, &Request::Offer { items: offer, total }).map_err(lost)?;
    progress(0, total, "Waiting for the other device to accept…");
    // Wait for the other user, but notice a local cancel.
    conn.tls.sock.set_read_timeout(Some(Duration::from_millis(500)))?;
    let answer = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = conn.tls.sock.shutdown(std::net::Shutdown::Both);
            bail!(Cancelled);
        }
        match wire::read_frame(&mut conn.tls) {
            Ok(wire::Frame::Json(body)) => break serde_json::from_slice::<Response>(&body)?,
            Ok(wire::Frame::Data(_)) => bail!("unexpected data"),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(e) => return Err(lost(e)),
        }
    };
    conn.tls.sock.set_read_timeout(Some(IO_TIMEOUT))?;
    match answer {
        Response::Ok => {}
        Response::Error { kind, message } => bail!(PeerError { kind, message }),
        _ => bail!("unexpected answer"),
    }
    let mut done = 0u64;
    let mut buf = vec![0u8; CHUNK];
    for (item, local) in &items {
        let Some(local) = local else { continue };
        let mut file = fs::File::open(local).with_context(|| local.display().to_string())?;
        loop {
            if cancel.load(Ordering::Relaxed) {
                let _ = conn.tls.sock.shutdown(std::net::Shutdown::Both);
                bail!(Cancelled);
            }
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            wire::write_data(&mut conn.tls, &buf[..n]).map_err(lost)?;
            done += n as u64;
            progress(done, total, &item.path);
        }
        wire::write_data(&mut conn.tls, &[]).map_err(lost)?;
    }
    conn.tls.flush().map_err(lost)?;
    match wire::read_json(&mut conn.tls).map_err(lost)? {
        Response::Done => Ok(()),
        Response::Error { kind, message } => Err(PeerError { kind, message }.into()),
        _ => bail!("unexpected answer"),
    }
}

/// Browsing a paired device's shared folders. Changes are only sent for the
/// shares it lists as writable.
pub(crate) struct PeerBackend {
    conn: PeerConn,
}

impl PeerBackend {
    /// `info.username` holds the device id; host/port where it was last seen.
    pub(crate) fn connect(info: &ConnectionInfo) -> Result<Self> {
        let store = PeerStore::app()?;
        let expected = (!info.username.is_empty()).then_some(info.username.as_str());
        let conn = PeerConn::open(&store, &info.host, info.port, expected)?;
        if !conn.trusts_us || !store.is_trusted(&conn.info.id) {
            return Err(conn.not_paired());
        }
        Ok(Self { conn })
    }
}

fn read_only() -> anyhow::Error {
    PeerError { kind: ErrorKind::Denied, message: "Shared folders on other devices are read-only".into() }.into()
}

impl PeerBackend {
    /// `path`, normalized, if it lies in a share the other device lets us change.
    fn writable(&self, path: &str) -> Result<String> {
        let path = paths::normalize(path);
        let mut parts = path.split('/').filter(|p| !p.is_empty());
        let share = parts.next().unwrap_or_default();
        if self.conn.info.writable.iter().any(|w| w == share) {
            if parts.next().is_none() {
                // Also keeps a recursive delete of a share from emptying it first.
                bail!(PeerError { kind: ErrorKind::Denied, message: format!("The shared folder “{share}” itself can't be changed") });
            }
            Ok(path)
        } else if share.is_empty() || !self.conn.info.writable.is_empty() {
            Err(PeerError { kind: ErrorKind::Denied, message: format!("“{share}” on {} is read-only", self.conn.info.name) }.into())
        } else {
            Err(read_only())
        }
    }

    fn change(&mut self, req: Request) -> Result<()> {
        match self.conn.call(&req)? {
            Response::Ok => Ok(()),
            _ => bail!("unexpected answer"),
        }
    }
}

impl Backend for PeerBackend {
    fn login_dir(&mut self) -> Result<String> {
        Ok("/".into())
    }

    fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>> {
        match self.conn.call(&Request::List { path: paths::normalize(path) })? {
            Response::Entries { entries } => Ok(entries.into_iter().map(Into::into).collect()),
            _ => bail!("unexpected answer"),
        }
    }

    fn stat(&mut self, path: &str) -> Result<RemoteEntry> {
        match self.conn.call(&Request::Stat { path: paths::normalize(path) })? {
            Response::Entry { entry } => Ok(entry.into()),
            _ => bail!("unexpected answer"),
        }
    }

    fn mkdir(&mut self, path: &str) -> Result<()> {
        let path = self.writable(path)?;
        self.change(Request::Mkdir { path })
    }

    fn remove_file(&mut self, path: &str) -> Result<()> {
        let path = self.writable(path)?;
        self.change(Request::RemoveFile { path })
    }

    fn remove_dir(&mut self, path: &str) -> Result<()> {
        let path = self.writable(path)?;
        self.change(Request::RemoveDir { path })
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        let (from, to) = (self.writable(from)?, self.writable(to)?);
        self.change(Request::Rename { from, to })
    }

    fn read_file(&mut self, path: &str, sink: &mut dyn FnMut(&[u8]) -> Result<bool>) -> Result<()> {
        match self.conn.call(&Request::Read { path: paths::normalize(path) })? {
            Response::Ok => {}
            _ => bail!("unexpected answer"),
        }
        loop {
            let chunk = match wire::read_data(&mut self.conn.tls) {
                Ok(Some(c)) => c,
                Ok(None) => return Ok(()),
                Err(e) => {
                    if e.downcast_ref::<PeerError>().is_none() {
                        self.conn.alive = false;
                        return Err(lost(e));
                    }
                    return Err(e);
                }
            };
            if !sink(&chunk)? {
                // The rest of the file is still in flight: reconnect next time.
                self.conn.alive = false;
                return Ok(());
            }
        }
    }

    fn write_file(&mut self, path: &str, src: &mut dyn Read, size: u64) -> Result<()> {
        let path = self.writable(path)?;
        self.change(Request::Write { path, size })?;
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = match src.read(&mut buf) {
                Ok(n) => n,
                Err(e) => {
                    // Mid-file there is no way to stop in-band: drop the connection
                    // (the other side discards the partial file).
                    self.conn.alive = false;
                    let _ = self.conn.tls.sock.shutdown(std::net::Shutdown::Both);
                    return Err(match e.get_ref().is_some_and(|inner| inner.is::<Cancelled>()) {
                        true => Cancelled.into(),
                        false => e.into(),
                    });
                }
            };
            if n == 0 {
                break;
            }
            if let Err(e) = wire::write_data(&mut self.conn.tls, &buf[..n]) {
                self.conn.alive = false;
                return Err(lost(e));
            }
        }
        let finished = wire::write_data(&mut self.conn.tls, &[])
            .and_then(|_| self.conn.tls.flush())
            .map_err(lost)
            .and_then(|_| wire::read_json::<Response>(&mut self.conn.tls).map_err(lost));
        match finished {
            Ok(Response::Done) => Ok(()),
            Ok(Response::Error { kind, message }) => Err(PeerError { kind, message }.into()),
            Ok(_) => bail!("unexpected answer"),
            Err(e) => {
                self.conn.alive = false;
                Err(e)
            }
        }
    }

    fn is_alive(&self) -> bool {
        self.conn.alive
    }

    fn atomic_writes(&self) -> bool {
        true
    }

    fn remove_all(&mut self, path: &str) -> Option<Result<()>> {
        // Refuse up front: the generic removal would empty a share (or a
        // read-only one up to the first refusal) before failing on its root.
        self.writable(path).err().map(Err)
    }
}

/// `MacBook Pro · ab12-cd34`, for messages.
pub fn describe(info: &DeviceInfo) -> String {
    if info.name.is_empty() { short_id(&info.id) } else { info.name.clone() }
}
