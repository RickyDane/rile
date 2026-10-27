#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;

use rile_remote::{RemoteFs, known_hosts};

/// Point known_hosts at a per-process temp file (never the user's real one).
pub fn isolate_known_hosts() -> PathBuf {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    let dir = DIR.get_or_init(|| tempfile::tempdir().unwrap());
    let p = dir.path().join("known_hosts");
    known_hosts::set_path_override(Some(p.clone()));
    p
}

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

pub fn wait_for_port(port: u16) {
    for _ in 0..200 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("server on port {port} did not start");
}

/// Deterministic pseudo-random bytes.
pub fn data(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2654435761).wrapping_add(1);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 8) as u8
        })
        .collect()
}

/// Create a small local tree and return the total file bytes.
pub fn make_tree(root: &Path) -> u64 {
    fs::create_dir_all(root.join("sub/deeper")).unwrap();
    fs::create_dir_all(root.join("empty dir")).unwrap();
    fs::write(root.join("big.bin"), data(700_000, 1)).unwrap();
    fs::write(root.join("sub/b.txt"), b"hello from b\n").unwrap();
    fs::write(root.join("sub/deeper/c ü.txt"), data(1234, 2)).unwrap();
    fs::write(root.join("zero.txt"), b"").unwrap();
    700_000 + 13 + 1234
}

pub fn assert_same_tree(a: &Path, b: &Path) {
    let mut ea: Vec<_> = fs::read_dir(a)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    let mut eb: Vec<_> = fs::read_dir(b)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    ea.sort();
    eb.sort();
    assert_eq!(
        ea,
        eb,
        "entries differ in {} vs {}",
        a.display(),
        b.display()
    );
    for name in ea {
        let (pa, pb) = (a.join(&name), b.join(&name));
        if pa.is_dir() {
            assert!(pb.is_dir());
            assert_same_tree(&pa, &pb);
        } else {
            assert_eq!(
                fs::read(&pa).unwrap(),
                fs::read(&pb).unwrap(),
                "{}",
                pa.display()
            );
        }
    }
}

/// Exercise the whole RemoteFs surface against a server whose storage root is
/// `server_root` (the remote path `base` must map to `server_root/<base>`).
pub fn exercise(fs_: &mut dyn RemoteFs, base: &str, server_root: &Path) {
    let srv = |rel: &str| server_root.join(base.trim_start_matches('/')).join(rel);
    let no_cancel = AtomicBool::new(false);
    let local = tempfile::tempdir().unwrap();

    // mkdir + create_file + list + stat
    let dir = format!("{base}/work");
    fs_.mkdir(&dir).unwrap();
    fs_.create_file(&format!("{dir}/empty.txt")).unwrap();
    let entries = fs_.list(&dir).unwrap();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].name, "empty.txt");
    assert_eq!(entries[0].path, format!("{dir}/empty.txt"));
    assert!(!entries[0].is_dir);
    assert_eq!(entries[0].size, 0);
    let st = fs_.stat(&dir).unwrap();
    assert!(st.is_dir);
    assert_eq!(st.name, "work");
    assert!(fs_.stat(&format!("{dir}/nope")).is_err());

    // upload a tree with progress
    let src = local.path().join("src");
    let total = make_tree(&src);
    let mut last = (0, 0);
    let mut calls = 0;
    fs_.upload(&src, &format!("{dir}/up"), &no_cancel, &mut |d, t| {
        assert!(d <= t);
        last = (d, t);
        calls += 1;
    })
    .unwrap();
    assert_eq!(last, (total, total));
    assert!(calls >= 3);
    assert_same_tree(&src, &srv("work/up"));

    // listing details
    let mut up = fs_.list(&format!("{dir}/up")).unwrap();
    up.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<_> = up.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["big.bin", "empty dir", "sub", "zero.txt"]);
    assert_eq!(up[0].size, 700_000);
    assert!(up[0].modified.is_some());
    assert!(up[1].is_dir && up[2].is_dir && !up[3].is_dir);
    let st = fs_.stat(&format!("{dir}/up/sub/deeper/c ü.txt")).unwrap();
    assert_eq!(st.size, 1234);
    assert_eq!(st.name, "c ü.txt");

    // read_head
    let head = fs_.read_head(&format!("{dir}/up/big.bin"), 100).unwrap();
    assert_eq!(head, data(700_000, 1)[..100]);
    let head = fs_.read_head(&format!("{dir}/up/sub/b.txt"), 4096).unwrap();
    assert_eq!(head, b"hello from b\n");
    // The connection must still be usable after an early-terminated read.
    assert_eq!(fs_.list(&format!("{dir}/up/sub")).unwrap().len(), 2);

    // download the tree back
    let dst = local.path().join("dst");
    let mut last = (0, 0);
    fs_.download(&format!("{dir}/up"), &dst, &no_cancel, &mut |d, t| {
        last = (d, t)
    })
    .unwrap();
    assert_eq!(last, (total, total));
    assert_same_tree(&src, &dst);
    // single file download
    let one = local.path().join("one.bin");
    fs_.download(
        &format!("{dir}/up/big.bin"),
        &one,
        &no_cancel,
        &mut |_, _| {},
    )
    .unwrap();
    assert_eq!(fs::read(&one).unwrap(), data(700_000, 1));

    // rename
    fs_.rename(&format!("{dir}/up/big.bin"), &format!("{dir}/renamed.bin"))
        .unwrap();
    assert!(fs_.stat(&format!("{dir}/up/big.bin")).is_err());
    assert_eq!(
        fs_.stat(&format!("{dir}/renamed.bin")).unwrap().size,
        700_000
    );

    // cancellation (upload and download)
    let cancel = AtomicBool::new(false);
    let err = fs_
        .upload(
            &src.join("big.bin"),
            &format!("{dir}/cancelled.bin"),
            &cancel,
            &mut |d, _| {
                if d > 0 {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            },
        )
        .unwrap_err();
    assert!(rile_remote::is_cancelled(&err), "{err:#}");
    assert!(
        fs_.stat(&format!("{dir}/cancelled.bin")).is_err(),
        "partial upload removed"
    );
    let cancel = AtomicBool::new(false);
    let target = local.path().join("cancelled.bin");
    let err = fs_
        .download(
            &format!("{dir}/renamed.bin"),
            &target,
            &cancel,
            &mut |d, _| {
                if d > 0 {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            },
        )
        .unwrap_err();
    assert!(rile_remote::is_cancelled(&err), "{err:#}");
    assert!(!target.exists(), "partial download removed");
    // still usable
    assert!(fs_.stat(&format!("{dir}/renamed.bin")).is_ok());

    // recursive remove
    fs_.remove(&dir).unwrap();
    assert!(fs_.stat(&dir).is_err());
    assert!(!srv("work").exists());
    assert!(fs_.remove("/").is_err());
}

/// A TCP forwarder whose live connections can be severed to simulate network drops.
pub struct Proxy {
    pub port: u16,
    conns: std::sync::Arc<std::sync::Mutex<Vec<std::net::TcpStream>>>,
}

impl Proxy {
    pub fn start(target_port: u16) -> Proxy {
        use std::net::{Shutdown, TcpListener, TcpStream};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let conns = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TcpStream>::new()));
        let c2 = conns.clone();
        std::thread::spawn(move || {
            for client in listener.incoming() {
                let Ok(client) = client else { continue };
                let Ok(server) = TcpStream::connect(("127.0.0.1", target_port)) else {
                    continue;
                };
                {
                    let mut g = c2.lock().unwrap();
                    g.push(client.try_clone().unwrap());
                    g.push(server.try_clone().unwrap());
                }
                for (mut a, mut b) in [
                    (client.try_clone().unwrap(), server.try_clone().unwrap()),
                    (server, client),
                ] {
                    std::thread::spawn(move || {
                        let _ = std::io::copy(&mut a, &mut b);
                        let _ = b.shutdown(Shutdown::Both);
                        let _ = a.shutdown(Shutdown::Both);
                    });
                }
            }
        });
        Proxy { port, conns }
    }

    /// Abruptly close every proxied connection.
    pub fn sever(&self) {
        for c in self.conns.lock().unwrap().drain(..) {
            let _ = c.shutdown(std::net::Shutdown::Both);
        }
    }
}
