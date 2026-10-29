//! The side of a Rile device that other devices connect to: serves its shared
//! folders (read-only unless a share allows changes) and receives files it accepts.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rustls::{ServerConnection, StreamOwned};

use super::identity::{PeerStore, TrustedPeer};
use super::tls;
use super::wire::{
    self, DeviceInfo, ErrorKind, OfferItem, PROTOCOL_VERSION, PeerError, Request, Response, WireEntry,
};
use crate::paths;

/// How long a pair request or file offer waits for the user.
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(120);
/// Idle connections are closed after this; clients reconnect transparently.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const CHUNK: usize = 256 * 1024;

/// A folder this device shares with paired devices.
#[derive(Clone, Debug, PartialEq)]
pub struct Share {
    pub name: String,
    pub path: PathBuf,
    /// Paired devices may create, upload, rename and delete inside it.
    pub writable: bool,
}

/// What this device publishes. The app replaces it when settings change.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalDevice {
    pub info: DeviceInfo,
    pub shares: Vec<Share>,
}

/// The answer to a [`PeerEvent`]. Dropping it without answering declines.
pub struct Reply<T> {
    tx: SyncSender<T>,
    abandoned: Arc<AtomicBool>,
}

impl<T> Reply<T> {
    pub fn send(self, value: T) {
        let _ = self.tx.try_send(value);
    }

    /// The other device gave up (or the prompt timed out): the question can
    /// be dismissed.
    pub fn is_abandoned(&self) -> bool {
        self.abandoned.load(Ordering::Relaxed)
    }

    /// A shareable handle on [`Reply::is_abandoned`].
    pub fn abandoned_flag(&self) -> Arc<AtomicBool> {
        self.abandoned.clone()
    }
}

fn reply_pair<T>() -> (Reply<T>, std::sync::mpsc::Receiver<T>, Arc<AtomicBool>) {
    let (tx, rx) = sync_channel(1);
    let abandoned = Arc::new(AtomicBool::new(false));
    (Reply { tx, abandoned: abandoned.clone() }, rx, abandoned)
}

/// Progress of files being received.
#[derive(Debug)]
pub struct Transfer {
    pub total: u64,
    pub files_total: usize,
    pub cancel: Arc<AtomicBool>,
    done: AtomicU64,
    files_done: AtomicUsize,
    current: Mutex<Option<String>>,
    outcome: Mutex<Option<Result<Vec<PathBuf>, String>>>,
}

impl Transfer {
    fn new(total: u64, files_total: usize) -> Self {
        Self {
            total,
            files_total,
            cancel: Arc::new(AtomicBool::new(false)),
            done: AtomicU64::new(0),
            files_done: AtomicUsize::new(0),
            current: Mutex::new(None),
            outcome: Mutex::new(None),
        }
    }

    /// Bytes received, files finished, and the file being received.
    pub fn progress(&self) -> (u64, usize, Option<String>) {
        (
            self.done.load(Ordering::Relaxed),
            self.files_done.load(Ordering::Relaxed),
            self.current.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        )
    }

    /// `Some` once finished: the received top-level items, or an error.
    pub fn outcome(&self) -> Option<Result<Vec<PathBuf>, String>> {
        self.outcome.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn finish(&self, outcome: Result<Vec<PathBuf>, String>) {
        *self.outcome.lock().unwrap_or_else(|e| e.into_inner()) = Some(outcome);
    }
}

/// Something only the user can decide.
pub enum PeerEvent {
    /// `device` wants to pair. Both screens show `code`; answer `true` to pair.
    PairRequest { device: DeviceInfo, code: u32, reply: Reply<bool> },
    /// A paired device wants to send files. Answer with the folder to save
    /// them in, or `None` to decline. `transfer` reports progress afterwards.
    Offer { device: DeviceInfo, items: Vec<OfferItem>, total: u64, reply: Reply<Option<PathBuf>>, transfer: Arc<Transfer> },
}

impl std::fmt::Debug for PeerEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeerEvent::PairRequest { device, code, .. } => write!(f, "PairRequest({}, {code})", device.name),
            PeerEvent::Offer { device, items, .. } => write!(f, "Offer({}, {} items)", device.name, items.len()),
        }
    }
}

type Events = Arc<dyn Fn(PeerEvent) + Send + Sync>;

struct Ctx {
    store: Arc<PeerStore>,
    config: Arc<rustls::ServerConfig>,
    local: RwLock<LocalDevice>,
    events: Events,
    stop: AtomicBool,
}

/// Accepts connections from other devices until dropped.
pub struct PeerServer {
    ctx: Arc<Ctx>,
    port: u16,
}

impl PeerServer {
    /// Listens on `preferred_port` (any free port if that is taken) on all interfaces.
    pub fn start(
        store: Arc<PeerStore>,
        local: LocalDevice,
        preferred_port: u16,
        events: impl Fn(PeerEvent) + Send + Sync + 'static,
    ) -> Result<Self> {
        let listener = TcpListener::bind(("0.0.0.0", preferred_port))
            .or_else(|_| TcpListener::bind(("0.0.0.0", 0)))
            .context("starting the Nearby Devices server")?;
        let port = listener.local_addr()?.port();
        let ctx = Arc::new(Ctx {
            config: tls::server_config(&store.identity)?,
            store,
            local: RwLock::new(local),
            events: Arc::new(events),
            stop: AtomicBool::new(false),
        });
        let accept_ctx = ctx.clone();
        std::thread::Builder::new().name("rile-peer-accept".into()).spawn(move || {
            for stream in listener.incoming() {
                if accept_ctx.stop.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let ctx = accept_ctx.clone();
                let _ = std::thread::Builder::new().name("rile-peer-conn".into()).spawn(move || {
                    let peer = stream.peer_addr().ok();
                    if let Err(e) = serve(stream, &ctx) {
                        log::debug!("nearby: connection from {peer:?} ended: {e:#}");
                    }
                });
            }
        })?;
        Ok(Self { ctx, port })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Replaces what this device publishes (name, shares, ...).
    pub fn update(&self, local: LocalDevice) {
        *self.ctx.local.write().unwrap_or_else(|e| e.into_inner()) = local;
    }
}

impl Drop for PeerServer {
    fn drop(&mut self) {
        self.ctx.stop.store(true, Ordering::Relaxed);
        // Wake the blocking accept so its thread can exit.
        let _ = TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], self.port)), Duration::from_millis(200));
    }
}

type Tls = StreamOwned<ServerConnection, TcpStream>;

fn serve(sock: TcpStream, ctx: &Ctx) -> Result<()> {
    if ctx.stop.load(Ordering::Relaxed) {
        return Ok(());
    }
    sock.set_nodelay(true)?;
    sock.set_read_timeout(Some(Duration::from_secs(15)))?;
    let mut tls = StreamOwned::new(ServerConnection::new(ctx.config.clone())?, sock);
    while tls.conn.is_handshaking() {
        tls.conn.complete_io(&mut tls.sock)?;
    }
    let client_id = tls::remote_id(&tls.conn).ok_or_else(|| anyhow!("no client certificate"))?;
    let Request::Hello { mut info, .. } = wire::read_json(&mut tls)? else {
        bail!("expected hello");
    };
    info.id = client_id.clone();
    let trusted = ctx.store.is_trusted(&client_id);
    if trusted {
        let _ = ctx.store.refresh(&info);
    }
    let mut ours = {
        let local = ctx.local.read().unwrap_or_else(|e| e.into_inner());
        let mut info = local.info.clone();
        info.writable = local.shares.iter().filter(|s| s.writable).map(|s| s.name.clone()).collect();
        info
    };
    ours.id = ctx.store.id().to_string();
    if !trusted {
        ours.shares.clear();
        ours.writable.clear();
    }
    wire::write_json(&mut tls, &Response::Hello { version: PROTOCOL_VERSION, info: ours, trusted })?;
    tls.sock.set_read_timeout(Some(IDLE_TIMEOUT))?;

    loop {
        let req = match wire::read_json::<Request>(&mut tls) {
            Ok(req) => req,
            Err(_) => return Ok(()), // closed or idle
        };
        if ctx.stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        match req {
            Request::Pair => pair(&mut tls, ctx, &info)?,
            Request::Hello { .. } | Request::PairConfirm { .. } => {
                send_error(&mut tls, ErrorKind::Other, "unexpected request")?;
            }
            req if !ctx.store.is_trusted(&client_id) => {
                drop(req);
                send_error(&mut tls, ErrorKind::NotPaired, &format!("{} isn't paired with this device", local_name(ctx)))?;
            }
            Request::Offer { items, total } => {
                // A failed transfer leaves the stream mid-file: close it.
                receive(&mut tls, ctx, &info, items, total)?;
            }
            req @ (Request::Mkdir { .. }
            | Request::RemoveFile { .. }
            | Request::RemoveDir { .. }
            | Request::Rename { .. }
            | Request::Write { .. }) => {
                let shares = ctx.local.read().unwrap_or_else(|e| e.into_inner()).shares.clone();
                // Errors are answered in-band; only a broken upload closes the connection.
                change(&mut tls, &shares, req)?;
            }
            req => {
                let shares = ctx.local.read().unwrap_or_else(|e| e.into_inner()).shares.clone();
                if let Err(e) = browse(&mut tls, &shares, req) {
                    // Errors while streaming file data can't be recovered in-band.
                    if e.downcast_ref::<StreamBroken>().is_some() {
                        return Err(e);
                    }
                    send_error(&mut tls, error_kind(&e), &format!("{e:#}"))?;
                }
            }
        }
    }
}

/// File data was being streamed when something failed: the connection is out of sync.
#[derive(Debug)]
struct StreamBroken(std::io::Error);

impl std::fmt::Display for StreamBroken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "streaming file data: {}", self.0)
    }
}
impl std::error::Error for StreamBroken {}

fn error_kind(e: &anyhow::Error) -> ErrorKind {
    if let Some(p) = e.chain().find_map(|c| c.downcast_ref::<PeerError>()) {
        return p.kind;
    }
    match e.chain().find_map(|c| c.downcast_ref::<std::io::Error>()).map(|io| io.kind()) {
        Some(std::io::ErrorKind::NotFound) => ErrorKind::NotFound,
        Some(std::io::ErrorKind::PermissionDenied) => ErrorKind::Denied,
        _ => ErrorKind::Other,
    }
}

fn local_name(ctx: &Ctx) -> String {
    ctx.local.read().unwrap_or_else(|e| e.into_inner()).info.name.clone()
}

fn send_error(tls: &mut Tls, kind: ErrorKind, message: &str) -> Result<()> {
    wire::write_json(tls, &Response::Error { kind, message: message.to_string() })?;
    Ok(())
}

fn pair(tls: &mut Tls, ctx: &Ctx, device: &DeviceInfo) -> Result<()> {
    let code = tls::pairing_code(&tls::export(&tls.conn)?, ctx.store.id(), &device.id);
    let (reply, rx, abandoned) = reply_pair();
    (ctx.events)(PeerEvent::PairRequest { device: device.clone(), code, reply });
    tls.sock.set_read_timeout(Some(PROMPT_TIMEOUT))?;
    let theirs = matches!(wire::read_json::<Request>(tls), Ok(Request::PairConfirm { ok: true }));
    let ours = theirs && rx.recv_timeout(PROMPT_TIMEOUT).unwrap_or(false);
    abandoned.store(true, Ordering::Relaxed);
    tls.sock.set_read_timeout(Some(IDLE_TIMEOUT))?;
    if ours {
        ctx.store.trust(TrustedPeer::from_info(device))?;
    }
    wire::write_json(tls, &Response::Paired { ok: ours })?;
    Ok(())
}

/// A path in this device's shared namespace: `/` lists the shares,
/// `/<share>/...` is inside one.
enum Resolved {
    Root,
    In { real: PathBuf, root: PathBuf },
}

fn resolve(shares: &[Share], path: &str) -> Result<Resolved> {
    let path = paths::normalize(&format!("/{path}"));
    let mut parts = path.split('/').filter(|p| !p.is_empty());
    let Some(share) = parts.next() else { return Ok(Resolved::Root) };
    let share = shares
        .iter()
        .find(|s| s.name == share)
        .ok_or_else(|| PeerError { kind: ErrorKind::NotFound, message: format!("No shared folder named “{share}”") })?;
    let mut real = share.path.clone();
    for part in parts {
        if part == "." || part == ".." || part.contains('\\') || (cfg!(windows) && part.contains(':')) {
            bail!(PeerError { kind: ErrorKind::Denied, message: "Invalid path".into() });
        }
        real.push(part);
    }
    let root = dunce_canonical(&share.path)?;
    // Symlinks may not lead out of the shared folder.
    let canonical = dunce_canonical(&real)?;
    if !canonical.starts_with(&root) {
        bail!(PeerError { kind: ErrorKind::Denied, message: "Outside the shared folder".into() });
    }
    Ok(Resolved::In { real, root })
}

fn denied(message: impl Into<String>) -> PeerError {
    PeerError { kind: ErrorKind::Denied, message: message.into() }
}

fn bad_name(part: &str) -> bool {
    part.is_empty()
        || part == "."
        || part == ".."
        || part.contains('\\')
        || part.contains('\0')
        || (cfg!(windows) && part.contains(':'))
}

/// Something inside a writable share that a change applies to. Its last
/// component is not followed (it may be a link, or not exist yet); everything
/// above it is checked like [`resolve`].
struct Target {
    path: PathBuf,
    share: String,
}

fn resolve_target(shares: &[Share], path: &str) -> Result<Target> {
    let path = paths::normalize(&format!("/{path}"));
    let parent = paths::parent(&path).unwrap_or_else(|| "/".into());
    let name = paths::file_name(&path).to_string();
    let Some(share_name) = path.split('/').find(|p| !p.is_empty()) else {
        bail!(denied("The list of shared folders can't be changed"));
    };
    let share = shares
        .iter()
        .find(|s| s.name == share_name)
        .ok_or_else(|| PeerError { kind: ErrorKind::NotFound, message: format!("No shared folder named “{share_name}”") })?;
    if !share.writable {
        bail!(denied(format!("“{}” is read-only", share.name)));
    }
    if parent == "/" {
        bail!(denied("Shared folders themselves can't be changed"));
    }
    if bad_name(&name) {
        bail!(denied("Invalid name"));
    }
    let Resolved::In { real, .. } = resolve(shares, &parent)? else { bail!(denied("Invalid path")) };
    if !fs::metadata(&real)?.is_dir() {
        bail!("{parent} is not a folder");
    }
    Ok(Target { path: real.join(name), share: share.name.clone() })
}

/// Applies a change to a writable share. Errors are answered in-band; the
/// result is an error only when the connection can't continue (a broken upload).
fn change(tls: &mut Tls, shares: &[Share], req: Request) -> Result<()> {
    let prepared = (|| -> Result<Option<(Target, PathBuf, fs::File)>> {
        match req {
            Request::Mkdir { path } => fs::create_dir(resolve_target(shares, &path)?.path)?,
            Request::RemoveFile { path } => {
                let t = resolve_target(shares, &path)?;
                if fs::symlink_metadata(&t.path)?.is_dir() {
                    bail!("{path} is a folder");
                }
                fs::remove_file(&t.path)?;
            }
            Request::RemoveDir { path } => {
                let t = resolve_target(shares, &path)?;
                if !fs::symlink_metadata(&t.path)?.is_dir() {
                    bail!("{path} is not a folder");
                }
                fs::remove_dir(&t.path)?;
            }
            Request::Rename { from, to } => {
                let (a, b) = (resolve_target(shares, &from)?, resolve_target(shares, &to)?);
                if a.share != b.share {
                    bail!(denied("Items can't be moved between shared folders"));
                }
                fs::symlink_metadata(&a.path)?;
                if fs::symlink_metadata(&b.path).is_ok() {
                    bail!("{to} already exists");
                }
                fs::rename(&a.path, &b.path)?;
            }
            Request::Write { path, .. } => {
                let t = resolve_target(shares, &path)?;
                if fs::symlink_metadata(&t.path).is_ok_and(|m| m.is_dir()) {
                    bail!("{path} is a folder");
                }
                let name = t.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let tmp = t.path.with_file_name(format!(".{name}.rile-part"));
                let _ = fs::remove_file(&tmp);
                // create_new: never write through a link someone left there.
                let out = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
                return Ok(Some((t, tmp, out)));
            }
            _ => bail!("unexpected request"),
        }
        Ok(None)
    })();
    let (target, tmp, mut out) = match prepared {
        Ok(None) => return Ok(wire::write_json(tls, &Response::Ok)?),
        Ok(Some(upload)) => upload,
        Err(e) => return send_error(tls, error_kind(&e), &format!("{e:#}")),
    };
    wire::write_json(tls, &Response::Ok)?;
    tls.sock.set_read_timeout(Some(crate::IO_TIMEOUT))?;
    let received = (|| -> Result<()> {
        while let Some(chunk) = wire::read_data(tls)? {
            out.write_all(&chunk)?;
        }
        out.sync_all()?;
        Ok(())
    })();
    drop(out);
    tls.sock.set_read_timeout(Some(IDLE_TIMEOUT))?;
    if let Err(e) = received {
        // The client gave up (or the disk is full) mid-file: the stream can't be resynced.
        let _ = fs::remove_file(&tmp);
        let _ = send_error(tls, error_kind(&e), &format!("{e:#}"));
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, &target.path) {
        let _ = fs::remove_file(&tmp);
        return send_error(tls, error_kind(&e.into()), "Couldn't save the file");
    }
    wire::write_json(tls, &Response::Done)?;
    Ok(())
}

fn dunce_canonical(p: &Path) -> Result<PathBuf> {
    let c = fs::canonicalize(p).with_context(|| format!("{}", p.display()))?;
    #[cfg(windows)]
    {
        let s = c.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            if !rest.starts_with("UNC\\") {
                return Ok(PathBuf::from(rest));
            }
        }
    }
    Ok(c)
}

fn entry(virt_dir: &str, name: &str, real: &Path) -> Result<WireEntry> {
    let link = fs::symlink_metadata(real)?;
    let meta = if link.file_type().is_symlink() { fs::metadata(real).unwrap_or(link.clone()) } else { link.clone() };
    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        Some(meta.permissions().mode() & 0o7777)
    };
    #[cfg(not(unix))]
    let permissions = None;
    Ok(WireEntry {
        name: name.to_string(),
        path: paths::join(virt_dir, name),
        is_dir: meta.is_dir(),
        is_symlink: link.file_type().is_symlink(),
        size: if meta.is_dir() { 0 } else { meta.len() },
        modified: meta.modified().ok().and_then(wire::millis),
        permissions,
    })
}

fn share_entry(share: &Share) -> WireEntry {
    let modified = fs::metadata(&share.path).and_then(|m| m.modified()).ok().and_then(wire::millis);
    WireEntry {
        name: share.name.clone(),
        path: format!("/{}", share.name),
        is_dir: true,
        is_symlink: false,
        size: 0,
        modified,
        permissions: None,
    }
}

fn browse(tls: &mut Tls, shares: &[Share], req: Request) -> Result<()> {
    match req {
        Request::List { path } => {
            let entries = match resolve(shares, &path)? {
                Resolved::Root => shares.iter().map(share_entry).collect(),
                Resolved::In { real, root } => {
                    let virt = paths::normalize(&format!("/{path}"));
                    let mut entries = Vec::new();
                    for item in fs::read_dir(&real)? {
                        let Ok(item) = item else { continue };
                        let name = item.file_name().to_string_lossy().into_owned();
                        let child = item.path();
                        if item.file_type().is_ok_and(|t| t.is_symlink())
                            && !dunce_canonical(&child).is_ok_and(|c| c.starts_with(&root))
                        {
                            continue; // leads outside the share
                        }
                        if let Ok(e) = entry(&virt, &name, &child) {
                            entries.push(e);
                        }
                    }
                    entries
                }
            };
            wire::write_json(tls, &Response::Entries { entries })?;
        }
        Request::Stat { path } => {
            let entry = match resolve(shares, &path)? {
                Resolved::Root => WireEntry {
                    name: String::new(),
                    path: "/".into(),
                    is_dir: true,
                    is_symlink: false,
                    size: 0,
                    modified: None,
                    permissions: None,
                },
                Resolved::In { real, .. } => {
                    let virt = paths::normalize(&format!("/{path}"));
                    let parent = paths::parent(&virt).unwrap_or_else(|| "/".into());
                    let name = paths::file_name(&virt);
                    if parent == "/" {
                        let share = shares.iter().find(|s| s.name == name).context("share")?;
                        share_entry(share)
                    } else {
                        entry(&parent, name, &real)?
                    }
                }
            };
            wire::write_json(tls, &Response::Entry { entry })?;
        }
        Request::Read { path } => {
            let Resolved::In { real, .. } = resolve(shares, &path)? else { bail!("Not a file") };
            let mut file = fs::File::open(&real)?;
            if file.metadata()?.is_dir() {
                bail!("Is a folder");
            }
            wire::write_json(tls, &Response::Ok).map_err(StreamBroken)?;
            let mut buf = vec![0u8; CHUNK];
            loop {
                let n = file.read(&mut buf).map_err(StreamBroken)?;
                if n == 0 {
                    break;
                }
                wire::write_data(tls, &buf[..n]).map_err(StreamBroken)?;
            }
            wire::write_data(tls, &[]).map_err(StreamBroken)?;
            tls.flush().map_err(StreamBroken)?;
        }
        _ => bail!("unexpected request"),
    }
    Ok(())
}

fn receive(tls: &mut Tls, ctx: &Ctx, device: &DeviceInfo, items: Vec<OfferItem>, total: u64) -> Result<()> {
    if items.is_empty() || items.iter().any(|i| wire::safe_relative(&i.path).is_none()) {
        return send_error(tls, ErrorKind::Denied, "Invalid file names");
    }
    let files = items.iter().filter(|i| !i.is_dir).count();
    let transfer = Arc::new(Transfer::new(total, files));
    let (reply, rx, abandoned) = reply_pair();
    (ctx.events)(PeerEvent::Offer { device: device.clone(), items: items.clone(), total, reply, transfer: transfer.clone() });
    let dest = rx.recv_timeout(PROMPT_TIMEOUT).ok().flatten();
    abandoned.store(true, Ordering::Relaxed);
    let Some(dest) = dest else {
        transfer.finish(Err("Declined".into()));
        return send_error(tls, ErrorKind::Declined, &format!("{} declined", local_name(ctx)));
    };
    wire::write_json(tls, &Response::Ok)?;
    tls.sock.set_read_timeout(Some(crate::IO_TIMEOUT))?;
    let mut part: Option<PathBuf> = None;
    let result = receive_items(tls, &dest, &items, &transfer, &mut part);
    if let Some(part) = part {
        let _ = fs::remove_file(part);
    }
    tls.sock.set_read_timeout(Some(IDLE_TIMEOUT))?;
    match result {
        Ok(tops) => {
            transfer.finish(Ok(tops));
            wire::write_json(tls, &Response::Done)?;
            Ok(())
        }
        Err(e) => {
            let msg = if transfer.cancel.load(Ordering::Relaxed) { "Cancelled".to_string() } else { format!("{e:#}") };
            transfer.finish(Err(msg.clone()));
            let _ = send_error(tls, ErrorKind::Other, &msg);
            Err(e)
        }
    }
}

fn receive_items(
    tls: &mut Tls,
    dest: &Path,
    items: &[OfferItem],
    transfer: &Transfer,
    part: &mut Option<PathBuf>,
) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(dest)?;
    let mut tops: HashMap<String, PathBuf> = HashMap::new();
    let mut order = Vec::new();
    for item in items {
        let rel = wire::safe_relative(&item.path).context("invalid name")?;
        let mut comps = rel.components();
        let top = comps.next().context("empty")?.as_os_str().to_string_lossy().into_owned();
        let rest: PathBuf = comps.collect();
        let top_path = tops
            .entry(top.clone())
            .or_insert_with(|| {
                let p = rile_core::ops::unique_destination(dest, &top);
                order.push(p.clone());
                p
            })
            .clone();
        let target = if rest.as_os_str().is_empty() { top_path } else { top_path.join(rest) };
        if item.is_dir {
            fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        *transfer.current.lock().unwrap_or_else(|e| e.into_inner()) = Some(item.path.clone());
        let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tmp = target.with_file_name(format!(".{name}.rile-part"));
        *part = Some(tmp.clone());
        let mut out = fs::File::create(&tmp)?;
        while let Some(chunk) = wire::read_data(tls)? {
            if transfer.cancel.load(Ordering::Relaxed) {
                bail!(crate::Cancelled);
            }
            out.write_all(&chunk)?;
            transfer.done.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }
        out.sync_all()?;
        drop(out);
        fs::rename(&tmp, &target)?;
        *part = None;
        transfer.files_done.fetch_add(1, Ordering::Relaxed);
    }
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_stays_inside_shares() {
        let dir = tempfile::tempdir().unwrap();
        let share = dir.path().join("pub");
        fs::create_dir_all(share.join("sub")).unwrap();
        fs::write(dir.path().join("secret.txt"), "x").unwrap();
        let shares = vec![Share { name: "Pub".into(), path: share.clone(), writable: false }];
        assert!(matches!(resolve(&shares, "/").unwrap(), Resolved::Root));
        assert!(matches!(resolve(&shares, "/Pub/sub").unwrap(), Resolved::In { .. }));
        assert!(resolve(&shares, "/Pub/../secret.txt").is_err());
        assert!(resolve(&shares, "Pub/../../secret.txt").is_err());
        assert!(resolve(&shares, "/Other").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path().join("secret.txt"), share.join("link")).unwrap();
            assert!(resolve(&shares, "/Pub/link").is_err());
        }
    }

    #[test]
    fn changes_need_a_writable_share_and_spare_its_root() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("ro")).unwrap();
        fs::create_dir_all(dir.path().join("rw/sub")).unwrap();
        let shares = vec![
            Share { name: "RO".into(), path: dir.path().join("ro"), writable: false },
            Share { name: "RW".into(), path: dir.path().join("rw"), writable: true },
        ];
        let kind = |p: &str| resolve_target(&shares, p).err().and_then(|e| e.downcast_ref::<PeerError>().map(|p| p.kind));
        assert_eq!(kind("/RO/new"), Some(ErrorKind::Denied));
        assert_eq!(kind("/RW"), Some(ErrorKind::Denied));
        assert_eq!(kind("/"), Some(ErrorKind::Denied));
        assert_eq!(kind("/Other/x"), Some(ErrorKind::NotFound));
        assert!(resolve_target(&shares, "/RW/../ro/x").is_err());
        let t = resolve_target(&shares, "/RW/sub/new.txt").unwrap();
        assert_eq!(t.path, dir.path().join("rw/sub/new.txt"));
        assert!(resolve_target(&shares, "/RW/missing/new.txt").is_err());
    }
}
