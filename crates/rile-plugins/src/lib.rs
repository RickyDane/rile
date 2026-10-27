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
            notifications: Vec::new(),
        }
    }

    fn set_context(&mut self, ctx: &Context) {
        self.selection = ctx
            .selection
            .iter()
            .filter_map(|p| fs::canonicalize(p).ok())
            .collect();
        self.current_dir = ctx
            .current_dir
            .as_ref()
            .and_then(|p| fs::canonicalize(p).ok());
        self.notifications.clear();
    }

    fn read_allowed(&self, path: &Path) -> bool {
        self.selection.iter().any(|p| p == path)
    }

    fn list_allowed(&self, path: &Path) -> bool {
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
        let canonical = fs::canonicalize(&path).map_err(|_| format!("access denied: {path}"))?;
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
        let canonical = fs::canonicalize(&path).map_err(|_| format!("access denied: {path}"))?;
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
        let idx = self
            .plugins
            .iter()
            .position(|p| p.info.id == plugin_id)
            .ok_or_else(|| anyhow!("unknown plugin `{plugin_id}`"))?;
        if !self.plugins[idx].info.enabled {
            bail!("plugin `{plugin_id}` is disabled");
        }
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

    /// Calls into plugin `idx` with a fresh fuel budget and the given context.
    /// Returns the result together with the queued notifications.
    fn call<R>(
        &mut self,
        idx: usize,
        ctx: &Context,
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
        store.data_mut().set_context(ctx);
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
    }
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
    }
}
