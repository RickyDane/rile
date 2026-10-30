//! Remote (FTP/FTPS/SFTP) connection registry shared by the UI thread and
//! background workers. Connection details persist in `connections.toml`;
//! secrets live in the OS keychain (or only in memory for this session).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context as _, Result};
use rile_core::Entry;
use rile_core::settings::Settings;
use rile_remote::{ConnectionInfo, Pool, RemoteFs, paths};
use serde::{Deserialize, Serialize};

type Session = Arc<Mutex<Box<dyn RemoteFs>>>;

struct Registry {
    infos: HashMap<String, ConnectionInfo>,
    /// Secrets entered this session (not persisted unless saved to the keychain).
    secrets: HashMap<String, String>,
}

fn registry() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Registry { infos: HashMap::new(), secrets: HashMap::new() }))
}

#[derive(Serialize, Deserialize, Default)]
struct ConnectionsFile {
    #[serde(default)]
    connection: Vec<ConnectionInfo>,
}

fn connections_path() -> Option<PathBuf> {
    Some(Settings::config_dir()?.join("connections.toml"))
}

/// Saved connections, registering them for background use.
pub fn load_saved() -> Vec<ConnectionInfo> {
    let list = connections_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| toml::from_str::<ConnectionsFile>(&s).ok())
        .map(|f| f.connection)
        .unwrap_or_default();
    for info in &list {
        register(info.clone(), None);
    }
    list
}

pub fn save_all(list: &[ConnectionInfo]) -> Result<()> {
    let path = connections_path().context("no config dir")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = ConnectionsFile { connection: list.to_vec() };
    std::fs::write(path, toml::to_string_pretty(&file)?)?;
    Ok(())
}

pub fn register(info: ConnectionInfo, secret: Option<String>) {
    let mut r = registry().lock().unwrap();
    if let Some(s) = secret {
        r.secrets.insert(info.id.clone(), s);
    }
    r.infos.insert(info.id.clone(), info);
}

pub fn forget(id: &str) {
    let mut r = registry().lock().unwrap();
    r.infos.remove(id);
    r.secrets.remove(id);
    Pool::global().disconnect(id);
    rile_remote::credentials::delete(id).ok();
}

pub fn info(id: &str) -> Option<ConnectionInfo> {
    registry().lock().unwrap().infos.get(id).cloned()
}

/// Live session for a connection, connecting (blocking) if needed.
/// Call from background threads only.
pub fn session(id: &str) -> Result<Session> {
    if crate::archive_fs::is_archive_conn(id) {
        return crate::archive_fs::session(id);
    }
    let (info, secret) = {
        let r = registry().lock().unwrap();
        let info = r.infos.get(id).cloned().context("unknown connection")?;
        (info, r.secrets.get(id).cloned())
    };
    let secret = secret.or_else(|| load_cached(id));
    let session = Pool::global().get_or_connect(&info, secret.as_deref())?;
    let server_key = format!("{}://{}:{}", info.protocol.label(), info.host.trim().to_lowercase(), info.port);
    rile_services::rewards::record_event(rile_services::rewards::Event::Server(server_key));
    Ok(session)
}

/// A connection of its own rather than the shared session, for long-running
/// work such as backups that must not block browsing. Returns it with the
/// connection's label. Call from background threads only.
pub fn connect_dedicated(id: &str) -> Result<(Box<dyn RemoteFs>, String)> {
    if let Some(peer_id) = rile_remote::peer::peer_of_conn(id) {
        return connect_nearby_backup(id, peer_id);
    }
    let (info, secret) = {
        let r = registry().lock().unwrap();
        let info = r.infos.get(id).cloned().context("The server connection of this backup no longer exists")?;
        (info, r.secrets.get(id).cloned())
    };
    let secret = secret.or_else(|| load_cached(id));
    let fs = rile_remote::connect(&info, secret.as_deref())?;
    Ok((fs, label(id)))
}

/// Agents have no UI discovery loop or in-memory registry. Resolve the paired
/// identity afresh when needed; never persist a potentially stale LAN address.
fn connect_nearby_backup(id: &str, peer_id: &str) -> Result<(Box<dyn RemoteFs>, String)> {
    use rile_remote::peer::{PeerDiscovery, PeerStore, connection_info};
    let store = PeerStore::app()?;
    let trusted = store.trusted_peer(peer_id).context("This backup device is no longer paired. Pair it again under Nearby.")?;
    if let Some(info) = info(id)
        && let Ok(fs) = rile_remote::connect(&info, None)
    {
        return Ok((fs, trusted.name));
    }
    let mut discovery = PeerDiscovery::new(store.id()).context("Couldn't discover the backup device")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(peer) = discovery.poll().into_iter().find(|p| p.id == peer_id) {
            let info = connection_info(peer_id, &peer.name, &peer.host, peer.port);
            let fs = rile_remote::connect(&info, None)?;
            register(info, None);
            return Ok((fs, peer.name));
        }
        anyhow::ensure!(std::time::Instant::now() < deadline, "Backup device “{}” is offline. Keep Rile running with Nearby enabled on the same network.", trusted.name);
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// The secret for connection `id`: this session's, else the keychain's.
pub fn saved_secret(id: &str) -> Option<String> {
    let session = registry().lock().ok().and_then(|r| r.secrets.get(id).cloned());
    session.or_else(|| load_cached(id))
}

/// Read the secret from the keychain and keep it for the rest of the run, so
/// macOS asks for keychain access at most once per connection per launch.
fn load_cached(id: &str) -> Option<String> {
    let s = rile_remote::credentials::load(id).ok().flatten()?;
    if let Ok(mut r) = registry().lock() {
        r.secrets.insert(id.to_string(), s.clone());
    }
    Some(s)
}

pub fn is_connected(id: &str) -> bool {
    if crate::archive_fs::is_archive_conn(id) {
        return true;
    }
    Pool::global().is_connected(id)
}

/// Best-effort startup connection. Never falls back to an interactive keychain
/// read or a browser sign-in. Sources without saved authorization stay offline.
pub fn auto_connect(info: &ConnectionInfo) -> Result<bool> {
    if info.protocol == rile_remote::Protocol::Peer {
        return Ok(false); // Nearby devices have their own pairing/lifecycle.
    }
    if is_connected(&info.id) {
        return Ok(true);
    }
    let secret = registry().lock().unwrap().secrets.get(&info.id).cloned();
    let secret = match secret {
        Some(secret) => Some(secret),
        None if info.auth.uses_secret() => rile_remote::credentials::load_noninteractive(&info.id)?,
        None => None,
    };
    if info.auth.uses_secret() && secret.is_none() {
        return Ok(false);
    }
    register(info.clone(), secret.clone());
    Pool::global().get_or_connect(info, secret.as_deref())?;
    Ok(true)
}

pub enum CapacityError {
    /// The connection is in use (e.g. a transfer); try later.
    Busy,
    Failed,
}

/// Used/total space at the connection's start folder. Only for connections
/// that are already open; never waits for a busy one. Background threads only.
pub fn capacity(id: &str) -> std::result::Result<Option<rile_remote::Capacity>, CapacityError> {
    let Some(session) = Pool::global().get(id) else {
        return Ok(None);
    };
    let mut fs = match session.try_lock() {
        Ok(fs) => fs,
        Err(std::sync::TryLockError::WouldBlock) => return Err(CapacityError::Busy),
        Err(std::sync::TryLockError::Poisoned(_)) => return Err(CapacityError::Failed),
    };
    let home = fs.home_dir().map_err(|_| CapacityError::Failed)?;
    fs.capacity(&home).map_err(|_| CapacityError::Failed)
}

/// Lists a remote directory as regular entries (paths are remote paths).
pub fn list(id: &str, dir: &str, show_hidden: bool) -> Result<Vec<Entry>> {
    let session = session(id)?;
    let mut fs = session.lock().map_err(|_| anyhow::anyhow!("connection busy"))?;
    let items = fs.list(dir)?;
    Ok(items
        .into_iter()
        .map(|e| Entry::remote(&e.name, &e.path, e.is_dir, e.is_symlink, e.size, e.modified))
        .filter(|e| show_hidden || !e.hidden)
        .collect())
}

pub fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

pub fn parent(path: &str) -> Option<String> {
    paths::parent(path)
}

pub fn join(base: &str, name: &str) -> String {
    paths::join(base, name)
}

/// Where remote files are downloaded for opening/previewing.
pub fn temp_path(id: &str, remote: &str) -> PathBuf {
    if let Some(archive) = crate::archive_fs::archive_of(id) {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        archive.hash(&mut h);
        return std::env::temp_dir().join("rile-remote").join(format!("archive-{:x}", h.finish())).join(paths::file_name(remote));
    }
    // Device ids look like `peer:…`; `:` isn't allowed in Windows file names.
    std::env::temp_dir().join("rile-remote").join(id.replace(':', "-")).join(paths::file_name(remote))
}

/// Lowercase "user@host" label for breadcrumbs and titles.
pub fn label(id: &str) -> String {
    if let Some(archive) = crate::archive_fs::archive_of(id) {
        return archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| id.to_string());
    }
    info(id).map(|i| if i.name.is_empty() { i.display() } else { i.name.clone() }).unwrap_or_else(|| id.to_string())
}

/// Icon of a connection: a computer for nearby Rile devices, else a globe.
pub fn icon(id: &str) -> gpui_kit::assets::IconName {
    if crate::archive_fs::is_archive_conn(id) {
        return gpui_kit::assets::IconName::FileArchive;
    }
    if rile_remote::peer::peer_of_conn(id).is_some() {
        gpui_kit::assets::IconName::Laptop
    } else {
        gpui_kit::assets::IconName::Globe
    }
}
