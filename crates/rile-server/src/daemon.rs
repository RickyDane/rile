//! The background service: a Rile Nearby device without a screen.
//!
//! Serves the configured shares to paired Rile apps, announces itself on the
//! LAN (DNS-SD), saves files sent to it in the inbox, and lets `rilectl`
//! inspect and steer it over a Unix socket. Pair requests are only accepted
//! while `rilectl pair` is open, and only after its user confirms the code.

use std::fs;
use std::io::BufReader;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use rile_remote::peer::{
    self, DeviceInfo, LocalDevice, NearbyPeer, PROMPT_TIMEOUT, PeerDiscovery, PeerEvent, PeerServer, PeerStore, Reply,
    Transfer,
};

use crate::Paths;
use crate::config::Config;
use crate::facts::{self, Facts};
use crate::ipc::{self, Device, Request, Response, ShareStatus, Status};

/// Longest pairing window `rilectl pair` may ask for.
pub const MAX_PAIR_WINDOW: Duration = Duration::from_secs(3600);
const POLL: Duration = Duration::from_secs(2);
const REFRESH: Duration = Duration::from_secs(60);

pub struct Options {
    pub paths: Paths,
    /// Overrides the configured port (0 = any free port).
    pub port: Option<u16>,
    /// Announce on and browse the LAN. Off in tests.
    pub mdns: bool,
}

/// What a pair window receives.
#[allow(clippy::large_enum_variant)]
enum PairMsg {
    Ask { device: DeviceInfo, code: u32, reply: Reply<bool> },
    Answer(bool),
    Closed,
}

struct Shared {
    paths: Paths,
    facts: Facts,
    store: Arc<PeerStore>,
    config: RwLock<Config>,
    local: Mutex<LocalDevice>,
    server: OnceLock<PeerServer>,
    discovery: Mutex<Option<PeerDiscovery>>,
    nearby: Mutex<Vec<NearbyPeer>>,
    /// The open pair window: its id and inbox.
    pair: Mutex<Option<(u64, Sender<PairMsg>)>>,
    next_window: AtomicU64,
    /// The configured port when the server started (changes need a restart).
    started_port: u16,
    stop: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A running server. Dropping it stops everything.
pub struct Daemon {
    shared: Arc<Shared>,
}

impl Daemon {
    pub fn start(opts: Options) -> Result<Self> {
        let config = Config::load(&opts.paths.config)?;
        let facts = facts::read();
        let local = facts.local_device(&config);
        let store = PeerStore::open(opts.paths.state.join("peer"))
            .with_context(|| format!("opening {}", opts.paths.state.display()))?;
        peer::set_local_device(local.clone());
        let started_port = config.port;
        let port = opts.port.unwrap_or(config.port);
        let shared = Arc::new(Shared {
            paths: opts.paths,
            facts,
            store: store.clone(),
            config: RwLock::new(config),
            local: Mutex::new(local.clone()),
            server: OnceLock::new(),
            discovery: Mutex::new(None),
            nearby: Mutex::new(Vec::new()),
            pair: Mutex::new(None),
            next_window: AtomicU64::new(1),
            started_port,
            stop: AtomicBool::new(false),
        });

        let weak: Weak<Shared> = Arc::downgrade(&shared);
        // Answering can block (pairing waits for a person): never in the server's thread.
        let server = PeerServer::start(store.clone(), local.clone(), port, move |ev| {
            if let Some(shared) = weak.upgrade() {
                std::thread::spawn(move || shared.on_peer_event(ev));
            }
        })?;
        let port = server.port();
        if opts.port.is_none() && port != started_port {
            log::warn!("port {started_port} is taken; listening on {port} instead");
        }
        let _ = shared.server.set(server);

        if opts.mdns {
            let mut discovery = PeerDiscovery::new(store.id())?;
            discovery.advertise(&local.info, port)?;
            *lock(&shared.discovery) = Some(discovery);
        }
        shared.listen()?;
        let poller = Arc::downgrade(&shared);
        std::thread::Builder::new().name("rile-server-poll".into()).spawn(move || {
            let mut refreshed = Instant::now();
            loop {
                std::thread::sleep(POLL);
                let Some(shared) = poller.upgrade() else { break };
                if shared.stop.load(Ordering::Relaxed) {
                    break;
                }
                if let Some(d) = lock(&shared.discovery).as_mut() {
                    *lock(&shared.nearby) = d.poll();
                }
                if refreshed.elapsed() >= REFRESH {
                    refreshed = Instant::now();
                    shared.republish(false); // storage numbers, shares whose folder appeared
                }
            }
        })?;

        log::info!(
            "{} ({}) listening on port {port}, id {}",
            local.info.name,
            local.info.os,
            peer::short_id(store.id())
        );
        for s in &local.shares {
            log::info!("sharing “{}” = {} ({})", s.name, s.path.display(), if s.writable { "read-write" } else { "read-only" });
        }
        for s in shared.config.read().unwrap().missing() {
            log::warn!("not sharing “{}”: {} doesn't exist", s.name, s.path.display());
        }
        Ok(Self { shared })
    }

    pub fn port(&self) -> u16 {
        self.shared.server.get().map(|s| s.port()).unwrap_or(0)
    }

    pub fn id(&self) -> &str {
        self.shared.store.id()
    }

    /// Re-reads the configuration file.
    pub fn reload(&self) -> Result<()> {
        self.shared.reload()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        lock(&self.shared.discovery).take(); // unregisters from the network
        let _ = UnixStream::connect(&self.shared.paths.socket); // wake the accept loop
        let _ = fs::remove_file(&self.shared.paths.socket);
    }
}

impl Shared {
    fn reload(&self) -> Result<()> {
        let config = Config::load(&self.paths.config)?;
        config.validate()?;
        if config.port != self.started_port {
            log::warn!("the port changed to {}; it takes effect after a restart", config.port);
        }
        *self.config.write().unwrap_or_else(|e| e.into_inner()) = config;
        self.republish(true);
        log::info!("configuration reloaded");
        Ok(())
    }

    /// Publishes the current configuration and facts. `announce` re-announces
    /// on the network even if only the shares changed.
    fn republish(&self, log_shares: bool) {
        let config = self.config.read().unwrap_or_else(|e| e.into_inner()).clone();
        let local = self.facts.local_device(&config);
        let mut current = lock(&self.local);
        if *current == local {
            return;
        }
        let renamed = current.info.name != local.info.name;
        if let Some(server) = self.server.get() {
            server.update(local.clone());
            if renamed && let Some(d) = lock(&self.discovery).as_mut() {
                let _ = d.advertise(&local.info, server.port());
            }
        }
        peer::set_local_device(local.clone());
        if log_shares && current.shares != local.shares {
            let names: Vec<_> = local.shares.iter().map(|s| s.name.as_str()).collect();
            log::info!("now sharing: {}", if names.is_empty() { "nothing".to_string() } else { names.join(", ") });
        }
        *current = local;
    }

    fn on_peer_event(&self, ev: PeerEvent) {
        match ev {
            PeerEvent::PairRequest { device, code, reply } => {
                let window = lock(&self.pair).as_ref().map(|(_, tx)| tx.clone());
                let name = peer::describe(&device);
                match window {
                    Some(tx) if tx.send(PairMsg::Ask { device, code, reply }).is_ok() => {}
                    _ => log::warn!(
                        "declined pair request from {name}: no pairing window is open (run `sudo rilectl pair` on this server, then pair again)"
                    ),
                }
            }
            PeerEvent::Offer { device, items, total, reply, transfer } => {
                let name = peer::describe(&device);
                let inbox = self.config.read().unwrap_or_else(|e| e.into_inner()).inbox.clone();
                let Some(inbox) = inbox else {
                    log::info!("declined {} item(s) from {name}: no inbox is set (`rilectl inbox <folder>`)", items.len());
                    reply.send(None);
                    return;
                };
                if let Err(e) = fs::create_dir_all(&inbox) {
                    log::warn!("declined files from {name}: can't create the inbox {}: {e}", inbox.display());
                    reply.send(None);
                    return;
                }
                log::info!("receiving {} item(s), {total} bytes, from {name} into {}", items.len(), inbox.display());
                reply.send(Some(inbox));
                watch_transfer(name, transfer);
            }
        }
    }

    fn status(&self) -> Status {
        let config = self.config.read().unwrap_or_else(|e| e.into_inner()).clone();
        let local = lock(&self.local).clone();
        let nearby = lock(&self.nearby).clone();
        let address = |id: &str| nearby.iter().find(|p| p.id == id).map(|p| format!("{}:{}", p.host, p.port));
        let mut devices: Vec<Device> = self
            .store
            .trusted()
            .into_iter()
            .map(|t| Device { address: address(&t.id), id: t.id, name: t.name, model: t.model, os: t.os, paired: true })
            .collect();
        for p in &nearby {
            if !devices.iter().any(|d| d.id == p.id) {
                devices.push(Device {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    model: p.model.clone(),
                    os: p.os.clone(),
                    address: Some(format!("{}:{}", p.host, p.port)),
                    paired: false,
                });
            }
        }
        Status {
            id: self.store.id().to_string(),
            name: local.info.name,
            model: local.info.model,
            os: local.info.os,
            port: self.server.get().map(|s| s.port()).unwrap_or(0),
            version: local.info.version,
            shares: config
                .shares
                .iter()
                .map(|s| ShareStatus {
                    name: s.name.clone(),
                    path: s.path.clone(),
                    writable: s.access == crate::config::Access::ReadWrite,
                    available: s.path.is_dir(),
                })
                .collect(),
            inbox: config.inbox,
            devices,
        }
    }

    fn listen(self: &Arc<Self>) -> Result<()> {
        let path = &self.paths.socket;
        if UnixStream::connect(path).is_ok() {
            bail!("another rile-server is already running ({} is in use)", path.display());
        }
        let _ = fs::remove_file(path);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let listener = UnixListener::bind(path).with_context(|| format!("listening on {}", path.display()))?;
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        let weak = Arc::downgrade(self);
        std::thread::Builder::new().name("rile-server-ipc".into()).spawn(move || {
            for conn in listener.incoming() {
                let Some(shared) = weak.upgrade() else { break };
                if shared.stop.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(conn) = conn else { continue };
                std::thread::spawn(move || {
                    if let Err(e) = shared.serve_ipc(conn) {
                        log::debug!("control connection: {e:#}");
                    }
                });
            }
        })?;
        Ok(())
    }

    fn serve_ipc(&self, conn: UnixStream) -> Result<()> {
        conn.set_read_timeout(Some(Duration::from_secs(30)))?;
        let mut reader = BufReader::new(conn.try_clone()?);
        let mut out = conn;
        let Some(req) = ipc::read_line::<Request>(&mut reader)? else { return Ok(()) };
        let answer = match req {
            Request::Status => Response::Status(self.status()),
            Request::Reload => match self.reload() {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error { message: format!("{e:#}") },
            },
            Request::Forget { id } => {
                let found = self.store.trusted().into_iter().find(|t| t.id == id || t.id.starts_with(&id.replace('-', "")));
                match found {
                    Some(t) => match self.store.forget(&t.id) {
                        Ok(()) => {
                            log::info!("unpaired {}", t.name);
                            Response::Ok
                        }
                        Err(e) => Response::Error { message: format!("{e:#}") },
                    },
                    None => Response::Error { message: format!("no paired device {id}") },
                }
            }
            Request::Pair { secs } => return self.pair_window(reader, out, Duration::from_secs(secs).min(MAX_PAIR_WINDOW)),
            Request::Answer { .. } => Response::Error { message: "no pair request is waiting".into() },
        };
        ipc::write_line(&mut out, &answer)?;
        Ok(())
    }

    /// Hands pair requests to `rilectl pair` until it closes or `secs` pass.
    fn pair_window(&self, mut reader: BufReader<UnixStream>, mut out: UnixStream, length: Duration) -> Result<()> {
        let (tx, rx) = mpsc::channel::<PairMsg>();
        let id = self.next_window.fetch_add(1, Ordering::Relaxed);
        // A newer window replaces an older one.
        *lock(&self.pair) = Some((id, tx.clone()));
        log::info!("pairing window open for {} s", length.as_secs());
        reader.get_ref().set_read_timeout(None)?;
        std::thread::spawn(move || {
            loop {
                match ipc::read_line::<Request>(&mut reader) {
                    Ok(Some(Request::Answer { accept })) => {
                        if tx.send(PairMsg::Answer(accept)).is_err() {
                            break;
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => {
                        let _ = tx.send(PairMsg::Closed);
                        break;
                    }
                }
            }
        });
        let result = (|| -> Result<()> {
            ipc::write_line(&mut out, &Response::PairOpen { secs: length.as_secs() })?;
            let deadline = Instant::now() + length;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() || self.stop.load(Ordering::Relaxed) {
                    return Ok(());
                }
                let (device, code, reply) = match rx.recv_timeout(left.min(Duration::from_secs(1))) {
                    Ok(PairMsg::Ask { device, code, reply }) => (device, code, reply),
                    Ok(PairMsg::Answer(_)) => continue,
                    Ok(PairMsg::Closed) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
                    Err(RecvTimeoutError::Timeout) => {
                        if !matches!(*lock(&self.pair), Some((current, _)) if current == id) {
                            return Ok(()); // replaced by a newer window
                        }
                        continue;
                    }
                };
                let name = peer::describe(&device);
                ipc::write_line(
                    &mut out,
                    &Response::PairRequest {
                        id: device.id.clone(),
                        name: name.clone(),
                        model: device.model.clone(),
                        os: device.os.clone(),
                        code: peer::format_code(code),
                    },
                )?;
                // Wait for the person at the terminal; other requests meanwhile are declined.
                let asked = Instant::now();
                let accept = loop {
                    match rx.recv_timeout(Duration::from_millis(500)) {
                        Ok(PairMsg::Answer(a)) => break Some(a),
                        Ok(PairMsg::Ask { device, .. }) => log::info!("declined a second pair request from {}", peer::describe(&device)),
                        Ok(PairMsg::Closed) | Err(RecvTimeoutError::Disconnected) => break None,
                        Err(RecvTimeoutError::Timeout) => {
                            if reply.is_abandoned() || asked.elapsed() > PROMPT_TIMEOUT {
                                break Some(false);
                            }
                        }
                    }
                };
                let finished = reply.abandoned_flag();
                reply.send(accept.unwrap_or(false));
                let Some(accept) = accept else { return Ok(()) };
                // The pairing completes once the other device has answered too.
                let until = Instant::now() + PROMPT_TIMEOUT + Duration::from_secs(5);
                while !finished.load(Ordering::Relaxed) && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(100));
                }
                let mut paired = false;
                for _ in 0..20 {
                    paired = self.store.is_trusted(&device.id);
                    if paired || !accept {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                log::info!("{} {name}", if paired { "paired with" } else { "did not pair with" });
                ipc::write_line(&mut out, &Response::PairResult { name, paired })?;
            }
        })();
        {
            let mut slot = lock(&self.pair);
            if matches!(*slot, Some((current, _)) if current == id) {
                *slot = None;
            }
        }
        log::info!("pairing window closed");
        let _ = ipc::write_line(&mut out, &Response::PairClosed);
        result
    }
}

fn watch_transfer(from: String, transfer: Arc<Transfer>) {
    std::thread::spawn(move || {
        loop {
            if let Some(outcome) = transfer.outcome() {
                match outcome {
                    Ok(items) => {
                        let names: Vec<_> = items.iter().map(|p| p.display().to_string()).collect();
                        log::info!("received from {from}: {}", names.join(", "));
                    }
                    Err(e) => log::warn!("receiving from {from} failed: {e}"),
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}
