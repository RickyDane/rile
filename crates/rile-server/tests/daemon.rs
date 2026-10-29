//! A daemon and a Rile "app" (a peer store) in one process.

use std::fs;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use rile_remote::peer::{self, Pairing, PeerStore};
use rile_server::Paths;
use rile_server::config::{Access, Config, ShareConfig};
use rile_server::daemon::{Daemon, Options};
use rile_server::ipc::{Client, Request, Response};

fn status(paths: &Paths) -> rile_server::ipc::Status {
    match Client::call(&paths.socket, &Request::Status).unwrap() {
        Response::Status(s) => s,
        other => panic!("{other:?}"),
    }
}

#[test]
fn pairs_through_the_cli_window_serves_shares_and_fills_the_inbox() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        config: dir.path().join("etc/config.toml"),
        state: dir.path().join("state"),
        // Unix socket paths are short-limited: keep it near the root of the temp dir.
        socket: dir.path().join("c.sock"),
    };
    let (media, dropbox) = (dir.path().join("media"), dir.path().join("drop"));
    fs::create_dir_all(&media).unwrap();
    fs::create_dir_all(&dropbox).unwrap();
    fs::write(media.join("song.flac"), "la la").unwrap();
    let inbox = dir.path().join("inbox");
    let mut config = Config { name: "Test Box".into(), inbox: Some(inbox.clone()), ..Default::default() };
    config.shares.push(ShareConfig { name: "Media".into(), path: media.clone(), access: Access::ReadOnly });
    config.save(&paths.config).unwrap();

    let daemon = Daemon::start(Options { paths: paths.clone(), port: Some(0), mdns: false }).unwrap();
    let port = daemon.port();
    let s = status(&paths);
    assert_eq!((s.name.as_str(), s.port, s.shares.len()), ("Test Box", port, 1));

    // The app's side.
    unsafe { std::env::set_var("RILE_CONFIG_DIR", dir.path().join("app")) };
    let app = PeerStore::global().unwrap();
    PeerStore::set_app(app.clone());

    // Without a pairing window, requests are declined.
    let pairing = Pairing::start(app.clone(), "127.0.0.1", port, Some(daemon.id())).unwrap();
    assert!(!pairing.finish(true).unwrap());

    // `rilectl pair`.
    let mut cli = Client::connect(&paths.socket).unwrap();
    cli.send(&Request::Pair { secs: 30 }).unwrap();
    assert_eq!(cli.recv().unwrap(), Some(Response::PairOpen { secs: 30 }));
    let pairing = Pairing::start(app.clone(), "127.0.0.1", port, Some(daemon.id())).unwrap();
    let shown = peer::format_code(pairing.code);
    let finishing = std::thread::spawn(move || pairing.finish(true).unwrap());
    let Some(Response::PairRequest { code, .. }) = cli.recv().unwrap() else { panic!("pair request") };
    assert_eq!(code, shown, "both sides show the same code");
    cli.send(&Request::Answer { accept: true }).unwrap();
    assert!(finishing.join().unwrap());
    assert!(matches!(cli.recv().unwrap(), Some(Response::PairResult { paired: true, .. })));
    drop(cli);
    assert!(status(&paths).devices.iter().any(|d| d.paired && d.id == app.id()));

    let info = peer::connection_info(daemon.id(), "Test Box", "127.0.0.1", port);
    let mut remote = rile_remote::connect(&info, None).unwrap();
    assert_eq!(remote.list("/").unwrap().into_iter().map(|e| e.name).collect::<Vec<_>>(), ["Media"]);
    assert!(remote.mkdir("/Media/new").is_err(), "read-only");

    // `rilectl share add --read-write` → reload.
    config.shares.push(ShareConfig { name: "Drop".into(), path: dropbox.clone(), access: Access::ReadWrite });
    config.save(&paths.config).unwrap();
    assert_eq!(Client::call(&paths.socket, &Request::Reload).unwrap(), Response::Ok);
    let mut remote = rile_remote::connect(&info, None).unwrap();
    let mut names: Vec<_> = remote.list("/").unwrap().into_iter().map(|e| e.name).collect();
    names.sort();
    assert_eq!(names, ["Drop", "Media"]);
    remote.mkdir("/Drop/new").unwrap();
    assert!(dropbox.join("new").is_dir());

    // "Send to" lands in the inbox without asking.
    let file = dir.path().join("hello.txt");
    fs::write(&file, "hi").unwrap();
    peer::send(&app, "127.0.0.1", port, None, &[file], &AtomicBool::new(false), &mut |_, _, _| {}).unwrap();
    assert_eq!(fs::read_to_string(inbox.join("hello.txt")).unwrap(), "hi");

    // A broken config is refused and the old one kept.
    fs::write(&paths.config, "[[share]]\nname = \"a/b\"\npath = \"/\"\n").unwrap();
    assert!(Client::call(&paths.socket, &Request::Reload).is_err());
    assert_eq!(status(&paths).shares.len(), 2);

    assert_eq!(Client::call(&paths.socket, &Request::Forget { id: peer::short_id(app.id()) }).unwrap(), Response::Ok);
    assert!(!status(&paths).devices.iter().any(|d| d.paired));
    drop(daemon);
    std::thread::sleep(Duration::from_millis(100));
    assert!(!paths.socket.exists());
}
