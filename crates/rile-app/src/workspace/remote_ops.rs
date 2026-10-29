//! Remote (FTP/FTPS/SFTP/SMB) operations and the "Connect to Server" dialog.
//! Remote panes reuse the normal listing/selection machinery; anything that
//! touches the server runs as a background operation with progress.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Disableable, Sizable, WindowExt, h_flex,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Input, InputEvent, InputState},
    notification::Notification,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render, SharedString, Styled,
    Subscription, Task, WeakEntity, Window, div, prelude::FluentBuilder, px, StatefulInteractiveElement,
};
use rile_core::history::Location;
use rile_core::ops::{self, Progress};
use rile_remote::{Auth, ConnectionInfo, Protocol};

use super::{Workspace, item_count};
use crate::remote;

impl Workspace {
    pub fn is_connecting(&self, id: &str) -> bool {
        !remote::is_connected(id)
            && (self.auto_connecting.contains(id) || self.ops.iter().any(|op| op.connecting_to.as_deref() == Some(id)))
    }

    /// Initial readiness includes metadata, not just an authenticated session.
    /// A cached `None` means capacity was checked but isn't available.
    pub fn is_source_loading(&self, id: &str) -> bool {
        if self.is_connecting(id) {
            return true;
        }
        if let Some(peer) = rile_remote::peer::peer_of_conn(id) {
            return self.nearby.is_loading(peer);
        }
        remote::is_connected(id) && !self.remote_capacity.contains_key(id)
    }

    /// Try each saved source once at startup, independently and without changing
    /// panes or producing notifications for offline/unauthorized sources.
    pub(crate) fn auto_connect_saved(&mut self, cx: &mut Context<Self>) {
        if !self.settings.remote_enabled {
            return;
        }
        for info in self.connections.clone() {
            if info.protocol == Protocol::Peer || remote::is_connected(&info.id) || self.is_connecting(&info.id) {
                continue;
            }
            self.auto_connecting.insert(info.id.clone());
            cx.spawn(async move |this, cx| {
                let id = info.id.clone();
                cx.background_spawn(async move {
                    match remote::auto_connect(&info) {
                        Ok(connected) => connected,
                        Err(e) => {
                            eprintln!("Auto-connect to {} failed: {e:#}", info.name);
                            false
                        }
                    }
                }).await;
                this.update(cx, |this, cx| {
                    this.auto_connecting.remove(&id);
                    cx.notify();
                }).ok();
            }).detach();
        }
        cx.notify();
    }

    /// Copies or moves between any combination of local disk and servers.
    #[allow(clippy::too_many_arguments)]
    pub fn transfer_between(
        &mut self,
        paths: Vec<PathBuf>,
        src_conn: Option<String>,
        dest: PathBuf,
        dest_conn: Option<String>,
        copy: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        let dual_transfer = self.dual && self.other_dir(cx).is_some_and(|(other, conn)| other == dest && conn == dest_conn)
            && (src_conn != dest_conn || paths.iter().any(|p| p.parent() != Some(dest.as_path())));
        match (src_conn, dest_conn) {
            (None, None) => self.transfer(paths, dest, copy, window, cx),
            (Some(a), Some(b)) if a == b && !copy => {
                // Move within one server: rename into the destination.
                let label = format!("Moving {} on {}", item_count(paths.len()), remote::label(&a));
                self.start_op(label, window, cx, move |_, _| {
                    let session = remote::session(&a)?;
                    let mut fs = session.lock().map_err(|_| anyhow::anyhow!("connection busy"))?;
                    let dest = remote::path_str(&dest);
                    for p in &paths {
                        let from = remote::path_str(p);
                        fs.rename(&from, &remote::join(&dest, rile_remote::paths::file_name(&from)))?;
                    }
                    drop(fs);
                    if dual_transfer { rile_services::rewards::record_event(rile_services::rewards::Event::DualTransfer); }
                    Ok(Vec::new())
                });
            }
            (Some(_), Some(_)) => window.push_notification(
                Notification::info("Copying between servers isn't supported — download first, then upload"),
                cx,
            ),
            (None, Some(conn)) => {
                let label = format!("Uploading {} to {}", item_count(paths.len()), remote::label(&conn));
                self.start_op(label, window, cx, move |cancel, report| {
                    let session = remote::session(&conn)?;
                    let dest = remote::path_str(&dest);
                    let n = paths.len();
                    for (i, p) in paths.iter().enumerate() {
                        let name = p.file_name().context("no file name")?.to_string_lossy().into_owned();
                        let target = remote::join(&dest, &name);
                        let mut fs = session.lock().map_err(|_| anyhow::anyhow!("connection busy"))?;
                        let current: std::sync::Arc<str> = name.as_str().into();
                        fs.upload(p, &target, cancel, &mut |done, total| {
                            report(item_progress(&current, done, total, i, n))
                        })?;
                        drop(fs);
                        if !copy {
                            ops::remove_permanently(p)?;
                        }
                    }
                    if dual_transfer && !cancel.load(std::sync::atomic::Ordering::Relaxed) {
                        rile_services::rewards::record_event(rile_services::rewards::Event::DualTransfer);
                    }
                    Ok(Vec::new())
                });
            }
            (Some(conn), None) => {
                let label = format!("Downloading {} from {}", item_count(paths.len()), remote::label(&conn));
                self.start_op(label, window, cx, move |cancel, report| {
                    let session = remote::session(&conn)?;
                    let n = paths.len();
                    let mut created = Vec::new();
                    for (i, p) in paths.iter().enumerate() {
                        let from = remote::path_str(p);
                        let local = ops::unique_destination(&dest, rile_remote::paths::file_name(&from));
                        let mut fs = session.lock().map_err(|_| anyhow::anyhow!("connection busy"))?;
                        let current: std::sync::Arc<str> = rile_remote::paths::file_name(&from).into();
                        fs.download(&from, &local, cancel, &mut |done, total| {
                            report(item_progress(&current, done, total, i, n))
                        })?;
                        if !copy {
                            fs.remove(&from)?;
                        }
                        created.push(local);
                    }
                    if dual_transfer && !cancel.load(std::sync::atomic::Ordering::Relaxed) {
                        rile_services::rewards::record_event(rile_services::rewards::Event::DualTransfer);
                    }
                    Ok(created)
                });
            }
        }
    }

    pub fn remote_delete(&mut self, conn: String, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let label = format!("Deleting {} on {}", item_count(paths.len()), remote::label(&conn));
        self.start_op(label, window, cx, move |_, report| {
            let session = remote::session(&conn)?;
            let mut fs = session.lock().map_err(|_| anyhow::anyhow!("connection busy"))?;
            let n = paths.len();
            for (i, p) in paths.iter().enumerate() {
                fs.remove(&remote::path_str(p))?;
                report(Progress { items_done: i + 1, items_total: n, ..Default::default() });
            }
            Ok(Vec::new())
        });
    }

    pub fn remote_rename(&mut self, conn: String, path: PathBuf, new_name: String, window: &mut Window, cx: &mut Context<Self>) {
        if new_name.is_empty() || new_name.contains('/') {
            window.push_notification(Notification::error(format!("Invalid name \"{new_name}\"")), cx);
            return;
        }
        let from = remote::path_str(&path);
        let to = remote::join(&remote::parent(&from).unwrap_or_else(|| "/".into()), &new_name);
        let label = format!("Renaming {}", rile_remote::paths::file_name(&from));
        self.start_op(label, window, cx, move |_, _| {
            let session = remote::session(&conn)?;
            session.lock().map_err(|_| anyhow::anyhow!("connection busy"))?.rename(&from, &to)?;
            Ok(vec![PathBuf::from(to)])
        });
    }

    /// New folder/file on the server, then the rename dialog for it.
    pub fn remote_create(&mut self, folder: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (conn, dir, taken) = {
            let pane = self.pane().read(cx);
            let taken: Vec<String> = pane.entries().iter().map(|e| e.name.to_string()).collect();
            (pane.conn(), pane.dir_path(), taken)
        };
        let (Some(conn), Some(dir)) = (conn, dir) else { return };
        let base = if folder { "untitled folder" } else { "untitled.txt" };
        let name = (1..)
            .map(|n| match (n, folder) {
                (1, _) => base.to_string(),
                (n, true) => format!("{base} {n}"),
                (n, false) => format!("untitled {n}.txt"),
            })
            .find(|n| !taken.contains(n))
            .unwrap();
        let path = remote::join(&remote::path_str(&dir), &name);
        let target = PathBuf::from(&path);
        let label = format!("Creating {name}");
        self.start_op_then(
            label,
            window,
            cx,
            move |_, _| {
                let session = remote::session(&conn)?;
                let mut fs = session.lock().map_err(|_| anyhow::anyhow!("connection busy"))?;
                if folder { fs.mkdir(&path)? } else { fs.create_file(&path)? }
                Ok(vec![PathBuf::from(path)])
            },
            move |this, window, cx| this.rename_path(target, window, cx),
        );
    }

    /// Downloads a remote file to a temp folder and opens it.
    pub fn open_remote(&mut self, conn: String, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let local = remote::temp_path(&conn, &path);
        let open_path = local.clone();
        let label = format!("Opening {}", rile_remote::paths::file_name(&path));
        self.start_op_then(
            label,
            window,
            cx,
            move |cancel, report| {
                if let Some(dir) = local.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let session = remote::session(&conn)?;
                session.lock().map_err(|_| anyhow::anyhow!("connection busy"))?.download(
                    &path,
                    &local,
                    cancel,
                    &mut |done, total| report(Progress { bytes_done: done, bytes_total: total, items_done: 0, items_total: 1, ..Default::default() }),
                )?;
                Ok(Vec::new())
            },
            move |_, _, cx| super::open_with::open_file(&open_path, cx),
        );
    }

    /// Connects (in the background) and shows the server in the active pane.
    pub fn connect(&mut self, info: ConnectionInfo, secret: Option<String>, remember: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.start_connect(info, secret, remember, true, window, cx);
    }

    /// Connects (in the background) to a saved connection without navigating to it.
    pub fn connect_only(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(info) = self.connections.iter().find(|c| c.id == id).cloned() else { return };
        self.start_connect(info, None, false, false, window, cx);
    }

    fn start_connect(
        &mut self,
        info: ConnectionInfo,
        secret: Option<String>,
        remember: bool,
        navigate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !remote::is_connected(&info.id) || remote::info(&info.id).as_ref() != Some(&info) || secret.is_some() {
            self.remote_capacity.remove(&info.id);
        }
        if let Some(s) = &secret
            && remember
            && let Err(e) = rile_remote::credentials::save(&info.id, s)
        {
            window.push_notification(Notification::warning(format!("Couldn't save password to the keychain: {e:#}")), cx);
        }
        if secret.is_some() && info.protocol.is_cloud() {
            // A new sign-in: don't keep using a session with the old tokens.
            rile_remote::Pool::global().disconnect(&info.id);
        }
        remote::register(info.clone(), secret);
        match self.connections.iter_mut().find(|c| c.id == info.id) {
            Some(c) => *c = info.clone(),
            None => self.connections.push(info.clone()),
        }
        if let Err(e) = remote::save_all(&self.connections) {
            window.push_notification(Notification::error(format!("Couldn't save connection: {e:#}")), cx);
        }
        let start = Arc::new(Mutex::new(String::new()));
        let start_out = start.clone();
        let id = info.id.clone();
        let conn_id = info.id.clone();
        let label = format!("Connecting to {}", remote::label(&id));
        // The user may switch panes or tabs while the connection is opening.
        let target_pane = self.pane().downgrade();
        let connecting_to = info.id.clone();
        self.start_op_then(
            label,
            window,
            cx,
            move |_, _| {
                let session = remote::session(&id)?;
                let home = session.lock().map_err(|_| anyhow::anyhow!("connection busy"))?.home_dir()?;
                *start_out.lock().unwrap() = home;
                Ok(Vec::new())
            },
            move |this, window, cx| {
                if !navigate {
                    window.push_notification(Notification::info(format!("Connected to {}", remote::label(&conn_id))), cx);
                    cx.notify();
                    return;
                }
                let path = start.lock().unwrap().clone();
                this.editing_path = false;
                _ = target_pane.update(cx, |pane, cx| {
                    pane.navigate(Location::Remote { conn: conn_id, path }, cx);
                    if pane.active { pane.focus(window, cx); }
                });
                cx.notify();
            },
        );
        if let Some(op) = self.ops.last_mut() {
            op.connecting_to = Some(connecting_to);
        }
    }

    pub fn open_connect_dialog(&mut self, editing: Option<ConnectionInfo>, window: &mut Window, cx: &mut Context<Self>) {
        let ws = cx.entity().downgrade();
        let title = if editing.is_some() { "Edit Connection" } else { "Connect to Server" };
        let view = cx.new(|cx| ConnectView::new(ws, editing, window, cx));
        let host = view.read(cx).host_input();
        let first = std::cell::Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first.replace(false) {
                host.update(cx, |i, cx| i.focus(window, cx));
            }
            crate::views::kit::dialog(
                dialog,
                gpui_kit::assets::IconName::Server,
                title,
                Some("Browse a server or cloud drive like a local folder".into()),
                540.,
                cx,
            )
            .pb_0()
            .child(view.clone())
        });
    }

    /// Opens a saved connection: reuses the live session or connects.
    pub fn open_connection(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(info) = self.connections.iter().find(|c| c.id == id).cloned() else { return };
        self.connect(info, None, false, window, cx);
    }

    pub fn disconnect(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        for pane in self.all_panes() {
            if pane.read(cx).conn().as_deref() == Some(id) {
                pane.update(cx, |p, cx| p.navigate(Location::Home, cx));
            }
        }
        rile_remote::Pool::global().disconnect(id);
        self.remote_capacity.remove(id);
        window.push_notification(Notification::info(format!("Disconnected from {}", remote::label(id))), cx);
        cx.notify();
    }

    pub fn remove_connection(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.disconnect(id, window, cx);
        remote::forget(id);
        self.connections.retain(|c| c.id != id);
        remote::save_all(&self.connections).ok();
        cx.notify();
    }
}

/// Fields of the connect dialog; kept between openings.
pub struct ConnectView {
    ws: WeakEntity<Workspace>,
    editing: Option<ConnectionInfo>,
    protocol: Protocol,
    auth: AuthKind,
    remember: bool,
    name: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    user: Entity<InputState>,
    secret: Entity<InputState>,
    key_path: Entity<InputState>,
    path: Entity<InputState>,
    client_id: Entity<InputState>,
    client_secret: Entity<InputState>,
    /// Set while a cloud sign-in waits for the browser; setting it cancels.
    signing_in: Option<Arc<std::sync::atomic::AtomicBool>>,
    sign_in_url: Option<String>,
    _sign_in_task: Option<Task<()>>,
    error: Option<SharedString>,
    servers: Vec<rile_remote::discovery::DiscoveredServer>,
    discovery_error: Option<SharedString>,
    scroll: gpui_kit::ScrollHandle,
    has_more_below: bool,
    _discovery_task: Task<()>,
    _subs: Vec<Subscription>,
}

#[derive(Clone, Copy, PartialEq)]
enum AuthKind {
    Password,
    Key,
    Agent,
    Anonymous,
}

impl ConnectView {
    pub fn new(ws: WeakEntity<Workspace>, editing: Option<ConnectionInfo>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let e = editing.clone();
        let field = |ph: &'static str, value: String, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(ph).default_value(value))
        };
        let name = field("Optional, shown in the sidebar", e.as_ref().map(|i| i.name.clone()).unwrap_or_default(), window, cx);
        let host = field("example.com", e.as_ref().map(|i| i.host.clone()).unwrap_or_default(), window, cx);
        let port = field("Default", e.as_ref().map(|i| i.port.to_string()).unwrap_or_default(), window, cx);
        let user = field("e.g. deploy", e.as_ref().map(|i| i.username.clone()).unwrap_or_default(), window, cx);
        let secret = cx.new(|cx| InputState::new(window, cx).placeholder("Required to connect").masked(true));
        let key_path = field(
            "~/.ssh/id_ed25519",
            e.as_ref()
                .and_then(|i| match &i.auth {
                    Auth::KeyFile { path } => Some(path.display().to_string()),
                    _ => None,
                })
                .unwrap_or_default(),
            window,
            cx,
        );
        let path = field("Optional · leave empty to start at the top", e.as_ref().and_then(|i| i.initial_path.clone()).unwrap_or_default(), window, cx);
        let client_id = field(
            "From your OAuth app",
            e.as_ref()
                .and_then(|i| match &i.auth {
                    Auth::OAuth { client_id } => Some(client_id.clone()),
                    _ => None,
                })
                .unwrap_or_default(),
            window,
            cx,
        );
        let client_secret = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(if e.as_ref().is_some_and(|i| i.protocol.is_cloud()) { "Saved · leave empty to keep" } else { "From your OAuth app" })
                .masked(true)
        });
        let subs = [&name, &host, &port, &user, &secret, &key_path, &path, &client_id, &client_secret]
            .into_iter()
            .map(|i| {
                cx.subscribe_in(i, window, |this: &mut Self, _, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        this.submit(window, cx);
                    }
                })
            })
            .collect();
        let auth = match e.as_ref().map(|i| &i.auth) {
            Some(Auth::KeyFile { .. }) => AuthKind::Key,
            Some(Auth::Agent) => AuthKind::Agent,
            Some(Auth::Anonymous) => AuthKind::Anonymous,
            _ => AuthKind::Password,
        };
        let discovery_task = cx.spawn(async move |this, cx| {
            let discovery = cx.background_spawn(async { rile_remote::discovery::Discovery::new() }).await;
            let mut discovery = match discovery {
                Ok(discovery) => discovery,
                Err(error) => {
                    _ = this.update(cx, |this, cx| {
                        this.discovery_error = Some(format!("Discovery unavailable: {error}. Enter a server manually.").into());
                        cx.notify();
                    });
                    return;
                }
            };
            loop {
                let servers = discovery.poll();
                if this.update(cx, |this, cx| {
                    if this.servers != servers {
                        this.servers = servers;
                        cx.notify();
                    }
                }).is_err() { break; }
                cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
            }
        });
        Self {
            ws,
            protocol: e.as_ref().map(|i| i.protocol).unwrap_or(Protocol::Sftp),
            editing,
            auth,
            remember: true,
            name,
            host,
            port,
            user,
            secret,
            key_path,
            path,
            client_id,
            client_secret,
            signing_in: None,
            sign_in_url: None,
            _sign_in_task: None,
            error: None,
            servers: Vec::new(),
            discovery_error: None,
            scroll: gpui_kit::ScrollHandle::new(),
            has_more_below: false,
            _discovery_task: discovery_task,
            _subs: subs,
        }
    }

    pub fn host_input(&self) -> Entity<InputState> {
        self.host.clone()
    }

    fn choose_server(&mut self, server: &rile_remote::discovery::DiscoveredServer, window: &mut Window, cx: &mut Context<Self>) {
        self.protocol = server.protocol;
        if self.protocol != Protocol::Sftp && matches!(self.auth, AuthKind::Key | AuthKind::Agent) {
            self.auth = AuthKind::Password;
        }
        self.host.update(cx, |i, cx| i.set_value(server.host.clone(), window, cx));
        self.port.update(cx, |i, cx| i.set_value(server.port.to_string(), window, cx));
        self.name.update(cx, |i, cx| i.set_value(server.name.clone(), window, cx));
        self.path.update(cx, |i, cx| i.set_value("", window, cx));
        self.secret.update(cx, |i, cx| i.set_value("", window, cx));
        self.error = None;
        self.user.update(cx, |i, cx| i.focus(window, cx));
        cx.notify();
    }

    /// Editing a cloud connection without changing its account settings: no new sign-in needed.
    fn keeps_cloud_sign_in(&self, cx: &App) -> bool {
        let client_id = self.client_id.read(cx).value().trim().to_string();
        self.editing.as_ref().is_some_and(|e| {
            e.protocol == self.protocol && matches!(&e.auth, Auth::OAuth { client_id: c } if *c == client_id)
        }) && self.client_secret.read(cx).value().trim().is_empty()
    }

    fn cloud_fields(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        use crate::views::kit;
        use gpui_kit::FontWeight;
        let ch = crate::theme::chrome(cx);
        let google = self.protocol == Protocol::GoogleDrive;
        let mut out = Vec::new();
        if let Some(e) = self.editing.as_ref().filter(|e| e.protocol == self.protocol && !e.username.is_empty()) {
            out.push(
                kit::card(cx)
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .px(px(12.))
                            .py(px(10.))
                            .child(crate::icons::protocol_glyph(self.protocol, 20., ch.text))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(2.))
                                    .child(div().text_size(px(11.)).text_color(ch.text_faint).child("Signed in as"))
                                    .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).text_color(ch.text_strong).truncate().child(e.username.clone())),
                            )
                            .child(crate::icons::icon(IconName::CircleCheck).size(px(16.)).text_color(ch.accent)),
                    )
                    .into_any_element(),
            );
        }
        out.push(kit::field("Client ID", input_with_icon(&self.client_id, IconName::IdCard, cx), cx).into_any_element());
        if google {
            out.push(kit::field("Client secret", input_with_icon(&self.client_secret, IconName::Lock, cx), cx).into_any_element());
        }
        if self.signing_in.is_some() {
            let url = self.sign_in_url.clone();
            out.push(
                kit::card(cx)
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .px(px(12.))
                            .py(px(8.))
                            .child(crate::icons::icon(IconName::Globe).size(px(16.)).text_color(ch.accent))
                            .child(div().flex_1().min_w_0().text_size(px(12.)).text_color(ch.text).child("Finish signing in in your browser…"))
                            .children(url.map(|url| {
                                Button::new("connect-open-sign-in").cursor_pointer().ghost().small()
                                    .icon(crate::icons::icon(IconName::ExternalLink))
                                    .label("Open Again")
                                    .on_click(move |_, _, cx| cx.open_url(&url))
                            })),
                    )
                    .into_any_element(),
            );
        }
        let (steps, console, console_url): (&[&str], _, _) = if google {
            (
                &[
                    "In Google Cloud Console, enable the Google Drive API.",
                    "Create an OAuth client ID of type \"Desktop app\".",
                    "Paste its client ID and client secret above.",
                ],
                "Open Google Cloud Console",
                "https://console.cloud.google.com/apis/credentials",
            )
        } else {
            (
                &[
                    "In Microsoft Entra, register an app and add the \"Mobile and desktop applications\" platform with redirect URI http://localhost.",
                    "Allow public client flows.",
                    "Add the delegated permissions Files.ReadWrite.All and User.Read, then paste the client ID above.",
                ],
                "Open Microsoft Entra",
                "https://entra.microsoft.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade",
            )
        };
        out.push(
            kit::card(cx)
                .px(px(12.))
                .py(px(10.))
                .gap(px(8.))
                .child(
                    h_flex()
                        .gap(px(8.))
                        .child(crate::icons::icon(IconName::Lightbulb).size(px(14.)).text_color(ch.accent))
                        .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(ch.text_strong).child("Uses your own OAuth app")),
                )
                .children(steps.iter().enumerate().map(|(i, step)| {
                    h_flex()
                        .items_start()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_none()
                                .size(px(16.))
                                .rounded_full()
                                .bg(ch.hover_strong)
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(10.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ch.text_muted)
                                .child(format!("{}", i + 1)),
                        )
                        .child(div().flex_1().min_w_0().text_size(px(11.)).line_height(px(16.)).text_color(ch.text_muted).child(*step))
                }))
                .child(
                    h_flex()
                        .justify_between()
                        .gap(px(8.))
                        .child(kit::hint("Sign-in happens in your browser; tokens stay in your Keychain.", cx).flex_1().min_w_0())
                        .child(
                            Button::new("connect-console").cursor_pointer().ghost().small()
                                .icon(crate::icons::icon(IconName::ExternalLink))
                                .label(console)
                                .on_click(move |_, _, cx| cx.open_url(console_url)),
                        ),
                )
                .into_any_element(),
        );
        out
    }

    fn cancel_sign_in(&mut self, cx: &mut Context<Self>) {
        if let Some(flag) = self.signing_in.take() {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.sign_in_url = None;
        self._sign_in_task = None;
        cx.notify();
    }

    fn submit_cloud(&mut self, reauth: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.signing_in.is_some() {
            return;
        }
        let text = |i: &Entity<InputState>| i.read(cx).value().trim().to_string();
        let protocol = self.protocol;
        let client_id = text(&self.client_id);
        let initial_path = Some(text(&self.path)).filter(|p| !p.is_empty());
        let name = text(&self.name);
        if !reauth && self.keeps_cloud_sign_in(cx) {
            let mut info = self.editing.clone().expect("editing");
            info.name = name;
            info.initial_path = initial_path;
            window.close_dialog(cx);
            self.ws.update(cx, |ws, cx| ws.connect(info, None, false, window, cx)).ok();
            return;
        }
        if client_id.is_empty() {
            self.error = Some("Enter the client ID of your OAuth app".into());
            cx.notify();
            return;
        }
        let mut client_secret = Some(text(&self.client_secret)).filter(|s| !s.is_empty());
        if client_secret.is_none()
            && let Some(e) = &self.editing
            && e.protocol == protocol
        {
            // Signing in again: reuse the saved client secret.
            client_secret = remote::saved_secret(&e.id).and_then(|s| rile_remote::oauth::saved_client_secret(&s));
        }
        let pending = match rile_remote::oauth::begin(protocol, &client_id, client_secret.as_deref()) {
            Ok(p) => p,
            Err(e) => {
                self.error = Some(e.to_string().into());
                cx.notify();
                return;
            }
        };
        cx.open_url(&pending.url);
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.signing_in = Some(cancel.clone());
        self.sign_in_url = Some(pending.url.clone());
        self.error = None;
        let id = self.editing.as_ref().filter(|e| e.protocol == protocol).map(|e| e.id.clone()).unwrap_or_else(ConnectionInfo::new_id);
        self._sign_in_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { pending.finish(&cancel) }).await;
            _ = this.update_in(cx, |this, window, cx| {
                this.signing_in = None;
                this.sign_in_url = None;
                match result {
                    Ok(signed_in) => {
                        let info = ConnectionInfo {
                            id,
                            name,
                            protocol,
                            host: protocol.cloud_host().unwrap_or_default().to_string(),
                            port: protocol.default_port(),
                            username: signed_in.account,
                            auth: Auth::OAuth { client_id },
                            initial_path,
                        };
                        window.close_dialog(cx);
                        this.ws.update(cx, |ws, cx| ws.connect(info, Some(signed_in.secret), true, window, cx)).ok();
                    }
                    Err(e) if rile_remote::is_cancelled(&e) => {}
                    Err(e) => this.error = Some(format!("{e:#}").into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.protocol.is_cloud() {
            return self.submit_cloud(false, window, cx);
        }
        let text = |i: &Entity<InputState>| i.read(cx).value().trim().to_string();
        let host = text(&self.host);
        if host.is_empty() {
            self.error = Some("Enter a host".into());
            cx.notify();
            return;
        }
        let port = match text(&self.port) {
            p if p.is_empty() => self.protocol.default_port(),
            p => match p.parse() {
                Ok(p) if p != 0 => p,
                _ => {
                    self.error = Some("Port must be between 1 and 65535".into());
                    cx.notify();
                    return;
                }
            },
        };
        let auth = match self.auth {
            AuthKind::Password => Auth::Password,
            AuthKind::Agent => Auth::Agent,
            AuthKind::Anonymous => Auth::Anonymous,
            AuthKind::Key => {
                let k = text(&self.key_path);
                if k.is_empty() {
                    self.error = Some("Choose a private key file".into());
                    cx.notify();
                    return;
                }
                Auth::KeyFile { path: super::expand_tilde(&k) }
            }
        };
        let mut info = ConnectionInfo {
            id: self.editing.as_ref().map(|i| i.id.clone()).unwrap_or_else(ConnectionInfo::new_id),
            name: text(&self.name),
            protocol: self.protocol,
            host,
            port,
            username: text(&self.user),
            auth,
            initial_path: Some(text(&self.path)).filter(|p| !p.is_empty()),
        };
        if let Err(error) = info.normalize_smb_address() {
            self.error = Some(error.to_string().into());
            cx.notify();
            return;
        }
        // Passwords and passphrases may intentionally contain surrounding whitespace.
        let secret = Some(self.secret.read(cx).value().to_string()).filter(|s| !s.is_empty());
        let remember = self.remember;
        window.close_dialog(cx);
        self.ws.update(cx, |ws, cx| ws.connect(info, secret, remember, window, cx)).ok();
    }
}

/// The protocols in the order the dialog offers them.
const PROTOCOLS: [Protocol; 6] = [Protocol::Sftp, Protocol::Ftp, Protocol::Ftps, Protocol::Smb, Protocol::GoogleDrive, Protocol::OneDrive];

/// A selectable protocol card: glyph or logo, name and a short description.
fn protocol_tile(p: Protocol, active: bool, cx: &App) -> gpui_kit::Stateful<gpui_kit::Div> {
    use gpui_kit::FontWeight;
    let ch = crate::theme::chrome(cx);
    let detail = match p {
        Protocol::Sftp => "Over SSH",
        Protocol::Ftp => "Classic FTP",
        Protocol::Ftps => "FTP over TLS",
        Protocol::Smb => "Windows & NAS",
        Protocol::GoogleDrive => "Google account",
        Protocol::OneDrive => "Microsoft account",
        Protocol::Peer => "Nearby Rile device",
    };
    h_flex()
        .id(p.label())
        .debug_selector(move || format!("connect-protocol-{}", p.label()))
        .gap(px(9.))
        .px(px(10.))
        .py(px(8.))
        .rounded(px(9.))
        .border_1()
        .cursor_pointer()
        .when(active, |d| d.border_color(ch.accent).bg(ch.accent.opacity(0.12)))
        .when(!active, |d| d.border_color(ch.hairline).hover(|s| s.bg(ch.hover)))
        .child(
            div()
                .flex_none()
                .size(px(28.))
                .rounded(px(7.))
                .flex()
                .items_center()
                .justify_center()
                .bg(if p.is_cloud() { gpui_kit::white() } else { ch.hover_strong })
                .child(crate::icons::protocol_glyph(p, if p.is_cloud() { 18. } else { 15. }, if active { ch.accent } else { ch.text_muted })),
        )
        .child(
            v_flex()
                .min_w_0()
                .gap(px(1.))
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if active { ch.text_strong } else { ch.text })
                        .truncate()
                        .child(p.label()),
                )
                .child(div().text_size(px(10.)).text_color(ch.text_faint).truncate().child(detail)),
        )
}

/// A text field with a leading icon.
fn input_with_icon(state: &Entity<InputState>, icon: IconName, cx: &App) -> Input {
    let ch = crate::theme::chrome(cx);
    Input::new(state).prefix(crate::icons::icon(icon).size(px(14.)).text_color(ch.text_faint))
}

impl Drop for ConnectView {
    fn drop(&mut self) {
        // Closing the dialog stops a sign-in that's still waiting for the browser.
        if let Some(flag) = &self.signing_in {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

impl Render for ConnectView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::views::kit;
        let ch = crate::theme::chrome(cx);
        let proto = |p: Protocol, this: &Self, cx: &mut Context<Self>| {
            protocol_tile(p, this.protocol == p, cx).on_click(cx.listener(move |this, _, window, cx| {
                if this.protocol != p {
                    this.port.update(cx, |i, cx| i.set_value("", window, cx));
                    this.cancel_sign_in(cx);
                }
                this.protocol = p;
                this.error = None;
                if p != Protocol::Sftp && matches!(this.auth, AuthKind::Key | AuthKind::Agent) {
                    this.auth = AuthKind::Password;
                }
                cx.notify();
            }))
        };
        let auth = |id: &'static str, label: &'static str, kind: AuthKind, this: &Self, cx: &mut Context<Self>| {
            let icon = match kind {
                AuthKind::Password => IconName::Lock,
                AuthKind::Key => IconName::FileKey,
                AuthKind::Agent => IconName::KeyRound,
                AuthKind::Anonymous => IconName::User,
            };
            kit::segment_with_icon(id, icon, label, this.auth == kind, cx).on_click(cx.listener(move |this, _, _, cx| {
                this.auth = kind;
                cx.notify();
            }))
        };
        let sftp = self.protocol == Protocol::Sftp;
        let cloud = self.protocol.is_cloud();
        let secret_label = if self.auth == AuthKind::Key { "Passphrase" } else { "Password" };
        let body = v_flex()
            .id("connect-form")
            .debug_selector(|| "connect-body".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .pb(px(14.))
            .gap(px(14.))
            .text_size(px(13.))
            .child(
                div()
                    .grid()
                    .grid_cols(3)
                    .gap(px(8.))
                    .children(PROTOCOLS.iter().map(|p| proto(*p, self, cx))),
            )
            .when(!cloud, |d| d
                .child(kit::field("Nearby servers", v_flex().id("discovered-servers").gap(px(2.)).max_h(px(100.)).overflow_y_scroll().children(
                self.servers.iter().enumerate().map(|(index, server)| {
                    let server = server.clone();
                    Button::new(("discovered-server", index)).cursor_pointer().ghost().cursor_pointer()
                        .label(format!("{} · {} · {}:{}", server.name, server.protocol.label(), server.host, server.port))
                        .on_click(cx.listener(move |this, _, window, cx| this.choose_server(&server, window, cx)))
                })
            ), cx))
                .when(self.servers.is_empty() && self.discovery_error.is_none(), |d| d.child(kit::hint("Looking for advertised servers on your local network… You can also enter one manually.", cx)))
                .when_some(self.discovery_error.clone(), |d, error| d.child(kit::notice(error, false, cx)))
            )
            .child(kit::field("Name", input_with_icon(&self.name, IconName::Tag, cx), cx))
            .when(cloud, |d| d.children(self.cloud_fields(cx)))
            .when(!cloud, |d| d
                .child(
                h_flex()
                    .gap(px(10.))
                    .items_start()
                    .child(kit::field("Server", input_with_icon(&self.host, IconName::Globe, cx), cx).flex_1())
                    .child(kit::field("Port", Input::new(&self.port), cx).w(px(92.))),
            )
                .child(kit::field("Username", input_with_icon(&self.user, IconName::User, cx), cx))
                .child(
                kit::field(
                    "Sign in with",
                    h_flex().child(
                        kit::segmented(cx)
                            .child(auth("a-pw", "Password", AuthKind::Password, self, cx))
                            .when(sftp, |d| {
                                d.child(auth("a-key", "Key file", AuthKind::Key, self, cx))
                                    .child(auth("a-agent", "SSH agent", AuthKind::Agent, self, cx))
                            })
                            .when(!sftp, |d| d.child(auth("a-anon", if self.protocol == Protocol::Smb { "Guest" } else { "Anonymous" }, AuthKind::Anonymous, self, cx))),
                    ),
                    cx,
                ),
            )
                .when(self.auth == AuthKind::Key, |d| d.child(kit::field("Key file", input_with_icon(&self.key_path, IconName::FileKey, cx), cx)))
                .when(matches!(self.auth, AuthKind::Password | AuthKind::Key), |d| {
                d.child(kit::field(secret_label, input_with_icon(&self.secret, if self.auth == AuthKind::Key { IconName::KeyRound } else { IconName::Lock }, cx), cx)).child(
                    Checkbox::new("remember").cursor_pointer()
                        .label(if self.auth == AuthKind::Key { "Save passphrase in Keychain" } else { "Save password in Keychain" })
                        .checked(self.remember)
                        .on_click(cx.listener(|this, v: &bool, _, cx| {
                            this.remember = *v;
                            cx.notify();
                        })),
                )
            })
            )
            .child(kit::field("Start folder", input_with_icon(&self.path, IconName::FolderOpen, cx), cx))
            .when(self.protocol == Protocol::Smb, |d| d.child(kit::hint("Leave empty to choose a share after connecting, or enter /share/folder. Use DOMAIN\\user for domain accounts.", cx)))
            .when(self.auth == AuthKind::Agent && !cloud, |d| d.child(kit::hint("Uses the keys loaded in your SSH agent.", cx)))
            .when_some(self.error.clone(), |d, e| d.child(kit::notice(e, true, cx)));

        let keep_sign_in = cloud && self.keeps_cloud_sign_in(cx);
        let go_label = match self.protocol {
            _ if self.signing_in.is_some() => "Waiting for Browser…",
            _ if keep_sign_in => "Save",
            Protocol::GoogleDrive => "Sign In with Google",
            Protocol::OneDrive => "Sign In with Microsoft",
            _ => "Connect",
        };
        let view = cx.entity().downgrade();
        let scroll = self.scroll.clone();
        let has_more_below = self.has_more_below;
        let footer =
                kit::footer()
                    .debug_selector(|| "connect-footer".into())
                    .flex_none()
                    .relative()
                    .mx(px(-16.))
                    .p_4()
                    .rounded_b(px(14.))
                    .bg(ch.surface_colors().1)
                    .when(has_more_below, |d| d.shadow(vec![gpui_kit::BoxShadow {
                        color: gpui_kit::black().opacity(if ch.dark { 0.3 } else { 0.12 }),
                        offset: gpui_kit::point(px(0.), px(-4.)),
                        blur_radius: px(12.),
                        spread_radius: px(-4.),
                        inset: false,
                    }]))
                    // Measure after layout so scrolling, resizing and changing
                    // authentication fields all update the shadow correctly.
                    .child(div()
                        .debug_selector(move || if has_more_below { "connect-footer-shadow" } else { "connect-footer-no-shadow" }.into())
                        .absolute()
                        .inset_0()
                        .child(gpui_kit::canvas(move |_, _, cx| {
                            let more = scroll.max_offset().y + scroll.offset().y > px(1.);
                            if more != has_more_below {
                                cx.defer(move |cx| {
                                    _ = view.update(cx, |this, cx| {
                                        this.has_more_below = more;
                                        cx.notify();
                                    });
                                });
                            }
                        }, |_, _, _, _| {}).size_full()))
                    .child(Button::new("connect-cancel").cursor_pointer().ghost()
                        .icon(crate::icons::icon(gpui_kit::assets::IconName::X))
                        .label("Cancel").on_click(cx.listener(|this, _, window, cx| {
                            if this.signing_in.is_some() {
                                this.cancel_sign_in(cx);
                            } else {
                                window.close_dialog(cx);
                            }
                        })))
                    .when(cloud && keep_sign_in && self.signing_in.is_none(), |d| d.child(
                        Button::new("connect-reauth").cursor_pointer().ghost()
                            .label("Sign In Again")
                            .on_click(cx.listener(|this, _, window, cx| this.submit_cloud(true, window, cx))),
                    ))
                    .child(
                        Button::new("connect-go").cursor_pointer()
                            .primary()
                            .when(!cloud || keep_sign_in, |b| b.icon(crate::icons::icon(if cloud { IconName::Check } else { IconName::Server })))
                            .when(cloud && !keep_sign_in, |b| b.child(
                                // Provider logos need a light backdrop to read on the accent fill.
                                div().size(px(18.)).flex().items_center().justify_center().rounded(px(5.)).bg(gpui_kit::white())
                                    .child(crate::icons::protocol_glyph(self.protocol, 12., ch.text)),
                            ))
                            .child(go_label)
                            .disabled(self.signing_in.is_some())
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    );
        v_flex()
            .max_h((_window.viewport_size().height - px(180.)).max(px(160.)))
            .child(body)
            .child(footer)
    }
}

/// Progress of item `i` of `n` in a server transfer: byte totals are only
/// known per item, so they describe the current one.
fn item_progress(current: &std::sync::Arc<str>, done: u64, total: u64, i: usize, n: usize) -> Progress {
    Progress {
        bytes_done: done,
        bytes_total: total,
        items_done: i,
        items_total: n,
        current: Some(current.clone()),
        file_bytes_done: done,
        file_bytes_total: total,
        ..Default::default()
    }
}
