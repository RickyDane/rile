//! Two Rile devices in one process, talking over 127.0.0.1.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rile_remote::peer::{
    self, DeviceInfo, LocalDevice, Pairing, PeerEvent, PeerServer, PeerStore, Share,
};

struct Device {
    store: Arc<PeerStore>,
    server: PeerServer,
    events: mpsc::Receiver<PeerEvent>,
}

fn device(dir: &std::path::Path, name: &str, shares: Vec<Share>) -> Device {
    let store = PeerStore::open(dir.join("peer")).unwrap();
    let (tx, events) = mpsc::channel();
    let tx = Mutex::new(tx);
    let local = LocalDevice { info: DeviceInfo { name: name.into(), ..Default::default() }, shares };
    // Port 0: never collide with a running Rile.
    let server = PeerServer::start(store.clone(), local, 0, move |e| {
        let _ = tx.lock().unwrap().send(e);
    })
    .unwrap();
    Device { store, server, events }
}

fn next_event(d: &Device) -> PeerEvent {
    d.events.recv_timeout(Duration::from_secs(10)).expect("event")
}

/// Pairs `client` with `host`, both users answering `client_ok` / `host_ok`.
fn pair(client: &Device, host: &Device, client_ok: bool, host_ok: bool) -> bool {
    let pairing = Pairing::start(client.store.clone(), "127.0.0.1", host.server.port(), Some(host.store.id())).unwrap();
    let PeerEvent::PairRequest { code, reply, device } = next_event(host) else { panic!("expected pair request") };
    assert_eq!(code, pairing.code, "both screens show the same code");
    assert_eq!(device.id, client.store.id());
    reply.send(host_ok);
    pairing.finish(client_ok).unwrap()
}

#[test]
fn pairing_needs_both_sides_and_gates_everything_else() {
    let dir = tempfile::tempdir().unwrap();
    let a = device(&dir.path().join("a"), "Alpha", vec![]);
    let b = device(&dir.path().join("b"), "Beta", vec![]);

    // Not paired: sending is refused without bothering the user.
    let file = dir.path().join("hello.txt");
    fs::write(&file, "hi").unwrap();
    let err = peer::send(&a.store, "127.0.0.1", b.server.port(), None, &[file.clone()], &AtomicBool::new(false), &mut |_, _, _| {})
        .unwrap_err();
    assert!(peer::is_not_paired(&err), "{err:#}");

    assert!(!pair(&a, &b, true, false));
    assert!(!pair(&a, &b, false, true));
    assert!(!a.store.is_trusted(b.store.id()) && !b.store.is_trusted(a.store.id()));

    assert!(pair(&a, &b, true, true));
    assert!(a.store.is_trusted(b.store.id()) && b.store.is_trusted(a.store.id()));
    assert_eq!(a.store.trusted_peer(b.store.id()).unwrap().name, "Beta");

    // A different device at the expected address is rejected.
    let err = peer::probe(&a.store, "127.0.0.1", b.server.port(), Some(&"0".repeat(64))).unwrap_err();
    assert!(err.downcast_ref::<peer::IdentityMismatch>().is_some());
}

#[test]
fn refused_pairing_on_the_client_side_trusts_nobody() {
    let dir = tempfile::tempdir().unwrap();
    let a = device(&dir.path().join("a"), "Alpha", vec![]);
    let b = device(&dir.path().join("b"), "Beta", vec![]);
    let pairing = Pairing::start(a.store.clone(), "127.0.0.1", b.server.port(), None).unwrap();
    let PeerEvent::PairRequest { reply, .. } = next_event(&b) else { panic!() };
    assert!(!pairing.finish(false).unwrap());
    reply.send(true);
    std::thread::sleep(Duration::from_millis(200));
    assert!(!a.store.is_trusted(b.store.id()) && !b.store.is_trusted(a.store.id()));
}

#[test]
fn offers_are_accepted_into_the_chosen_folder_or_declined() {
    let dir = tempfile::tempdir().unwrap();
    let a = device(&dir.path().join("a"), "Alpha", vec![]);
    let b = device(&dir.path().join("b"), "Beta", vec![]);
    assert!(pair(&a, &b, true, true));

    let src = dir.path().join("src");
    fs::create_dir_all(src.join("Album/sub")).unwrap();
    fs::write(src.join("Album/a.txt"), "aaa").unwrap();
    fs::write(src.join("Album/sub/b.bin"), vec![7u8; 600_000]).unwrap();
    fs::write(src.join("note.md"), "# hi").unwrap();
    let inbox = dir.path().join("inbox");
    fs::create_dir_all(&inbox).unwrap();
    fs::write(inbox.join("note.md"), "existing").unwrap();

    let paths = vec![src.join("Album"), src.join("note.md")];
    let (store, port) = (a.store.clone(), b.server.port());
    let sender = {
        let paths = paths.clone();
        std::thread::spawn(move || {
            peer::send(&store, "127.0.0.1", port, None, &paths, &AtomicBool::new(false), &mut |_, _, _| {})
        })
    };
    let PeerEvent::Offer { device, items, total, reply, transfer } = next_event(&b) else { panic!("offer") };
    assert_eq!(device.id, a.store.id());
    assert_eq!(total, 3 + 600_000 + 4);
    assert!(items.iter().any(|i| i.path == "Album/sub/b.bin"));
    reply.send(Some(inbox.clone()));
    sender.join().unwrap().unwrap();
    let got = transfer.outcome().expect("finished").unwrap();
    assert_eq!(got, vec![inbox.join("Album"), inbox.join("note copy.md")]);
    assert_eq!(fs::read(inbox.join("Album/sub/b.bin")).unwrap().len(), 600_000);
    assert_eq!(fs::read_to_string(inbox.join("note copy.md")).unwrap(), "# hi");
    assert_eq!(fs::read_to_string(inbox.join("note.md")).unwrap(), "existing");
    assert!(!fs::read_dir(inbox.join("Album")).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().ends_with("rile-part")));

    // Declined: nothing is written.
    let (store, port) = (a.store.clone(), b.server.port());
    let sender = std::thread::spawn(move || {
        peer::send(&store, "127.0.0.1", port, None, &paths, &AtomicBool::new(false), &mut |_, _, _| {})
    });
    let PeerEvent::Offer { reply, .. } = next_event(&b) else { panic!("offer") };
    reply.send(None);
    let err = sender.join().unwrap().unwrap_err();
    assert!(err.to_string().contains("declined"), "{err:#}");
    assert_eq!(fs::read_dir(&inbox).unwrap().count(), 3);
}

#[test]
fn browsing_a_paired_device_is_read_only_and_stays_inside_shares() {
    let dir = tempfile::tempdir().unwrap();
    // The browsing side uses the global store: point it at a temp config.
    let config: PathBuf = dir.path().join("client-config");
    unsafe { std::env::set_var("RILE_CONFIG_DIR", &config) };
    let client_store = PeerStore::global().unwrap();

    let shared = dir.path().join("shared");
    fs::create_dir_all(shared.join("Docs")).unwrap();
    fs::write(shared.join("Docs/report.txt"), "quarterly").unwrap();
    fs::write(dir.path().join("secret.txt"), "nope").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.path().join("secret.txt"), shared.join("escape")).unwrap();
    let host = device(&dir.path().join("host"), "Studio", vec![Share { name: "Work".into(), path: shared.clone() }]);

    let info = peer::connection_info(host.store.id(), "Studio", "127.0.0.1", host.server.port());
    // Not paired yet.
    assert!(rile_remote::connect(&info, None).is_err());

    let pairing = Pairing::start(client_store.clone(), "127.0.0.1", host.server.port(), Some(host.store.id())).unwrap();
    let PeerEvent::PairRequest { reply, .. } = next_event(&host) else { panic!() };
    reply.send(true);
    assert!(pairing.finish(true).unwrap());

    let (shown, trusted) = peer::probe(&client_store, "127.0.0.1", host.server.port(), None).unwrap();
    assert!(trusted);
    assert_eq!(shown.shares, Vec::<String>::new(), "shares are published by the app, not the store");

    let mut fs_ = rile_remote::connect(&info, None).unwrap();
    assert_eq!(fs_.home_dir().unwrap(), "/");
    let root = fs_.list("/").unwrap();
    assert_eq!(root.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["Work"]);
    let work: Vec<_> = fs_.list("/Work").unwrap().into_iter().map(|e| e.name).collect();
    assert_eq!(work, ["Docs"], "links out of the share are hidden");
    let report = fs_.stat("/Work/Docs/report.txt").unwrap();
    assert_eq!((report.size, report.is_dir), (9, false));
    assert_eq!(fs_.read_head("/Work/Docs/report.txt", 4).unwrap(), b"quar");
    // After an early stop the connection is replaced transparently.
    assert_eq!(fs_.read_head("/Work/Docs/report.txt", 100).unwrap(), b"quarterly");

    let local = dir.path().join("downloaded");
    fs_.download("/Work/Docs", &local, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(fs::read_to_string(local.join("report.txt")).unwrap(), "quarterly");

    assert!(fs_.stat("/Work/escape").is_err());
    assert!(fs_.stat("/Work/../secret.txt").is_err());
    assert!(fs_.list("/Nope").is_err());
    assert!(fs_.mkdir("/Work/new").is_err());
    assert!(fs_.remove("/Work/Docs/report.txt").is_err());
    assert!(fs::metadata(shared.join("Docs/report.txt")).is_ok());
}
