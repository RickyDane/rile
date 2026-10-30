//! Nearby Devices: finding other Rile installations on the local network,
//! pairing with them, browsing their shared folders and sending them files.
//! The protocol lives in [`rile_remote::peer`]; this is the app side.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::{WindowExt, h_flex, notification::Notification, v_flex};
use gpui_kit::{App, AppContext as _, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, Styled, Task, Window, div, px};
use rile_core::history::Location;
use rile_core::ops::Progress;
use rile_core::settings::Settings;
use rile_remote::peer::{
    self, DeviceInfo, DeviceKind, LocalDevice, NearbyPeer, Pairing, PeerDiscovery, PeerEvent, PeerServer, PeerStore, Share,
    TrustedPeer,
};

use super::{Workspace, item_count};
use crate::views::kit;

/// How often discovery results are picked up.
const POLL: Duration = Duration::from_millis(1500);
/// How often paired devices are asked for fresh details (storage, shares).
const PROBE_EVERY: Duration = Duration::from_secs(60);

/// This computer's name, model, OS and form factor (read once; slow-ish).
#[derive(Clone, Debug, Default)]
pub struct DeviceFacts {
    pub name: String,
    pub model: String,
    pub os: String,
    pub kind: DeviceKind,
    /// Installed RAM in bytes; 0 when unknown.
    pub memory: u64,
}

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = rile_services::no_window(std::process::Command::new(cmd).args(args)).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

static FACTS: std::sync::OnceLock<DeviceFacts> = std::sync::OnceLock::new();

/// [`device_facts`] once they have been read.
pub fn cached_facts() -> Option<DeviceFacts> {
    FACTS.get().cloned()
}

/// [`read_device_facts`], read once per process.
pub fn device_facts() -> DeviceFacts {
    FACTS.get_or_init(read_device_facts).clone()
}

fn read_device_facts() -> DeviceFacts {
    #[cfg(target_os = "macos")]
    {
        let hw = run("system_profiler", &["SPHardwareDataType"]).unwrap_or_default();
        let field = |key: &str| {
            hw.lines().find_map(|l| l.trim().strip_prefix(key).map(|v| v.trim().to_string())).filter(|v| !v.is_empty())
        };
        let model_name = field("Model Name:").unwrap_or_else(|| "Mac".into());
        let chip = field("Chip:").map(|c| c.trim_start_matches("Apple ").to_string());
        let kind = if model_name.contains("MacBook") { DeviceKind::Laptop } else { DeviceKind::Desktop };
        DeviceFacts {
            name: run("scutil", &["--get", "ComputerName"]).unwrap_or_else(|| model_name.clone()),
            model: match chip {
                Some(chip) => format!("{model_name}, {chip}"),
                None => model_name,
            },
            os: run("sw_vers", &["-productVersion"]).map(|v| format!("macOS {v}")).unwrap_or_else(|| "macOS".into()),
            kind,
            memory: rile_services::local_model::system_memory().unwrap_or(0),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let name = std::env::var("COMPUTERNAME")
            .ok()
            .or_else(sysinfo::System::host_name)
            .unwrap_or_else(|| "This computer".into());
        let os = sysinfo::System::long_os_version().unwrap_or_else(|| std::env::consts::OS.to_string());
        let laptop = cfg!(windows) && run("powershell", &["-NoProfile", "-Command", "(Get-CimInstance Win32_Battery) -ne $null"]).as_deref() == Some("True");
        let cpu = {
            let mut sys = sysinfo::System::new();
            sys.refresh_cpu_all();
            sys.cpus().first().map(|c| c.brand().trim().to_string()).filter(|b| !b.is_empty())
        };
        DeviceFacts {
            name,
            model: cpu.unwrap_or_else(|| if cfg!(windows) { "Windows PC".into() } else { "PC".into() }),
            os,
            kind: if laptop { DeviceKind::Laptop } else { DeviceKind::Desktop },
            memory: rile_services::local_model::system_memory().unwrap_or(0),
        }
    }
}

/// A device shown under Nearby: paired ones (online or not) and unpaired
/// ones currently on the network.
#[derive(Clone, Debug)]
pub struct DeviceRow {
    pub id: String,
    pub name: String,
    pub model: String,
    pub os: String,
    pub kind: DeviceKind,
    /// Installed RAM in bytes; 0 when unknown.
    pub memory: u64,
    pub paired: bool,
    /// Where it can be reached right now.
    pub online: Option<(String, u16)>,
    /// Details it last sent (paired devices only).
    pub details: Option<DeviceInfo>,
    /// It no longer has this device in its paired list.
    pub forgot_us: bool,
}

#[derive(Default)]
pub struct NearbyState {
    pub store: Option<Arc<PeerStore>>,
    server: Option<PeerServer>,
    discovery: Option<PeerDiscovery>,
    pub facts: Option<DeviceFacts>,
    pub local: LocalDevice,
    pub online: Vec<NearbyPeer>,
    pub trusted: Vec<TrustedPeer>,
    /// Device id → (details, whether it trusts us, when asked).
    details: HashMap<String, (DeviceInfo, bool, Instant)>,
    probing: HashSet<String>,
    /// Devices a pairing is running with (their rows show a spinner).
    pub pairing: HashSet<String>,
    pub error: Option<String>,
    starting: bool,
    _poll: Option<Task<()>>,
    _events: Option<Task<()>>,
}

#[test]
fn nearby_loading_includes_initial_details_only() {
    let mut state = NearbyState::default();
    let id = "loading-test";
    assert!(!state.is_loading(id));
    state.probing.insert(id.into());
    assert!(state.is_loading(id));
    state.details.insert(id.into(), (DeviceInfo::default(), true, Instant::now()));
    // A routine refresh with details already available shouldn't spin.
    assert!(!state.is_loading(id));
    state.details.remove(id);
    state.probing.remove(id);
    assert!(!state.is_loading(id)); // Failed probe: no endless spinner.
    state.pairing.insert(id.into());
    assert!(state.is_loading(id));
}

impl NearbyState {
    pub fn is_loading(&self, id: &str) -> bool {
        self.pairing.contains(id) || (self.probing.contains(id) && !self.details.contains_key(id))
    }

    pub fn running(&self) -> bool {
        self.server.is_some()
    }

    pub fn port(&self) -> Option<u16> {
        self.server.as_ref().map(|s| s.port())
    }

    /// Paired devices first (online before offline), then unpaired ones nearby.
    pub fn rows(&self) -> Vec<DeviceRow> {
        let mut rows: Vec<DeviceRow> = Vec::new();
        for t in &self.trusted {
            let online = self.online.iter().find(|p| p.id == t.id);
            let details = self.details.get(&t.id);
            rows.push(DeviceRow {
                id: t.id.clone(),
                name: details.map(|d| d.0.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| t.name.clone()),
                model: t.model.clone(),
                os: t.os.clone(),
                kind: t.kind,
                memory: details.map(|d| d.0.memory).filter(|m| *m > 0).or(online.map(|p| p.memory).filter(|m| *m > 0)).unwrap_or(t.memory),
                paired: true,
                online: online.map(|p| (p.host.clone(), p.port)),
                details: details.map(|d| d.0.clone()),
                forgot_us: details.is_some_and(|d| !d.1),
            });
        }
        for p in &self.online {
            if rows.iter().any(|r| r.id == p.id) {
                continue;
            }
            rows.push(DeviceRow {
                id: p.id.clone(),
                name: p.name.clone(),
                model: p.model.clone(),
                os: p.os.clone(),
                kind: p.kind,
                memory: p.memory,
                paired: false,
                online: Some((p.host.clone(), p.port)),
                details: None,
                forgot_us: false,
            });
        }
        rows.sort_by_key(|r| (!r.paired, r.online.is_none(), r.name.to_lowercase()));
        rows
    }

    pub fn row(&self, id: &str) -> Option<DeviceRow> {
        self.rows().into_iter().find(|r| r.id == id)
    }
}

/// What this device publishes, from settings and the boot volume.
pub fn local_device(settings: &Settings, facts: &DeviceFacts, volumes: &[rile_core::places::Volume]) -> LocalDevice {
    let boot = volumes
        .iter()
        .find(|v| v.mount_point == std::path::Path::new("/") || v.mount_point.to_string_lossy().eq_ignore_ascii_case("C:\\"))
        .or_else(|| volumes.iter().find(|v| !v.removable));
    let shares: Vec<Share> = settings
        .nearby
        .shares
        .iter()
        .filter(|s| s.path.is_dir())
        .map(|s| Share { name: s.name.clone(), path: s.path.clone(), writable: false })
        .collect();
    let name = Some(settings.nearby.device_name.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| facts.name.clone());
    LocalDevice {
        info: DeviceInfo {
            id: String::new(),
            name,
            model: facts.model.clone(),
            os: facts.os.clone(),
            kind: facts.kind,
            memory: facts.memory,
            storage_used: boot.map(|v| v.total.saturating_sub(v.available)).unwrap_or(0),
            storage_total: boot.map(|v| v.total).unwrap_or(0),
            shares: shares.iter().map(|s| s.name.clone()).collect(),
            writable: Vec::new(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        shares,
    }
}

fn name_of(info: &DeviceInfo) -> String {
    peer::describe(info)
}

impl Workspace {
    /// Starts, updates or stops Nearby Devices to match the settings.
    pub(crate) fn sync_nearby(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settings.nearby.enabled || cfg!(test) && std::env::var_os("RILE_TEST_NEARBY").is_none() {
            if self.nearby.running() || self.nearby.starting {
                self.nearby = NearbyState::default();
                cx.notify();
            }
            return;
        }
        if self.nearby.server.is_some() {
            self.update_local_device();
            return;
        }
        if self.nearby.starting {
            return;
        }
        self.nearby.starting = true;
        let (tx, rx) = smol::channel::unbounded::<PeerEvent>();
        let cached = self.nearby.facts.clone();
        let (settings, volumes) = (self.settings.clone(), self.volumes.clone());
        let port = std::env::var("RILE_NEARBY_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(peer::DEFAULT_PORT);
        let start = cx.background_spawn(async move {
            let facts = cached.unwrap_or_else(device_facts);
            let local = local_device(&settings, &facts, &volumes);
            let store = PeerStore::global()?;
            PeerStore::set_app(store.clone());
            let server = PeerServer::start(store.clone(), local.clone(), port, move |ev| {
                let _ = tx.try_send(ev);
            })?;
            let mut discovery = PeerDiscovery::new(store.id())?;
            discovery.advertise(&local.info, server.port())?;
            anyhow::Ok((facts, local, store, server, discovery))
        });
        self.nearby._events = Some(cx.spawn_in(window, async move |this, cx| {
            while let Ok(ev) = rx.recv().await {
                if this.update_in(cx, |ws, window, cx| ws.handle_peer_event(ev, window, cx)).is_err() {
                    break;
                }
            }
        }));
        self.nearby._poll = Some(cx.spawn_in(window, async move |this, cx| {
            let started = start.await;
            let ok = this
                .update_in(cx, |ws, window, cx| {
                    ws.nearby.starting = false;
                    match started {
                        Ok((facts, local, store, server, discovery)) => {
                            peer::set_local_device(local.clone());
                            ws.nearby.trusted = store.trusted();
                            ws.nearby.facts = Some(facts);
                            ws.nearby.local = local;
                            ws.nearby.store = Some(store);
                            ws.nearby.server = Some(server);
                            ws.nearby.discovery = Some(discovery);
                            ws.nearby.error = None;
                            // Settings may have changed while starting.
                            ws.sync_nearby(window, cx);
                            true
                        }
                        Err(e) => {
                            eprintln!("nearby: {e:#}");
                            ws.nearby.error = Some(format!("{e:#}"));
                            cx.notify();
                            false
                        }
                    }
                })
                .unwrap_or(false);
            if !ok {
                return;
            }
            loop {
                cx.background_executor().timer(POLL).await;
                if this.update(cx, |ws, cx| ws.poll_nearby(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// Republishes this device after a change of name, shares or storage.
    fn update_local_device(&mut self) {
        let (Some(server), Some(facts)) = (&self.nearby.server, &self.nearby.facts) else { return };
        let local = local_device(&self.settings, facts, &self.volumes);
        // Storage changes constantly; only re-announce for visible changes.
        let announce = local.info.name != self.nearby.local.info.name;
        if local != self.nearby.local {
            server.update(local.clone());
            peer::set_local_device(local.clone());
            if announce && let Some(d) = self.nearby.discovery.as_mut() {
                let _ = d.advertise(&local.info, server.port());
            }
            self.nearby.local = local;
        }
    }

    fn poll_nearby(&mut self, cx: &mut Context<Self>) {
        self.update_local_device();
        let (Some(discovery), Some(store)) = (self.nearby.discovery.as_mut(), self.nearby.store.clone()) else { return };
        let online = discovery.poll();
        let trusted = store.trusted();
        let changed = online != self.nearby.online || trusted != self.nearby.trusted;
        self.nearby.online = online;
        self.nearby.trusted = trusted;
        for p in self.nearby.online.clone() {
            let paired = self.nearby.trusted.iter().any(|t| t.id == p.id);
            if !paired {
                continue;
            }
            crate::remote::register(peer::connection_info(&p.id, &p.name, &p.host, p.port), None);
            let stale = self.nearby.details.get(&p.id).is_none_or(|d| d.2.elapsed() > PROBE_EVERY);
            if stale && self.nearby.probing.insert(p.id.clone()) {
                self.probe_peer(p, store.clone(), cx);
                cx.notify();
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn probe_peer(&mut self, p: NearbyPeer, store: Arc<PeerStore>, cx: &mut Context<Self>) {
        let id = p.id.clone();
        let task = cx.background_spawn(async move { peer::probe(&store, &p.host, p.port, Some(&p.id)) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |ws, cx| {
                ws.nearby.probing.remove(&id);
                match result {
                    Ok((info, trusts_us)) => {
                        ws.nearby.details.insert(id, (info, trusts_us, Instant::now()));
                    }
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens the Devices page.
    pub fn open_devices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_location(Location::Devices, window, cx);
    }

    /// Opens a paired device's shared folders, or offers to pair.
    pub fn open_device(&mut self, id: &str, share: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.nearby.row(id) else { return };
        if !row.paired {
            self.pair_with(id, window, cx);
            return;
        }
        let Some((host, port)) = row.online.clone() else {
            window.push_notification(Notification::info(format!("{} isn't on the network right now", row.name)), cx);
            return;
        };
        if row.forgot_us {
            window.push_notification(Notification::warning(format!("{} isn't paired with this computer anymore. Pair again to browse it.", row.name)), cx);
            self.pair_with(id, window, cx);
            return;
        }
        crate::remote::register(peer::connection_info(id, &row.name, &host, port), None);
        let path = share.map(|s| format!("/{s}")).unwrap_or_else(|| "/".into());
        let conn = peer::conn_id(id);
        self.pane().update(cx, |p, cx| p.navigate(Location::Remote { conn, path }, cx));
    }

    /// Starts pairing with a device on the network: both screens show a code
    /// the users compare before confirming.
    pub fn pair_with(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(row), Some(store)) = (self.nearby.row(id), self.nearby.store.clone()) else { return };
        let Some((host, port)) = row.online.clone() else {
            window.push_notification(Notification::info(format!("{} isn't on the network right now", row.name)), cx);
            return;
        };
        if !self.nearby.pairing.insert(id.to_string()) {
            return;
        }
        cx.notify();
        let peer_id = id.to_string();
        let task = cx.background_spawn({
            let peer_id = peer_id.clone();
            async move { Pairing::start(store, &host, port, Some(&peer_id)) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let started = task.await;
            this.update_in(cx, |ws, window, cx| match started {
                Ok(pairing) => ws.confirm_pairing(peer_id, pairing, window, cx),
                Err(e) => {
                    ws.nearby.pairing.remove(&peer_id);
                    window.push_notification(Notification::error(format!("Couldn't pair: {e:#}")), cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn confirm_pairing(&mut self, peer_id: String, pairing: Pairing, window: &mut Window, cx: &mut Context<Self>) {
        let device = pairing.device().clone();
        let code = pairing.code;
        // Closing the dialog any other way than "Pair" drops the guard, which
        // tells the other device this side declined.
        let guard = Arc::new(Mutex::new(PairGuard(Some(pairing))));
        let weak = cx.entity().downgrade();
        let name = name_of(&device);
        // Rile Server has no screen: its admin confirms with `rilectl pair`.
        let server = device.kind == DeviceKind::Server;
        let on_pair = {
            let (guard, weak, peer_id, name) = (guard.clone(), weak.clone(), peer_id.clone(), name.clone());
            move |window: &mut Window, cx: &mut App| {
                let Some(pairing) = guard.lock().ok().and_then(|mut g| g.0.take()) else { return };
                window.push_notification(Notification::info(format!("Waiting for {name} to confirm…")), cx);
                let task = cx.background_spawn(async move { pairing.finish(true) });
                let (weak, peer_id, name) = (weak.clone(), peer_id.clone(), name.clone());
                window
                    .spawn(cx, async move |cx| {
                        let result = task.await;
                        cx.update(|window, cx| {
                            weak.update(cx, |ws, cx| {
                                ws.nearby.pairing.remove(&peer_id);
                                ws.nearby.details.remove(&peer_id);
                                if let Some(store) = &ws.nearby.store {
                                    ws.nearby.trusted = store.trusted();
                                }
                                let msg = match result {
                                    Ok(true) => Notification::success(format!("Paired with {name}")),
                                    Ok(false) if server => Notification::warning(format!(
                                        "{name} didn't confirm the pairing. Run `sudo rilectl pair` on the server, then pair again."
                                    )),
                                    Ok(false) => Notification::warning(format!("{name} didn't confirm the pairing")),
                                    Err(e) => Notification::error(format!("Pairing failed: {e:#}")),
                                };
                                window.push_notification(msg, cx);
                                cx.notify();
                            })
                            .ok();
                        })
                        .ok();
                    })
                    .detach();
            }
        };
        let on_cancel = {
            let weak = weak.clone();
            move |_: &mut Window, cx: &mut App| {
                weak.update(cx, |ws, cx| {
                    ws.nearby.pairing.remove(&peer_id);
                    cx.notify();
                })
                .ok();
            }
        };
        code_dialog(
            format!("Pair with {name}?"),
            if server {
                format!(
                    "{name} is a Rile Server. Run `sudo rilectl pair` on it, check that it shows the same code and accept there, then choose Pair. You'll only need to do this once."
                )
            } else {
                format!("Check that {name} shows the same code, then choose Pair. You'll only need to do this once.")
            },
            code,
            "Pair",
            on_pair,
            on_cancel,
            guard,
            window,
            cx,
        );
    }

    /// Forgets a paired device.
    pub fn unpair(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.nearby.store.clone() else { return };
        let name = self.nearby.row(id).map(|r| r.name).unwrap_or_else(|| peer::short_id(id));
        let id = id.to_string();
        let weak = cx.entity().downgrade();
        crate::dialogs::confirm(format!("Unpair {name}?"), "Unpair", true, window, cx, move |window, cx| {
            if let Err(e) = store.forget(&id) {
                window.push_notification(Notification::error(format!("{e:#}")), cx);
            }
            let conn = peer::conn_id(&id);
            rile_remote::Pool::global().disconnect(&conn);
            weak.update(cx, |ws, cx| {
                ws.nearby.trusted = store.trusted();
                ws.nearby.details.remove(&id);
                for pane in ws.all_panes() {
                    if pane.read(cx).conn().as_deref() == Some(conn.as_str()) {
                        pane.update(cx, |p, cx| p.navigate(Location::Devices, cx));
                    }
                }
                cx.notify();
            })
            .ok();
        });
    }

    /// Switches the Nearby page between cards and list rows (persisted).
    pub fn set_nearby_list_view(&mut self, list: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.nearby.list_view != list {
            self.settings.nearby.list_view = list;
            self.save_settings(window, cx);
            cx.notify();
        }
    }

    /// Sends files or folders to a paired device; its user accepts first.
    pub fn send_to_device(&mut self, id: &str, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(row), Some(store)) = (self.nearby.row(id), self.nearby.store.clone()) else { return };
        if paths.is_empty() {
            return;
        }
        let Some((host, port)) = row.online.clone() else {
            window.push_notification(Notification::info(format!("{} isn't on the network right now", row.name)), cx);
            return;
        };
        if !row.paired {
            self.pair_with(id, window, cx);
            return;
        }
        let label = format!("Sending {} to {}", item_count(paths.len()), row.name);
        let id = id.to_string();
        self.start_op(label, window, cx, move |cancel, progress| {
            let items_total = paths.len();
            peer::send(&store, &host, port, Some(&id), &paths, cancel, &mut |done, total, current| {
                progress(Progress {
                    bytes_done: done,
                    bytes_total: total,
                    items_total,
                    current: Some(current.into()),
                    ..Default::default()
                });
            })?;
            Ok(Vec::new())
        });
    }

    /// Lets the user pick files to send to a device.
    pub fn pick_and_send(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let id = id.to_string();
        let picked = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Send".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            this.update_in(cx, |ws, window, cx| ws.send_to_device(&id, paths, window, cx)).ok();
        })
        .detach();
    }

    /// Shares a local folder with paired devices (read-only).
    pub fn share_folder(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if !path.is_dir() || self.settings.nearby.shares.iter().any(|s| s.path == path) {
            return;
        }
        let name = self.settings.nearby.share_name_for(&path);
        self.settings.nearby.shares.push(rile_core::settings::SharedFolder { name: name.clone(), path });
        window.push_notification(Notification::success(format!("“{name}” is now shared with your paired devices")), cx);
        self.settings_changed(window, cx);
    }

    pub fn unshare_folder(&mut self, path: &std::path::Path, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.nearby.shares.retain(|s| s.path != path);
        self.settings_changed(window, cx);
    }

    pub fn is_shared(&self, path: &std::path::Path) -> bool {
        self.settings.nearby.shares.iter().any(|s| s.path == path)
    }

    pub(crate) fn handle_peer_event(&mut self, ev: PeerEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            PeerEvent::PairRequest { device, code, reply } => {
                crate::background::show(window, cx);
                let name = name_of(&device);
                let reply = Arc::new(Mutex::new(Some(reply)));
                let accept = {
                    let reply = reply.clone();
                    move |_: &mut Window, _: &mut App| {
                        if let Some(r) = reply.lock().ok().and_then(|mut r| r.take()) {
                            r.send(true);
                        }
                    }
                };
                code_dialog(
                    format!("{name} wants to pair"),
                    format!("Pair only if {name} shows the same code. Paired devices can see the folders you share and send you files (you're always asked first)."),
                    code,
                    "Pair",
                    accept,
                    |_, _| {},
                    reply,
                    window,
                    cx,
                );
            }
            PeerEvent::Offer { device, items, total, reply, transfer } => {
                crate::background::show(window, cx);
                let name = name_of(&device);
                let tops: Vec<&str> = items.iter().filter(|i| !i.path.contains('/')).map(|i| i.path.as_str()).collect();
                let what = match tops.as_slice() {
                    [one] => format!("“{one}”"),
                    many => item_count(many.len()),
                };
                let inbox = self.settings.nearby.inbox_dir();
                let detail = format!(
                    "{name} wants to send you {what} ({}). Accepted files are saved in {}.",
                    humansize::format_size(total, humansize::DECIMAL),
                    inbox.display()
                );
                let reply = Arc::new(Mutex::new(Some(reply)));
                let weak = cx.entity().downgrade();
                let accept = {
                    let reply = reply.clone();
                    move |window: &mut Window, cx: &mut App| {
                        let Some(r) = reply.lock().ok().and_then(|mut r| r.take()) else { return };
                        r.send(Some(inbox.clone()));
                        let (transfer, name, inbox) = (transfer.clone(), name.clone(), inbox.clone());
                        weak.update(cx, |ws, cx| ws.watch_incoming(name, transfer, inbox, window, cx)).ok();
                    }
                };
                crate::dialogs::choose(
                    format!("Receive from {}?", name_of(&device)),
                    detail,
                    false,
                    vec![crate::dialogs::Choice::new("Accept", gpui_kit::component::button::ButtonVariant::Primary, accept)],
                    window,
                    cx,
                );
                // `reply` stays alive inside the dialog; closing it declines.
                drop(reply);
            }
        }
    }

    /// Shows an accepted transfer as a job until it finishes.
    fn watch_incoming(&mut self, from: String, transfer: Arc<peer::Transfer>, inbox: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let job = self.track_job(format!("Receiving from {from}"), transfer.cancel.clone(), cx);
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                let outcome = transfer.outcome();
                let (done, files, current) = transfer.progress();
                let stop = this
                    .update_in(cx, |ws, window, cx| {
                        if let Some(op) = ws.ops.iter_mut().find(|o| o.id == job) {
                            op.progress = Progress {
                                bytes_done: done,
                                bytes_total: transfer.total,
                                files_done: files,
                                files_total: transfer.files_total,
                                current: current.map(Into::into),
                                ..Default::default()
                            };
                            cx.notify();
                        }
                        let Some(outcome) = outcome else { return false };
                        match outcome {
                            Ok(paths) => {
                                ws.finish_tracked_job(job, None, cx);
                                window.push_notification(
                                    Notification::success(format!("Received {} from {from}", item_count(paths.len()))),
                                    cx,
                                );
                                // Show them if the inbox is open.
                                for pane in ws.all_panes() {
                                    if pane.read(cx).current_dir() == Some(inbox.as_path()) {
                                        let paths = paths.clone();
                                        pane.update(cx, |p, cx| {
                                            p.select_after_reload(paths);
                                            p.reload(cx);
                                        });
                                    }
                                }
                            }
                            Err(e) => {
                                ws.finish_tracked_job(job, Some(e.clone().into()), cx);
                                if !transfer.cancel.load(Ordering::Relaxed) {
                                    window.push_notification(Notification::error(format!("Receiving from {from} failed: {e}")), cx);
                                }
                            }
                        }
                        true
                    })
                    .unwrap_or(true);
                if stop {
                    break;
                }
            }
        })
        .detach();
    }
}

/// Declines a pairing that was never confirmed.
struct PairGuard(Option<Pairing>);

impl Drop for PairGuard {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            std::thread::spawn(move || {
                let _ = p.finish(false);
            });
        }
    }
}

/// A dialog showing a pairing code large, with Cancel and a confirm button.
/// `keep` lives as long as the dialog (dropped when it closes).
#[allow(clippy::too_many_arguments)]
fn code_dialog<K: 'static>(
    title: String,
    detail: String,
    code: u32,
    ok: &'static str,
    on_ok: impl Fn(&mut Window, &mut App) + 'static,
    on_cancel: impl Fn(&mut Window, &mut App) + 'static,
    keep: Arc<K>,
    window: &mut Window,
    cx: &mut App,
) {
    let on_ok = std::rc::Rc::new(on_ok);
    let on_cancel = std::rc::Rc::new(on_cancel);
    let code = SharedString::from(peer::format_code(code));
    window.open_dialog(cx, move |dialog, _, cx| {
        let _keep = &keep;
        let ch = crate::theme::chrome(cx);
        let (on_ok, on_cancel, on_close) = (on_ok.clone(), on_cancel.clone(), on_cancel.clone());
        kit::dialog(dialog, IconName::Link, title.clone(), None, 440., cx)
            .on_close(move |_, window, cx| on_close(window, cx))
            .child(
                v_flex()
                    .gap(px(16.))
                    .child(div().text_size(px(13.)).line_height(px(19.)).text_color(ch.text).child(detail.clone()))
                    .child(
                        h_flex().justify_center().py(px(10.)).rounded(px(10.)).bg(ch.hover).child(
                            div()
                                .debug_selector(|| "pair-code".into())
                                .text_size(px(34.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .font_family("Menlo")
                                .text_color(ch.text_strong)
                                .child(code.clone()),
                        ),
                    )
                    .child(
                        kit::footer()
                            .child(
                                kit::button("pair-cancel", "Cancel", kit::Tone::Ghost, false, cx)
                                    .debug_selector(|| "pair-cancel".into())
                                    .on_click(move |_, window, cx| {
                                        on_cancel(window, cx);
                                        window.close_dialog(cx);
                                    }),
                            )
                            .child(
                                kit::button("pair-ok", ok, kit::Tone::Primary, false, cx)
                                    .debug_selector(|| "pair-ok".into())
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        on_ok(window, cx);
                                    }),
                            ),
                    )
                    .into_any_element(),
            )
    });
}
