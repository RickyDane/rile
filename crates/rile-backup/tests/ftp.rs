//! Backups to a server, against an in-process FTP server.

use std::fs;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use rile_backup::{BackupJob, Destination, Outcome, Progress, RunOptions, Trigger};
use rile_remote::{Auth, ConnectionInfo, Protocol, RemoteFs};
use unftp_core::auth::{AuthenticationError, Authenticator, Credentials, Principal};
use unftp_sbe_fs::Filesystem;

#[derive(Debug)]
struct AnyUser;

#[async_trait]
impl Authenticator for AnyUser {
    async fn authenticate(&self, username: &str, _: &Credentials) -> Result<Principal, AuthenticationError> {
        Ok(Principal { username: username.into() })
    }
}

fn start_server(root: std::path::PathBuf) -> u16 {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async move {
            let server = libunftp::ServerBuilder::with_authenticator(Box::new(move || Filesystem::new(root.clone()).unwrap()), Arc::new(AnyUser))
                .passive_ports(40000..=49999)
                .build()
                .unwrap();
            server.listen(format!("127.0.0.1:{port}")).await.unwrap();
        });
    });
    for _ in 0..200 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return port;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("FTP server did not start");
}

#[test]
fn backs_up_incrementally_to_a_server() {
    let server_root = tempfile::tempdir().unwrap();
    let port = start_server(server_root.path().to_path_buf());
    let info = ConnectionInfo {
        id: "ftp".into(),
        name: "test".into(),
        protocol: Protocol::Ftp,
        host: "127.0.0.1".into(),
        port,
        username: String::new(),
        auth: Auth::Anonymous,
        initial_path: None,
    };
    let connect = move |_: &str| -> anyhow::Result<(Box<dyn RemoteFs>, String)> { Ok((rile_remote::connect(&info, None)?, "test".into())) };

    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("Photos");
    fs::create_dir_all(src.join("2026")).unwrap();
    fs::write(src.join("2026/a.jpg"), vec![7u8; 300_000]).unwrap();
    fs::write(src.join("b.txt"), "bravo").unwrap();
    let job = BackupJob {
        sources: vec![src.clone()],
        destination: Destination::Remote { conn: "ftp".into(), path: "/backups/mac".into() },
        ..Default::default()
    };
    let state = tmp.path().join("state");
    let run = |verify: bool| {
        rile_backup::run_job_in(&job, &state, Trigger::Manual, RunOptions { verify }, &connect, &AtomicBool::new(false), &Progress::default()).unwrap()
    };

    let r = run(false);
    assert_eq!((r.outcome.clone(), r.copied), (Outcome::Success, 2), "{r:?}");
    let mirror = server_root.path().join("backups/mac/Photos");
    assert_eq!(fs::read(mirror.join("2026/a.jpg")).unwrap().len(), 300_000);
    assert_eq!(run(false).copied, 0);

    // Edit + delete: the edited file is replaced, both old copies are kept.
    fs::write(src.join("b.txt"), "bravo 2").unwrap();
    fs::OpenOptions::new().write(true).open(src.join("b.txt")).unwrap().set_modified(SystemTime::now() + Duration::from_secs(5)).unwrap();
    fs::remove_file(src.join("2026/a.jpg")).unwrap();
    let r = run(false);
    assert_eq!((r.copied, r.removed), (1, 1), "{r:?}");
    assert_eq!(fs::read_to_string(mirror.join("b.txt")).unwrap(), "bravo 2");
    assert!(!mirror.join("2026/a.jpg").exists());
    let versions: Vec<_> = fs::read_dir(server_root.path().join("backups/mac/.rile-versions")).unwrap().collect();
    assert_eq!(versions.len(), 1);

    // Servers don't keep modification times, so without the local manifest
    // files can't be proven unchanged and are uploaded again (safely: the
    // previous copy goes to the versions).
    fs::remove_dir_all(&state).unwrap();
    let r = run(true);
    assert_eq!((r.outcome.clone(), r.copied, r.files), (Outcome::Success, 1, 1));
    assert_eq!(run(false).copied, 0, "the rebuilt manifest is trusted again");
}
