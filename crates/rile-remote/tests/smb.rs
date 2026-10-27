//! Run with an isolated Impacket installation:
//! `RILE_SMB_TEST_PYTHON=/path/to/venv/bin/python3 cargo test -p rile-remote --test smb -- --ignored`

mod common;

use std::process::{Child, Command, Stdio};

use rile_remote::{Auth, ConnectionInfo, Protocol, connect};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "requires a Python environment with Impacket; see module documentation"]
fn smb_authentication_and_file_operations() {
    let executable = std::env::var("RILE_SMB_TEST_PYTHON")
        .expect("Set RILE_SMB_TEST_PYTHON to a Python interpreter with Impacket installed");
    let root = tempfile::tempdir().unwrap();
    let share = "Données";
    let storage = root.path().join(share);
    std::fs::create_dir(&storage).unwrap();
    let port = common::free_port();
    let _server = Server(
        Command::new(executable)
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/smb_server.py"))
            .args([&port.to_string(), share])
            .arg(&storage)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    common::wait_for_port(port);
    let base = format!("/{share}");
    let info = ConnectionInfo {
        id: ConnectionInfo::new_id(),
        name: "SMB test".into(),
        protocol: Protocol::Smb,
        host: "127.0.0.1".into(),
        port,
        username: "alice".into(),
        auth: Auth::Password,
        initial_path: Some(base.clone()),
    };
    assert!(connect(&info, Some("wrong")).is_err());
    let mut fs = connect(&info, Some("s3cret")).unwrap();
    assert_eq!(fs.home_dir().unwrap(), base);
    // Impacket rejects NDR64, reproducing Samba/NAS transfer-syntax rejection.
    let mut server_info = info.clone();
    server_info.id = ConnectionInfo::new_id();
    server_info.initial_path = None;
    let mut server = connect(&server_info, Some("s3cret")).unwrap();
    let shares = server.list("/").unwrap();
    assert!(
        shares
            .iter()
            .any(|entry| entry.name.to_lowercase() == share.to_lowercase()
                && entry.is_dir
                && entry.path == format!("/{}", entry.name)),
        "{shares:?}"
    );
    assert!(
        shares
            .iter()
            .any(|entry| entry.name.eq_ignore_ascii_case("Second")),
        "{shares:?}"
    );
    assert!(
        !shares
            .iter()
            .any(|entry| entry.name.eq_ignore_ascii_case("IPC$")),
        "{shares:?}"
    );
    // Repeated enumeration must close and reopen the pipe cleanly.
    assert_eq!(server.list("/").unwrap().len(), shares.len());
    common::exercise(fs.as_mut(), &base, root.path());
    assert!(fs.rename(&base, "/Renamed").is_err());
    // Deleting a share must fail without touching its contents.
    std::fs::write(storage.join("keep.txt"), b"do not remove").unwrap();
    assert!(fs.remove(&base).is_err());
    assert!(storage.is_dir());
    assert!(storage.join("keep.txt").is_file());
    let mut direct = connect(&info, Some("s3cret")).unwrap();
    assert_eq!(direct.home_dir().unwrap(), base);
    assert!(direct.stat(&base).unwrap().is_dir);
}
