//! Integration tests against an in-process FTP/FTPS server (libunftp + unftp-sbe-fs).

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rile_remote::{Auth, ConnectionInfo, Pool, Protocol, connect};
use unftp_core::auth::{AuthenticationError, Authenticator, Credentials, Principal};
use unftp_sbe_fs::Filesystem;

const USER: &str = "alice";
const PASS: &str = "s3cret";

#[derive(Debug)]
struct PasswordAuth;

#[async_trait]
impl Authenticator for PasswordAuth {
    async fn authenticate(
        &self,
        username: &str,
        creds: &Credentials,
    ) -> Result<Principal, AuthenticationError> {
        if username == "anonymous" {
            return Ok(Principal {
                username: username.into(),
            });
        }
        if username != USER {
            return Err(AuthenticationError::BadUser);
        }
        if creds.password.as_deref() != Some(PASS) {
            return Err(AuthenticationError::BadPassword);
        }
        Ok(Principal {
            username: username.into(),
        })
    }
}

struct Server {
    port: u16,
    root: tempfile::TempDir,
}

#[derive(Default)]
struct Opts {
    tls: Option<(PathBuf, PathBuf)>,
    idle_timeout: Option<u64>,
}

fn start(opts: Opts) -> Server {
    let root = tempfile::tempdir().unwrap();
    let port = common::free_port();
    let root_path = root.path().to_path_buf();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let mut b = libunftp::ServerBuilder::with_authenticator(
                Box::new(move || Filesystem::new(root_path.clone()).unwrap()),
                Arc::new(PasswordAuth),
            )
            .greeting("rile test server")
            .passive_ports(40000..=49999);
            if let Some(secs) = opts.idle_timeout {
                b = b.idle_session_timeout(secs);
            }
            if let Some((cert, key)) = opts.tls {
                b = b.ftps(cert, key);
            }
            let server = b.build().unwrap();
            server.listen(format!("127.0.0.1:{port}")).await.unwrap();
        });
    });
    common::wait_for_port(port);
    Server { port, root }
}

fn info(port: u16, protocol: Protocol) -> ConnectionInfo {
    ConnectionInfo {
        id: ConnectionInfo::new_id(),
        name: "test".into(),
        protocol,
        host: "127.0.0.1".into(),
        port,
        username: USER.into(),
        auth: Auth::Password,
        initial_path: None,
    }
}

#[test]
fn ftp_full_roundtrip() {
    common::isolate_known_hosts();
    let server = start(Opts::default());
    let info = info(server.port, Protocol::Ftp);
    let mut fs = connect(&info, Some(PASS)).unwrap();
    assert_eq!(fs.home_dir().unwrap(), "/");
    common::exercise(fs.as_mut(), "", server.root.path());
}

#[test]
fn ftp_login_failure_and_anonymous() {
    common::isolate_known_hosts();
    let server = start(Opts::default());
    let info = info(server.port, Protocol::Ftp);
    let err = connect(&info, Some("wrong")).err().unwrap();
    assert!(format!("{err:#}").contains("login failed"), "{err:#}");

    let anon = ConnectionInfo {
        username: String::new(),
        auth: Auth::Anonymous,
        initial_path: Some("sub".into()),
        ..info.clone()
    };
    std::fs::create_dir(server.root.path().join("sub")).unwrap();
    let mut fs = connect(&anon, None).unwrap();
    assert_eq!(fs.home_dir().unwrap(), "/sub");
    assert!(
        fs.list("/")
            .unwrap()
            .iter()
            .any(|e| e.name == "sub" && e.is_dir)
    );

    let key = ConnectionInfo {
        auth: Auth::Agent,
        ..info
    };
    assert!(connect(&key, None).is_err());
}

#[test]
fn ftp_reconnects_after_server_drops_idle_session() {
    common::isolate_known_hosts();
    let server = start(Opts {
        idle_timeout: Some(1),
        ..Default::default()
    });
    std::fs::write(server.root.path().join("f.txt"), b"abc").unwrap();
    let mut fs = connect(&info(server.port, Protocol::Ftp), Some(PASS)).unwrap();
    assert_eq!(fs.list("/").unwrap().len(), 1);
    std::thread::sleep(Duration::from_millis(2500));
    // The server has closed the control connection; this must transparently reconnect.
    assert_eq!(fs.read_head("/f.txt", 10).unwrap(), b"abc");
    assert_eq!(fs.list("/").unwrap().len(), 1);
}

#[test]
fn pool_shares_connections() {
    common::isolate_known_hosts();
    let server = start(Opts::default());
    let info = info(server.port, Protocol::Ftp);
    let pool = Pool::global();
    assert!(!pool.is_connected(&info.id));
    let a = pool.get_or_connect(&info, Some(PASS)).unwrap();
    let b = pool.get_or_connect(&info, Some(PASS)).unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    assert!(pool.is_connected(&info.id));
    // Use from another thread.
    let a2 = a.clone();
    let n = std::thread::spawn(move || a2.lock().unwrap().list("/").unwrap().len())
        .join()
        .unwrap();
    assert_eq!(n, 0);
    // Changed settings for the same id replace the connection.
    let renamed = ConnectionInfo {
        name: "renamed".into(),
        ..info.clone()
    };
    let c = pool.get_or_connect(&renamed, Some(PASS)).unwrap();
    assert!(!Arc::ptr_eq(&a, &c));
    pool.disconnect(&info.id);
    assert!(!pool.is_connected(&info.id));
}

fn write_self_signed(dir: &Path) -> (PathBuf, PathBuf) {
    let cert =
        rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()]).unwrap();
    let c = dir.join("cert.pem");
    let k = dir.join("key.pem");
    std::fs::write(&c, cert.cert.pem()).unwrap();
    std::fs::write(&k, cert.signing_key.serialize_pem()).unwrap();
    (c, k)
}

#[test]
fn ftps_self_signed_tofu() {
    let kh = common::isolate_known_hosts();
    let certs = tempfile::tempdir().unwrap();
    let server = start(Opts {
        tls: Some(write_self_signed(certs.path())),
        ..Default::default()
    });
    let info = info(server.port, Protocol::Ftps);

    // First use: certificate is pinned.
    let mut fs = connect(&info, Some(PASS)).unwrap();
    let pins = std::fs::read_to_string(&kh).unwrap();
    let pattern = format!("[127.0.0.1]:{} x509-sha256 ", server.port);
    assert!(pins.contains(&pattern), "{pins}");
    common::exercise(fs.as_mut(), "", server.root.path());
    drop(fs);

    // Second use: pin matches.
    connect(&info, Some(PASS)).unwrap();

    // Tamper with the pin: must be rejected with a clear error.
    let tampered = pins
        .lines()
        .map(|l| {
            if l.starts_with(&pattern) {
                format!("{pattern}00:11:22")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&kh, tampered + "\n").unwrap();
    let err = connect(&info, Some(PASS)).err().unwrap();
    let msg = format!("{err:#}");
    assert!(msg.contains("has changed"), "{msg}");

    rile_remote::known_hosts::forget("127.0.0.1", server.port).unwrap();
    connect(&info, Some(PASS)).unwrap();
}

#[test]
fn connect_timeout_and_refused() {
    common::isolate_known_hosts();
    let port = common::free_port(); // nothing listening
    let start = std::time::Instant::now();
    assert!(connect(&info(port, Protocol::Ftp), Some(PASS)).is_err());
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn ftp_reconnects_after_connection_drop() {
    common::isolate_known_hosts();
    let server = start(Opts::default());
    std::fs::write(server.root.path().join("f.txt"), b"abc").unwrap();
    let proxy = common::Proxy::start(server.port);
    let mut fs = connect(&info(proxy.port, Protocol::Ftp), Some(PASS)).unwrap();
    assert_eq!(fs.list("/").unwrap().len(), 1);
    proxy.sever();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(fs.stat("/f.txt").unwrap().size, 3);
    proxy.sever();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(fs.read_head("/f.txt", 10).unwrap(), b"abc");
}
