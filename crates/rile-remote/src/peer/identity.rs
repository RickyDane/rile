//! This device's identity (a self-signed certificate) and the devices it
//! has paired with.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};

use super::wire::{DeviceInfo, DeviceKind};

const CERT_FILE: &str = "identity.der";
const KEY_FILE: &str = "identity.key";
const TRUST_FILE: &str = "peers.toml";

/// Hex SHA-256 of a certificate: the stable id of the device holding its key.
pub fn fingerprint(cert: &[u8]) -> String {
    let d = ring::digest::digest(&ring::digest::SHA256, cert);
    d.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// `ab12-cd34`: a short, readable form of a device id.
pub fn short_id(id: &str) -> String {
    let id: String = id.chars().take(8).collect();
    if id.len() < 8 { return id; }
    format!("{}-{}", &id[..4], &id[4..])
}

pub(crate) struct Identity {
    pub id: String,
    pub cert: CertificateDer<'static>,
    key: Vec<u8>,
}

impl Identity {
    pub fn key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.clone()))
    }

    fn load_or_create(dir: &Path) -> Result<Self> {
        let (cert_path, key_path) = (dir.join(CERT_FILE), dir.join(KEY_FILE));
        if let (Ok(cert), Ok(key)) = (fs::read(&cert_path), fs::read(&key_path)) {
            return Ok(Self { id: fingerprint(&cert), cert: CertificateDer::from(cert), key });
        }
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let generated = rcgen::generate_simple_self_signed(vec!["rile.local".to_string()])?;
        let cert = generated.cert.der().to_vec();
        let key = generated.signing_key.serialize_der();
        write_private(&key_path, &key)?;
        fs::write(&cert_path, &cert)?;
        Ok(Self { id: fingerprint(&cert), cert: CertificateDer::from(cert), key })
    }
}

fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        f.write_all(data)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, data)?;
        Ok(())
    }
}

/// A device this one has paired with, and what it last reported about itself
/// (so it can be shown while offline).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct TrustedPeer {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub kind: DeviceKind,
    /// Installed RAM in bytes; 0 when unknown.
    #[serde(default)]
    pub memory: u64,
    /// Unix seconds.
    #[serde(default)]
    pub paired_at: i64,
}

impl TrustedPeer {
    pub fn from_info(info: &DeviceInfo) -> Self {
        Self {
            id: info.id.clone(),
            name: info.name.clone(),
            model: info.model.clone(),
            os: info.os.clone(),
            kind: info.kind,
            memory: info.memory,
            paired_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0),
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
struct TrustFile {
    #[serde(default, rename = "peer")]
    peers: Vec<TrustedPeer>,
}

static APP_STORE: std::sync::RwLock<Option<Arc<PeerStore>>> = std::sync::RwLock::new(None);

/// Identity plus the list of paired devices, stored in one directory.
pub struct PeerStore {
    dir: PathBuf,
    pub(crate) identity: Identity,
    trust: Mutex<()>,
}

impl PeerStore {
    /// Opens (creating the identity on first use) the store in `dir`.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Arc<Self>> {
        let dir = dir.into();
        let identity = Identity::load_or_create(&dir)?;
        Ok(Arc::new(Self { dir, identity, trust: Mutex::new(()) }))
    }

    /// The store under Rile's config directory (`<config>/peer`). One per
    /// directory per process, so `RILE_CONFIG_DIR` keeps tests apart.
    pub fn global() -> Result<Arc<Self>> {
        static STORES: OnceLock<Mutex<HashMap<PathBuf, Arc<PeerStore>>>> = OnceLock::new();
        let dir = rile_core::settings::Settings::config_dir()
            .ok_or_else(|| anyhow::anyhow!("no config directory"))?
            .join("peer");
        let mut map = STORES.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
        if let Some(store) = map.get(&dir) {
            return Ok(store.clone());
        }
        let store = Self::open(dir.clone())?;
        map.insert(dir, store.clone());
        Ok(store)
    }

    /// The store the app uses (set with [`PeerStore::set_app`]), else [`PeerStore::global`].
    pub fn app() -> Result<Arc<Self>> {
        match APP_STORE.read().unwrap_or_else(|e| e.into_inner()).clone() {
            Some(store) => Ok(store),
            None => Self::global(),
        }
    }

    /// Makes `store` the one browsing connections ([`crate::Protocol::Peer`]) use.
    pub fn set_app(store: Arc<Self>) {
        *APP_STORE.write().unwrap_or_else(|e| e.into_inner()) = Some(store);
    }

    /// This device's id.
    pub fn id(&self) -> &str {
        &self.identity.id
    }

    pub fn trusted(&self) -> Vec<TrustedPeer> {
        let _guard = self.trust.lock().unwrap_or_else(|e| e.into_inner());
        self.read().peers
    }

    pub fn trusted_peer(&self, id: &str) -> Option<TrustedPeer> {
        self.trusted().into_iter().find(|p| p.id == id)
    }

    pub fn is_trusted(&self, id: &str) -> bool {
        self.trusted_peer(id).is_some()
    }

    /// Adds `peer`, or refreshes what is remembered about it (keeping when it
    /// was paired).
    pub fn trust(&self, mut peer: TrustedPeer) -> Result<()> {
        let _guard = self.trust.lock().unwrap_or_else(|e| e.into_inner());
        let mut file = self.read();
        if let Some(existing) = file.peers.iter_mut().find(|p| p.id == peer.id) {
            if existing.paired_at != 0 {
                peer.paired_at = existing.paired_at;
            }
            if *existing == peer {
                return Ok(());
            }
            *existing = peer;
        } else {
            file.peers.push(peer);
        }
        self.write(&file)
    }

    /// Updates what is remembered about an already paired device; no-op otherwise.
    pub fn refresh(&self, info: &DeviceInfo) -> Result<()> {
        if self.is_trusted(&info.id) { self.trust(TrustedPeer::from_info(info)) } else { Ok(()) }
    }

    pub fn forget(&self, id: &str) -> Result<()> {
        let _guard = self.trust.lock().unwrap_or_else(|e| e.into_inner());
        let mut file = self.read();
        file.peers.retain(|p| p.id != id);
        self.write(&file)
    }

    fn read(&self) -> TrustFile {
        fs::read_to_string(self.dir.join(TRUST_FILE))
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn write(&self, file: &TrustFile) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        let tmp = self.dir.join(format!("{TRUST_FILE}.tmp"));
        fs::write(&tmp, toml::to_string(file)?)?;
        fs::rename(&tmp, self.dir.join(TRUST_FILE))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_stable_and_trust_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let a = PeerStore::open(dir.path()).unwrap();
        let b = PeerStore::open(dir.path()).unwrap();
        assert_eq!(a.id(), b.id());
        assert_eq!(a.id().len(), 64);
        assert_eq!(short_id(a.id()).len(), 9);

        let info = DeviceInfo { id: "f00d".into(), name: "Studio".into(), ..Default::default() };
        assert!(!a.is_trusted("f00d"));
        a.refresh(&info).unwrap();
        assert!(!a.is_trusted("f00d"), "refresh must not pair");
        a.trust(TrustedPeer::from_info(&info)).unwrap();
        let paired_at = a.trusted_peer("f00d").unwrap().paired_at;
        a.refresh(&DeviceInfo { name: "Studio 2".into(), ..info.clone() }).unwrap();
        let p = b.trusted_peer("f00d").unwrap();
        assert_eq!((p.name.as_str(), p.paired_at), ("Studio 2", paired_at));
        a.forget("f00d").unwrap();
        assert!(!b.is_trusted("f00d"));
    }
}
