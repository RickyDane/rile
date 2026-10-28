//! Explicit recursive search and the advanced search form.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::component::{
    WindowExt, h_flex,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use gpui_kit::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window, div, px,
};
use rile_core::Entry;
use rile_services::search::{self, ContentMatch, KindFilter, NameMatch, SearchQuery};

use super::Workspace;
use crate::actions::*;

impl Workspace {
    pub(super) fn on_search_input_event(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = input.read(cx).value().to_string();
        match event {
            InputEvent::PressEnter { shift: true, .. } => self.ai_from_search(window, cx),
            InputEvent::PressEnter { shift: false, .. } => {
                if let Some(conn) = self.pane().read(cx).conn() {
                    let Some(root) = self.pane().read(cx).dir_path() else { return };
                    if !value.trim().is_empty() {
                        self.run_remote_search(conn, root, value.trim().to_string(), window, cx);
                        let focus = self.pane().read(cx).focus_handle.clone();
                        focus.focus(window, cx);
                    }
                    return;
                }
                let Some(root) = self.current_dir(cx) else { return };
                if value.trim().is_empty() {
                    return;
                }
                let query = SearchQuery {
                    name: NameMatch::Substring(value.trim().to_string()),
                    ..self.default_query(root)
                };
                self.run_search(query, window, cx);
                let focus = self.pane().read(cx).focus_handle.clone();
                focus.focus(window, cx);
            }
            InputEvent::Focus | InputEvent::Blur => {
                let focused = matches!(event, InputEvent::Focus);
                if self.search_focus.0 != focused {
                    self.search_focus = (focused, self.search_focus.1 + 1);
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    pub fn default_query(&self, root: PathBuf) -> SearchQuery {
        SearchQuery {
            root,
            name: NameMatch::Substring(String::new()),
            case_sensitive: false,
            content: None,
            include_hidden: self.settings.show_hidden,
            respect_gitignore: self.settings.search_respect_gitignore,
            max_depth: None,
            kind: KindFilter::Any,
            extensions: Vec::new(),
            min_size: None,
            max_size: None,
            modified_after: None,
            modified_before: None,
            max_results: self.settings.search_max_results,
        }
    }

    /// Runs a recursive, index-free search in the active pane.
    pub fn run_search(&mut self, query: SearchQuery, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(e) = search::validate(&query) {
            window.push_notification(gpui_kit::component::notification::Notification::error(format!("{e:#}")), cx);
            return;
        }
        let label: SharedString = describe(&query).into();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        self.search_input.update(cx, |i, cx| i.set_value("", window, cx));
        self.pane().update(cx, |pane, cx| {
            pane.start_search(label, cancel, cx, move |push, scanned| {
                // Folders seen before answer instantly; the walk then fills in
                // the rest without repeating them.
                let remembered = rile_services::index::query(&query);
                let seen: Mutex<std::collections::HashSet<std::path::PathBuf>> =
                    Mutex::new(remembered.iter().map(|e| e.path.clone()).collect());
                if !remembered.is_empty() {
                    push(remembered);
                }
                let batch = Mutex::new((Vec::<Entry>::new(), Instant::now()));
                let stats = search::search(&query, &worker_cancel, |hit| {
                    if !seen.lock().unwrap().insert(hit.entry.path.clone()) {
                        return;
                    }
                    let mut b = batch.lock().unwrap();
                    b.0.push(hit.entry);
                    if b.0.len() >= 256 || b.1.elapsed() > Duration::from_millis(50) {
                        push(std::mem::take(&mut b.0));
                        b.1 = Instant::now();
                    }
                })?;
                let rest = std::mem::take(&mut batch.lock().unwrap().0);
                if !rest.is_empty() {
                    push(rest);
                }
                scanned(stats.scanned as u64);
                if !worker_cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    rile_services::rewards::record_event(rile_services::rewards::Event::Search);
                }
                Ok(())
            });
        });
    }

    /// Searches a server folder and its subfolders by name, listing folders
    /// breadth-first over the connection (no index on the server needed).
    pub fn run_remote_search(&mut self, conn: String, root: PathBuf, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let max = self.settings.search_max_results.max(1);
        let hidden = self.settings.show_hidden;
        let label: SharedString = format!("name contains \"{text}\" on {}", crate::remote::label(&conn)).into();
        self.search_input.update(cx, |i, cx| i.set_value("", window, cx));
        self.pane().update(cx, |pane, cx| {
            pane.start_search(label, cancel, cx, move |push, scanned| {
                remote_walk(&conn, &crate::remote::path_str(&root), &text, hidden, max, &worker_cancel, push, scanned)?;
                if !worker_cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    rile_services::rewards::record_event(rile_services::rewards::Event::Search);
                }
                Ok(())
            });
        });
    }

    /// Searches via the OS index (Spotlight, Windows Search) instead of walking.
    pub fn run_spotlight_search(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.current_dir(cx);
        let max = self.settings.search_max_results;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let label: SharedString = format!("{}: \"{text}\"", crate::platform_text::os_search()).into();
        self.search_input.update(cx, |i, cx| i.set_value("", window, cx));
        self.pane().update(cx, |pane, cx| {
            pane.start_search(label, cancel, cx, move |push, _| {
                let paths = search::spotlight_search(&text, root.as_deref(), max, &worker_cancel)?;
                push(paths.into_iter().filter_map(Entry::from_path).collect());
                if !worker_cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    rile_services::rewards::record_event(rile_services::rewards::Event::Search);
                }
                Ok(())
            });
        });
    }

    /// Leaves the header search field (Escape or a click elsewhere), handing
    /// focus back to the active pane. The typed filter is kept.
    pub(crate) fn dismiss_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search_focus.0 {
            return;
        }
        let focus = self.pane().read(cx).focus_handle.clone();
        focus.focus(window, cx);
    }

    pub(crate) fn focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.search_input.update(cx, |i, cx| {
            i.focus(window, cx);
            i.select_all(window, cx);
        });
    }

    pub(crate) fn advanced_search(&mut self, _: &AdvancedSearch, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.current_dir(cx) else {
            window.push_notification(
                gpui_kit::component::notification::Notification::info("Open a folder to search in"),
                cx,
            );
            return;
        };
        let ws = cx.entity().downgrade();
        let form = match &self.search_form {
            Some(f) => f.clone(),
            None => {
                let defaults = (self.settings.show_hidden, self.settings.search_respect_gitignore);
                let f = cx.new(|cx| SearchForm::new(ws, defaults, window, cx));
                self.search_form = Some(f.clone());
                f
            }
        };
        form.update(cx, |f, cx| {
            f.root = root;
            f.error = None;
            f.use_spotlight = self.settings.search_use_spotlight;
            cx.notify();
        });
        let focus = form.read(cx).name.clone();
        let first = std::cell::Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first.replace(false) {
                focus.update(cx, |i, cx| i.focus(window, cx));
            }
            crate::views::kit::dialog(
                dialog,
                gpui_kit::assets::IconName::SlidersHorizontal,
                "Advanced Search",
                Some("Walks the folder and its subfolders · no index".into()),
                600.,
                cx,
            )
            .child(form.clone())
        });
    }
}

fn describe(q: &SearchQuery) -> String {
    let mut parts = Vec::new();
    match &q.name {
        NameMatch::Substring(s) if !s.is_empty() => parts.push(format!("name contains \"{s}\"")),
        NameMatch::Glob(g) => parts.push(format!("name matches {g}")),
        NameMatch::Regex(r) => parts.push(format!("name =~ /{r}/")),
        _ => {}
    }
    if let Some(c) = &q.content {
        parts.push(if c.regex { format!("content =~ /{}/", c.pattern) } else { format!("content contains \"{}\"", c.pattern) });
    }
    if !q.extensions.is_empty() {
        parts.push(format!("type {}", q.extensions.join(", ")));
    }
    if q.min_size.is_some() || q.max_size.is_some() {
        parts.push("size filter".into());
    }
    if q.modified_after.is_some() {
        parts.push("recently modified".into());
    }
    let what = if parts.is_empty() { "everything".into() } else { parts.join(", ") };
    format!("Search in {}: {what}", super::display_name(&q.root))
}

#[derive(Clone, Copy, PartialEq)]
enum NameMode {
    Substring,
    Glob,
    Regex,
}

/// Advanced search dialog content. Kept alive between openings so the
/// last query is remembered.
pub struct SearchForm {
    ws: WeakEntity<Workspace>,
    pub root: PathBuf,
    pub name: Entity<InputState>,
    content: Entity<InputState>,
    extensions: Entity<InputState>,
    min_size: Entity<InputState>,
    max_size: Entity<InputState>,
    modified_days: Entity<InputState>,
    max_depth: Entity<InputState>,
    name_mode: NameMode,
    case_sensitive: bool,
    content_regex: bool,
    include_hidden: bool,
    respect_gitignore: bool,
    kind: KindFilter,
    pub use_spotlight: bool,
    pub error: Option<SharedString>,
    _subs: Vec<Subscription>,
}

impl SearchForm {
    /// Created inside a workspace update: defaults are passed in, not read from `ws`.
    fn new(ws: WeakEntity<Workspace>, (hidden, gitignore): (bool, bool), window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = |ph: &'static str, window: &mut Window, cx: &mut Context<Self>| cx.new(|cx| InputState::new(window, cx).placeholder(ph));
        let name = input("report, *.pdf or ^IMG_\\d+", window, cx);
        let content = input("Optional", window, cx);
        let extensions = input("pdf, docx", window, cx);
        let min_size = input("Any", window, cx);
        let max_size = input("Any", window, cx);
        let modified_days = input("Any", window, cx);
        let max_depth = input("Unlimited", window, cx);
        let subs = [&name, &content, &extensions, &min_size, &max_size, &modified_days, &max_depth]
            .into_iter()
            .map(|i| {
                cx.subscribe_in(i, window, |this: &mut Self, _, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        this.submit(window, cx);
                    }
                })
            })
            .collect();
        Self {
            ws,
            root: PathBuf::new(),
            name,
            content,
            extensions,
            min_size,
            max_size,
            modified_days,
            max_depth,
            name_mode: NameMode::Substring,
            case_sensitive: false,
            content_regex: false,
            include_hidden: hidden,
            respect_gitignore: gitignore,
            kind: KindFilter::Any,
            use_spotlight: false,
            error: None,
            _subs: subs,
        }
    }

    fn build_query(&self, cx: &gpui_kit::App) -> Result<SearchQuery, String> {
        let text = |i: &Entity<InputState>| i.read(cx).value().trim().to_string();
        let num = |i: &Entity<InputState>, what: &str| -> Result<Option<f64>, String> {
            let t = text(i);
            if t.is_empty() {
                return Ok(None);
            }
            t.parse::<f64>().map(Some).map_err(|_| format!("{what} must be a number"))
        };
        let name = text(&self.name);
        let ws = self.ws.upgrade().ok_or("closed")?;
        let mut q = ws.read(cx).default_query(self.root.clone());
        q.name = match self.name_mode {
            NameMode::Substring => NameMatch::Substring(name),
            NameMode::Glob => NameMatch::Glob(if name.is_empty() { "*".into() } else { name }),
            NameMode::Regex => NameMatch::Regex(name),
        };
        q.case_sensitive = self.case_sensitive;
        let content = text(&self.content);
        q.content = (!content.is_empty()).then_some(ContentMatch {
            pattern: content,
            regex: self.content_regex,
            case_sensitive: self.case_sensitive,
        });
        q.extensions = text(&self.extensions)
            .split([',', ' '])
            .map(|e| e.trim().trim_start_matches('.').to_lowercase())
            .filter(|e| !e.is_empty())
            .collect();
        q.min_size = num(&self.min_size, "Min size")?.map(|mb| (mb * 1_000_000.) as u64);
        q.max_size = num(&self.max_size, "Max size")?.map(|mb| (mb * 1_000_000.) as u64);
        q.modified_after = num(&self.modified_days, "Days")?
            .map(|d| SystemTime::now() - Duration::from_secs_f64(d * 86_400.));
        q.max_depth = num(&self.max_depth, "Max depth")?.map(|d| d as usize);
        q.include_hidden = self.include_hidden;
        q.respect_gitignore = self.respect_gitignore;
        q.kind = self.kind;
        search::validate(&q).map_err(|e| format!("{e:#}"))?;
        Ok(q)
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.use_spotlight {
            let text = self.name.read(cx).value().trim().to_string();
            if text.is_empty() {
                self.error = Some(format!("Enter a name or phrase for {}", crate::platform_text::os_search()).into());
                cx.notify();
                return;
            }
            window.close_dialog(cx);
            self.ws.update(cx, |ws, cx| ws.run_spotlight_search(text, window, cx)).ok();
            return;
        }
        match self.build_query(cx) {
            Ok(q) => {
                self.error = None;
                window.close_dialog(cx);
                self.ws.update(cx, |ws, cx| ws.run_search(q, window, cx)).ok();
            }
            Err(e) => {
                self.error = Some(e.into());
                cx.notify();
            }
        }
    }
}

impl Render for SearchForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::views::kit;
        let ch = crate::theme::chrome(cx);
        let mode = |id: &'static str, label: &'static str, mode: NameMode, this: &Self, cx: &mut Context<Self>| {
            kit::segment(id, label, this.name_mode == mode, cx).on_click(cx.listener(move |this, _, _, cx| {
                this.name_mode = mode;
                cx.notify();
            }))
        };
        let kind = |id: &'static str, label: &'static str, kind: KindFilter, this: &Self, cx: &mut Context<Self>| {
            kit::segment(id, label, this.kind == kind, cx).on_click(cx.listener(move |this, _, _, cx| {
                this.kind = kind;
                cx.notify();
            }))
        };
        let check = |id: &'static str, label: &'static str, value: bool, set: fn(&mut Self, bool), cx: &mut Context<Self>| {
            Checkbox::new(id).cursor_pointer().label(label).checked(value).on_click(cx.listener(move |this, v: &bool, _, cx| {
                set(this, *v);
                cx.notify();
            }))
        };
        v_flex()
            .gap(px(14.))
            .text_size(px(13.))
            .child(
                h_flex()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(8.))
                    .bg(ch.hover)
                    .border_1()
                    .border_color(ch.hairline)
                    .child(crate::icons::icon(gpui_kit::assets::IconName::Folder).size(px(14.)).text_color(ch.accent))
                    .child(div().min_w_0().truncate().text_size(px(12.)).text_color(ch.text_muted).child(self.root.display().to_string())),
            )
            .child(
                v_flex()
                    .gap(px(6.))
                    .child(
                        h_flex()
                            .justify_between()
                            .child(div().text_size(px(11.)).font_weight(gpui_kit::FontWeight::MEDIUM).text_color(ch.text_muted).child("Name"))
                            .child(
                                kit::segmented(cx)
                                    .child(mode("nm-sub", "Contains", NameMode::Substring, self, cx))
                                    .child(mode("nm-glob", "Glob", NameMode::Glob, self, cx))
                                    .child(mode("nm-re", "Regex", NameMode::Regex, self, cx)),
                            ),
                    )
                    .child(Input::new(&self.name)),
            )
            .child(kit::field("Text inside files", Input::new(&self.content), cx))
            .child(
                h_flex()
                    .gap(px(10.))
                    .child(kit::field("Extensions", Input::new(&self.extensions), cx).flex_1())
                    .child(kit::field("Folder depth", Input::new(&self.max_depth), cx).w(px(130.))),
            )
            .child(
                h_flex()
                    .gap(px(10.))
                    .child(kit::field("Min size (MB)", Input::new(&self.min_size), cx).flex_1())
                    .child(kit::field("Max size (MB)", Input::new(&self.max_size), cx).flex_1())
                    .child(kit::field("Changed within (days)", Input::new(&self.modified_days), cx).flex_1()),
            )
            .child(kit::field(
                "Kind",
                h_flex().child(
                    kit::segmented(cx)
                        .child(kind("k-any", "Anything", KindFilter::Any, self, cx))
                        .child(kind("k-files", "Files", KindFilter::Files, self, cx))
                        .child(kind("k-dirs", "Folders", KindFilter::Dirs, self, cx)),
                ),
                cx,
            ))
            .child(
                kit::card(cx).p(px(12.)).child(
                    h_flex()
                        .flex_wrap()
                        .gap_x(px(18.))
                        .gap_y(px(10.))
                        .child(check("c-case", "Case sensitive", self.case_sensitive, |t, v| t.case_sensitive = v, cx))
                        .child(check("c-cre", "Content is regex", self.content_regex, |t, v| t.content_regex = v, cx))
                        .child(check("c-hidden", "Include hidden", self.include_hidden, |t, v| t.include_hidden = v, cx))
                        .child(check("c-git", "Respect .gitignore", self.respect_gitignore, |t, v| t.respect_gitignore = v, cx))
                        .when_os_search(|d| d.child(check("c-spot", if cfg!(windows) { "Use Windows Search index" } else { "Use Spotlight index" }, self.use_spotlight, |t, v| t.use_spotlight = v, cx))),
                ),
            )
            .children(self.error.clone().map(|e| kit::notice(e, true, cx)))
            .child(
                kit::footer()
                    .child(Button::new("adv-search-cancel").cursor_pointer().ghost().label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(
                        Button::new("adv-search-go").cursor_pointer()
                            .primary()
                            .label("Search")
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}

trait WhenOsSearch: Sized {
    fn when_os_search(self, f: impl FnOnce(Self) -> Self) -> Self {
        if crate::platform_text::HAS_OS_SEARCH { f(self) } else { self }
    }
}
impl<T: ParentElement + Sized> WhenOsSearch for T {}

/// Folders deeper than this below the search root aren't listed.
const REMOTE_MAX_DEPTH: usize = 12;

/// Breadth-first name search over a server connection.
#[allow(clippy::too_many_arguments)]
fn remote_walk(
    conn: &str,
    root: &str,
    text: &str,
    hidden: bool,
    max: usize,
    cancel: &AtomicBool,
    push: &(dyn Fn(Vec<Entry>) + Sync),
    scanned: &(dyn Fn(u64) + Sync),
) -> anyhow::Result<()> {
    let needle = text.to_lowercase();
    let mut queue = std::collections::VecDeque::from([(root.to_string(), 0usize)]);
    let (mut found, mut seen) = (0usize, 0u64);
    let mut batch = Vec::new();
    let mut last = Instant::now();
    while let Some((dir, depth)) = queue.pop_front() {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        // Unreadable folders (permissions) are skipped, not fatal.
        let Ok(entries) = crate::remote::list(conn, &dir, hidden) else { continue };
        for e in entries {
            seen += 1;
            if e.is_dir && depth < REMOTE_MAX_DEPTH && e.kind != rile_core::EntryKind::Symlink {
                queue.push_back((crate::remote::path_str(&e.path), depth + 1));
            }
            if e.name.to_lowercase().contains(&needle) {
                batch.push(e);
                found += 1;
                if found >= max {
                    push(std::mem::take(&mut batch));
                    scanned(seen);
                    return Ok(());
                }
            }
        }
        if last.elapsed() > Duration::from_millis(150) {
            if !batch.is_empty() {
                push(std::mem::take(&mut batch));
            }
            scanned(seen);
            last = Instant::now();
        }
    }
    if !batch.is_empty() {
        push(batch);
    }
    scanned(seen);
    Ok(())
}
