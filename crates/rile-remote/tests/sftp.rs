//! Integration tests against an in-process SSH/SFTP server (russh + russh-sftp
//! server, backed by a temp directory). Does not use the machine's sshd.
//!
//! The keys under tests/keys are throwaway test keys generated for this suite.

mod common;

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rile_remote::{Auth, ConnectionInfo, HostKeyMismatch, Protocol, connect, known_hosts};
use russh::keys::{PrivateKey, PublicKey, decode_secret_key};
use russh::server::{Auth as SrvAuth, ChannelOpenHandle, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use tokio::sync::Mutex;

const USER: &str = "bob";
const PASS: &str = "hunter2";
const HOST_KEY: &str = include_str!("keys/host_ed25519");
const HOST_KEY2: &str = include_str!("keys/host2_ed25519");
const CLIENT_KEY_PUB: &str = include_str!("keys/client_ed25519_encrypted.pub");
const CLIENT_KEY_PASSPHRASE: &str = "pass phrase";
const RSA_KEY_PUB: &str = include_str!("keys/client_rsa.pub");

fn keys_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/keys")
}

// ------------------------------------------------------------------ SSH server

#[derive(Clone)]
struct Server {
    root: PathBuf,
    authorized: Vec<PublicKey>,
}

impl russh::server::Server for Server {
    type Handler = Conn;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Conn {
        Conn {
            root: self.root.clone(),
            authorized: self.authorized.clone(),
            channels: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

struct Conn {
    root: PathBuf,
    authorized: Vec<PublicKey>,
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
}

impl russh::server::Handler for Conn {
    type Error = anyhow::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<SrvAuth, Self::Error> {
        Ok(if user == USER && password == PASS {
            SrvAuth::Accept
        } else {
            SrvAuth::reject()
        })
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        key: russh::server::VerifiedPublicKey<'_>,
    ) -> Result<SrvAuth, Self::Error> {
        let ok = user == USER
            && self
                .authorized
                .iter()
                .any(|k| k.key_data() == key.public_key.key_data());
        Ok(if ok {
            SrvAuth::Accept
        } else {
            SrvAuth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.lock().await.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.close(channel)?;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name != "sftp" {
            session.channel_failure(channel_id)?;
            return Ok(());
        }
        let channel = self.channels.lock().await.remove(&channel_id).unwrap();
        session.channel_success(channel_id)?;
        let handler = Sftp {
            root: self.root.clone(),
            handles: HashMap::new(),
            next: 0,
        };
        russh_sftp::server::run(channel.into_stream(), handler).await;
        Ok(())
    }
}

// ------------------------------------------------------------------ SFTP server (fs backed)

enum H {
    File(fs::File),
    Dir(Option<Vec<File>>),
}

struct Sftp {
    root: PathBuf,
    handles: HashMap<String, H>,
    next: u64,
}

fn code(e: std::io::Error) -> StatusCode {
    match e.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".into(),
        language_tag: "en-US".into(),
    }
}

impl Sftp {
    fn real(&self, p: &str) -> PathBuf {
        // Relative paths resolve against "/"; ".." cannot escape the root.
        let norm = rile_remote::paths::normalize(&format!("/{p}"));
        self.root.join(norm.trim_start_matches('/'))
    }
    fn handle(&mut self, h: H) -> String {
        self.next += 1;
        let s = format!("h{}", self.next);
        self.handles.insert(s.clone(), h);
        s
    }
}

impl russh_sftp::server::Handler for Sftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(&mut self, _v: u32, _e: HashMap<String, String>) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let p = rile_remote::paths::normalize(&format!("/{path}"));
        Ok(Name {
            id,
            files: vec![File::dummy(p)],
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let m = fs::metadata(self.real(&path)).map_err(code)?;
        Ok(Attrs {
            id,
            attrs: (&m).into(),
        })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let m = fs::symlink_metadata(self.real(&path)).map_err(code)?;
        Ok(Attrs {
            id,
            attrs: (&m).into(),
        })
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let f = fs::OpenOptions::new()
            .read(pflags.contains(OpenFlags::READ))
            .write(pflags.contains(OpenFlags::WRITE))
            .create(pflags.contains(OpenFlags::CREATE))
            .truncate(pflags.contains(OpenFlags::TRUNCATE))
            .append(pflags.contains(OpenFlags::APPEND))
            .open(self.real(&filename))
            .map_err(code)?;
        Ok(Handle {
            id,
            handle: self.handle(H::File(f)),
        })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let Some(H::File(f)) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure);
        };
        f.seek(SeekFrom::Start(offset)).map_err(code)?;
        let mut buf = vec![0u8; len as usize];
        let n = f.read(&mut buf).map_err(code)?;
        if n == 0 {
            return Err(StatusCode::Eof);
        }
        buf.truncate(n);
        Ok(Data { id, data: buf })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let Some(H::File(f)) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure);
        };
        f.seek(SeekFrom::Start(offset)).map_err(code)?;
        f.write_all(&data).map_err(code)?;
        Ok(ok(id))
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.handles.remove(&handle);
        Ok(ok(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let dir = self.real(&path);
        let mut files = vec![
            File::new(".", FileAttributes::default()),
            File::new("..", FileAttributes::default()),
        ];
        for e in fs::read_dir(&dir).map_err(code)? {
            let e = e.map_err(code)?;
            let m = fs::symlink_metadata(e.path()).map_err(code)?;
            files.push(File::new(e.file_name().to_string_lossy(), (&m).into()));
        }
        Ok(Handle {
            id,
            handle: self.handle(H::Dir(Some(files))),
        })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        match self.handles.get_mut(&handle) {
            Some(H::Dir(files)) => match files.take() {
                Some(files) => Ok(Name { id, files }),
                None => Err(StatusCode::Eof),
            },
            _ => Err(StatusCode::Failure),
        }
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _a: FileAttributes,
    ) -> Result<Status, Self::Error> {
        fs::create_dir(self.real(&path)).map_err(code)?;
        Ok(ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        fs::remove_dir(self.real(&path)).map_err(code)?;
        Ok(ok(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        fs::remove_file(self.real(&filename)).map_err(code)?;
        Ok(ok(id))
    }

    async fn rename(&mut self, id: u32, from: String, to: String) -> Result<Status, Self::Error> {
        fs::rename(self.real(&from), self.real(&to)).map_err(code)?;
        Ok(ok(id))
    }

    async fn setstat(
        &mut self,
        id: u32,
        _p: String,
        _a: FileAttributes,
    ) -> Result<Status, Self::Error> {
        Ok(ok(id))
    }

    async fn fsetstat(
        &mut self,
        id: u32,
        _h: String,
        _a: FileAttributes,
    ) -> Result<Status, Self::Error> {
        Ok(ok(id))
    }
}

struct Running {
    port: u16,
    root: tempfile::TempDir,
}

fn start(host_key: &str) -> Running {
    let root = tempfile::tempdir().unwrap();
    let server = Server {
        root: root.path().to_path_buf(),
        authorized: vec![
            PublicKey::from_openssh(CLIENT_KEY_PUB.trim()).unwrap(),
            PublicKey::from_openssh(RSA_KEY_PUB.trim()).unwrap(),
        ],
    };
    let key: PrivateKey = decode_secret_key(host_key, None).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let config = russh::server::Config {
                auth_rejection_time: Duration::from_millis(10),
                auth_rejection_time_initial: Some(Duration::from_millis(0)),
                keys: vec![key],
                ..Default::default()
            };
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            let mut server = server;
            server
                .run_on_socket(Arc::new(config), &listener)
                .await
                .unwrap();
        });
    });
    Running { port, root }
}

fn info(port: u16, auth: Auth) -> ConnectionInfo {
    ConnectionInfo {
        id: ConnectionInfo::new_id(),
        name: "sftp test".into(),
        protocol: Protocol::Sftp,
        host: "127.0.0.1".into(),
        port,
        username: USER.into(),
        auth,
        initial_path: None,
    }
}

#[test]
fn sftp_password_full_roundtrip() {
    common::isolate_known_hosts();
    let srv = start(HOST_KEY);
    let mut fs_ = connect(&info(srv.port, Auth::Password), Some(PASS)).unwrap();
    assert_eq!(fs_.home_dir().unwrap(), "/");
    common::exercise(fs_.as_mut(), "", srv.root.path());
}

#[test]
fn sftp_auth_methods() {
    common::isolate_known_hosts();
    let srv = start(HOST_KEY);

    let err = connect(&info(srv.port, Auth::Password), Some("nope"))
        .err()
        .unwrap();
    assert!(
        format!("{err:#}").contains("authentication failed"),
        "{err:#}"
    );

    let enc = Auth::KeyFile {
        path: keys_dir().join("client_ed25519_encrypted"),
    };
    let err = connect(&info(srv.port, enc.clone()), None).err().unwrap();
    assert!(format!("{err:#}").contains("passphrase"), "{err:#}");
    let mut fs_ = connect(&info(srv.port, enc), Some(CLIENT_KEY_PASSPHRASE)).unwrap();
    assert!(fs_.list("/").unwrap().is_empty());

    let rsa = Auth::KeyFile {
        path: keys_dir().join("client_rsa"),
    };
    let mut fs_ = connect(&info(srv.port, rsa), None).unwrap();
    assert!(fs_.list("/").unwrap().is_empty());

    let missing = Auth::KeyFile {
        path: keys_dir().join("does-not-exist"),
    };
    assert!(connect(&info(srv.port, missing), None).is_err());

    assert!(connect(&info(srv.port, Auth::Anonymous), None).is_err());
}

#[test]
fn sftp_host_key_tofu() {
    let kh = common::isolate_known_hosts();
    let srv = start(HOST_KEY);
    let i = info(srv.port, Auth::Password);
    connect(&i, Some(PASS)).unwrap();
    let pattern = format!("[127.0.0.1]:{} ssh-ed25519 ", srv.port);
    assert!(fs::read_to_string(&kh).unwrap().contains(&pattern));
    connect(&i, Some(PASS)).unwrap();

    // Same host:port now presents a different key -> rejected with a clear error.
    let other = start(HOST_KEY2);
    let content = fs::read_to_string(&kh).unwrap();
    let line = content
        .lines()
        .find(|l| l.starts_with(&pattern))
        .unwrap()
        .to_string();
    let other_pattern = format!("[127.0.0.1]:{} ssh-ed25519 ", other.port);
    fs::write(
        &kh,
        format!("{content}{}\n", line.replace(&pattern, &other_pattern)),
    )
    .unwrap();
    let err = connect(&info(other.port, Auth::Password), Some(PASS))
        .err()
        .unwrap();
    assert!(err.downcast_ref::<HostKeyMismatch>().is_some(), "{err:#}");
    assert!(format!("{err}").contains("has changed"));

    known_hosts::forget("127.0.0.1", other.port).unwrap();
    connect(&info(other.port, Auth::Password), Some(PASS)).unwrap();
}

#[cfg(unix)]
#[test]
fn sftp_symlinks() {
    common::isolate_known_hosts();
    let srv = start(HOST_KEY);
    let root = srv.root.path();
    fs::create_dir(root.join("real")).unwrap();
    fs::write(root.join("real/f.txt"), b"x").unwrap();
    std::os::unix::fs::symlink(root.join("real"), root.join("dirlink")).unwrap();
    std::os::unix::fs::symlink(root.join("real/f.txt"), root.join("filelink")).unwrap();
    std::os::unix::fs::symlink(root.join("missing"), root.join("broken")).unwrap();
    let mut fs_ = connect(&info(srv.port, Auth::Password), Some(PASS)).unwrap();
    let mut l = fs_.list("/").unwrap();
    l.sort_by(|a, b| a.name.cmp(&b.name));
    let summary: Vec<_> = l
        .iter()
        .map(|e| (e.name.as_str(), e.is_dir, e.is_symlink))
        .collect();
    assert_eq!(
        summary,
        [
            ("broken", false, true),
            ("dirlink", true, true),
            ("filelink", false, true),
            ("real", true, false)
        ]
    );
    assert_eq!(l[2].size, 1);
    // Removing a symlink to a directory must not touch the target.
    fs_.remove("/dirlink").unwrap();
    assert!(root.join("real/f.txt").exists());
    assert!(!root.join("dirlink").exists());
}

#[test]
fn sftp_reconnects_after_connection_drop() {
    common::isolate_known_hosts();
    let srv = start(HOST_KEY);
    fs::write(srv.root.path().join("f.txt"), b"abc").unwrap();
    let proxy = common::Proxy::start(srv.port);
    let mut fs_ = connect(&info(proxy.port, Auth::Password), Some(PASS)).unwrap();
    assert_eq!(fs_.list("/").unwrap().len(), 1);
    proxy.sever();
    std::thread::sleep(Duration::from_millis(200));
    // Transparent reconnect on the next operation.
    assert_eq!(fs_.read_head("/f.txt", 10).unwrap(), b"abc");
    proxy.sever();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(fs_.list("/").unwrap().len(), 1);
    assert_eq!(fs_.stat("/f.txt").unwrap().size, 3);
}
