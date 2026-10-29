//! SFTP backend: `russh` + `russh-sftp`, driven by a private tokio runtime and
//! exposed through the blocking [`Backend`] trait.

use std::io::{self, Read};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use russh::MethodKind;
use russh::client::{self, AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::{self, HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{FileAttributes, StatusCode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Runtime;

use crate::backend::{Backend, CHUNK};
use crate::{
    Auth, CONNECT_TIMEOUT, ConnectionInfo, ConnectionLost, HostKeyMismatch, IO_TIMEOUT,
    RemoteEntry, known_hosts, paths,
};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);
/// Generous: agents (e.g. 1Password, YubiKey) may wait for user confirmation.
const AUTH_TIMEOUT: Duration = Duration::from_secs(90);

/// Shared runtime for all SFTP connections. Multi-threaded (2 workers) so that
/// keepalives and channel bookkeeping keep running between blocking calls.
fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("rile-sftp")
            .enable_all()
            .build()
            .expect("failed to start SFTP runtime")
    })
}

// ------------------------------------------------------------------ host keys

struct ClientHandler {
    host: String,
    port: u16,
    rejection: Arc<Mutex<Option<anyhow::Error>>>,
}

fn split_openssh(key: &PublicKey) -> Result<(String, String)> {
    let mut key = key.clone();
    key.set_comment("");
    let line = key.to_openssh().context("encoding host key")?;
    let mut it = line.split_whitespace();
    match (it.next(), it.next()) {
        (Some(t), Some(d)) => Ok((t.to_string(), d.to_string())),
        _ => bail!("malformed host key"),
    }
}

fn describe(key_type: &str, data: &str) -> String {
    match PublicKey::from_openssh(&format!("{key_type} {data}")) {
        Ok(k) => format!("{} {}", key_type, k.fingerprint(HashAlg::Sha256)),
        Err(_) => format!("{key_type} {data}"),
    }
}

impl ClientHandler {
    fn verify(&self, key: &PublicKey) -> Result<()> {
        let (ty, data) = split_openssh(key)?;
        match known_hosts::check(&self.host, self.port, &ty, &data)? {
            known_hosts::Check::Match => Ok(()),
            known_hosts::Check::Unknown => {
                log::info!(
                    "adding host key for {}:{} to {} ({})",
                    self.host,
                    self.port,
                    known_hosts::path().display(),
                    describe(&ty, &data)
                );
                known_hosts::add(&self.host, self.port, &ty, &data)
            }
            known_hosts::Check::Mismatch { expected } => Err(HostKeyMismatch {
                host: format!("{}:{}", self.host, self.port),
                expected: describe(&ty, &expected),
                actual: describe(&ty, &data),
                known_hosts: known_hosts::path(),
            }
            .into()),
        }
    }
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // Certificates are not validated against a CA; their key is pinned like a plain key.
        let key = match server_public_key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            PublicKeyOrCertificate::Certificate(cert) => {
                PublicKey::new(cert.public_key().clone(), "")
            }
        };
        match self.verify(&key) {
            Ok(()) => Ok(true),
            Err(e) => {
                *self.rejection.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
                Ok(false)
            }
        }
    }
}

// ------------------------------------------------------------------ errors

fn map_sftp(e: SftpError) -> anyhow::Error {
    match e {
        SftpError::Status(ref s)
            if matches!(
                s.status_code,
                StatusCode::ConnectionLost | StatusCode::NoConnection
            ) =>
        {
            ConnectionLost(e.to_string()).into()
        }
        SftpError::Status(s) => {
            let msg = s.error_message.trim();
            let code = s.status_code.to_string();
            if msg.is_empty() || msg.eq_ignore_ascii_case(&code) {
                anyhow!("{code}")
            } else {
                anyhow!("{code}: {msg}")
            }
        }
        SftpError::IO(_) | SftpError::Timeout => ConnectionLost(e.to_string()).into(),
        other => anyhow!("SFTP error: {other}"),
    }
}

fn io_err(e: io::Error) -> anyhow::Error {
    anyhow!("I/O error: {e}")
}

// ------------------------------------------------------------------ backend

pub(crate) struct SftpBackend {
    handle: Handle<ClientHandler>,
    sftp: SftpSession,
    login_dir: String,
    alive: bool,
}

impl Drop for SftpBackend {
    fn drop(&mut self) {
        // Polite disconnect (bounded to 2s). Skipped when dropped from inside a tokio
        // runtime, where blocking is not allowed; the session then just closes.
        let rt = runtime();
        if self.alive && !self.handle.is_closed() && tokio::runtime::Handle::try_current().is_err()
        {
            let fut = self
                .handle
                .disconnect(russh::Disconnect::ByApplication, "", "en");
            let _ = rt.block_on(async { tokio::time::timeout(Duration::from_secs(2), fut).await });
        }
    }
}

async fn tcp_connect(host: &str, port: u16) -> Result<tokio::net::TcpStream> {
    let addrs: Vec<_> =
        tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((host, port)))
            .await
            .map_err(|_| anyhow!("timed out resolving {host}"))?
            .with_context(|| format!("could not resolve {host}"))?
            .collect();
    let mut last: Option<anyhow::Error> = None;
    for addr in addrs {
        match tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(addr)).await {
            Ok(Ok(s)) => {
                s.set_nodelay(true).ok();
                return Ok(s);
            }
            Ok(Err(e)) => last = Some(e.into()),
            Err(_) => last = Some(anyhow!("connection timed out")),
        }
    }
    Err(last
        .unwrap_or_else(|| anyhow!("{host} resolved to no addresses"))
        .context(format!("could not connect to {host}:{port}")))
}

fn auth_failed(r: &AuthResult) -> Option<Vec<MethodKind>> {
    match r {
        AuthResult::Success => None,
        AuthResult::Failure {
            remaining_methods, ..
        } => Some(remaining_methods.to_vec()),
    }
}

async fn keyboard_interactive(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    password: &str,
) -> Result<bool> {
    let mut resp = handle
        .authenticate_keyboard_interactive_start(user, None)
        .await?;
    // Answer every prompt with the password; bounded number of rounds.
    for _ in 0..5 {
        match resp {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|_| password.to_string()).collect();
                resp = handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await?;
            }
        }
    }
    Ok(false)
}

#[cfg(unix)]
async fn agent_auth(handle: &mut Handle<ClientHandler>, user: &str) -> Result<bool> {
    let mut agent = keys::agent::client::AgentClient::connect_env()
        .await
        .context("could not connect to ssh-agent (is SSH_AUTH_SOCK set?)")?;
    agent_try_all(handle, user, &mut agent).await
}

#[cfg(windows)]
async fn agent_auth(handle: &mut Handle<ClientHandler>, user: &str) -> Result<bool> {
    // Windows' built-in OpenSSH agent first, then PuTTY's Pageant.
    if let Ok(mut agent) = keys::agent::client::AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await {
        if agent_try_all(handle, user, &mut agent).await? {
            return Ok(true);
        }
    }
    let mut agent = keys::agent::client::AgentClient::connect_pageant()
        .await
        .context("could not connect to the OpenSSH Authentication Agent service or Pageant")?;
    agent_try_all(handle, user, &mut agent).await
}

#[cfg(not(any(unix, windows)))]
async fn agent_auth(_handle: &mut Handle<ClientHandler>, _user: &str) -> Result<bool> {
    bail!("ssh-agent is not supported on this platform")
}

#[cfg(any(unix, windows))]
async fn agent_try_all<S>(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    agent: &mut keys::agent::client::AgentClient<S>,
) -> Result<bool>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let identities = agent
        .request_identities()
        .await
        .context("ssh-agent: listing keys failed")?;
    if identities.is_empty() {
        bail!("ssh-agent holds no keys (add one with ssh-add)");
    }
    let rsa_hash = handle.best_supported_rsa_hash().await?.flatten();
    for id in identities {
        let key = id.public_key().into_owned();
        let hash = if key.algorithm().is_rsa() {
            rsa_hash
        } else {
            None
        };
        match handle
            .authenticate_publickey_with(user, key, hash, agent)
            .await
        {
            Ok(r) if r.success() => return Ok(true),
            Ok(_) => {}
            Err(e) => log::debug!("agent key rejected: {e}"),
        }
    }
    Ok(false)
}

async fn authenticate(
    handle: &mut Handle<ClientHandler>,
    info: &ConnectionInfo,
    secret: Option<&str>,
) -> Result<()> {
    let user = info.username.as_str();
    let ok = match &info.auth {
        Auth::Password => {
            let pw = secret.unwrap_or("");
            let r = handle.authenticate_password(user, pw).await?;
            match auth_failed(&r) {
                None => true,
                Some(methods) if methods.contains(&MethodKind::KeyboardInteractive) => {
                    keyboard_interactive(handle, user, pw).await?
                }
                Some(_) => false,
            }
        }
        Auth::KeyFile { path } => {
            let path = expand_tilde(path);
            let key = match keys::load_secret_key(&path, secret.filter(|s| !s.is_empty())) {
                Ok(k) => k,
                Err(keys::Error::KeyIsEncrypted) => {
                    bail!(
                        "the key {} is encrypted; a passphrase is required",
                        path.display()
                    )
                }
                Err(e) => {
                    return Err(anyhow!(e))
                        .with_context(|| format!("could not load key {}", path.display()));
                }
            };
            let hash = if key.algorithm().is_rsa() {
                handle.best_supported_rsa_hash().await?.flatten()
            } else {
                None
            };
            let r = handle
                .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                .await?;
            r.success()
        }
        Auth::Agent => agent_auth(handle, user).await?,
        Auth::Anonymous => handle.authenticate_none(user).await?.success(),
        Auth::OAuth { .. } => bail!("OAuth sign-in is only supported for cloud drives"),
    };
    if !ok {
        bail!("authentication failed for {}", info.display());
    }
    Ok(())
}

fn expand_tilde(p: &std::path::Path) -> std::path::PathBuf {
    match p.strip_prefix("~") {
        Ok(rest) => dirs::home_dir()
            .map(|h| h.join(rest))
            .unwrap_or_else(|| p.to_path_buf()),
        Err(_) => p.to_path_buf(),
    }
}

impl SftpBackend {
    pub(crate) fn connect(info: &ConnectionInfo, secret: Option<&str>) -> Result<Self> {
        runtime().block_on(Self::connect_async(info, secret))
    }

    async fn connect_async(info: &ConnectionInfo, secret: Option<&str>) -> Result<Self> {
        let tcp = tcp_connect(&info.host, info.port).await?;
        let config = client::Config {
            inactivity_timeout: None,
            keepalive_interval: Some(Duration::from_secs(20)),
            keepalive_max: 3,
            nodelay: true,
            ..Default::default()
        };
        let rejection = Arc::new(Mutex::new(None));
        let handler = ClientHandler {
            host: info.host.clone(),
            port: info.port,
            rejection: rejection.clone(),
        };
        let handshake = tokio::time::timeout(
            HANDSHAKE_TIMEOUT,
            client::connect_stream(Arc::new(config), tcp, handler),
        )
        .await
        .map_err(|_| anyhow!("SSH handshake with {} timed out", info.display()))?;
        let mut handle = match handshake {
            Ok(h) => h,
            Err(e) => {
                if let Some(rej) = rejection.lock().unwrap_or_else(|p| p.into_inner()).take() {
                    return Err(rej);
                }
                return Err(anyhow!(e))
                    .context(format!("SSH handshake with {} failed", info.display()));
            }
        };
        tokio::time::timeout(AUTH_TIMEOUT, authenticate(&mut handle, info, secret))
            .await
            .map_err(|_| anyhow!("authentication timed out"))??;

        let channel = handle
            .channel_open_session()
            .await
            .context("opening SSH session channel")?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .context("requesting sftp subsystem")?;
        let sftp = tokio::time::timeout(HANDSHAKE_TIMEOUT, SftpSession::new(channel.into_stream()))
            .await
            .map_err(|_| anyhow!("SFTP initialisation timed out"))?
            .map_err(|e| {
                anyhow!("SFTP initialisation failed: {e} (is the SFTP subsystem enabled?)")
            })?;
        sftp.set_timeout(IO_TIMEOUT.as_secs());
        let login_dir = sftp
            .canonicalize(".")
            .await
            .map(|p| paths::normalize(&p))
            .unwrap_or_else(|_| "/".to_string());
        Ok(Self {
            handle,
            sftp,
            login_dir,
            alive: true,
        })
    }

    /// Post-process an operation result: if the SSH session died, mark the
    /// connection dead and classify the error as [`ConnectionLost`].
    fn check<T>(&mut self, r: Result<T>) -> Result<T> {
        match r {
            Err(e) if crate::is_connection_lost(&e) => {
                self.alive = false;
                Err(e)
            }
            Err(e) if self.handle.is_closed() => {
                self.alive = false;
                Err(e.context(ConnectionLost("SSH session closed".into())))
            }
            other => other,
        }
    }
}

fn entry_from(path: &str, attrs: &FileAttributes, target: Option<&FileAttributes>) -> RemoteEntry {
    let is_symlink = attrs.is_symlink();
    let eff = if is_symlink {
        target.unwrap_or(attrs)
    } else {
        attrs
    };
    let is_dir = eff.is_dir() && (!is_symlink || target.is_some());
    RemoteEntry {
        name: paths::file_name(path).to_string(),
        path: path.to_string(),
        is_dir,
        is_symlink,
        size: if is_dir { 0 } else { eff.size.unwrap_or(0) },
        modified: attrs.mtime.and_then(|_| attrs.modified().ok()),
        permissions: attrs.permissions.map(|p| p & 0o7777),
    }
}

impl Backend for SftpBackend {
    fn login_dir(&mut self) -> Result<String> {
        Ok(self.login_dir.clone())
    }

    fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>> {
        let sftp = &self.sftp;
        let fut = async move {
            let rd = sftp.read_dir(path).await.map_err(map_sftp)?;
            let mut out = Vec::new();
            for e in rd {
                let name = e.file_name();
                if name == "." || name == ".." {
                    continue;
                }
                let full = paths::join(path, &name);
                let attrs = e.metadata();
                let target = if attrs.is_symlink() {
                    sftp.metadata(full.clone()).await.ok()
                } else {
                    None
                };
                out.push(entry_from(&full, &attrs, target.as_ref()));
            }
            Ok(out)
        };
        let r = runtime().block_on(fut);
        self.check(r).with_context(|| format!("listing {path}"))
    }

    fn stat(&mut self, path: &str) -> Result<RemoteEntry> {
        let sftp = &self.sftp;
        let fut = async move {
            let attrs = sftp.symlink_metadata(path).await.map_err(map_sftp)?;
            let target = if attrs.is_symlink() {
                sftp.metadata(path).await.ok()
            } else {
                None
            };
            let mut e = entry_from(path, &attrs, target.as_ref());
            if path == "/" {
                e.is_dir = true;
            }
            Ok(e)
        };
        let r = runtime().block_on(fut);
        self.check(r).with_context(|| path.to_string())
    }

    fn mkdir(&mut self, path: &str) -> Result<()> {
        let sftp = &self.sftp;
        let r = runtime().block_on(async move { sftp.create_dir(path).await.map_err(map_sftp) });
        self.check(r)
            .with_context(|| format!("creating directory {path}"))
    }

    fn remove_file(&mut self, path: &str) -> Result<()> {
        let sftp = &self.sftp;
        let r = runtime().block_on(async move { sftp.remove_file(path).await.map_err(map_sftp) });
        self.check(r).with_context(|| format!("deleting {path}"))
    }

    fn remove_dir(&mut self, path: &str) -> Result<()> {
        let sftp = &self.sftp;
        let r = runtime().block_on(async move { sftp.remove_dir(path).await.map_err(map_sftp) });
        self.check(r)
            .with_context(|| format!("removing directory {path}"))
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        let sftp = &self.sftp;
        let r = runtime().block_on(async move { sftp.rename(from, to).await.map_err(map_sftp) });
        self.check(r)
            .with_context(|| format!("renaming {from} to {to}"))
    }

    fn read_file(&mut self, path: &str, sink: &mut dyn FnMut(&[u8]) -> Result<bool>) -> Result<()> {
        let sftp = &self.sftp;
        let fut = async move {
            let mut file = sftp.open(path).await.map_err(map_sftp)?;
            let mut buf = vec![0u8; CHUNK];
            let res = loop {
                let n = match file.read(&mut buf).await {
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => break Err(io_err(e)),
                };
                if n == 0 {
                    break Ok(());
                }
                match sink(&buf[..n]) {
                    Ok(true) => {}
                    Ok(false) => break Ok(()),
                    Err(e) => break Err(e),
                }
            };
            let _ = file.shutdown().await;
            res
        };
        let r = runtime().block_on(fut);
        self.check(r)
    }

    fn write_file(&mut self, path: &str, src: &mut dyn Read, _size: u64) -> Result<()> {
        let sftp = &self.sftp;
        let fut = async move {
            let mut file = sftp.create(path).await.map_err(map_sftp)?;
            let mut buf = vec![0u8; CHUNK];
            let res: Result<()> = loop {
                let n = match src.read(&mut buf) {
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => break Err(e.into()),
                };
                if n == 0 {
                    break Ok(());
                }
                if let Err(e) = file.write_all(&buf[..n]).await {
                    break Err(io_err(e));
                }
            };
            let closed = file.shutdown().await.map_err(io_err);
            res.and(closed)
        };
        let r = runtime().block_on(fut);
        self.check(r).with_context(|| format!("writing {path}"))
    }

    fn is_alive(&self) -> bool {
        self.alive && !self.handle.is_closed()
    }
}
