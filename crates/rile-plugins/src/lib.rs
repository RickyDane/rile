//! Sandboxed WebAssembly component plugin host for Rile.
//!
//! Plugins are WebAssembly components (e.g. built for `wasm32-wasip2`) that
//! implement the `rile:plugin/plugin` world defined in `wit/rile-plugin.wit`.
//!
//! Sandboxing:
//! * WASI is provided with no preopened directories, no environment, no
//!   arguments, closed stdin, discarded stdout/stderr and no network access.
//! * The only file access is through the `host` interface: `read-text` is
//!   limited to the paths in the current invocation's selection, `list-dir` to
//!   the current directory, selected entries, their parents and direct children
//!   of those.
//! * Every call into a plugin is limited by a fuel budget and a memory cap, so a
//!   runaway plugin traps instead of hanging the UI. A trapped instance is
//!   discarded and re-instantiated on the next call.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

mod bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "plugin",
    });
}

use bindings::Plugin;
use bindings::rile::plugin::types as wit;

/// Fuel granted to each call into a plugin (roughly: wasm instructions).
const FUEL_PER_CALL: u64 = 5_000_000_000;
/// Maximum linear memory a plugin instance may use.
const MAX_MEMORY_BYTES: usize = 256 * 1024 * 1024;
/// Hard cap on `read-text`'s `max-bytes`.
const MAX_READ_BYTES: u32 = 16 * 1024 * 1024;
/// Maximum number of entries `list-dir` returns.
const MAX_LIST_ENTRIES: usize = 10_000;
/// Maximum queued notifications per call.
const MAX_NOTIFICATIONS: usize = 64;
/// Maximum length of a log/notification message (bytes).
const MAX_MESSAGE_LEN: usize = 4096;
/// Maximum number of nodes in a page view.
const MAX_PAGE_NODES: usize = 20_000;
/// Maximum number of bubbles in one chart.
const MAX_BUBBLES: usize = 500;
/// Maximum size of a page's opaque state (bytes).
const MAX_PAGE_STATE: usize = 4 * 1024 * 1024;
/// Maximum entries `dir-size` walks before giving up.
const MAX_DIR_SIZE_ENTRIES: usize = 2_000_000;

/// The explorer state a plugin call is made in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Context {
    pub current_dir: Option<PathBuf>,
    pub selection: Vec<PathBuf>,
}

/// A command contributed by a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub id: String,
    pub label: String,
}

/// Something the host application should do after a plugin command ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Open(PathBuf),
    Navigate(PathBuf),
    Refresh,
    CopyText(String),
    Notify(String),
    /// Pages only: ask the user for a new page root.
    ChooseRoot,
    /// Pages only: set the page root.
    SetRoot(PathBuf),
}

/// A custom page contributed by a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub id: String,
    pub title: String,
    pub icon: Option<String>,
}

/// Input of a page call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageRequest {
    pub page_id: String,
    pub root: Option<PathBuf>,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bubble {
    pub label: String,
    pub detail: String,
    pub value: f64,
    pub color: Option<u32>,
    pub event: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    pub label: String,
    pub event: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    pub title: String,
    pub detail: String,
    pub trailing: String,
    pub event: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    Column,
    Row,
    Heading(String),
    Text(String),
    Muted(String),
    Button(Button),
    ListItem(ListItem),
    Progress(f32),
    Bubbles(Vec<Bubble>),
    Spacer,
}

impl NodeKind {
    pub fn is_container(&self) -> bool {
        matches!(self, NodeKind::Column | NodeKind::Row)
    }
}

/// One page element. `parent` is validated by the host: it is always a
/// container with a smaller index, or `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub parent: Option<usize>,
    pub kind: NodeKind,
}

/// A declarative page UI (flat tree).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageView {
    pub nodes: Vec<Node>,
}

impl PageView {
    /// Indices of the direct children of `parent` (`None` = top level), in order.
    pub fn children(&self, parent: Option<usize>) -> impl Iterator<Item = usize> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .filter(move |(_, n)| n.parent == parent)
            .map(|(i, _)| i)
    }
}

/// Result of a page call.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageUpdate {
    pub view: PageView,
    pub state: String,
    pub actions: Vec<Action>,
}

/// Metadata about a loaded plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub path: PathBuf,
    pub enabled: bool,
}

/// Per-store host state.
struct HostState {
    plugin_id: String,
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
    /// Canonicalized selection of the current invocation.
    selection: Vec<PathBuf>,
    /// Canonicalized current directory of the current invocation.
    current_dir: Option<PathBuf>,
    /// Canonicalized page root during page calls; everything below is listable.
    page_root: Option<PathBuf>,
    notifications: Vec<String>,
}

impl HostState {
    fn new(plugin_id: String) -> Self {
        let wasi = WasiCtx::builder()
            .allow_tcp(false)
            .allow_udp(false)
            .allow_ip_name_lookup(false)
            .build();
        Self {
            plugin_id,
            wasi,
            table: ResourceTable::new(),
            limits: StoreLimitsBuilder::new()
                .memory_size(MAX_MEMORY_BYTES)
                .instances(64)
                .tables(64)
                .memories(16)
                .build(),
            selection: Vec::new(),
            current_dir: None,
            page_root: None,
            notifications: Vec::new(),
        }
    }

    fn set_context(&mut self, ctx: &Context, page_root: Option<&Path>) {
        self.page_root = page_root.and_then(|p| dunce::canonicalize(p).ok());
        self.selection = ctx
            .selection
            .iter()
            .filter_map(|p| dunce::canonicalize(p).ok())
            .collect();
        self.current_dir = ctx
            .current_dir
            .as_ref()
            .and_then(|p| dunce::canonicalize(p).ok());
        self.notifications.clear();
    }

    fn read_allowed(&self, path: &Path) -> bool {
        self.selection.iter().any(|p| p == path)
    }

    fn in_page_root(&self, path: &Path) -> bool {
        self.page_root.as_deref().is_some_and(|root| path.starts_with(root))
    }

    fn list_allowed(&self, path: &Path) -> bool {
        if self.in_page_root(path) {
            return true;
        }
        let mut roots = self
            .selection
            .iter()
            .flat_map(|p| [Some(p.as_path()), p.parent()])
            .flatten()
            .chain(self.current_dir.as_deref());
        let parent = path.parent();
        roots.any(|root| root == path || parent == Some(root))
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

fn truncate_msg(mut msg: String) -> String {
    if msg.len() > MAX_MESSAGE_LEN {
        let mut end = MAX_MESSAGE_LEN;
        while !msg.is_char_boundary(end) {
            end -= 1;
        }
        msg.truncate(end);
        msg.push('…');
    }
    msg
}

impl wit::Host for HostState {}

impl bindings::rile::plugin::host::Host for HostState {
    fn log(&mut self, msg: String) {
        eprintln!("[plugin {}] {}", self.plugin_id, truncate_msg(msg));
    }

    fn notify(&mut self, msg: String) {
        if self.notifications.len() < MAX_NOTIFICATIONS {
            self.notifications.push(truncate_msg(msg));
        }
    }

    fn read_text(&mut self, path: String, max_bytes: u32) -> Result<String, String> {
        let canonical = dunce::canonicalize(&path).map_err(|_| format!("access denied: {path}"))?;
        if !self.read_allowed(&canonical) {
            return Err(format!("access denied: {path}"));
        }
        let file = fs::File::open(&canonical).map_err(|e| format!("{path}: {e}"))?;
        let limit = u64::from(max_bytes.min(MAX_READ_BYTES));
        let mut buf = Vec::new();
        file.take(limit)
            .read_to_end(&mut buf)
            .map_err(|e| format!("{path}: {e}"))?;
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    fn list_dir(&mut self, path: String) -> Result<Vec<String>, String> {
        let canonical = dunce::canonicalize(&path).map_err(|_| format!("access denied: {path}"))?;
        if !self.list_allowed(&canonical) {
            return Err(format!("access denied: {path}"));
        }
        let entries = fs::read_dir(&canonical).map_err(|e| format!("{path}: {e}"))?;
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .take(MAX_LIST_ENTRIES)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        Ok(names)
    }

    fn read_dir(&mut self, path: String) -> Result<Vec<wit::Entry>, String> {
        let canonical = dunce::canonicalize(&path).map_err(|_| format!("access denied: {path}"))?;
        if !self.list_allowed(&canonical) {
            return Err(format!("access denied: {path}"));
        }
        let entries = fs::read_dir(&canonical).map_err(|e| format!("{path}: {e}"))?;
        let mut out: Vec<wit::Entry> = entries
            .filter_map(|e| e.ok())
            .take(MAX_LIST_ENTRIES)
            .map(|e| {
                // DirEntry::metadata does not follow symlinks.
                let meta = e.metadata().ok();
                let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
                wit::Entry {
                    name: e.file_name().to_string_lossy().into_owned(),
                    is_dir,
                    size: meta.as_ref().filter(|_| !is_dir).map_or(0, |m| m.len()),
                    modified: meta
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs()),
                }
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn dir_size(&mut self, path: String) -> Result<u64, String> {
        let canonical = dunce::canonicalize(&path).map_err(|_| format!("access denied: {path}"))?;
        if !self.in_page_root(&canonical) {
            return Err(format!("access denied: {path}"));
        }
        dir_size(&canonical).map_err(|e| format!("{path}: {e}"))
    }
}

/// Sum of file sizes below `path` (no symlink following, bounded walk).
fn dir_size(path: &Path) -> std::io::Result<u64> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        return Ok(meta.len());
    }
    let mut total = 0u64;
    let mut seen = 0usize;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            seen += 1;
            if seen > MAX_DIR_SIZE_ENTRIES {
                return Ok(total);
            }
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() {
                total = total.saturating_add(meta.len());
            }
        }
    }
    Ok(total)
}

/// A loaded plugin.
struct LoadedPlugin {
    info: PluginInfo,
    component: Component,
    /// Live instance; `None` after a trap until re-instantiated.
    instance: Option<(Store<HostState>, Plugin)>,
}

/// Hosts a set of WebAssembly component plugins.
pub struct PluginHost {
    engine: Engine,
    linker: Linker<HostState>,
    plugins: Vec<LoadedPlugin>,
}

impl PluginHost {
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.consume_fuel(true);
        let engine = Engine::new(&config).map_err(|e| anyhow!("creating wasm engine: {e}"))?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|e| anyhow!("linking WASI: {e}"))?;
        Plugin::add_to_linker::<_, HasSelf<HostState>>(&mut linker, |s| s)
            .map_err(|e| anyhow!("linking host interface: {e}"))?;
        Ok(Self {
            engine,
            linker,
            plugins: Vec::new(),
        })
    }

    /// Loads every `*.wasm` file in `dir`. Returns one error per file that
    /// failed to load; a missing directory is not an error.
    pub fn load_dir(&mut self, dir: &Path) -> Vec<anyhow::Error> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(e) => return vec![anyhow!("reading plugin dir {}: {e}", dir.display())],
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|ext| ext == "wasm"))
            .collect();
        paths.sort();
        paths
            .into_iter()
            .filter_map(|p| self.load_file(&p).err())
            .collect()
    }

    /// Compiles, instantiates and registers a single plugin component.
    pub fn load_file(&mut self, path: &Path) -> Result<()> {
        let component = Component::from_file(&self.engine, path)
            .map_err(|e| anyhow!("{e:?}"))
            .with_context(|| format!("compiling plugin {}", path.display()))?;
        let (mut store, plugin) = self
            .instantiate(&component, "<loading>".to_owned())
            .with_context(|| format!("instantiating plugin {}", path.display()))?;
        store.set_fuel(FUEL_PER_CALL).map_err(|e| anyhow!("{e}"))?;
        let manifest = plugin
            .call_manifest(&mut store)
            .map_err(|e| anyhow!("{e:?}"))
            .with_context(|| format!("reading manifest of {}", path.display()))?;
        if manifest.id.trim().is_empty() {
            bail!("plugin {} has an empty id", path.display());
        }
        if self.plugins.iter().any(|p| p.info.id == manifest.id) {
            bail!(
                "plugin {} : duplicate plugin id `{}`",
                path.display(),
                manifest.id
            );
        }
        store.data_mut().plugin_id = manifest.id.clone();
        self.plugins.push(LoadedPlugin {
            info: PluginInfo {
                id: manifest.id,
                name: manifest.name,
                version: manifest.version,
                description: manifest.description,
                path: path.to_path_buf(),
                enabled: true,
            },
            component,
            instance: Some((store, plugin)),
        });
        Ok(())
    }

    fn instantiate(
        &self,
        component: &Component,
        plugin_id: String,
    ) -> Result<(Store<HostState>, Plugin)> {
        let mut store = Store::new(&self.engine, HostState::new(plugin_id));
        store.limiter(|s| &mut s.limits);
        store.set_fuel(FUEL_PER_CALL).map_err(|e| anyhow!("{e}"))?;
        let plugin = Plugin::instantiate(&mut store, component, &self.linker)
            .map_err(|e| anyhow!("{e:?}"))?;
        Ok((store, plugin))
    }

    pub fn plugins(&self) -> Vec<PluginInfo> {
        self.plugins.iter().map(|p| p.info.clone()).collect()
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) {
        if let Some(p) = self.plugins.iter_mut().find(|p| p.info.id == id) {
            p.info.enabled = enabled;
        }
    }

    /// Collects the commands of all enabled plugins. Plugins that fail are skipped.
    pub fn commands(&mut self, ctx: &Context) -> Vec<(String, Command)> {
        let wit_ctx = to_wit_context(ctx);
        let mut out = Vec::new();
        for idx in 0..self.plugins.len() {
            if !self.plugins[idx].info.enabled {
                continue;
            }
            let id = self.plugins[idx].info.id.clone();
            match self.call(idx, ctx, |plugin, store| {
                plugin.call_commands(store, &wit_ctx)
            }) {
                Ok((commands, _)) => out.extend(commands.into_iter().map(|c| {
                    (
                        id.clone(),
                        Command {
                            id: c.id,
                            label: c.label,
                        },
                    )
                })),
                Err(e) => eprintln!("[plugin {id}] commands failed: {e:#}"),
            }
        }
        out
    }

    /// Runs a command. Notifications queued through the host `notify` import
    /// are appended to the returned actions as [`Action::Notify`].
    pub fn run_command(
        &mut self,
        plugin_id: &str,
        command_id: &str,
        ctx: &Context,
    ) -> Result<Vec<Action>> {
        let idx = self.enabled_index(plugin_id)?;
        let wit_ctx = to_wit_context(ctx);
        let (result, notifications) = self.call(idx, ctx, |plugin, store| {
            plugin.call_run_command(store, command_id, &wit_ctx)
        })?;
        let mut actions: Vec<Action> = result
            .map_err(|e| anyhow!("{plugin_id}: {e}"))?
            .into_iter()
            .map(from_wit_action)
            .collect();
        actions.extend(notifications.into_iter().map(Action::Notify));
        Ok(actions)
    }

    /// Returns the preview text from the first enabled plugin that supports the file.
    /// The previewed path is the only path the plugin may read.
    pub fn preview(&mut self, path: &Path) -> Option<String> {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let ctx = Context {
            current_dir: path.parent().map(Path::to_path_buf),
            selection: vec![path.to_path_buf()],
        };
        let path_str = path.to_string_lossy().into_owned();
        for idx in 0..self.plugins.len() {
            if !self.plugins[idx].info.enabled {
                continue;
            }
            match self.call(idx, &ctx, |plugin, store| {
                plugin.call_preview(store, &path_str, &ext)
            }) {
                Ok((Some(text), _)) => return Some(text),
                Ok((None, _)) => {}
                Err(e) => eprintln!(
                    "[plugin {}] preview failed: {e:#}",
                    self.plugins[idx].info.id
                ),
            }
        }
        None
    }

    /// Pages of all enabled plugins as `(plugin id, page)`. Plugins that fail are skipped.
    pub fn pages(&mut self) -> Vec<(String, Page)> {
        let mut out = Vec::new();
        for idx in 0..self.plugins.len() {
            if !self.plugins[idx].info.enabled {
                continue;
            }
            let id = self.plugins[idx].info.id.clone();
            match self.call(idx, &Context::default(), |plugin, store| plugin.call_pages(store)) {
                Ok((pages, _)) => out.extend(pages.into_iter().map(|p| {
                    (
                        id.clone(),
                        Page {
                            id: p.id,
                            title: p.title,
                            icon: p.icon,
                        },
                    )
                })),
                Err(e) => eprintln!("[plugin {id}] pages failed: {e:#}"),
            }
        }
        out
    }

    /// Renders a page. The plugin may list and size anything below `req.root`.
    pub fn render_page(&mut self, plugin_id: &str, req: &PageRequest) -> Result<PageUpdate> {
        self.page_call(plugin_id, req, None)
    }

    /// Delivers a UI event (from a button, bubble or list item) to a page.
    pub fn page_event(&mut self, plugin_id: &str, req: &PageRequest, event: &str) -> Result<PageUpdate> {
        self.page_call(plugin_id, req, Some(event))
    }

    fn page_call(&mut self, plugin_id: &str, req: &PageRequest, event: Option<&str>) -> Result<PageUpdate> {
        let idx = self.enabled_index(plugin_id)?;
        let ctx = Context {
            current_dir: req.root.clone(),
            selection: Vec::new(),
        };
        let wit_req = wit::PageRequest {
            page_id: req.page_id.clone(),
            root: req.root.as_ref().map(|p| p.to_string_lossy().into_owned()),
            state: req.state.clone(),
        };
        let (result, notifications) =
            self.call_scoped(idx, &ctx, req.root.as_deref(), |plugin, store| match event {
                Some(ev) => plugin.call_page_event(store, &wit_req, ev),
                None => plugin.call_render_page(store, &wit_req),
            })?;
        let update = result.map_err(|e| anyhow!("{plugin_id}: {e}"))?;
        if update.state.len() > MAX_PAGE_STATE {
            bail!("{plugin_id}: page state exceeds {MAX_PAGE_STATE} bytes");
        }
        let mut actions: Vec<Action> = update.actions.into_iter().map(from_wit_action).collect();
        actions.extend(notifications.into_iter().map(Action::Notify));
        Ok(PageUpdate {
            view: from_wit_view(update.view),
            state: update.state,
            actions,
        })
    }

    fn enabled_index(&self, plugin_id: &str) -> Result<usize> {
        let idx = self
            .plugins
            .iter()
            .position(|p| p.info.id == plugin_id)
            .ok_or_else(|| anyhow!("unknown plugin `{plugin_id}`"))?;
        if !self.plugins[idx].info.enabled {
            bail!("plugin `{plugin_id}` is disabled");
        }
        Ok(idx)
    }

    /// Calls into plugin `idx` with a fresh fuel budget and the given context.
    /// Returns the result together with the queued notifications.
    fn call<R>(
        &mut self,
        idx: usize,
        ctx: &Context,
        f: impl FnOnce(&Plugin, &mut Store<HostState>) -> wasmtime::Result<R>,
    ) -> Result<(R, Vec<String>)> {
        self.call_scoped(idx, ctx, None, f)
    }

    fn call_scoped<R>(
        &mut self,
        idx: usize,
        ctx: &Context,
        page_root: Option<&Path>,
        f: impl FnOnce(&Plugin, &mut Store<HostState>) -> wasmtime::Result<R>,
    ) -> Result<(R, Vec<String>)> {
        if self.plugins[idx].instance.is_none() {
            let id = self.plugins[idx].info.id.clone();
            let instance = self.instantiate(&self.plugins[idx].component, id)?;
            self.plugins[idx].instance = Some(instance);
        }
        let entry = &mut self.plugins[idx];
        let (store, plugin) = entry.instance.as_mut().expect("instance present");
        store.set_fuel(FUEL_PER_CALL).map_err(|e| anyhow!("{e}"))?;
        store.data_mut().set_context(ctx, page_root);
        let result = f(plugin, store);
        let notifications = std::mem::take(&mut store.data_mut().notifications);
        match result {
            Ok(r) => Ok((r, notifications)),
            Err(e) => {
                // A trapped component instance cannot be re-entered; drop it.
                entry.instance = None;
                let out_of_fuel = matches!(
                    e.downcast_ref::<wasmtime::Trap>(),
                    Some(wasmtime::Trap::OutOfFuel)
                );
                if out_of_fuel {
                    bail!("plugin `{}` exceeded its execution budget", entry.info.id);
                }
                Err(anyhow!("plugin `{}` trapped: {e:?}", entry.info.id))
            }
        }
    }
}

fn to_wit_context(ctx: &Context) -> wit::Context {
    wit::Context {
        current_dir: ctx
            .current_dir
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        selection: ctx
            .selection
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
    }
}

fn from_wit_action(action: wit::Action) -> Action {
    match action {
        wit::Action::Open(p) => Action::Open(PathBuf::from(p)),
        wit::Action::Navigate(p) => Action::Navigate(PathBuf::from(p)),
        wit::Action::Refresh => Action::Refresh,
        wit::Action::CopyText(t) => Action::CopyText(t),
        wit::Action::Notify(m) => Action::Notify(m),
        wit::Action::ChooseRoot => Action::ChooseRoot,
        wit::Action::SetRoot(p) => Action::SetRoot(PathBuf::from(p)),
    }
}

/// Converts and sanitizes a guest view: caps sizes, drops invalid parents
/// (non-container or forward references become top-level), clamps numbers.
fn from_wit_view(view: wit::PageView) -> PageView {
    let mut nodes: Vec<Node> = Vec::with_capacity(view.nodes.len().min(MAX_PAGE_NODES));
    for (idx, node) in view.nodes.into_iter().take(MAX_PAGE_NODES).enumerate() {
        let parent = node
            .parent
            .map(|p| p as usize)
            .filter(|&p| p < idx && nodes[p].kind.is_container());
        let kind = match node.kind {
            wit::NodeKind::Column => NodeKind::Column,
            wit::NodeKind::Row => NodeKind::Row,
            wit::NodeKind::Heading(t) => NodeKind::Heading(t),
            wit::NodeKind::Text(t) => NodeKind::Text(t),
            wit::NodeKind::Muted(t) => NodeKind::Muted(t),
            wit::NodeKind::Button(b) => NodeKind::Button(Button {
                label: b.label,
                event: b.event,
            }),
            wit::NodeKind::ListItem(l) => NodeKind::ListItem(ListItem {
                title: l.title,
                detail: l.detail,
                trailing: l.trailing,
                event: l.event,
            }),
            wit::NodeKind::Progress(v) => {
                NodeKind::Progress(if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 })
            }
            wit::NodeKind::Bubbles(bubbles) => NodeKind::Bubbles(
                bubbles
                    .into_iter()
                    .filter(|b| b.value.is_finite() && b.value > 0.0)
                    .take(MAX_BUBBLES)
                    .map(|b| Bubble {
                        label: b.label,
                        detail: b.detail,
                        value: b.value,
                        color: b.color.map(|c| c & 0xFF_FFFF),
                        event: b.event,
                    })
                    .collect(),
            ),
            wit::NodeKind::Spacer => NodeKind::Spacer,
        };
        nodes.push(Node { parent, kind });
    }
    PageView { nodes }
}

/// The default plugin directory: `$RILE_CONFIG_DIR/plugins`, otherwise
/// `<config dir>/rile/plugins`.
pub fn default_plugin_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("RILE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir).join("plugins"));
    }
    dirs::config_dir().map(|d| d.join("rile").join("plugins"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn _assert_send() {
        fn is_send<T: Send>() {}
        is_send::<PluginHost>();
    }

    #[test]
    fn host_creates() {
        let host = PluginHost::new().unwrap();
        assert!(host.plugins().is_empty());
    }

    #[test]
    fn load_dir_missing_is_empty() {
        let mut host = PluginHost::new().unwrap();
        assert!(host.load_dir(Path::new("/definitely/not/here")).is_empty());
    }

    #[test]
    fn load_dir_reports_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("bad.wasm"), b"not wasm").unwrap();
        fs::write(dir.path().join("ignored.txt"), b"x").unwrap();
        let mut host = PluginHost::new().unwrap();
        let errors = host.load_dir(dir.path());
        assert_eq!(errors.len(), 1);
        assert!(host.plugins().is_empty());
    }

    #[test]
    fn list_sandbox_rules() {
        let mut s = HostState::new("t".into());
        s.selection = vec![PathBuf::from("/a/b/file.txt")];
        s.current_dir = Some(PathBuf::from("/c"));
        assert!(s.read_allowed(Path::new("/a/b/file.txt")));
        assert!(!s.read_allowed(Path::new("/a/b/other.txt")));
        assert!(s.list_allowed(Path::new("/a/b")));
        assert!(s.list_allowed(Path::new("/c")));
        assert!(s.list_allowed(Path::new("/c/sub")));
        assert!(!s.list_allowed(Path::new("/c/sub/deeper")));
        assert!(!s.list_allowed(Path::new("/")));
        s.page_root = Some(PathBuf::from("/r"));
        assert!(s.list_allowed(Path::new("/r/x/y/z")));
        assert!(!s.list_allowed(Path::new("/rx")));
        assert!(!s.in_page_root(Path::new("/c")));
    }

    #[test]
    fn view_sanitizes_parents() {
        let view = from_wit_view(wit::PageView {
            nodes: vec![
                wit::Node { parent: None, kind: wit::NodeKind::Column },
                wit::Node { parent: Some(0), kind: wit::NodeKind::Text("a".into()) },
                wit::Node { parent: Some(1), kind: wit::NodeKind::Text("b".into()) },
                wit::Node { parent: Some(9), kind: wit::NodeKind::Progress(7.0) },
            ],
        });
        let parents: Vec<_> = view.nodes.iter().map(|n| n.parent).collect();
        assert_eq!(parents, vec![None, Some(0), None, None]);
        assert_eq!(view.nodes[3].kind, NodeKind::Progress(1.0));
        assert_eq!(view.children(None).collect::<Vec<_>>(), vec![0, 2, 3]);
    }

    #[test]
    fn dir_size_sums_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a"), [0u8; 10]).unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/b"), [0u8; 5]).unwrap();
        assert_eq!(dir_size(dir.path()).unwrap(), 15);
    }
}
