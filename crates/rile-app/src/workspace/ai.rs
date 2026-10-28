//! On-device AI (Apple Foundation Models or a local GGUF/MLX model):
//! natural-language "intelligent search" from the search bar, and
//! summaries from the context menu.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use gpui_kit::component::{WindowExt, h_flex, notification::Notification, spinner::Spinner, text::TextView, Sizable, v_flex};
use gpui_kit::{
    AppContext, Context, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Task, Window, div, prelude::FluentBuilder, px,
};
use rile_services::search::{ContentMatch, KindFilter, NameMatch, SearchQuery};

use super::Workspace;
use crate::actions::*;

/// A reply without the reasoning that thinking models (Qwen3, LFM2.5) put in a
/// leading `<think>…</think>` block. An unclosed block (some models open one
/// and call a tool right away) only loses its tag.
pub(crate) fn visible(reply: &str) -> &str {
    let Some(rest) = reply.trim_start().strip_prefix("<think>") else {
        return reply;
    };
    rest.split_once("</think>").map_or(rest, |(_, after)| after).trim_start()
}

/// The reasoning in a reply's leading `<think>` block, and whether the block
/// is closed yet.
pub(crate) fn thinking(reply: &str) -> Option<(&str, bool)> {
    let rest = reply.trim_start().strip_prefix("<think>")?;
    Some(match rest.split_once("</think>") {
        Some((thought, _)) => (thought.trim(), true),
        None => (rest.trim(), false),
    })
}

/// Thin, platform-gated wrapper over the model.
pub(super) mod backend {
    pub type Stream = smol::channel::Receiver<Result<String, String>>;

    pub fn availability() -> Result<(), String> {
        let settings = rile_core::settings::Settings::load();
        if let Some(path) = settings.ai_model_path {
            return rile_services::local_model::validate(&path).map_err(|e| e.to_string());
        }
        apple_availability()
    }

    /// The selected local model and the runtime executable that runs it.
    pub fn local_target(settings: &rile_core::settings::Settings) -> Option<(std::path::PathBuf, String)> {
        let path = settings.ai_model_path.clone()?;
        let runtime = match rile_services::local_model::format(&path) {
            rile_services::local_model::ModelFormat::Mlx => settings.ai_mlx_server.clone(),
            rile_services::local_model::ModelFormat::Gguf => rile_services::model_runtime::resolve(&settings.ai_llama_server),
        };
        Some((path, runtime))
    }

    pub fn stream(prompt: &str, instructions: &str, spotlight: bool) -> Result<Stream, String> {
        let settings = rile_core::settings::Settings::load();
        let Some((path, runtime)) = local_target(&settings) else { return apple_stream(prompt, instructions, spotlight); };
        rile_services::local_model::validate(&path).map_err(|e| e.to_string())?;
        let (tx, rx) = smol::channel::unbounded();
        let (prompt, instructions) = (prompt.to_owned(), instructions.to_owned());
        std::thread::spawn(move || {
            let result = rile_services::local_model::generate(&path, &runtime, &prompt, &instructions, settings.ai_thinking, |text| {
                if text.is_empty() { !tx.is_closed() } else { tx.send_blocking(Ok(text)).is_ok() }
            });
            if let Err(error) = result { let _ = tx.send_blocking(Err(format!("{error:#}"))); }
        });
        Ok(rx)
    }

    #[cfg(all(feature = "ai", target_os = "macos", target_arch = "aarch64"))]
    fn apple_availability() -> Result<(), String> {
        use spotlight_llm::Availability::*;
        match spotlight_llm::availability() {
            Available => Ok(()),
            DeviceNotEligible => Err("This Mac can't run Apple Intelligence.".into()),
            AppleIntelligenceNotEnabled => Err("Turn on Apple Intelligence in System Settings to use AI features.".into()),
            ModelNotReady => Err("The on-device model is still downloading. Try again later.".into()),
            Unknown => Err("The on-device model is unavailable.".into()),
        }
    }

    /// Streams snapshots of the whole response so far.
    #[cfg(all(feature = "ai", target_os = "macos", target_arch = "aarch64"))]
    fn apple_stream(prompt: &str, instructions: &str, spotlight: bool) -> Result<Stream, String> {
        use std::sync::OnceLock;
        static SESSION: OnceLock<spotlight_llm::SpotlightLlmSession> = OnceLock::new();
        apple_availability()?;
        let session = SESSION.get_or_init(spotlight_llm::SpotlightLlmSession::new);
        let opts = spotlight_llm::RequestOptions::new().instructions(instructions).spotlight_tool(spotlight).temperature(0.2);
        let mut rx = session.respond_stream_once(prompt, &opts).map_err(|e| e.to_string())?;
        let (tx, out) = smol::channel::unbounded();
        // Bridge tokio's receiver onto a plain thread so any executor can await it.
        std::thread::spawn(move || {
            while let Some(item) = rx.blocking_recv() {
                if tx.send_blocking(item.map_err(|e| e.to_string())).is_err() {
                    break;
                }
            }
        });
        Ok(out)
    }

    /// Image analysis works on any Mac (Vision) or Windows PC (OCR only),
    /// without Apple Intelligence.
    pub fn vision_available() -> Result<(), String> {
        if cfg!(any(target_os = "macos", windows)) { Ok(()) } else { Err("Describing images needs macOS or Windows.".into()) }
    }

    #[cfg(not(all(feature = "ai", target_os = "macos", target_arch = "aarch64")))]
    fn apple_availability() -> Result<(), String> {
        Err(if cfg!(target_os = "macos") {
            "AI features need macOS 26+ on Apple Silicon (built with the `ai` feature).".into()
        } else {
            "Choose a local model in Settings → AI to use AI features.".into()
        })
    }

    #[cfg(not(all(feature = "ai", target_os = "macos", target_arch = "aarch64")))]
    fn apple_stream(_: &str, _: &str, _: bool) -> Result<Stream, String> {
        Err(apple_availability().unwrap_err())
    }
}

const SEARCH_INSTRUCTIONS: &str = r#"You convert a user's request for finding files into a JSON search filter.
Reply with ONLY a JSON object, no prose, using these optional keys:
"name": text the file name contains (or a glob like "*.pdf" when name_mode is "glob"),
"name_mode": "contains" | "glob" | "regex",
"extensions": list of lowercase extensions without dots, e.g. ["jpg","png"],
"content": text that must appear inside the file,
"modified_within_days": number,
"min_size_mb": number, "max_size_mb": number,
"kind": "files" | "folders" | "any".
Map concepts to extensions: photos/images -> jpg,jpeg,png,heic,gif,webp; videos -> mp4,mov,mkv; music -> mp3,m4a,flac,wav;
documents -> pdf,doc,docx,pages,txt,md; spreadsheets -> xlsx,xls,csv,numbers; presentations -> pptx,key; code -> rs,js,ts,py,go,swift,c,cpp.
Leave out keys that the request doesn't mention."#;

const SUMMARY_INSTRUCTIONS: &str =
    "You summarize files for a file manager. Be concise: a one-sentence overview, then up to 5 short bullet points.";

impl Workspace {
    /// Whether the configured model (Apple Intelligence or the selected local
    /// model) can run. Blocking; call it off the main thread.
    pub fn ai_availability() -> Result<(), String> {
        backend::availability()
    }

    /// Keeps the selected local model loaded in memory while AI is on, so
    /// requests don't wait for it; switching models unloads the previous one.
    pub(crate) fn sync_local_model(&self) {
        if cfg!(test) {
            return;
        }
        let target = self.settings.ai_enabled.then(|| backend::local_target(&self.settings)).flatten();
        rile_services::local_model::keep_loaded(target);
    }

    pub(crate) fn ai_summarize(&mut self, _: &AiSummarize, window: &mut Window, cx: &mut Context<Self>) {
        self.open_summary(window, cx);
    }

    /// Searches directly in the active pane, with its normal progress,
    /// cancellation and error UI; no assistant dialog is needed.
    pub(crate) fn ai_from_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
        use rile_services::search;

        if !self.settings.ai_enabled {
            window.push_notification(Notification::info("AI features are turned off in Settings"), cx);
            return;
        }
        if self.pane().read(cx).conn().is_some() {
            window.push_notification(Notification::info("Intelligent search is only available for local files"), cx);
            return;
        }
        let request = self.search_input.read(cx).value().trim().to_string();
        if request.is_empty() {
            self.focus_search(&FocusSearch, window, cx);
            return;
        }
        let root = self.current_dir(cx).or_else(dirs_home).unwrap_or_else(|| PathBuf::from("/"));
        let base = self.default_query(root.clone());
        let label = format!("AI: \"{request}\"").into();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        self.search_input.update(cx, |i, cx| i.set_value("", window, cx));
        self.pane().update(cx, |pane, cx| {
            pane.start_search(label, cancel, cx, move |push, scanned| {
                let found_files_flag = std::sync::atomic::AtomicBool::new(false);
                let found = super::ai_agent::smart_search(&request, &root, &base, &worker_cancel, &|_| {}, &|batch| {
                    if batch.iter().any(|entry| !entry.is_dir) {
                        found_files_flag.store(true, Ordering::Relaxed);
                    }
                    push(batch);
                })?;
                if worker_cancel.load(Ordering::Relaxed) {
                    return Ok(());
                }
                if found == 0 {
                    let reply = super::ai_agent::complete(&format!("Request: {request}"), SEARCH_INSTRUCTIONS)?;
                    if worker_cancel.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    let query = parse_filter(&reply, base).map_err(anyhow::Error::msg)?;
                    search::validate(&query)?;
                    let batch = Mutex::new((Vec::new(), std::time::Instant::now()));
                    let stats = search::search(&query, &worker_cancel, |hit| {
                        let mut batch = batch.lock().unwrap();
                        batch.0.push(hit.entry);
                        if batch.0.len() >= 256 || batch.1.elapsed() > Duration::from_millis(50) {
                            push(std::mem::take(&mut batch.0));
                            batch.1 = std::time::Instant::now();
                        }
                    })?;
                    let rest = batch.into_inner().unwrap().0;
                    if !rest.is_empty() {
                        push(rest);
                    }
                    scanned(stats.scanned as u64);
                } else {
                    if found_files_flag.load(Ordering::Relaxed) {
                        rile_services::rewards::record_event(rile_services::rewards::Event::AiFound);
                    }
                }
                if !worker_cancel.load(Ordering::Relaxed) {
                    rile_services::rewards::record_event(rile_services::rewards::Event::Search);
                }
                Ok(())
            });
            pane.focus(window, cx);
        });
    }

    /// Describes the selected images and documents (folders: what's in
    /// them) so AI search and the photo library can find them by content.
    pub(crate) fn smart_tag(&mut self, _: &SmartTag, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self.selection(cx);
        if paths.is_empty() || self.pane().read(cx).conn().is_some() {
            return;
        }
        self.start_tagging(paths, true, window, cx);
    }

    /// Runs smart tagging over `paths` as a background job.
    pub fn start_tagging(&mut self, paths: Vec<PathBuf>, use_model: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(e) = backend::vision_available() {
            window.push_notification(Notification::info(e), cx);
            return;
        }
        let label = format!("Describing {}", super::item_count(paths.len()));
        let model_ok = use_model && backend::availability().is_ok();
        self.start_op(label, window, cx, move |cancel, report| {
            let files = super::ai_agent::collect_taggable(&paths, 5_000, cancel);
            let mut p = rile_core::ops::Progress { files_total: files.len(), items_total: files.len(), ..Default::default() };
            report(p.clone());
            let mut failed = 0;
            for f in &files {
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    anyhow::bail!("Cancelled");
                }
                p.current = f.file_name().map(|n| n.to_string_lossy().into());
                report(p.clone());
                if super::ai_agent::tag_file(f, model_ok).is_err() {
                    failed += 1;
                }
                p.files_done += 1;
                p.items_done += 1;
                report(p.clone());
            }
            rile_services::tags::save().ok();
            if failed > 0 && failed == files.len() {
                anyhow::bail!("Couldn't describe any of the files");
            }
            Ok(Vec::new())
        });
    }

    /// Starts the background job that finds every photo and describes the
    /// ones without fresh smart tags (Vision labels + text), so the photo
    /// library and AI search find them by content. `manual`: the user asked
    /// (otherwise only when "Describe photos automatically" is on).
    pub fn start_photo_job(&mut self, manual: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !cfg!(any(target_os = "macos", windows)) || !self.settings.ai_enabled || (!manual && !self.settings.ai_smart_tags) {
            return;
        }
        if self.photo_job.is_some_and(|id| self.ops.iter().any(|o| o.id == id)) {
            return;
        }
        self.photo_job = Some(self.next_op_id);
        self.start_op("Describing photos", window, cx, |cancel, report| {
            // Run at utility priority so describing never competes with the
            // UI or file operations; the pool thread gets its class back after.
            let _qos = LowPriority::enter();
            describe_photos(cancel, report)
        });
        cx.notify();
    }

    /// Summarizes the selection (or the current folder) in a read-only
    /// dialog; only reachable from the context menu.
    fn open_summary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settings.ai_enabled {
            window.push_notification(Notification::info("AI features are turned off in Settings"), cx);
            return;
        }
        let mut selection = self.selection(cx);
        // Summarizing with nothing selected describes the folder itself.
        if selection.is_empty() {
            selection.extend(self.current_dir(cx));
        }
        if selection.is_empty() {
            return;
        }
        let view = cx.new(|cx| AiView::new(selection, cx));
        view.update(cx, |v, cx| v.start(cx));
        window.open_dialog(cx, move |dialog, _window, cx| {
            let glyph = crate::icons::intelligence_glyph(17., crate::theme::chrome(cx).accent, view.entity_id(), cx);
            crate::views::kit::dialog_with_glyph(
                dialog,
                glyph,
                "AI Summary",
                Some("On this Mac with Apple's Foundation Models".into()),
                600.,
                cx,
            )
            .child(view.clone())
        });
    }
}

pub struct AiView {
    selection: Vec<PathBuf>,
    output: String,
    running: bool,
    error: Option<SharedString>,
    /// None while the (blocking) availability check runs in the background.
    availability: Option<Result<(), String>>,
    _task: Option<Task<()>>,
    _availability_task: Task<()>,
}

impl AiView {
    fn new(selection: Vec<PathBuf>, cx: &mut Context<Self>) -> Self {
        let check = cx.background_spawn(async { backend::availability() });
        let availability_task = cx.spawn(async move |this, cx| {
            let result = check.await;
            this.update(cx, |this, cx| {
                this.availability = Some(result);
                cx.notify();
            })
            .ok();
        });
        Self {
            selection,
            output: String::new(),
            running: false,
            error: None,
            availability: None,
            _task: None,
            _availability_task: availability_task,
        }
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        self.output.clear();
        self.error = None;
        self.running = true;
        let selection = self.selection.clone();
        // Reading files and starting the model (first use loads it) both
        // block, so they run off the UI thread.
        let start = cx.background_spawn(async move {
            let prompt = super::ai_agent::folder_summary_prompt(&selection, "")?;
            backend::stream(&prompt, SUMMARY_INSTRUCTIONS, false)
        });
        self._task = Some(cx.spawn(async move |this, cx| {
            let rx = match start.await {
                Ok(rx) => rx,
                Err(e) => {
                    this.update(cx, |this, cx| {
                        this.running = false;
                        this.error = Some(e.into());
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            while let Ok(item) = rx.recv().await {
                let ok = this.update(cx, |this, cx| {
                    match item {
                        Ok(snapshot) => this.output = visible(&snapshot).to_string(),
                        Err(e) => this.error = Some(e.into()),
                    }
                    cx.notify();
                });
                if ok.is_err() {
                    return;
                }
            }
            this.update(cx, |this, cx| {
                this.running = false;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

fn dirs_home() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Finds every photo, then describes those without fresh smart tags,
/// newest first. Progress counts the photos that need describing.
fn describe_photos(cancel: &std::sync::atomic::AtomicBool, report: &mut dyn FnMut(rile_core::ops::Progress)) -> anyhow::Result<Vec<PathBuf>> {
    use std::sync::atomic::Ordering;
    let mut found = Vec::new();
    rile_services::gallery::find_images(cancel, |batch| found.extend(batch));
    if cancel.load(Ordering::Relaxed) {
        anyhow::bail!("Cancelled");
    }
    let secs = |t: Option<SystemTime>| t.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    let mut todo: Vec<(PathBuf, SystemTime)> = found
        .into_iter()
        .filter_map(|p| {
            let meta = std::fs::metadata(&p).ok()?;
            let modified = meta.modified().ok();
            (!rile_services::tags::is_fresh(&p, secs(modified), meta.len())).then(|| (p, modified.unwrap_or(SystemTime::UNIX_EPOCH)))
        })
        .collect();
    todo.sort_by(|a, b| b.1.cmp(&a.1));
    let mut p = rile_core::ops::Progress { files_total: todo.len(), ..Default::default() };
    // The first image also loads Vision's models (can take a minute).
    p.current = Some("Preparing on-device image analysis…".into());
    report(p.clone());
    for (i, (path, _)) in todo.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            rile_services::tags::save().ok();
            anyhow::bail!("Cancelled");
        }
        if i > 0 {
            p.current = path.file_name().map(|n| n.to_string_lossy().into());
            report(p.clone());
        }
        // Files Vision can't read (e.g. SVG) get an empty record, so they
        // aren't retried on every launch until they change.
        if super::ai_agent::tag_file(path, false).is_err()
            && let Ok(meta) = std::fs::metadata(path)
        {
            rile_services::tags::put(
                path,
                rile_services::tags::TagRecord {
                    tags: Vec::new(),
                    description: String::new(),
                    text: String::new(),
                    mtime: secs(meta.modified().ok()),
                    size: meta.len(),
                    kind: rile_services::tags::TagKind::Image,
                },
            );
        }
        p.files_done = i + 1;
        report(p.clone());
        if i % 100 == 99 {
            rile_services::tags::save().ok();
        }
    }
    rile_services::tags::save().ok();
    Ok(Vec::new())
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    /// `<pthread/qos.h>`; not exported by the libc crate.
    fn qos_class_self() -> libc::qos_class_t;
}

/// Lowers the current thread's QoS class until dropped (macOS).
struct LowPriority(#[cfg(target_os = "macos")] libc::qos_class_t);

impl LowPriority {
    fn enter() -> Self {
        #[cfg(target_os = "macos")]
        // SAFETY: reads and changes only the calling thread's QoS class.
        unsafe {
            let previous = qos_class_self();
            libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
            Self(previous)
        }
        #[cfg(not(target_os = "macos"))]
        Self()
    }
}

impl Drop for LowPriority {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        // SAFETY: restores the class read in `enter` on the same thread.
        unsafe {
            libc::pthread_set_qos_class_self_np(self.0, 0);
        }
    }
}

/// Turns the model's JSON reply into a search query.
fn parse_filter(reply: &str, mut q: SearchQuery) -> Result<SearchQuery, String> {
    let start = reply.find('{').ok_or("no JSON object")?;
    let end = reply.rfind('}').ok_or("no JSON object")?;
    let v: serde_json::Value = serde_json::from_str(&reply[start..=end]).map_err(|e| e.to_string())?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::trim).filter(|x| !x.is_empty()).map(String::from);
    let n = |k: &str| v.get(k).and_then(|x| x.as_f64());
    if let Some(name) = s("name") {
        q.name = match s("name_mode").as_deref() {
            Some("glob") => NameMatch::Glob(name),
            Some("regex") => NameMatch::Regex(name),
            _ => NameMatch::Substring(name),
        };
    }
    if let Some(exts) = v.get("extensions").and_then(|x| x.as_array()) {
        q.extensions = exts
            .iter()
            .filter_map(|e| e.as_str())
            .map(|e| e.trim_start_matches('.').to_lowercase())
            .collect();
    }
    if let Some(c) = s("content") {
        q.content = Some(ContentMatch { pattern: c, regex: false, case_sensitive: false });
    }
    if let Some(d) = n("modified_within_days") {
        q.modified_after = Some(SystemTime::now() - Duration::from_secs_f64(d.max(0.) * 86_400.));
    }
    q.min_size = n("min_size_mb").map(|m| (m * 1e6) as u64);
    q.max_size = n("max_size_mb").map(|m| (m * 1e6) as u64);
    q.kind = match s("kind").as_deref() {
        Some("files") => KindFilter::Files,
        Some("folders") | Some("dirs") => KindFilter::Dirs,
        _ => KindFilter::Any,
    };
    Ok(q)
}

impl Render for AiView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::views::kit;
        let ch = crate::theme::chrome(cx);
        let subject: SharedString = match self.selection.as_slice() {
            [one] => format!("Summary of \u{201c}{}\u{201d}", one.file_name().map(|n| n.to_string_lossy()).unwrap_or_else(|| one.to_string_lossy())).into(),
            many => format!("Summary of {} items", many.len()).into(),
        };
        v_flex()
            .gap(px(14.))
            .text_size(px(13.))
            .child(kit::hint(subject, cx))
            .when_some(self.availability.clone().and_then(Result::err), |d, e| d.child(kit::notice(e, false, cx)))
            .when_some(self.error.clone(), |d, e| d.child(kit::notice(e, true, cx)))
            .when(self.running && self.output.is_empty(), |d| {
                d.child(
                    h_flex()
                        .id("ai-summary-loading")
                        .gap(px(8.))
                        .text_color(ch.text_muted)
                        .child(Spinner::new().xsmall())
                        .child("Summarizing\u{2026}"),
                )
            })
            .when(!self.output.is_empty(), |d| {
                d.child(
                    kit::card(cx).child(
                        div()
                            .id("ai-output")
                            .max_h(px(320.))
                            .overflow_y_scroll()
                            .p(px(14.))
                            .text_color(ch.text)
                            .line_height(px(20.))
                            // The model answers in Markdown; streamed snapshots often
                            // start with blank lines, which would show as a gap.
                            .child(TextView::markdown("ai-output-markdown", self.output.trim().to_string()).selectable(true)),
                    ),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_reasoning_from_replies() {
        assert_eq!(super::thinking("<think>\nHmm.\n</think>\nHi"), Some(("Hmm.", true)));
        assert_eq!(super::thinking("<think> Still"), Some(("Still", false)));
        assert_eq!(super::thinking("Hi"), None);
        assert_eq!(super::visible("<think>Hmm.</think>\nHi"), "Hi");
        assert_eq!(super::visible("Hi"), "Hi");
    }

    #[test]
    fn parses_model_filter() {
        let base = SearchQuery::new("/tmp");
        let q = parse_filter(
            "Sure! {\"extensions\": [\".PDF\"], \"content\": \"invoice\", \"modified_within_days\": 30, \"kind\": \"files\"}",
            base,
        )
        .unwrap();
        assert_eq!(q.extensions, ["pdf"]);
        assert_eq!(q.content.unwrap().pattern, "invoice");
        assert!(q.modified_after.is_some());
        assert_eq!(q.kind, KindFilter::Files);
        assert!(parse_filter("nope", SearchQuery::new("/")).is_err());
    }
}
