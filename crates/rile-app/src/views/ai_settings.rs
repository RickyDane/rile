//! Settings → AI: the master switch, which model powers intelligent search
//! and summaries (Apple Intelligence, a local GGUF model run by llama.cpp or
//! an MLX model run by mlx-lm), and on-device photo descriptions.

use crate::views::kit::{self, PointerUnless, Status, Tone};
use rile_services::local_model::{CatalogModel, InstalledModel, ModelFormat};
use crate::workspace::Workspace;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Sizable,
    h_flex,
    input::{Input, InputState},
    switch::Switch,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext, Context, Entity, FontWeight, Global, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, Styled, Subscription, Task, Window, div, prelude::FluentBuilder, px,
};
use gpui_kit::component::{WindowExt, notification::Notification};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub struct AiSettings {
    ws: Entity<Workspace>,
    /// The custom download's Hugging Face repository.
    repo: Entity<InputState>,
    runtime: Entity<InputState>,
    mlx_runtime: Entity<InputState>,
    message: String,
    cancel: Option<Arc<AtomicBool>>,
    job: Option<usize>,
    provider: Provider,
    /// The last model of each format, restored when switching back to its tab.
    last_gguf: Option<PathBuf>,
    last_mlx: Option<PathBuf>,
    /// The "custom model from Hugging Face" form is open.
    show_custom: bool,
    show_runtime: bool,
    /// The model being downloaded; None while idle or installing the runtime.
    downloading: Option<Download>,
    /// GGUF files found in the custom repository: (repo, files or error);
    /// `listing_busy` while looking.
    listing: Option<(String, Result<Vec<(String, u64)>, String>)>,
    listing_busy: bool,
    error: bool,
    transferred: Option<(u64, Option<u64>)>,
    /// Smoothed transfer speed in bytes per second, once measurable.
    speed: Option<f64>,
    /// When and at how many bytes the speed was last sampled.
    sample: Option<(std::time::Instant, u64)>,
    /// Whether the selected model can run; None while checking.
    availability: Option<Result<(), String>>,
    /// The inline "remove all descriptions?" confirmation is showing.
    confirm_clear: bool,
    /// GGUF and MLX models already in Rile's models folder.
    installed: Vec<rile_services::local_model::InstalledModel>,
    /// Whether the runtime holds the selected model in memory.
    load: rile_services::local_model::LoadState,
    _availability_task: Task<()>,
    _load_task: Task<()>,
    _scan_task: Task<()>,
    _delete_task: Task<()>,
    _list_task: Task<()>,
    _subs: Vec<Subscription>,
}

/// A model download: the repository and, for GGUF, the file in it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Download {
    repo: String,
    /// Empty for MLX, which downloads the whole repository.
    file: String,
}

impl Download {
    fn of(model: &CatalogModel) -> Self {
        Self { repo: model.repo.into(), file: model.file.into() }
    }

    fn is(&self, model: &CatalogModel) -> bool {
        self.repo == model.repo && self.file == model.file
    }

    /// What the user knows it as: the catalog name, else the file or repository.
    fn name(&self) -> String {
        rile_services::local_model::CATALOG.iter().find(|m| self.is(m)).map_or_else(
            || if self.file.is_empty() { self.repo.clone() } else { file_name(std::path::Path::new(&self.file)) },
            |m| m.name.to_string(),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provider {
    Apple,
    Local(ModelFormat),
}

impl Provider {
    fn format(self) -> Option<ModelFormat> {
        match self {
            Provider::Apple => None,
            Provider::Local(format) => Some(format),
        }
    }
}

/// The AI settings view with a download or runtime install in progress. Held
/// here so closing Settings doesn't drop the view (which cancels the
/// transfer); the next Settings for the same workspace shows it again.
struct RunningTransfer(Entity<AiSettings>);

impl Global for RunningTransfer {}

enum RuntimeEvent {
    Progress(rile_services::model_runtime::Progress),
    Done(Result<PathBuf, String>),
}

impl Drop for AiSettings {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl AiSettings {
    /// The view of `ws` whose transfer is still running, if any.
    pub fn running(ws: &Entity<Workspace>, cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<RunningTransfer>().map(|t| t.0.clone()).filter(|view| view.read(cx).ws == *ws)
    }

    /// Keeps this view alive until its transfer ends.
    fn hold(&mut self, label: &str, cx: &mut Context<Self>) {
        if let Some(cancel) = self.cancel.clone() {
            self.job = Some(self.ws.update(cx, |ws, cx| ws.track_job(label.to_string(), cancel, cx)));
        }
        let view = cx.entity();
        cx.set_global(RunningTransfer(view));
    }

    fn sync_job(&self, cx: &mut Context<Self>) {
        if let Some(id) = self.job {
            let (done, total) = self.transferred.unwrap_or((0, None));
            self.ws.update(cx, |ws, cx| {
                if let Some(op) = ws.ops.iter_mut().find(|op| op.id == id) {
                    op.update_progress(rile_core::ops::Progress {
                        bytes_done: done,
                        bytes_total: total.unwrap_or(0),
                        file_bytes_done: done,
                        file_bytes_total: total.unwrap_or(0),
                        current: Some(self.message.clone().into()),
                        ..Default::default()
                    });
                    cx.notify();
                }
            });
        }
    }

    /// Ends the transfer and lets the view go once Settings no longer shows
    /// it. `detached`: Settings was closed meanwhile, so report the outcome.
    fn finish(&mut self, detached: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.job.take() {
            let error = if self.error {
                Some(self.message.clone().into())
            } else if self.cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
                Some("Cancelled".into())
            } else {
                None
            };
            self.ws.update(cx, |ws, cx| ws.finish_tracked_job(id, error, cx));
        }
        self.cancel = None;
        self.downloading = None;
        if cx.try_global::<RunningTransfer>().is_some_and(|t| t.0 == cx.entity()) {
            cx.remove_global::<RunningTransfer>();
        }
        if detached && !self.message.is_empty() {
            let note = if self.error { Notification::error(self.message.clone()) } else { Notification::success(self.message.clone()) };
            window.push_notification(note, cx);
        }
    }

    /// Records transfer progress and updates the smoothed speed.
    fn record(&mut self, done: u64, total: Option<u64>) {
        self.record_at(done, total, std::time::Instant::now());
    }

    fn record_at(&mut self, done: u64, total: Option<u64>, now: std::time::Instant) {
        match self.sample {
            Some((at, bytes)) if done >= bytes => {
                let elapsed = now.duration_since(at).as_secs_f64();
                if elapsed >= 0.5 {
                    let current = (done - bytes) as f64 / elapsed;
                    self.speed = Some(self.speed.map_or(current, |speed| speed * 0.7 + current * 0.3));
                    self.sample = Some((now, done));
                }
            }
            _ => self.sample = Some((now, done)),
        }
        self.transferred = Some((done, total));
    }

    fn clear_transfer(&mut self) {
        (self.transferred, self.speed, self.sample) = (None, None, None);
    }

    /// Takes the AI settings as values: the settings dialog is built inside a
    /// workspace update, so the workspace can't be read here.
    pub fn new(ws: Entity<Workspace>, runtime: String, mlx_runtime: String, model: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.observe(&ws, |_, _, cx| cx.notify())];
        let format = model.as_deref().map(rile_services::local_model::format);
        let mut this = Self {
            ws,
            repo: cx.new(|cx| InputState::new(window, cx).placeholder("owner/repository")),
            runtime: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(runtime)
                    .placeholder("/path/to/llama-server")
            }),
            mlx_runtime: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(mlx_runtime)
                    .placeholder("/path/to/mlx_lm.server")
            }),
            message: String::new(),
            cancel: None,
            job: None,
            // Apple Intelligence exists only on macOS; elsewhere start on llama.cpp.
            provider: format.map_or(if cfg!(target_os = "macos") { Provider::Apple } else { Provider::Local(ModelFormat::Gguf) }, Provider::Local),
            last_gguf: model.clone().filter(|_| format == Some(ModelFormat::Gguf)),
            last_mlx: model.filter(|_| format == Some(ModelFormat::Mlx)),
            show_custom: false,
            show_runtime: false,
            downloading: None,
            listing: None,
            listing_busy: false,
            error: false,
            transferred: None,
            speed: None,
            sample: None,
            availability: None,
            confirm_clear: false,
            installed: Vec::new(),
            load: rile_services::local_model::load_state(),
            _availability_task: Task::ready(()),
            _load_task: Task::ready(()),
            _scan_task: Task::ready(()),
            _delete_task: Task::ready(()),
            _list_task: Task::ready(()),
            _subs: subs,
        };
        this.check_availability(cx);
        this.scan_installed(cx);
        this.watch_load(cx);
        this
    }

    /// Follows the background model load while the settings are open.
    fn watch_load(&mut self, cx: &mut Context<Self>) {
        self._load_task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
                let state = rile_services::local_model::load_state();
                let alive = this.update(cx, |this, cx| {
                    if this.load != state {
                        this.load = state;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
    }

    /// Selects `path`, first asking for confirmation when that replaces the
    /// local model in use, because switching unloads it from memory and loads
    /// the new one. `then` runs once the switch happens.
    fn switch_to(&mut self, path: Option<PathBuf>, then: impl Fn(&mut Self, &mut Context<Self>) + 'static, window: &mut Window, cx: &mut Context<Self>) {
        let settings = &self.ws.read(cx).settings;
        let current = settings.ai_model_path.clone().filter(|_| settings.ai_enabled);
        let Some(current) = current.filter(|current| path.as_ref() != Some(current)) else {
            self.select(path, window, cx);
            then(self, cx);
            cx.notify();
            return;
        };
        let new_name = path.as_deref().map_or_else(|| "Apple Intelligence".to_string(), file_name);
        let detail = match &path {
            Some(_) => format!(
                "{} will be unloaded from memory and {new_name} loaded in its place. AI features wait until the new model is ready.",
                file_name(&current)
            ),
            None => format!("{} will be unloaded from memory, and Apple Intelligence will power AI features.", file_name(&current)),
        };
        let view = cx.entity();
        crate::dialogs::choose(
            format!("Switch to {new_name}?"),
            detail,
            false,
            vec![crate::dialogs::Choice::new("Switch Model", gpui_kit::component::button::ButtonVariant::Primary, move |window, cx| {
                view.update(cx, |this, cx| {
                    this.select(path.clone(), window, cx);
                    then(this, cx);
                    cx.notify();
                });
            })],
            window,
            cx,
        );
    }

    /// Lists downloaded models off the main thread.
    fn scan_installed(&mut self, cx: &mut Context<Self>) {
        let scan = cx.background_spawn(async { rile_services::local_model::installed() });
        self._scan_task = cx.spawn(async move |this, cx| {
            let installed = scan.await;
            this.update(cx, |this, cx| {
                this.installed = installed;
                cx.notify();
            })
            .ok();
        });
    }

    /// Switches to a downloaded model, re-checking that it is still intact.
    fn use_installed(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match rile_services::local_model::validate(&path) {
            Ok(()) => self.switch_to(
                Some(path),
                |this, _| {
                    this.error = false;
                    this.message.clear();
                },
                window,
                cx,
            ),
            Err(error) => {
                self.error = true;
                self.message = format!("{error:#}");
                self.scan_installed(cx);
                cx.notify();
            }
        }
    }

    /// Asks, then deletes a downloaded model from disk. Deleting the model in
    /// use deselects it first, which unloads it from memory.
    fn delete_installed(&mut self, model: rile_services::local_model::InstalledModel, window: &mut Window, cx: &mut Context<Self>) {
        let name = model.name();
        let in_use = self.ws.read(cx).settings.ai_model_path.as_deref() == Some(model.path.as_path());
        let detail = format!(
            "{name} is removed from Rile's models folder, freeing {}.{} You can download it again later.",
            format_bytes(model.size),
            if in_use { " It is the model in use, so it is unloaded and AI features need another model." } else { "" }
        );
        let view = cx.entity();
        crate::dialogs::choose(
            format!("Delete {name}?"),
            detail,
            true,
            vec![crate::dialogs::Choice::new("Delete Model", gpui_kit::component::button::ButtonVariant::Danger, move |window, cx| {
                view.update(cx, |this, cx| this.remove_installed(model.path.clone(), window, cx));
            })],
            window,
            cx,
        );
    }

    fn remove_installed(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.ws.read(cx).settings.ai_model_path.as_deref() == Some(path.as_path()) {
            self.select(None, window, cx);
        }
        for last in [&mut self.last_gguf, &mut self.last_mlx] {
            if last.as_deref() == Some(path.as_path()) {
                *last = None;
            }
        }
        self.installed.retain(|m| m.path != path);
        self.message.clear();
        self.error = false;
        cx.notify();
        let name = file_name(&path);
        let remove = cx.background_spawn(async move { rile_services::local_model::remove(&path) });
        self._delete_task = cx.spawn(async move |this, cx| {
            let result = remove.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.message = format!("Deleted {name}."),
                    Err(error) => {
                        this.error = true;
                        this.message = format!("Couldn't delete {name}: {error:#}");
                    }
                }
                this.scan_installed(cx);
            })
            .ok();
        });
    }

    /// Re-checks, off the main thread, whether the selected model can run.
    fn check_availability(&mut self, cx: &mut Context<Self>) {
        self.availability = None;
        let check = cx.background_spawn(async { Workspace::ai_availability() });
        self._availability_task = cx.spawn(async move |this, cx| {
            let result = check.await;
            this.update(cx, |this, cx| {
                this.availability = Some(result);
                cx.notify();
            })
            .ok();
        });
    }

    fn select(&mut self, path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_transfer();
        if let Some(path) = &path {
            let format = rile_services::local_model::format(path);
            match format {
                ModelFormat::Gguf => self.last_gguf = Some(path.clone()),
                ModelFormat::Mlx => self.last_mlx = Some(path.clone()),
            }
            self.provider = Provider::Local(format);
        }
        let runtime = self.runtime.read(cx).value().trim().to_string();
        let mlx_runtime = self.mlx_runtime.read(cx).value().trim().to_string();
        self.ws.update(cx, |ws, cx| {
            ws.settings.ai_model_path = path;
            ws.settings.ai_llama_server = runtime;
            ws.settings.ai_mlx_server = mlx_runtime;
            ws.settings_changed(window, cx);
        });
        self.check_availability(cx);
        cx.notify();
    }

    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.message.clear();
        self.clear_transfer();
        self.error = false;
        cx.notify();
        let mlx = self.provider == Provider::Local(ModelFormat::Mlx);
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: !mlx,
            directories: mlx,
            multiple: false,
            prompt: Some(if mlx { "Use MLX model folder" } else { "Use GGUF model" }.into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update_in(cx, |this, window, cx| {
                    match rile_services::local_model::validate(&path) {
                        Ok(()) => this.switch_to(
                            Some(path),
                            |this, _| {
                                this.error = false;
                                this.message = "Local model selected.".into();
                            },
                            window,
                            cx,
                        ),
                        Err(error) => {
                            this.message = format!("{error:#}");
                            this.error = true;
                        }
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn install_runtime(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cancel.is_some() {
            return;
        }
        let mlx = self.provider == Provider::Local(ModelFormat::Mlx);
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.error = false;
        self.clear_transfer();
        self.message = "Preparing runtime installation…".into();
        self.show_runtime = false;
        self.hold(if mlx { "Install MLX runtime" } else { "Install llama.cpp runtime" }, cx);
        let (tx, rx) = smol::channel::unbounded();
        std::thread::spawn(move || {
            let progress = |message| {
                let _ = tx.try_send(RuntimeEvent::Progress(message));
            };
            let result = if mlx {
                rile_services::mlx_runtime::install(&cancel, progress)
            } else {
                rile_services::model_runtime::install(&cancel, progress)
            };
            let _ = tx.send_blocking(RuntimeEvent::Done(result.map_err(|e| format!("{e:#}"))));
        });
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let finished = matches!(&event, RuntimeEvent::Done(_));
                if this
                    .update_in(cx, |this, window, cx| {
                        // Checked first: a finished download may open a dialog.
                        let detached = finished && !window.has_active_dialog(cx);
                        match event {
                            RuntimeEvent::Progress(
                                rile_services::model_runtime::Progress::Stage(message),
                            ) => {
                                this.message = message;
                                this.clear_transfer();
                            }
                            RuntimeEvent::Progress(
                                rile_services::model_runtime::Progress::Download { done, total },
                            ) => {
                                this.message = "Downloading runtime".into();
                                this.record(done, (total > 0).then_some(total));
                            }
                            RuntimeEvent::Done(Ok(path)) => {
                                let input = if mlx { &this.mlx_runtime } else { &this.runtime };
                                input.update(cx, |input, cx| {
                                    input.set_value(path.to_string_lossy().to_string(), window, cx)
                                });
                                let model = this.ws.read(cx).settings.ai_model_path.clone();
                                this.select(model, window, cx);
                                this.show_runtime = false;
                                this.message = "Runtime installed and ready to use.".into();
                            }
                            RuntimeEvent::Done(Err(error)) => {
                                this.message = error;
                                this.error = !this
                                    .cancel
                                    .as_ref()
                                    .is_some_and(|c| c.load(Ordering::Relaxed));
                            }
                        }
                        this.sync_job(cx);
                        if finished {
                            this.finish(detached, window, cx);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                if finished {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// Looks up the GGUF files of the custom repository.
    fn find_files(&mut self, cx: &mut Context<Self>) {
        let repo = self.repo.read(cx).value().trim().to_string();
        self.listing = None;
        self.listing_busy = true;
        cx.notify();
        let lookup = {
            let repo = repo.clone();
            cx.background_spawn(async move { rile_services::local_model::repo_ggufs(&repo).map_err(|e| format!("{e:#}")) })
        };
        self._list_task = cx.spawn(async move |this, cx| {
            let files = lookup.await;
            this.update(cx, |this, cx| {
                this.listing = Some((repo, files));
                this.listing_busy = false;
                cx.notify();
            })
            .ok();
        });
    }

    /// Downloads `download` in the background, then switches to it.
    fn download(&mut self, download: Download, window: &mut Window, cx: &mut Context<Self>) {
        if self.cancel.is_some() {
            return;
        }
        let mlx = download.file.is_empty();
        let checked = if mlx {
            rile_services::local_model::valid_repo(&download.repo)
                .then_some(())
                .ok_or_else(|| "Enter a Hugging Face owner/repository, for example mlx-community/Qwen3.5-4B-MLX-4bit.".to_string())
        } else {
            rile_services::local_model::download_url(&download.repo, &download.file).map(|_| ()).map_err(|e| e.to_string())
        };
        if let Err(error) = checked {
            self.message = error;
            self.error = true;
            cx.notify();
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.error = false;
        self.clear_transfer();
        self.message = "Connecting to Hugging Face…".into();
        let download_name = download.name();
        let (repo, file) = (download.repo.clone(), download.file.clone());
        self.downloading = Some(download);
        self.hold(&format!("Download AI model · {download_name}"), cx);
        let (tx, rx) = smol::channel::unbounded();
        std::thread::spawn(move || {
            let mut last = std::time::Instant::now();
            let progress = |bytes, total| {
                if bytes == 0 || total == Some(bytes) || last.elapsed().as_millis() >= 250 {
                    let _ = tx.try_send(Ok(Err((bytes, total))));
                    last = std::time::Instant::now();
                }
                !cancel.load(Ordering::Relaxed)
            };
            let result = if mlx {
                rile_services::local_model::download_mlx(&repo, progress)
            } else {
                rile_services::local_model::download(&repo, &file, progress)
            };
            let _ = tx.send_blocking(result.map(Ok).map_err(|e| format!("{e:#}")));
        });
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let finished = !matches!(&event, Ok(Err(_)));
                if this
                    .update_in(cx, |this, window, cx| {
                        // Checked first: a finished download may open a dialog.
                        let detached = finished && !window.has_active_dialog(cx);
                        match event {
                            Ok(Err((bytes, total))) => {
                                this.message = format!("Downloading {download_name}");
                                this.record(bytes, total);
                            }
                            Ok(Ok(path)) => {
                                this.message = format!("{download_name} downloaded.");
                                this.scan_installed(cx);
                                let selected = format!("{download_name} downloaded and selected.");
                                this.switch_to(Some(path), move |this, _| this.message = selected.clone(), window, cx);
                            }
                            Err(error) => {
                                this.message = error;
                                this.error = !this
                                    .cancel
                                    .as_ref()
                                    .is_some_and(|c| c.load(Ordering::Relaxed));
                            }
                        }
                        this.sync_job(cx);
                        if finished {
                            this.finish(detached, window, cx);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                if finished {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }
}

/// File name of a model path, for display.
fn file_name(path: &std::path::Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

impl AiSettings {
    fn switch(&self, id: &'static str, value: bool, label: &'static str, disabled: bool, f: fn(&mut rile_core::settings::Settings, bool)) -> AnyElement {
        let ws = self.ws.clone();
        Switch::new(id)
            .checked(value)
            .accessibility_label(label)
            .pointer_unless(disabled)
            .on_click(move |checked, window, cx| {
                let checked = *checked;
                ws.update(cx, |ws, cx| {
                    f(&mut ws.settings, checked);
                    ws.settings_changed(window, cx);
                });
            })
            .into_any_element()
    }

    /// The master switch with what AI does here and whether it is ready.
    fn overview(&self, cx: &mut Context<Self>) -> AnyElement {
        let ch = crate::theme::chrome(cx);
        let settings = &self.ws.read(cx).settings;
        let enabled = settings.ai_enabled;
        let model = settings.ai_model_path.as_deref().map(file_name);
        let (text, state): (SharedString, Status) = if !enabled {
            ("Off · AI actions are hidden from search and menus".into(), Status::Off)
        } else {
            match &self.availability {
                None => ("Checking the model…".into(), Status::Pending),
                Some(Ok(())) => {
                    use rile_services::local_model::LoadState;
                    let selected = settings.ai_model_path.as_deref();
                    let name = model.as_deref().unwrap_or("Apple Intelligence");
                    match &self.load {
                        LoadState::Loading(path) if Some(path.as_path()) == selected => (format!("Loading {name} into memory…").into(), Status::Pending),
                        LoadState::Ready(path) if Some(path.as_path()) == selected => (format!("Ready · {name} · loaded in memory").into(), Status::Ready),
                        LoadState::Failed(path, error) if Some(path.as_path()) == selected => (error.clone().into(), Status::Warning),
                        _ => (format!("Ready · {name}").into(), Status::Ready),
                    }
                }
                Some(Err(error)) => (error.clone().into(), Status::Warning),
            }
        };
        let glyph = crate::icons::apple_intelligence(22., ch.accent, cx);
        kit::card(cx)
            .debug_selector(|| "ai-overview".into())
            .p(px(16.))
            .child(
                h_flex()
                    .gap(px(14.))
                    .items_start()
                    .child(
                        div()
                            .size(px(40.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(10.))
                            .bg(ch.accent_soft)
                            .border_1()
                            .border_color(ch.accent.opacity(0.18))
                            .child(glyph),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(4.))
                            .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(ch.text_strong).child("AI features"))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .line_height(px(17.))
                                    .text_color(ch.text_muted)
                                    .child("Intelligent search, summaries and photo descriptions, processed on this device."),
                            )
                            .child(div().pt(px(4.)).debug_selector(|| "ai-status".into()).child(kit::status(text, state, cx))),
                    )
                    .child(div().flex_none().pt(px(2.)).child(self.switch("s-ai", enabled, "Enable AI features", false, |s, v| s.ai_enabled = v))),
            )
            .into_any_element()
    }

    /// Switches the provider tab, restoring that format's last model
    /// (Apple Intelligence until one is chosen).
    fn open_provider(&mut self, provider: Provider, window: &mut Window, cx: &mut Context<Self>) {
        let last = match provider {
            Provider::Apple => None,
            Provider::Local(ModelFormat::Gguf) => self.last_gguf.clone(),
            Provider::Local(ModelFormat::Mlx) => self.last_mlx.clone(),
        };
        // The tab only changes once a confirmed switch happens.
        self.switch_to(
            last,
            move |this, _| {
                this.provider = provider;
                this.message.clear();
                this.error = false;
                this.show_custom = false;
                this.listing = None;
                this.show_runtime = false;
            },
            window,
            cx,
        );
    }

    /// Provider tabs plus the setup for the chosen one.
    fn model_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let ch = crate::theme::chrome(cx);
        let provider = self.provider;
        let busy = self.cancel.is_some();
        let tint = |on: bool| if on { ch.accent } else { ch.text_muted };
        let apple_glyph = cfg!(target_os = "macos").then(|| crate::icons::apple_intelligence(16., tint(provider == Provider::Apple), cx));
        let chip_glyph = |format| crate::icons::icon(IconName::Cpu).size(px(16.)).text_color(tint(provider == Provider::Local(format))).into_any_element();
        let tab = |selector: &'static str, id: &'static str, glyph: AnyElement, label: &'static str, target: Provider, cx: &mut Context<Self>| {
            div().flex_1().debug_selector(move || selector.into()).child(
                kit::tab(id, Some(glyph), label, provider == target, cx)
                    .w_full()
                    .pointer_unless(busy)
                    .on_click(cx.listener(move |this, _, window, cx| this.open_provider(target, window, cx))),
            )
        };
        let gguf_tab = tab("model-provider-local", "model-local", chip_glyph(ModelFormat::Gguf), "llama.cpp", Provider::Local(ModelFormat::Gguf), cx);
        let mlx_tab = rile_services::mlx_runtime::supported()
            .then(|| tab("model-provider-mlx", "model-mlx", chip_glyph(ModelFormat::Mlx), "MLX", Provider::Local(ModelFormat::Mlx), cx));
        v_flex()
            .debug_selector(|| "model-settings".into())
            .gap(px(10.))
            .min_w_0()
            .child(kit::heading("Model", Some(if cfg!(target_os = "macos") { "Powers intelligent search (⇧↵ in the search field) and summaries." } else { "Powers intelligent search (Shift+Enter in the search field) and summaries." }.into()), cx))
            .child(
                kit::tabs(cx)
                    .w_full()
                    .children(apple_glyph.map(|glyph| tab("model-provider-apple", "model-apple", glyph, "Apple Intelligence", Provider::Apple, cx)))
                    .child(gguf_tab)
                    .children(mlx_tab),
            )
            .when(provider == Provider::Apple, |v| {
                v.child(kit::card(cx).debug_selector(|| "model-apple-card".into()).child(kit::row(
                    "Built into your Mac",
                    "Apple's on-device model. Needs macOS 26 with Apple Intelligence turned on; no setup or downloads.",
                    div(),
                    true,
                    &ch,
                )))
            })
            .when_some(provider.format(), |v, format| v.child(self.local_setup(format, cx)).child(self.thinking_settings(format, cx)))
            .when(!self.message.is_empty() && self.cancel.is_none(), |v| v.child(self.transfer_status(cx)))
            .into_any_element()
    }

    fn thinking_settings(&self, format: ModelFormat, cx: &mut Context<Self>) -> AnyElement {
        use rile_core::settings::AiThinking;
        let selected = self.ws.read(cx).settings.ai_thinking;
        let options = [("none", "None", AiThinking::None), ("low", "Low (default)", AiThinking::Low),
            ("medium", "Medium", AiThinking::Medium), ("high", "High", AiThinking::High),
            ("unlimited", "Unlimited", AiThinking::Unlimited)];
        v_flex().gap(px(8.))
            .child(kit::heading("Thinking", Some("Limit reasoning before the answer. Low is the default.".into()), cx))
            .child(h_flex().flex_wrap().gap(px(4.)).children(options.into_iter().map(|(id, label, level)| {
                let ws = self.ws.clone();
                div().debug_selector(move || format!("ai-thinking-{id}")).child(
                    kit::button(format!("thinking-{id}"), label, if selected == level { Tone::Primary } else { Tone::Secondary }, true, cx)
                        .on_click(move |_, window, cx| ws.update(cx, |ws, cx| {
                            ws.settings.ai_thinking = level;
                            ws.settings_changed(window, cx);
                        })),
                )
            })))
            .child(kit::hint(if format == ModelFormat::Mlx {
                "MLX supports None or enabled thinking, not token budgets. Low, Medium, High and Unlimited enable thinking; templates may ignore None."
            } else {
                "None: off · Low: 64 tokens · Medium: 128 · High: 256 · Unlimited: no separate thinking cap. Requires a recent runtime. Not a time limit."
            }, cx))
            .into_any_element()
    }

    /// Numbered step marker; a check once the step is done.
    fn step(number: &'static str, done: bool, ch: &crate::theme::Chrome) -> AnyElement {
        div()
            .size(px(22.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .when(done, |d| d.bg(ch.success.opacity(0.16)).child(crate::icons::icon(IconName::Check).size(px(13.)).text_color(ch.success)))
            .when(!done, |d| d.bg(ch.hover_strong).text_color(ch.text_muted).child(number))
            .into_any_element()
    }

    /// One setup step: marker, title, state and actions, then any expanded form.
    fn step_row(number: &'static str, done: bool, title: &'static str, detail: SharedString, actions: AnyElement, ch: &crate::theme::Chrome) -> gpui_kit::Div {
        h_flex()
            .gap(px(12.))
            .px(px(14.))
            .py(px(12.))
            .child(Self::step(number, done, ch))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).text_color(ch.text).child(title))
                    .child(div().text_size(px(11.)).line_height(px(15.)).text_color(ch.text_muted).when(done, |d| d.truncate()).child(detail)),
            )
            .child(h_flex().flex_none().gap(px(8.)).child(actions))
    }

    fn local_setup(&self, format: ModelFormat, cx: &mut Context<Self>) -> AnyElement {
        let ch = crate::theme::chrome(cx);
        let busy = self.cancel.is_some();
        let mlx = format == ModelFormat::Mlx;
        let settings = &self.ws.read(cx).settings;
        let model = settings.ai_model_path.clone().filter(|path| rile_services::local_model::format(path) == format);
        let runtime_path = if mlx { settings.ai_mlx_server.clone() } else { rile_services::model_runtime::resolve(&settings.ai_llama_server) };
        let configured = std::path::Path::new(&runtime_path).is_file();
        let supported = if mlx { rile_services::mlx_runtime::supported() } else { rile_services::model_runtime::supported() };
        let engine = if mlx { "mlx-lm" } else { "llama.cpp" };
        let installing = busy && self.downloading.is_none();
        // The next step to take gets the primary button.
        let runtime_next = !configured && supported;

        let runtime_actions = h_flex()
            .gap(px(8.))
            .child(
                div().debug_selector(|| "model-runtime-options".into()).child(
                    kit::button("model-runtime-options", if self.show_runtime { "Hide Path" } else { "Custom Path…" }, Tone::Ghost, true, cx)
                        .toggled(self.show_runtime)
                        .pointer_unless(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_runtime = !this.show_runtime;
                            cx.notify();
                        })),
                ),
            )
            .when(supported && !installing, |h| {
                h.child(
                    kit::button(
                        "model-runtime-install",
                        if configured { "Check for Updates" } else { "Install Runtime" },
                        if runtime_next { Tone::Primary } else { Tone::Secondary },
                        true,
                        cx,
                    )
                    .pointer_unless(busy)
                    .on_click(cx.listener(|this, _, window, cx| this.install_runtime(window, cx))),
                )
            })
            .into_any_element();
        let runtime_detail: SharedString = if installing {
            self.message.clone().into()
        } else if configured {
            format!("{engine} · {runtime_path}").into()
        } else if mlx {
            "Needed to run MLX models. Installs once with its own Python; no Homebrew or Terminal.".into()
        } else if supported {
            "Needed to run local models. Installs once.".into()
        } else {
            "Automatic installation isn't available on this platform. Set a custom path.".into()
        };

        let model_detail: SharedString = match &model {
            Some(path) => format!("Using {}", self.display_name(path)).into(),
            None => "Download one below or use your own. Apple Intelligence is used until one is selected.".into(),
        };
        let more = h_flex()
            .gap(px(4.))
            .px(px(14.))
            .pb(px(10.))
            .ml(px(28.))
            .child(
                div().debug_selector(|| "model-hf-toggle".into()).child(
                    kit::button_with_glyph(
                        "model-hugging-face",
                        crate::icons::icon(if self.show_custom { IconName::ChevronUp } else { IconName::Plus }).size(px(14.)).into_any_element(),
                        "Custom from Hugging Face",
                        Tone::Ghost,
                        true,
                        cx,
                    )
                    .toggled(self.show_custom)
                    .pointer_unless(busy)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_custom = !this.show_custom;
                        cx.notify();
                    })),
                ),
            )
            .child(
                div().debug_selector(|| "model-choose-file".into()).child(
                    kit::button_with_glyph(
                        "model-choose-file",
                        crate::icons::icon(IconName::FolderOpen).size(px(14.)).into_any_element(),
                        if mlx { "Choose Folder…" } else { "Choose File…" },
                        Tone::Ghost,
                        true,
                        cx,
                    )
                    .pointer_unless(busy)
                    .on_click(cx.listener(|this, _, window, cx| this.choose(window, cx))),
                ),
            );

        let form = || v_flex().gap(px(10.)).px(px(14.)).pt(px(2.)).pb(px(14.)).ml(px(34.));
        kit::card(cx)
            .debug_selector(|| "model-runtime-card".into())
            .child(Self::step_row("1", configured, "Runtime", runtime_detail, runtime_actions, &ch))
            .when(installing, |card| card.child(form().child(self.progress(cx))))
            .when(self.show_runtime, |card| {
                card.child(
                    form()
                        .child(div().debug_selector(|| "model-runtime-field".into()).child(if mlx {
                            kit::field("mlx_lm.server executable", Input::new(&self.mlx_runtime).small().disabled(busy), cx)
                        } else {
                            kit::field("llama-server executable", Input::new(&self.runtime).small().disabled(busy), cx)
                        }))
                        .child(
                            h_flex().justify_end().gap(px(8.)).child(
                                kit::button("model-runtime", "Save Path", Tone::Primary, true, cx)
                                    .pointer_unless(busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let model = this.ws.read(cx).settings.ai_model_path.clone();
                                        this.select(model, window, cx);
                                        this.error = false;
                                        this.message = "Runtime path saved.".into();
                                        this.show_runtime = false;
                                    })),
                            ),
                        ),
                )
            })
            .child(
                div()
                    .debug_selector(|| "model-file-card".into())
                    .border_t_1()
                    .border_color(ch.hairline)
                    .child(Self::step_row("2", model.is_some(), "Model", model_detail, div().into_any_element(), &ch)),
            )
            .child(self.model_list(format, model.as_deref(), !runtime_next, cx))
            .child(more)
            .when(self.show_custom, |card| card.child(self.custom_form(mlx, busy, cx)))
            .into_any_element()
    }

    /// The name shown for a model path: its catalog name when it is one.
    fn display_name(&self, path: &std::path::Path) -> String {
        self.installed
            .iter()
            .find(|m| m.path == path)
            .and_then(|m| rile_services::local_model::CATALOG.iter().find(|c| c.is(m)))
            .map_or_else(|| file_name(path), |c| c.name.to_string())
    }

    /// Recommended models, then other downloaded ones, then a custom download
    /// in progress. Downloaded rows are a single-choice list (the selected one
    /// is in use); the others offer a download. `suggest`: give the
    /// recommended model the primary button while nothing is downloaded.
    fn model_list(&self, format: ModelFormat, selected: Option<&std::path::Path>, suggest: bool, cx: &mut Context<Self>) -> AnyElement {
        let ch = crate::theme::chrome(cx);
        let installed: Vec<&InstalledModel> = self.installed.iter().filter(|m| m.format == format).collect();
        let catalog: Vec<&'static CatalogModel> = rile_services::local_model::catalog(format).collect();
        let suggest = suggest && installed.is_empty() && selected.is_none();
        let caption = |text: String| div().pt(px(6.)).pb(px(2.)).pl(px(10.)).text_size(px(11.)).font_weight(FontWeight::MEDIUM).text_color(ch.text_muted).child(text);
        // Downloaded models first, numbered for test selectors, under their catalog name when they are one.
        let downloaded: Vec<AnyElement> = installed
            .iter()
            .enumerate()
            .map(|(i, model)| {
                let entry = catalog.iter().copied().find(|c| c.is(model));
                self.model_row(entry, Some(model), Some(i), selected, false, cx)
            })
            .collect();
        let mut available: Vec<AnyElement> = catalog
            .iter()
            .filter(|entry| !installed.iter().any(|m| entry.is(m)))
            .map(|entry| self.model_row(Some(entry), None, None, selected, suggest && entry.recommended, cx))
            .collect();
        if let Some(download) = self.downloading.as_ref().filter(|d| d.file.is_empty() == (format == ModelFormat::Mlx) && !catalog.iter().any(|c| d.is(c))) {
            available.push(self.pending_row(download, cx));
        }
        let memory = rile_services::local_model::system_memory()
            .map(|bytes| format!(" · this computer has {} GB memory", (bytes as f64 / (1u64 << 30) as f64).round()))
            .unwrap_or_default();
        v_flex()
            .debug_selector(|| "model-installed-list".into())
            .gap(px(2.))
            .px(px(14.))
            .pb(px(6.))
            .ml(px(28.))
            .when(!downloaded.is_empty(), |v| v.child(caption("On this computer".into())).children(downloaded))
            .when(!available.is_empty(), |v| v.child(caption(format!("Available to download{memory}"))).children(available))
            .into_any_element()
    }

    /// One model: a catalog entry, a downloaded model, or both.
    fn model_row(
        &self,
        entry: Option<&'static CatalogModel>,
        model: Option<&InstalledModel>,
        index: Option<usize>,
        selected: Option<&std::path::Path>,
        suggested: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants};
        let ch = crate::theme::chrome(cx);
        let busy = self.cancel.is_some();
        let active = model.is_some_and(|m| selected == Some(m.path.as_path()));
        let downloading = entry.is_some_and(|e| self.downloading.as_ref().is_some_and(|d| d.is(e)));
        let name = entry.map_or_else(|| model.map(|m| m.name()).unwrap_or_default(), |e| e.name.to_string());
        let warn: gpui_kit::Hsla = gpui_kit::rgba(0xe5b567ff).into();
        // Size and memory sit beside the name; the description goes below.
        let (meta, detail, roomy): (String, String, bool) = match (entry, model) {
            (Some(e), _) => {
                let fits = rile_services::local_model::fits(e.memory_gb);
                let detail = if fits { e.blurb.to_string() } else { format!("Needs {} GB memory, more than this computer has.", e.memory_gb) };
                (format!("{} · {} GB memory", format_bytes(e.size), e.memory_gb), detail, fits)
            }
            (None, Some(m)) => (format_bytes(m.size), m.repo.clone().unwrap_or_else(|| "Downloaded model".into()), true),
            (None, None) => Default::default(),
        };

        let meta = if active { format!("In use · {meta}") } else { meta };
        let leading = match model {
            Some(_) => div()
                .size(px(16.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .border_1()
                .border_color(if active { ch.accent } else { ch.text_faint })
                .when(active, |d| d.child(div().size(px(8.)).rounded_full().bg(ch.accent)))
                .into_any_element(),
            None => crate::icons::icon(IconName::ArrowDown).flex_none().size(px(16.)).text_color(ch.text_faint).into_any_element(),
        };
        let title = h_flex()
            .gap(px(6.))
            .min_w_0()
            .child(
                div()
                    .truncate()
                    .text_size(px(12.))
                    .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                    .text_color(if active { ch.text_strong } else { ch.text })
                    .child(name.clone()),
            )
            .when(entry.is_some_and(|e| e.recommended), |h| {
                h.child(
                    div()
                        .flex_none()
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(px(4.))
                        .bg(ch.accent_soft)
                        .text_size(px(10.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ch.accent)
                        .child("Recommended"),
                )
            })
            .child(div().flex_1())
            .child(div().flex_none().text_size(px(11.)).text_color(ch.text_faint).child(meta));
        let body = v_flex()
            .flex_1()
            .min_w_0()
            .gap(px(3.))
            .child(title)
            .when(downloading, |v| v.child(self.progress_bar(cx)))
            .when(!downloading, |v| {
                v.child(div().truncate().text_size(px(11.)).font_weight(FontWeight::NORMAL).text_color(if roomy { ch.text_muted } else { warn }).child(detail))
            });

        let row = h_flex().w_full().gap(px(10.)).child(leading).child(body);
        let Some(model) = model else {
            // Not downloaded: the row offers its download.
            let entry = entry.expect("a row is a catalog entry or a download");
            let action = if downloading {
                self.cancel_button(cx)
            } else {
                div()
                    .debug_selector(move || format!("model-get-{}", entry.repo))
                    .child(
                        kit::button(SharedString::from(format!("model-get-{}", entry.repo)), "Download", if suggested { Tone::Primary } else { Tone::Secondary }, true, cx)
                            .accessibility_label(format!("Download {name}"))
                            .pointer_unless(busy)
                            .on_click(cx.listener(move |this, _, window, cx| this.download(Download::of(entry), window, cx))),
                    )
                    .into_any_element()
            };
            return h_flex()
                .w_full()
                .min_h(px(44.))
                .px(px(10.))
                .py(px(6.))
                .gap(px(10.))
                .child(row.flex_1().min_w_0())
                .child(div().flex_none().child(action))
                .into_any_element();
        };

        let i = index.unwrap_or_default();
        let (bg, hover) = if active { (ch.hover_strong, ch.hover_strong) } else { (gpui_kit::transparent_black(), ch.hover) };
        let path = model.path.clone();
        let delete = {
            let model = model.clone();
            div().debug_selector(move || format!("model-delete-{i}")).child(
                Button::new(("model-delete", i))
                    .ghost()
                    .small()
                    .icon(IconName::Trash)
                    .tooltip("Delete Model…")
                    .accessibility_label(format!("Delete {name}"))
                    .pointer_unless(busy)
                    .on_click(cx.listener(move |this, _, window, cx| this.delete_installed(model.clone(), window, cx))),
            )
        };
        h_flex()
            .w_full()
            .gap(px(4.))
            .child(
                div().flex_1().min_w_0().debug_selector(move || format!("model-installed-{i}")).child(
                    Button::new(("model-installed", i))
                        .accessibility_label(format!("Use {name}"))
                        .toggled(active)
                        .custom(ButtonCustomVariant::new(cx).color(bg).foreground(ch.text).hover(hover).active(ch.hover_strong).shadow(false))
                        .bg(bg)
                        .w_full()
                        .h(px(44.))
                        .px(px(10.))
                        .rounded(px(7.))
                        .child(row)
                        .pointer_unless(busy)
                        .on_click(cx.listener(move |this, _, window, cx| this.use_installed(path.clone(), window, cx))),
                ),
            )
            .child(delete)
            .into_any_element()
    }

    /// A custom (not recommended) model being downloaded.
    fn pending_row(&self, download: &Download, cx: &mut Context<Self>) -> AnyElement {
        let ch = crate::theme::chrome(cx);
        h_flex()
            .w_full()
            .min_h(px(44.))
            .px(px(10.))
            .py(px(6.))
            .gap(px(10.))
            .child(crate::icons::icon(IconName::ArrowDown).flex_none().size(px(16.)).text_color(ch.accent))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(div().truncate().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(ch.text).child(download.name()))
                    .child(self.progress_bar(cx)),
            )
            .child(div().flex_none().child(self.cancel_button(cx)))
            .into_any_element()
    }

    /// Download from any public Hugging Face repository: GGUF repositories
    /// list their model files to pick from; MLX downloads the repository.
    fn custom_form(&self, mlx: bool, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let ch = crate::theme::chrome(cx);
        let lookup = if mlx {
            kit::button("model-download", "Download", Tone::Primary, true, cx)
                .pointer_unless(busy)
                .on_click(cx.listener(|this, _, window, cx| {
                    let repo = this.repo.read(cx).value().trim().to_string();
                    this.download(Download { repo, file: String::new() }, window, cx);
                }))
        } else {
            kit::button("model-find", if self.listing_busy { "Looking…" } else { "Find Files" }, Tone::Secondary, true, cx)
                .pointer_unless(busy || self.listing_busy)
                .on_click(cx.listener(|this, _, _, cx| this.find_files(cx)))
        };
        let files: Option<AnyElement> = match self.listing.as_ref().filter(|_| !mlx) {
            Some((_, Err(error))) => Some(kit::notice(error.clone(), true, cx).into_any_element()),
            Some((repo, Ok(files))) => Some(
                kit::card(cx)
                    .debug_selector(|| "model-custom-files".into())
                    .children(files.iter().enumerate().map(|(i, (file, size))| {
                        let download = Download { repo: repo.clone(), file: file.clone() };
                        let have = self.installed.iter().any(|m| m.repo.as_deref() == Some(repo.as_str()) && m.name() == file_name(std::path::Path::new(file)));
                        let control = if have {
                            div().text_size(px(11.)).text_color(ch.text_muted).child("Downloaded").into_any_element()
                        } else {
                            div()
                                .debug_selector(move || format!("model-custom-get-{i}"))
                                .child(
                                    kit::button(("model-custom-get", i), "Download", Tone::Secondary, true, cx)
                                        .pointer_unless(busy)
                                        .on_click(cx.listener(move |this, _, window, cx| this.download(download.clone(), window, cx))),
                                )
                                .into_any_element()
                        };
                        let balanced = file.to_ascii_uppercase().contains("Q4_K_M");
                        h_flex()
                            .gap(px(10.))
                            .px(px(12.))
                            .py(px(5.))
                            .when(i > 0, |h| h.border_t_1().border_color(ch.hairline))
                            .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(ch.text).child(file.clone()))
                            .when(balanced, |h| h.child(div().flex_none().text_size(px(10.)).font_weight(FontWeight::SEMIBOLD).text_color(ch.accent).child("Balanced")))
                            .child(div().flex_none().text_size(px(11.)).text_color(ch.text_faint).child(format_bytes(*size)))
                            .child(div().flex_none().child(control))
                    }))
                    .into_any_element(),
            ),
            None => None,
        };
        v_flex()
            .id("model-download-form")
            .debug_selector(|| "model-download-form".into())
            .gap(px(10.))
            .px(px(14.))
            .pb(px(14.))
            .ml(px(34.))
            .child(
                h_flex()
                    .gap(px(8.))
                    .items_end()
                    .child(div().flex_1().min_w_0().child(kit::field("Hugging Face repository", Input::new(&self.repo).small().disabled(busy), cx)))
                    .child(div().flex_none().child(lookup)),
            )
            .children(files)
            .child(kit::hint(
                if mlx {
                    "Downloads the whole repository, e.g. mlx-community/Qwen3.5-4B-MLX-4bit. Public, ungated models only; check the license and memory needs first."
                } else {
                    "For example unsloth/Qwen3.5-4B-GGUF. Q4_K_M files balance size and quality. Public, ungated, single-file models only; check the license first."
                },
                cx,
            ))
            .into_any_element()
    }

    /// Progress bar with "1.20 GB / 4.00 GB · 30% · …" below it.
    fn progress_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let fraction = self.transferred.and_then(|(done, total)| total.map(|total| (done as f32 / total as f32).clamp(0., 1.)));
        let detail = self.transfer_detail().unwrap_or_else(|| self.message.clone());
        v_flex()
            .gap(px(5.))
            .pt(px(2.))
            .child(
                div()
                    .id("model-download-progress")
                    .debug_selector(|| "model-download-progress".into())
                    .w_full()
                    .child(crate::progress::activity_bar("model-transfer", fraction, 6., cx)),
            )
            .child(kit::hint(detail, cx).truncate())
            .into_any_element()
    }

    fn cancel_button(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .debug_selector(|| "model-cancel".into())
            .child(kit::button("model-cancel", "Cancel", Tone::Ghost, true, cx).on_click(cx.listener(|this, _, _, cx| {
                if let Some(cancel) = &this.cancel {
                    cancel.store(true, Ordering::Relaxed);
                }
                this.message = "Cancelling…".into();
                cx.notify();
            })))
            .into_any_element()
    }

    /// Runtime install progress, with Cancel.
    fn progress(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .gap(px(12.))
            .child(div().flex_1().min_w_0().child(self.progress_bar(cx)))
            .child(div().flex_none().child(self.cancel_button(cx)))
            .into_any_element()
    }

    /// "1.20 GB / 4.00 GB · 30% · 25.0 MB/s · 2 min left"; speed and time
    /// left appear once measurable.
    fn transfer_detail(&self) -> Option<String> {
        let busy = self.cancel.is_some();
        let speed = self.speed.filter(|speed| *speed > 0.);
        self.transferred.map(|(done, total)| {
            let mut text = match total {
                Some(total) => format!("{} / {} · {:.0}%", format_bytes(done), format_bytes(total), (done as f64 / total.max(1) as f64).min(1.) * 100.),
                None => format!("{} downloaded", format_bytes(done)),
            };
            if busy && let Some(speed) = speed {
                text.push_str(&format!(" · {}/s", format_bytes(speed as u64)));
                if let Some(total) = total.filter(|total| *total > done) {
                    text.push_str(&format!(" · {} left", format_eta((total - done) as f64 / speed)));
                }
            }
            text
        })
    }

    /// Result or error of the last runtime install or download; progress
    /// shows inline on the runtime step or the model's row.
    fn transfer_status(&self, cx: &mut Context<Self>) -> AnyElement {
        let ch = crate::theme::chrome(cx);
        let state = if self.error { Status::Warning } else { Status::Ready };
        kit::card(cx)
            .id("model-transfer-status")
            .debug_selector(|| "model-transfer-status".into())
            .px(px(14.))
            .py(px(10.))
            .child(
                h_flex()
                    .gap(px(12.))
                    .justify_between()
                    .child(div().flex_1().min_w_0().child(kit::status(self.message.clone(), state, cx).text_size(px(12.)).text_color(ch.text)))
                    .child(kit::button("model-dismiss", "Dismiss", Tone::Ghost, true, cx).on_click(cx.listener(|this, _, _, cx| {
                        this.message.clear();
                        this.error = false;
                        cx.notify();
                    }))),
            )
            .into_any_element()
    }

    /// On-device photo and document descriptions (macOS Vision).
    fn photos_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let ch = crate::theme::chrome(cx);
        let ws = self.ws.read(cx);
        let enabled = ws.settings.ai_enabled;
        let auto = ws.settings.ai_smart_tags;
        let running = ws.photo_job.is_some_and(|id| ws.ops.iter().any(|o| o.id == id));
        let mac = cfg!(any(target_os = "macos", windows));
        let unavailable = !enabled || !mac;
        let tagged = rile_services::tags::stats();
        let confirm = self.confirm_clear && tagged > 0;

        let glyph = crate::icons::apple_intelligence(16., ch.text, cx);
        let describe = kit::button_with_glyph("s-tags-run", glyph, if running { "Describing…" } else { "Describe All" }, Tone::Secondary, true, cx)
            .pointer_unless(running || unavailable)
            .on_click(cx.listener(|this, _, window, cx| {
                this.ws.update(cx, |ws, cx| ws.start_photo_job(true, window, cx));
            }));
        let clear: AnyElement = if confirm {
            h_flex()
                .gap(px(8.))
                .child(kit::button("s-tags-clear-cancel", "Cancel", Tone::Ghost, true, cx).on_click(cx.listener(|this, _, _, cx| {
                    this.confirm_clear = false;
                    cx.notify();
                })))
                .child(div().debug_selector(|| "s-tags-clear-confirm".into()).child(
                    kit::button("s-tags-clear-confirm", "Remove", Tone::Destructive, true, cx).on_click(cx.listener(|this, _, _, cx| {
                        rile_services::tags::clear();
                        this.confirm_clear = false;
                        this.ws.update(cx, |_, cx| cx.notify());
                        cx.notify();
                    })),
                ))
                .into_any_element()
        } else {
            div()
                .debug_selector(|| "s-tags-clear".into())
                .child(
                    kit::button("s-tags-clear", "Clear…", Tone::Destructive, true, cx)
                        .pointer_unless(tagged == 0)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirm_clear = true;
                            cx.notify();
                        })),
                )
                .into_any_element()
        };
        let items = if tagged == 1 { "1 image or document".to_string() } else { format!("{tagged} images and documents") };
        let stored_hint = if confirm {
            format!("Remove descriptions of {items}? Describing again recreates them.")
        } else {
            format!("{items}, kept on {} only", crate::platform_text::this_computer_lower())
        };
        let reason = if !mac {
            Some("Describing photos needs macOS or Windows.")
        } else if !enabled {
            Some("Turn on AI features to describe photos.")
        } else {
            None
        };

        v_flex()
            .gap(px(10.))
            .child(kit::heading("Photo descriptions", Some("Lets AI search and Photos find images and documents by what's in them.".into()), cx))
            .child(
                kit::card(cx)
                    .child(kit::row(
                        "Describe new photos automatically",
                        "In the background after launch: objects, scenes and text",
                        self.switch("s-tags", auto, "Describe new photos automatically", unavailable, |s, v| s.ai_smart_tags = v),
                        true,
                        &ch,
                    ))
                    .child(kit::row(
                        "Describe existing photos",
                        if running { "Running · progress shows in the jobs panel" } else { "Runs as a background job" },
                        div().debug_selector(|| "s-tags-run".into()).child(describe),
                        false,
                        &ch,
                    ))
                    .child(kit::row("Stored descriptions", stored_hint, clear, false, &ch)),
            )
            .children(reason.map(|r| kit::hint(r, cx)))
            .into_any_element()
    }
}

impl Render for AiSettings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ch = crate::theme::chrome(cx);
        v_flex()
            .debug_selector(|| "ai-settings".into())
            .gap(px(22.))
            .min_w_0()
            .child(self.overview(cx))
            .child(self.model_section(cx))
            .child(self.photos_section(cx))
            .child(
                h_flex()
                    .gap(px(8.))
                    .items_start()
                    .child(crate::icons::icon(IconName::Info).flex_none().mt(px(1.)).size(px(13.)).text_color(ch.text_muted))
                    .pb(px(4.))
                    .child(kit::hint(
                        if cfg!(windows) {
                            "AI runs on your device. Search and summaries use the selected model; photo descriptions use Windows' text recognition."
                        } else {
                            "AI runs on your device. Search and summaries use the selected model; photo descriptions use macOS Vision."
                        },
                        cx,
                    ).flex_1().min_w_0()),
            )
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.2} GB", bytes as f64 / 1e9)
    } else {
        format!("{:.1} MB", bytes as f64 / 1e6)
    }
}

/// Remaining time, rounded to what is worth reading.
fn format_eta(seconds: f64) -> String {
    let seconds = seconds.ceil() as u64;
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3600 => format!("{} min", seconds.div_ceil(60)),
        _ => format!("{} h {} min", seconds / 3600, seconds % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::WindowExt;
    use gpui_kit::{AnyWindowHandle, TestAppContext, VisualTestContext, size};

    fn open(cx: &mut TestAppContext) -> (tempfile::TempDir, AnyWindowHandle, Entity<Workspace>, Entity<AiSettings>) {
        let dir = tempfile::tempdir().unwrap();
        let (window, ws) = crate::tests::setup(dir.path(), cx);
        let view = cx
            .update_window(window, |_, window, cx| {
                let view = cx.new(|cx| AiSettings::new(ws.clone(), "llama-server".into(), "mlx_lm.server".into(), None, window, cx));
                let content = view.clone();
                window.open_dialog(cx, move |dialog, _, _| dialog.w(px(540.)).child(content.clone()));
                view
            })
            .unwrap();
        (dir, window, ws, view)
    }

    fn click(cx: &mut VisualTestContext, selector: &'static str) {
        let at = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("missing {selector}"))
            .center();
        cx.simulate_click(at, gpui_kit::Modifiers::default());
        cx.run_until_parked();
    }

    // The Apple Intelligence tab exists only on macOS.
    #[cfg(target_os = "macos")]
    #[gpui_kit::test]
    fn progressive_setup_and_provider_switching(cx: &mut TestAppContext) {
        let (_dir, window, _ws, models) = open(cx);
        let _guard = crate::tests::config_lock();
        let mut cx = VisualTestContext::from_window(window, cx);
        cx.simulate_resize(size(px(900.), px(1000.)));
        cx.run_until_parked();
        assert!(cx.debug_bounds("model-settings").unwrap().size.height < px(200.));
        assert!(cx.debug_bounds("model-runtime-card").is_none());
        click(&mut cx, "model-provider-local");
        assert!(cx.debug_bounds("model-runtime-card").is_some());
        assert!(cx.debug_bounds("model-download-form").is_none());
        assert!(cx.debug_bounds("model-runtime-field").is_none());
        click(&mut cx, "model-hf-toggle");
        assert!(cx.debug_bounds("model-download-form").is_some());
        click(&mut cx, "model-hf-toggle");
        click(&mut cx, "model-runtime-options");
        assert!(cx.debug_bounds("model-runtime-field").is_some());
        click(&mut cx, "model-provider-apple");
        assert!(cx.debug_bounds("model-runtime-card").is_none());
        assert_eq!(cx.read(|cx| models.read(cx).provider), Provider::Apple);
    }

    // The Apple Intelligence tab exists only on macOS.
    #[cfg(target_os = "macos")]
    #[gpui_kit::test]
    fn download_progress_is_visible_and_cancellable(cx: &mut TestAppContext) {
        let (_dir, window, _ws, models) = open(cx);
        let _guard = crate::tests::config_lock();
        let cancel = Arc::new(AtomicBool::new(false));
        cx.update(|cx| {
            models.update(cx, |view, cx| {
                view.provider = Provider::Local(ModelFormat::Gguf);
                view.cancel = Some(cancel.clone());
                view.message = "Downloading model".into();
                view.transferred = Some((2_000_000_000, Some(4_000_000_000)));
                view.downloading = rile_services::local_model::catalog(ModelFormat::Gguf).nth(1).map(Download::of);
                cx.notify();
            })
        });
        let repo = rile_services::local_model::catalog(ModelFormat::Gguf).nth(1).unwrap().repo;
        let mut cx = VisualTestContext::from_window(window, cx);
        for width in [900., 650.] {
            cx.simulate_resize(size(px(width), px(1000.)));
            cx.run_until_parked();
            let bar = cx
                .debug_bounds("model-download-progress")
                .expect("visible download progress");
            let panel = cx.debug_bounds("model-settings").unwrap();
            assert!(bar.size.width > px(200.), "{bar:?}");
            assert!(bar.origin.x >= panel.origin.x && bar.right() <= panel.right());
            assert_eq!(bar.size.height, px(6.));
            // Progress replaces that model's Download button; no separate status card.
            assert!(cx.debug_bounds(Box::leak(format!("model-get-{repo}").into_boxed_str())).is_none());
            assert!(cx.debug_bounds("model-transfer-status").is_none());
        }
        click(&mut cx, "model-provider-apple");
        assert_eq!(cx.read(|cx| models.read(cx).provider), Provider::Local(ModelFormat::Gguf), "provider changes are disabled during transfer");
        click(&mut cx, "model-cancel");
        assert!(cancel.load(Ordering::Relaxed));
    }

    // The Apple Intelligence tab exists only on macOS.
    #[cfg(target_os = "macos")]
    #[gpui_kit::test]
    fn guide_controls_and_disabled_photo_actions(cx: &mut TestAppContext) {
        let (_dir, window, ws, view) = open(cx);
        let _guard = crate::tests::config_lock();
        let mut cx = VisualTestContext::from_window(window, cx);
        cx.simulate_resize(size(px(900.), px(1000.)));
        cx.run_until_parked();
        // Overview with status, provider tabs and the photo actions render.
        for selector in ["ai-overview", "ai-status", "model-apple-card", "s-tags-run", "s-tags-clear"] {
            assert!(cx.debug_bounds(selector).is_some(), "missing {selector}");
        }
        // Guide sizes: compact buttons are 27 px, tabs sit in a 4 px inset.
        assert_eq!(cx.debug_bounds("s-tags-run").unwrap().size.height, px(27.));
        let tab = cx.debug_bounds("model-provider-apple").unwrap();
        assert_eq!(tab.size.height, px(28.));
        // Nothing is stored in a fresh config, so Clear can't open the confirmation.
        click(&mut cx, "s-tags-clear");
        assert!(!cx.read(|cx| view.read(cx).confirm_clear));
        assert!(cx.debug_bounds("s-tags-clear-confirm").is_none());
        // With AI off, Describe All doesn't start a job.
        cx.update(|_, cx| ws.update(cx, |ws, cx| { ws.settings.ai_enabled = false; cx.notify(); }));
        cx.run_until_parked();
        click(&mut cx, "s-tags-run");
        assert!(cx.read(|cx| ws.read(cx).photo_job.is_none()));
    }

    #[gpui_kit::test]
    fn picks_a_downloaded_model(cx: &mut TestAppContext) {
        let (dir, window, ws, view) = open(cx);
        let _guard = crate::tests::config_lock();
        let models = dir.path().join("models/org/chat");
        std::fs::create_dir_all(&models).unwrap();
        let (first, second) = (models.join("a-Q4_K_M.gguf"), models.join("b-Q8_0.gguf"));
        for path in [&first, &second] {
            std::fs::write(path, b"GGUFtest").unwrap();
        }
        let installed = rile_services::local_model::installed_in(&dir.path().join("models"));
        assert_eq!(installed.len(), 2);
        // Let the constructor's own scan finish so it can't replace this list.
        cx.run_until_parked();
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.provider = Provider::Local(ModelFormat::Gguf);
                view.installed = installed;
                cx.notify();
            })
        });
        let mut cx = VisualTestContext::from_window(window, cx);
        cx.simulate_resize(size(px(900.), px(1000.)));
        cx.run_until_parked();
        assert!(cx.debug_bounds("model-installed-list").is_some());
        // The first local model needs no confirmation: nothing is unloaded.
        click(&mut cx, "model-installed-1");
        assert_eq!(cx.read(|cx| ws.read(cx).settings.ai_model_path.clone()), Some(second.clone()));
        // Replacing it asks first; cancelling keeps the loaded model.
        click(&mut cx, "model-installed-0");
        assert!(cx.debug_bounds("choose-0").is_some(), "switch confirmation");
        click(&mut cx, "choose-cancel");
        assert_eq!(cx.read(|cx| ws.read(cx).settings.ai_model_path.clone()), Some(second.clone()));
        click(&mut cx, "model-installed-0");
        click(&mut cx, "choose-0");
        assert_eq!(cx.read(|cx| ws.read(cx).settings.ai_model_path.clone()), Some(first.clone()));
        // Choosing the model already in use doesn't ask.
        click(&mut cx, "model-installed-0");
        assert!(cx.debug_bounds("choose-0").is_none());
        // A model removed since the scan reports an error and keeps the current one.
        std::fs::remove_file(&second).unwrap();
        click(&mut cx, "model-installed-1");
        assert_eq!(cx.read(|cx| ws.read(cx).settings.ai_model_path.clone()), Some(first));
        assert!(cx.read(|cx| view.read(cx).error));
    }

    #[gpui_kit::test]
    fn catalog_offers_downloads_and_lists_installed_entries(cx: &mut TestAppContext) {
        let (dir, window, ws, view) = open(cx);
        let _guard = crate::tests::config_lock();
        let [small, recommended] = [0, 1].map(|i| rile_services::local_model::catalog(ModelFormat::Gguf).nth(i).unwrap());
        assert!(recommended.recommended);
        // In this test's own folder: the config directory is shared by parallel tests.
        let path = dir.path().join("models").join(small.repo).join(small.file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"GGUFtest").unwrap();
        cx.run_until_parked();
        let installed = rile_services::local_model::installed_in(&dir.path().join("models"));
        cx.update(|cx| view.update(cx, |view, cx| {
            view.installed = installed;
            cx.notify();
        }));
        let mut cx = VisualTestContext::from_window(window, cx);
        cx.simulate_resize(size(px(900.), px(1000.)));
        click(&mut cx, "model-provider-local");
        let get = |repo: &str| Box::leak(format!("model-get-{repo}").into_boxed_str()) as &'static str;
        // The downloaded entry is a choice, the others offer Download.
        assert!(cx.debug_bounds(get(small.repo)).is_none());
        assert!(cx.debug_bounds(get(recommended.repo)).is_some());
        assert!(cx.debug_bounds("model-installed-1").is_none());
        click(&mut cx, "model-installed-0");
        assert_eq!(cx.read(|cx| ws.read(cx).settings.ai_model_path.clone()), Some(path.clone()));
        assert_eq!(cx.read(|cx| view.read(cx).display_name(&path)), small.name);
        // The custom form opens below the list.
        assert!(cx.debug_bounds("model-download-form").is_none());
        click(&mut cx, "model-hf-toggle");
        assert!(cx.debug_bounds("model-download-form").is_some());
        assert!(cx.debug_bounds("model-custom-files").is_none());
    }

    #[gpui_kit::test]
    fn deletes_a_downloaded_model(cx: &mut TestAppContext) {
        let (_dir, window, ws, view) = open(cx);
        let _guard = crate::tests::config_lock();
        let models = rile_services::local_model::models_dir().unwrap().join("org/chat");
        std::fs::create_dir_all(&models).unwrap();
        let (first, second) = (models.join("a.gguf"), models.join("b.gguf"));
        for path in [&first, &second] {
            std::fs::write(path, b"GGUFtest").unwrap();
        }
        cx.update(|cx| view.update(cx, |view, cx| view.scan_installed(cx)));
        cx.run_until_parked();
        let mut cx = VisualTestContext::from_window(window, cx);
        cx.simulate_resize(size(px(900.), px(1600.)));
        click(&mut cx, "model-provider-local");
        click(&mut cx, "model-installed-0");
        assert_eq!(cx.read(|cx| ws.read(cx).settings.ai_model_path.clone()), Some(first.clone()));
        // Deleting asks first; cancelling keeps the file.
        click(&mut cx, "model-delete-0");
        click(&mut cx, "choose-cancel");
        assert!(first.exists());
        // Deleting the model in use deselects it and removes the file.
        click(&mut cx, "model-delete-0");
        click(&mut cx, "choose-0");
        assert!(!first.exists() && second.exists());
        assert_eq!(cx.read(|cx| ws.read(cx).settings.ai_model_path.clone()), None);
        assert_eq!(cx.read(|cx| view.read(cx).installed.len()), 1);
        assert!(!cx.read(|cx| view.read(cx).error));
        // The last one goes along with its emptied folders.
        click(&mut cx, "model-delete-0");
        click(&mut cx, "choose-0");
        assert!(!models.exists());
        assert!(cx.debug_bounds("model-installed-0").is_none());
    }

    #[gpui_kit::test]
    fn mlx_tab_lists_only_mlx_models(cx: &mut TestAppContext) {
        if !rile_services::mlx_runtime::supported() {
            return;
        }
        let (dir, window, ws, view) = open(cx);
        let _guard = crate::tests::config_lock();
        let mlx = dir.path().join("models/mlx-community/Qwen3-4B-4bit");
        std::fs::create_dir_all(&mlx).unwrap();
        std::fs::write(mlx.join("config.json"), br#"{"model_type": "qwen3"}"#).unwrap();
        std::fs::write(mlx.join("model.safetensors"), [2, 0, 0, 0, 0, 0, 0, 0, b'{', b'}']).unwrap();
        let gguf = dir.path().join("models/org/chat/a.gguf");
        std::fs::create_dir_all(gguf.parent().unwrap()).unwrap();
        std::fs::write(&gguf, b"GGUFtest").unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.installed = rile_services::local_model::installed_in(&dir.path().join("models"));
                cx.notify();
            })
        });
        let mut cx = VisualTestContext::from_window(window, cx);
        cx.simulate_resize(size(px(900.), px(1000.)));
        cx.run_until_parked();
        click(&mut cx, "model-provider-mlx");
        assert_eq!(cx.read(|cx| view.read(cx).provider), Provider::Local(ModelFormat::Mlx));
        // Only the MLX folder is listed; choosing it selects the folder.
        assert!(cx.debug_bounds("model-installed-1").is_none());
        click(&mut cx, "model-installed-0");
        assert_eq!(cx.read(|cx| ws.read(cx).settings.ai_model_path.clone()), Some(mlx.clone()));
        // The GGUF tab lists its own models, and MLX comes back when reopened.
        // Leaving the loaded model's tab is a switch, confirmed first.
        click(&mut cx, "model-provider-local");
        assert_eq!(cx.read(|cx| view.read(cx).provider), Provider::Local(ModelFormat::Mlx), "tab waits for confirmation");
        click(&mut cx, "choose-0");
        assert_eq!(cx.read(|cx| view.read(cx).provider), Provider::Local(ModelFormat::Gguf));
        assert_eq!(cx.read(|cx| ws.read(cx).settings.ai_model_path.clone()), None);
        click(&mut cx, "model-installed-0");
        assert_eq!(cx.read(|cx| ws.read(cx).settings.ai_model_path.clone()), Some(gguf));
        click(&mut cx, "model-provider-mlx");
        click(&mut cx, "choose-0");
        assert_eq!(cx.read(|cx| ws.read(cx).settings.ai_model_path.clone()), Some(mlx));
        // The MLX download form asks for the repository only.
        click(&mut cx, "model-hf-toggle");
        assert!(cx.debug_bounds("model-download-form").is_some());
    }

    #[test]
    fn readable_time_left() {
        assert_eq!(format_eta(0.2), "1 s");
        assert_eq!(format_eta(59.), "59 s");
        assert_eq!(format_eta(61.), "2 min");
        assert_eq!(format_eta(3_900.), "1 h 5 min");
    }

    #[gpui_kit::test]
    fn transfer_outlives_closed_settings(cx: &mut TestAppContext) {
        let (_dir, window, ws, models) = open(cx);
        let _guard = crate::tests::config_lock();
        let cancel = Arc::new(AtomicBool::new(false));
        cx.update(|cx| {
            models.update(cx, |view, cx| {
                view.provider = Provider::Local(ModelFormat::Gguf);
                view.downloading = Some(Download { repo: "org/chat".into(), file: "chat.gguf".into() });
                view.cancel = Some(cancel.clone());
                view.message = "Downloading model".into();
                view.transferred = Some((1_000_000_000, Some(4_000_000_000)));
                view.speed = Some(25_000_000.);
                view.hold("Download AI model", cx);
                view.sync_job(cx);
                cx.notify();
            })
        });
        let mut cx = VisualTestContext::from_window(window, cx);
        cx.run_until_parked();
        assert!(cx.debug_bounds("model-download-progress").is_some());
        // Closing Settings drops the dialog's handle but not the transfer.
        let weak = models.downgrade();
        drop(models);
        cx.update(|window, cx| window.close_dialog(cx));
        cx.run_until_parked();
        assert!(!cancel.load(Ordering::Relaxed), "closing Settings must not cancel the download");
        let running = cx.update(|_, cx| AiSettings::running(&ws, cx)).expect("transfer view kept");
        assert_eq!(Some(running.clone()), weak.upgrade());
        cx.read(|cx| {
            let ws = ws.read(cx);
            assert_eq!(ws.ops.len(), 1);
            let op = &ws.ops[0];
            assert_eq!(op.label.as_ref(), "Download AI model");
            assert!(Arc::ptr_eq(&op.cancel, &cancel));
            assert!(op.backgrounded);
            let status = crate::jobs::op_status(op);
            assert_eq!(status.overall, Some(0.25));
            assert!(!status.totals.is_empty());
        });
        // Finishing releases it.
        cx.update(|window, cx| running.update(cx, |view, cx| view.finish(false, window, cx)));
        assert!(cx.update(|_, cx| AiSettings::running(&ws, cx)).is_none());
        cx.read(|cx| {
            let ws = ws.read(cx);
            assert!(ws.ops.is_empty());
            assert_eq!(ws.finished_jobs.len(), 1);
            assert!(ws.finished_jobs[0].error.is_none());
        });
    }

    #[gpui_kit::test]
    fn background_transfer_reports_cancellation_and_errors(cx: &mut TestAppContext) {
        let (_dir, window, ws, models) = open(cx);
        let _guard = crate::tests::config_lock();
        for failed in [false, true] {
            let cancel = Arc::new(AtomicBool::new(false));
            cx.update_window(window, |_, window, cx| {
                models.update(cx, |view, cx| {
                    view.cancel = Some(cancel.clone());
                    view.error = failed;
                    view.message = "Download failed".into();
                    view.hold("Download AI model", cx);
                    let id = view.job.unwrap();
                    assert_eq!(crate::jobs::op_status(&view.ws.read(cx).ops[0]).overall, None);
                    if !failed {
                        view.ws.update(cx, |ws, _| ws.cancel_op(id));
                        assert!(cancel.load(Ordering::Relaxed));
                    }
                    view.finish(false, window, cx);
                });
                let ws = ws.read(cx);
                assert!(ws.ops.is_empty());
                assert_eq!(ws.finished_jobs[0].error.as_deref(), Some(if failed { "Download failed" } else { "Cancelled" }));
            }).unwrap();
        }
    }

    #[gpui_kit::test]
    fn shows_speed_and_time_left(cx: &mut TestAppContext) {
        let (_dir, _window, _ws, models) = open(cx);
        let _guard = crate::tests::config_lock();
        let detail = cx.update(|cx| {
            models.update(cx, |view, _| {
                view.cancel = Some(Arc::new(AtomicBool::new(false)));
                let start = std::time::Instant::now();
                view.record_at(0, Some(4_000_000_000), start);
                assert_eq!(view.transfer_detail().unwrap(), "0.0 MB / 4.00 GB · 0%", "no speed before a second sample");
                view.record_at(1_000_000_000, Some(4_000_000_000), start + std::time::Duration::from_secs(40));
                let first = view.transfer_detail().unwrap();
                // A slower stretch moves the smoothed speed only partly.
                view.record_at(1_500_000_000, Some(4_000_000_000), start + std::time::Duration::from_secs(80));
                assert!((view.speed.unwrap() - 21_250_000.).abs() < 1., "{:?}", view.speed);
                view.clear_transfer();
                assert!(view.transfer_detail().is_none() && view.speed.is_none());
                first
            })
        });
        assert_eq!(detail, "1.00 GB / 4.00 GB · 25% · 25.0 MB/s · 2 min left");
    }

    #[test]
    fn readable_download_sizes() {
        assert_eq!(format_bytes(4_480_300_000), "4.48 GB");
        assert_eq!(format_bytes(16_500_000), "16.5 MB");
    }
}
