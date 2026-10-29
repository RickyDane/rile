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
    let (info, secret) = {
        let r = registry().lock().unwrap();
        let info = r.infos.get(id).cloned().context("unknown connection")?;
        (info, r.secrets.get(id).cloned())
    };
    let secret = match secret {
        Some(s) => Some(s),
        None => rile_remote::credentials::load(id).ok().flatten(),
    };
    let session = Pool::global().get_or_connect(&info, secret.as_deref())?;
    let server_key = format!("{}://{}:{}", info.protocol.label(), info.host.trim().to_lowercase(), info.port);
    rile_services::rewards::record_event(rile_services::rewards::Event::Server(server_key));
    Ok(session)
}

/// A connection of its own rather than the shared session, for long-running
/// work such as backups that must not block browsing. Returns it with the
/// connection's label. Call from background threads only.
pub fn connect_dedicated(id: &str) -> Result<(Box<dyn RemoteFs>, String)> {
    let (info, secret) = {
        let r = registry().lock().unwrap();
        let info = r.infos.get(id).cloned().context("The server connection of this backup no longer exists")?;
        (info, r.secrets.get(id).cloned())
    };
    let secret = match secret {
        Some(s) => Some(s),
        None => rile_remote::credentials::load(id).ok().flatten(),
    };
    let fs = rile_remote::connect(&info, secret.as_deref())?;
    Ok((fs, label(id)))
}

pub fn is_connected(id: &str) -> bool {
    Pool::global().is_connected(id)
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
    // Device ids look like `peer:…`; `:` isn't allowed in Windows file names.
    std::env::temp_dir().join("rile-remote").join(id.replace(':', "-")).join(paths::file_name(remote))
}

/// Lowercase "user@host" label for breadcrumbs and titles.
pub fn label(id: &str) -> String {
    info(id).map(|i| if i.name.is_empty() { i.display() } else { i.name.clone() }).unwrap_or_else(|| id.to_string())
}

/// Icon of a connection: a computer for nearby Rile devices, else a globe.
pub fn icon(id: &str) -> gpui_kit::assets::IconName {
    if rile_remote::peer::peer_of_conn(id).is_some() {
        gpui_kit::assets::IconName::Laptop
    } else {
        gpui_kit::assets::IconName::Globe
    }
}
