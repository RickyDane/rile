//! Remote (FTP/FTPS/SFTP/SMB) operations and the "Connect to Server" dialog.
//! Remote panes reuse the normal listing/selection machinery; anything that
//! touches the server runs as a background operation with progress.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use gpui_kit::component::{
    WindowExt, h_flex,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Input, InputEvent, InputState},
    notification::Notification,
    v_flex,
};
use gpui_kit::{
    AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render, SharedString, Styled,
    Subscription, Task, WeakEntity, Window, div, prelude::FluentBuilder, px, StatefulInteractiveElement,
};
use rile_core::history::Location;
use rile_core::ops::{self, Progress};
use rile_remote::{Auth, ConnectionInfo, Protocol};

use super::{Workspace, item_count};
use crate::remote;

impl Workspace {
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
        if let Some(s) = &secret
            && remember
            && let Err(e) = rile_remote::credentials::save(&info.id, s)
        {
            window.push_notification(Notification::warning(format!("Couldn't save password to the keychain: {e:#}")), cx);
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
                let path = start.lock().unwrap().clone();
                this.editing_path = false;
                _ = target_pane.update(cx, |pane, cx| {
                    pane.navigate(Location::Remote { conn: conn_id, path }, cx);
                    if pane.active { pane.focus(window, cx); }
                });
                cx.notify();
            },
        );
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
                Some("SFTP, FTP, FTPS or SMB · passwords stay in your Keychain".into()),
                500.,
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
        let path = field("Optional, e.g. /var/www or /share/folder", e.as_ref().and_then(|i| i.initial_path.clone()).unwrap_or_default(), window, cx);
        let subs = [&name, &host, &port, &user, &secret, &key_path, &path]
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

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

impl Render for ConnectView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::views::kit;
        let proto = |p: Protocol, this: &Self, cx: &mut Context<Self>| {
            kit::segment(p.label(), p.label(), this.protocol == p, cx).on_click(cx.listener(move |this, _, window, cx| {
                if this.protocol != p {
                    this.port.update(cx, |i, cx| i.set_value("", window, cx));
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
            kit::segment(id, label, this.auth == kind, cx).on_click(cx.listener(move |this, _, _, cx| {
                this.auth = kind;
                cx.notify();
            }))
        };
        let sftp = self.protocol == Protocol::Sftp;
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
                h_flex()
                    .justify_between()
                    .child(
                        kit::segmented(cx)
                            .child(proto(Protocol::Sftp, self, cx))
                            .child(proto(Protocol::Ftp, self, cx))
                            .child(proto(Protocol::Ftps, self, cx))
                            .child(proto(Protocol::Smb, self, cx)),
                    ),
            )
            .child(kit::field("Nearby servers", v_flex().id("discovered-servers").gap(px(2.)).max_h(px(100.)).overflow_y_scroll().children(
                self.servers.iter().enumerate().map(|(index, server)| {
                    let server = server.clone();
                    Button::new(("discovered-server", index)).ghost().cursor_pointer()
                        .label(format!("{} · {} · {}:{}", server.name, server.protocol.label(), server.host, server.port))
                        .on_click(cx.listener(move |this, _, window, cx| this.choose_server(&server, window, cx)))
                })
            ), cx))
            .when(self.servers.is_empty() && self.discovery_error.is_none(), |d| d.child(kit::hint("Looking for advertised servers on your local network… You can also enter one manually.", cx)))
            .when_some(self.discovery_error.clone(), |d, error| d.child(kit::notice(error, false, cx)))
            .child(kit::field("Name", Input::new(&self.name), cx))
            .child(
                h_flex()
                    .gap(px(10.))
                    .items_start()
                    .child(kit::field("Server", Input::new(&self.host), cx).flex_1())
                    .child(kit::field("Port", Input::new(&self.port), cx).w(px(92.))),
            )
            .child(kit::field("Username", Input::new(&self.user), cx))
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
            .when(self.auth == AuthKind::Key, |d| d.child(kit::field("Key file", Input::new(&self.key_path), cx)))
            .when(matches!(self.auth, AuthKind::Password | AuthKind::Key), |d| {
                d.child(kit::field(secret_label, Input::new(&self.secret), cx)).child(
                    Checkbox::new("remember").cursor_pointer()
                        .label(if self.auth == AuthKind::Key { "Save passphrase in Keychain" } else { "Save password in Keychain" })
                        .checked(self.remember)
                        .on_click(cx.listener(|this, v: &bool, _, cx| {
                            this.remember = *v;
                            cx.notify();
                        })),
                )
            })
            .child(kit::field("Start folder", Input::new(&self.path), cx))
            .when(self.protocol == Protocol::Smb, |d| d.child(kit::hint("Leave empty to choose a share after connecting, or enter /share/folder. Use DOMAIN\\user for domain accounts.", cx)))
            .when(self.auth == AuthKind::Agent, |d| d.child(kit::hint("Uses the keys loaded in your SSH agent.", cx)))
            .when_some(self.error.clone(), |d, e| d.child(kit::notice(e, true, cx)));

        let ch = crate::theme::chrome(cx);
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
                        .label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(
                        Button::new("connect-go").cursor_pointer()
                            .primary()
                            .icon(crate::icons::icon(gpui_kit::assets::IconName::Server))
                            .label("Connect")
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
