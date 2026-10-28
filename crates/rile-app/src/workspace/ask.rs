//! Ask palette (⌘K): a small floating chat near the top of the window that
//! answers questions with the active model (Apple Intelligence or the
//! selected local model). The model sees the current folder and selection
//! and runs a ReAct loop ([`super::ask_agent`]) over file tools; each tool
//! call shows in the chat as a step that expands to its results. Closing the
//! palette leaves a question running; the toolbar then shows an animated
//! AI symbol, and a dot once the answer is in ([`toolbar_indicator`]).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    WindowExt, h_flex,
    input::{Input, InputEvent, InputState},
    notification::Notification,
    spinner::Spinner,
    Sizable, v_flex,
};
use gpui_kit::{
    Animation, AnimationExt, AnyElement, AppContext, Context, Entity, EventEmitter, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Task, WeakEntity, Window, div, prelude::FluentBuilder, px,
};
use rile_core::Entry;

use super::Workspace;
use super::ask_agent::{self, Action, Scope, Tool, Update};
use crate::actions::*;

/// Earlier question/answer pairs sent along for follow-up questions.
const HISTORY: usize = 4;
/// Results listed in an expanded step.
const LISTED: usize = 8;
/// Size of the Apple Intelligence glyph, and height of the field beside it.
const GLYPH: f32 = 20.;

enum StepState {
    Running(Option<SharedString>),
    Done(SharedString),
    Failed(SharedString),
}

/// What a thinking model reasoned before a tool call or its answer.
struct Thinking {
    text: String,
    started: std::time::Instant,
    /// How long it thought; None while it still is.
    took: Option<std::time::Duration>,
    expanded: bool,
}

impl Thinking {
    fn finish(&mut self) {
        self.took.get_or_insert_with(|| self.started.elapsed());
    }
}

/// One tool call in the chat.
struct Step {
    /// The reasoning that led to this call.
    thinking: Option<Thinking>,
    action: Action,
    state: StepState,
    results: Arc<Vec<Entry>>,
    expanded: bool,
}

/// A question and everything done to answer it.
struct Turn {
    question: SharedString,
    steps: Vec<Step>,
    /// The reasoning after the last step: before the answer, or before a
    /// call that hasn't arrived yet.
    thinking: Option<Thinking>,
    answer: String,
    error: Option<SharedString>,
}

/// The palette started or stopped working, or its answer was seen; the
/// toolbar indicator re-renders on it.
pub struct AskChanged;

pub struct AskPalette {
    ws: WeakEntity<Workspace>,
    pub(crate) input: Entity<InputState>,
    scope: Arc<Scope>,
    model: SharedString,
    turns: Vec<Turn>,
    pub(crate) running: bool,
    /// Whether the palette is on screen.
    pub(crate) open: bool,
    /// An answer arrived while the palette was closed.
    pub(crate) unread: bool,
    cancel: Arc<AtomicBool>,
    scroll: ScrollHandle,
    _task: Option<Task<()>>,
    _sub: Subscription,
}

impl AskPalette {
    fn new(ws: WeakEntity<Workspace>, scope: Scope, model: SharedString, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Ask about your files or find something…"));
        let sub = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { shift, .. } = event {
                if *shift { this.search_files(window, cx) } else { this.ask(window, cx) }
            }
        });
        Self {
            ws,
            input,
            scope: Arc::new(scope),
            model,
            turns: Vec::new(),
            running: false,
            open: false,
            unread: false,
            cancel: Arc::default(),
            scroll: ScrollHandle::new(),
            _task: None,
            _sub: sub,
        }
    }

    /// Whether reopening the palette should continue this conversation: it's
    /// still working or has an unseen answer, or it still describes what the
    /// pane shows.
    fn keep(&self, dir: &Option<PathBuf>, selection: &[PathBuf], model: &SharedString) -> bool {
        self.running || self.unread || (&self.scope.dir == dir && self.scope.selection == selection && &self.model == model)
    }

    pub(crate) fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.open = open;
        if open {
            self.unread = false;
        }
        cx.emit(AskChanged);
        cx.notify();
    }

    /// Stops the running question, if any.
    fn stop(&mut self, cx: &mut Context<Self>) {
        if self.running {
            cx.emit(AskChanged);
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::default();
        self._task = None;
        self.running = false;
        if let Some(turn) = self.turns.last_mut() {
            for step in &mut turn.steps {
                if matches!(step.state, StepState::Running(_)) {
                    step.state = StepState::Failed("Stopped".into());
                }
            }
        }
    }

    fn ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let question = self.input.read(cx).value().trim().to_string();
        if question.is_empty() {
            return;
        }
        self.stop(cx);
        self.input.update(cx, |i, cx| i.set_value("", window, cx));
        let mut history: Vec<(String, String)> = self
            .turns
            .iter()
            .filter(|t| !t.answer.trim().is_empty())
            .map(|t| (t.question.to_string(), t.answer.trim().to_string()))
            .collect();
        history.drain(..history.len().saturating_sub(HISTORY));
        self.turns.push(Turn { question: question.clone().into(), steps: Vec::new(), thinking: None, answer: String::new(), error: None });
        self.running = true;
        cx.emit(AskChanged);
        let (scope, cancel) = (self.scope.clone(), self.cancel.clone());
        let (tx, rx) = smol::channel::unbounded();
        // Searching, listing and reading files and starting the model (first
        // use loads it) all block, so the loop runs off the UI thread.
        let work = cx.background_spawn(async move {
            ask_agent::run(&question, &history, &scope, &cancel, &|update| {
                let _ = tx.send_blocking(update);
            })
        });
        self._task = Some(cx.spawn(async move |this, cx| {
            while let Ok(update) = rx.recv().await {
                if this.update(cx, |this, cx| this.apply(update, cx)).is_err() {
                    return;
                }
            }
            let result = work.await;
            this.update(cx, |this, cx| {
                this.running = false;
                this.unread = !this.open;
                if let Some(thinking) = this.turns.last_mut().and_then(|t| t.thinking.as_mut()) {
                    thinking.finish();
                }
                if let (Err(e), Some(turn)) = (result, this.turns.last_mut()) {
                    turn.error = Some(e.into());
                }
                cx.emit(AskChanged);
                this.scroll.scroll_to_bottom();
                cx.notify();
            })
            .ok();
        }));
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn apply(&mut self, update: Update, cx: &mut Context<Self>) {
        let Some(turn) = self.turns.last_mut() else { return };
        let running = turn.steps.last_mut().filter(|s| matches!(s.state, StepState::Running(_)));
        match update {
            Update::Step(action) => {
                // Text the model streamed before deciding on a tool isn't the answer.
                turn.answer.clear();
                let thinking = turn.thinking.take().map(|mut t| {
                    t.finish();
                    t
                });
                turn.steps.push(Step { thinking, action, state: StepState::Running(None), results: Arc::default(), expanded: false });
            }
            Update::Status(status) => {
                if let Some(step) = running {
                    step.state = StepState::Running(Some(status.into()));
                }
            }
            Update::StepDone { summary, results } => {
                if let Some(step) = running {
                    step.state = StepState::Done(summary.into());
                    step.results = Arc::new(results);
                }
            }
            Update::StepFailed(error) => {
                if let Some(step) = running {
                    step.state = StepState::Failed(error.into());
                }
            }
            Update::Thinking(text, done) => {
                let thinking = turn.thinking.get_or_insert_with(|| Thinking {
                    text: String::new(),
                    started: std::time::Instant::now(),
                    took: None,
                    expanded: false,
                });
                thinking.text = text;
                if done {
                    thinking.finish();
                }
            }
            Update::Answer(snapshot) => {
                turn.answer = snapshot;
            }
        }
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// ⇧↩: hands the text to intelligent search in the active pane.
    fn search_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let request = self.input.read(cx).value().trim().to_string();
        if request.is_empty() {
            return;
        }
        self.input.update(cx, |i, cx| i.set_value("", window, cx));
        self.set_open(false, cx);
        window.close_dialog(cx);
        let ws = self.ws.clone();
        // The workspace may be mid-update when this input event fires.
        window.defer(cx, move |window, cx| {
            ws.update(cx, |ws, cx| {
                ws.search_input.update(cx, |i, cx| i.set_value(request, window, cx));
                ws.ai_from_search(window, cx);
            })
            .ok();
        });
    }

    /// Closes the palette and opens a folder, or selects a file in its folder.
    fn reveal(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.set_open(false, cx);
        window.close_dialog(cx);
        let ws = self.ws.clone();
        window.defer(cx, move |window, cx| {
            ws.update(cx, |ws, cx| {
                if path.is_dir() {
                    ws.open_location(rile_core::history::Location::Dir(path), window, cx);
                } else {
                    ws.reveal_in_pane(path, window, cx);
                }
            })
            .ok();
        });
    }

    /// Closes the palette and lists a step's results in the active pane.
    fn show_in_pane(&mut self, label: SharedString, found: Arc<Vec<Entry>>, window: &mut Window, cx: &mut Context<Self>) {
        self.set_open(false, cx);
        window.close_dialog(cx);
        let ws = self.ws.clone();
        window.defer(cx, move |window, cx| {
            ws.update(cx, |ws, cx| {
                ws.pane().update(cx, |pane, cx| {
                    pane.start_search(label, Arc::default(), cx, move |push, _| {
                        push(found.as_ref().clone());
                        Ok(())
                    });
                    pane.focus(window, cx);
                });
            })
            .ok();
        });
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.stop(cx);
        self.turns.clear();
        cx.notify();
    }

    fn context_label(&self) -> String {
        let place = self.scope.dir.as_deref().map(super::display_name).unwrap_or_else(|| self.scope.location.clone());
        match self.scope.selection.len() {
            0 => place,
            1 => format!("{place} · {}", self.scope.selection[0].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
            n => format!("{place} · {n} selected"),
        }
    }
}

impl EventEmitter<AskChanged> for AskPalette {}

/// Splits Markdown at blank lines into blocks (paragraphs, lists, tables),
/// keeping fenced code blocks whole.
pub(super) fn markdown_blocks(text: &str) -> Vec<String> {
    let mut blocks = vec![String::new()];
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        if line.trim().is_empty() && !in_fence {
            if !blocks.last().is_some_and(|b| b.is_empty()) {
                blocks.push(String::new());
            }
            continue;
        }
        let block = blocks.last_mut().unwrap();
        if !block.is_empty() {
            block.push('\n');
        }
        block.push_str(line);
    }
    blocks.retain(|b| !b.is_empty());
    blocks
}

impl Drop for AskPalette {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Workspace {
    /// Opens the Ask palette for the active pane. Reopening it on the same
    /// folder and selection keeps the conversation.
    pub(crate) fn ai_ask(&mut self, _: &AiAsk, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settings.ai_enabled {
            window.push_notification(Notification::info("AI features are turned off in Settings"), cx);
            return;
        }
        let pane = self.pane().read(cx);
        // Files on remote servers aren't read; the model only sees where the user is.
        let local = pane.conn().is_none();
        let dir = local.then(|| pane.current_dir().map(std::path::Path::to_path_buf)).flatten();
        let selection = if local { pane.selected_paths() } else { Vec::new() };
        let location = super::tabs::tab_title(&pane.location);
        let model: SharedString = match &self.settings.ai_model_path {
            Some(path) => path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default().into(),
            None => "Apple Intelligence".into(),
        };
        let palette = match &self.ask {
            Some(p) if p.read(cx).keep(&dir, &selection, &model) => p.clone(),
            _ => {
                // Searches cover the home folder, or the current folder when
                // it's elsewhere (another disk).
                let home = dirs::home_dir();
                let root = match (&dir, &home) {
                    (Some(d), Some(h)) if d.starts_with(h) => Some(h.clone()),
                    (Some(d), _) => Some(d.clone()),
                    (None, home) if local => home.clone(),
                    _ => None,
                };
                let scope = Scope { search_base: root.map(|r| self.default_query(r)), dir, location, selection };
                let ws = cx.entity().downgrade();
                let p = cx.new(|cx| AskPalette::new(ws, scope, model, window, cx));
                self._ask_sub = Some(cx.subscribe(&p, |_, _, _: &AskChanged, cx| cx.notify()));
                self.ask = Some(p.clone());
                p
            }
        };
        palette.update(cx, |p, cx| p.set_open(true, cx));
        let input = palette.read(cx).input.clone();
        let focus_pane = self.pane().read(cx).focus_handle.clone();
        let first = std::cell::Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first.replace(false) {
                input.update(cx, |i, cx| {
                    i.focus(window, cx);
                    i.select_all(window, cx);
                });
            }
            let (focus_pane, palette_closed) = (focus_pane.clone(), palette.downgrade());
            crate::views::kit::surface(dialog, cx)
                // Re-centred on every layout, so it stays in the middle as answers grow it.
                .centered()
                .w(px(640.))
                .close_button(false)
                .entrance(gpui_kit::component::dialog::DialogEntrance::Fade)
                // Esc, the backdrop or the close button; the question keeps running.
                .on_close(move |_, window, cx| {
                    palette_closed.update(cx, |p, cx| p.set_open(false, cx)).ok();
                    focus_pane.focus(window, cx);
                })
                // Return asks (handled by the input); the dialog's own Confirm would close it.
                .on_ok(|_, _, _| false)
                .child(palette.clone())
        });
    }
}

impl AskPalette {
    /// A tool call: icon, what it did and its outcome; expands to the thought
    /// and the items it found.
    fn render_step(&self, turn_ix: usize, step_ix: usize, step: &Step, cx: &mut Context<Self>) -> AnyElement {
        use crate::views::kit;
        let ch = crate::theme::chrome(cx);
        let input = step.action.input.clone();
        let running = matches!(step.state, StepState::Running(_));
        let (icon, label): (IconName, String) = match step.action.tool {
            Tool::SearchFiles => (IconName::Search, format!("{} \u{201c}{input}\u{201d}", if running { "Searching files for" } else { "Searched files for" })),
            Tool::ListFolder => (IconName::FolderOpen, format!("{} {input}", if running { "Opening" } else { "Opened" })),
            Tool::ReadFile => (IconName::FileText, format!("{} {input}", if running { "Reading" } else { "Read" })),
        };
        let (detail, failed) = match &step.state {
            StepState::Running(status) => (status.clone(), false),
            StepState::Done(summary) => (Some(summary.clone()), false),
            StepState::Failed(error) => (Some(error.clone()), true),
        };
        let can_expand = !running && (step.action.thought.is_some() || !step.results.is_empty());
        let id = turn_ix * 100 + step_ix;
        let header = h_flex()
            .id(("ask-step", id))
            .gap(px(8.))
            .px(px(8.))
            .py(px(5.))
            .rounded(px(6.))
            .text_size(px(12.))
            .text_color(ch.text_muted)
            .when(can_expand, |d| {
                d.cursor_pointer().hover(|s| s.bg(ch.hover_strong)).on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(step) = this.turns.get_mut(turn_ix).and_then(|t| t.steps.get_mut(step_ix)) {
                        step.expanded = !step.expanded;
                        cx.notify();
                    }
                }))
            })
            .child(
                div().flex_none().when_else(
                    running,
                    |d| d.child(Spinner::new().xsmall()),
                    |d| d.child(crate::icons::icon(icon).size(px(14.)).text_color(if failed { gpui_kit::rgba(0xf07178ff).into() } else { ch.text_muted })),
                ),
            )
            .child(div().min_w_0().truncate().text_color(ch.text).child(label))
            .when_some(detail, |d, detail| {
                d.child(div().flex_1().min_w_0().truncate().text_color(if failed { gpui_kit::rgba(0xf07178ff).into() } else { ch.text_faint }).child(detail))
            })
            .when(can_expand, |d| {
                d.child(
                    div()
                        .flex_none()
                        .ml_auto()
                        .child(crate::icons::icon(if step.expanded { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(14.)).text_color(ch.text_faint)),
                )
            });
        let body = step.expanded.then(|| {
            let home = dirs::home_dir().unwrap_or_default();
            let rows: Vec<_> = step
                .results
                .iter()
                .take(LISTED)
                .enumerate()
                .map(|(ix, entry)| {
                    let folder = entry.path.parent().map(|p| match p.strip_prefix(&home) {
                        Ok(rel) if rel.as_os_str().is_empty() => "~".to_string(),
                        Ok(rel) => format!("~{}{}", std::path::MAIN_SEPARATOR, rel.display()),
                        Err(_) => p.display().to_string(),
                    });
                    let path = entry.path.clone();
                    h_flex()
                        .id(("ask-result", id * 1000 + ix))
                        .gap(px(8.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|s| s.bg(ch.hover_strong))
                        .on_click(cx.listener(move |this, event: &gpui_kit::ClickEvent, window, cx| {
                            if event.modifiers().secondary(cx) {
                                super::open_with::open_file(&path, cx);
                            } else {
                                this.reveal(path.clone(), window, cx);
                            }
                        }))
                        .child(div().flex_none().child(crate::icons::entry_icon(entry, 16., ch.text_muted, cx)))
                        .child(div().flex_none().max_w(px(260.)).truncate().text_color(ch.text).child(SharedString::from(entry.name.to_string())))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(11.)).text_color(ch.text_faint).child(folder.unwrap_or_default()))
                })
                .collect();
            let found = step.results.clone();
            let pane_label: SharedString = format!("AI: \u{201c}{input}\u{201d}").into();
            v_flex()
                .ml(px(15.))
                .pl(px(12.))
                .border_l_1()
                .border_color(ch.hairline)
                .gap(px(2.))
                .when_some(step.action.thought.clone(), |d, thought| {
                    d.child(div().px(px(8.)).py(px(3.)).text_size(px(12.)).italic().text_color(ch.text_faint).child(thought))
                })
                .children(rows)
                .when(step.results.len() > 1 && step.action.tool != Tool::ReadFile, |d| {
                    let label = if step.results.len() > LISTED { format!("Show All {} in Pane", step.results.len()) } else { "Show in Pane".into() };
                    d.child(
                        h_flex().px(px(2.)).child(
                            kit::button(("ask-show", id), label, kit::Tone::Ghost, true, cx)
                                .on_click(cx.listener(move |this, _, window, cx| this.show_in_pane(pane_label.clone(), found.clone(), window, cx))),
                        ),
                    )
                })
        });
        v_flex().gap(px(2.)).child(header).children(body).into_any_element()
    }

    /// A collapsed row for the model's reasoning; `step` is the call it led
    /// to, None for the reasoning before the answer.
    fn render_thinking(&self, turn_ix: usize, step: Option<usize>, thinking: &Thinking, cx: &mut Context<Self>) -> AnyElement {
        let ch = crate::theme::chrome(cx);
        let live = self.running && thinking.took.is_none();
        let label = match thinking.took {
            None => "Thinking\u{2026}".to_string(),
            Some(took) if took.as_secs() < 1 => "Thought for a moment".to_string(),
            Some(took) => format!("Thought for {}s", took.as_secs()),
        };
        let can_expand = !thinking.text.is_empty();
        let header = h_flex()
            .id(("ask-thinking", turn_ix * 100 + step.unwrap_or(99)))
            .gap(px(8.))
            .px(px(8.))
            .py(px(5.))
            .rounded(px(6.))
            .text_size(px(12.))
            .text_color(ch.text_muted)
            .when(can_expand, |d| {
                d.cursor_pointer().hover(|s| s.bg(ch.hover_strong)).on_click(cx.listener(move |this, _, _, cx| {
                    let Some(turn) = this.turns.get_mut(turn_ix) else { return };
                    let thinking = match step {
                        Some(step) => turn.steps.get_mut(step).and_then(|s| s.thinking.as_mut()),
                        None => turn.thinking.as_mut(),
                    };
                    if let Some(thinking) = thinking {
                        thinking.expanded = !thinking.expanded;
                        cx.notify();
                    }
                }))
            })
            .child(div().flex_none().when_else(
                live,
                |d| d.child(Spinner::new().xsmall()),
                |d| d.child(crate::icons::icon(IconName::Brain).size(px(14.)).text_color(ch.text_muted)),
            ))
            .child(div().min_w_0().truncate().text_color(ch.text).child(label))
            .when(can_expand, |d| {
                d.child(
                    div()
                        .flex_none()
                        .ml_auto()
                        .child(crate::icons::icon(if thinking.expanded { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(14.)).text_color(ch.text_faint)),
                )
            });
        let body = (thinking.expanded && can_expand).then(|| {
            v_flex()
                .ml(px(15.))
                .pl(px(12.))
                .pr(px(8.))
                .py(px(2.))
                .border_l_1()
                .border_color(ch.hairline)
                .gap(px(6.))
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(ch.text_faint)
                .children(
                    thinking
                        .text
                        .split("\n\n")
                        .map(str::trim)
                        .filter(|p| !p.is_empty())
                        .map(|p| div().child(SharedString::from(p.replace('\n', " ")))),
                )
        });
        v_flex().gap(px(2.)).child(header).children(body).into_any_element()
    }

    fn render_turn(&self, ix: usize, turn: &Turn, cx: &mut Context<Self>) -> AnyElement {
        use crate::views::kit;
        let ch = crate::theme::chrome(cx);
        let live = self.running && ix + 1 == self.turns.len();
        // The reasoning row shows its own progress.
        let thinking = live
            && turn.answer.trim().is_empty()
            && turn.thinking.is_none()
            && !turn.steps.iter().any(|s| matches!(s.state, StepState::Running(_)));
        v_flex()
            .gap(px(8.))
            .when(ix > 0, |d| d.pt(px(12.)).border_t_1().border_color(ch.hairline))
            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(ch.text).child(turn.question.clone()))
            .when(!turn.steps.is_empty(), |d| {
                d.child(v_flex().mx(px(-8.)).gap(px(1.)).children(turn.steps.iter().enumerate().flat_map(|(i, s)| {
                    let thinking = s.thinking.as_ref().map(|t| self.render_thinking(ix, Some(i), t, cx));
                    thinking.into_iter().chain([self.render_step(ix, i, s, cx)]).collect::<Vec<_>>()
                })))
            })
            .when_some(turn.thinking.as_ref(), |d, t| d.child(div().mx(px(-8.)).child(self.render_thinking(ix, None, t, cx))))
            .when(thinking, |d| d.child(h_flex().gap(px(8.)).text_color(ch.text_muted).child(Spinner::new().xsmall()).child("Thinking\u{2026}")))
            .when(!turn.answer.is_empty(), |d| {
                let text = &turn.answer;
                let mut paths: Vec<_> = self.turns.iter().flat_map(|t| t.steps.iter()).flat_map(|s| s.results.iter().map(|e| e.path.clone()))
                    .chain(self.scope.selection.iter().cloned()).chain(self.scope.dir.iter().cloned()).collect();
                // Tool-input folders may be mentioned without appearing in results.
                for t in &self.turns {
                    for step in &t.steps {
                        if step.action.tool != Tool::SearchFiles {
                            paths.push(ask_agent::resolve(&step.action.input, &self.scope));
                        }
                    }
                }
                paths.sort();
                paths.dedup();
                let palette = cx.entity().downgrade();
                let on_file = std::rc::Rc::new(move |path: PathBuf, event: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut gpui_kit::App| {
                    if event.modifiers().secondary(cx) {
                        super::open_with::open_file(&path, cx);
                    } else {
                        palette.update(cx, |this, cx| this.reveal(path, window, cx)).ok();
                    }
                });
                d.child(super::ask_text::render(ix, text.trim(), &paths, on_file, cx))
            })
            .when_some(turn.error.clone(), |d, e| d.child(kit::notice(e, true, cx)))
            .into_any_element()
    }
}

impl Render for AskPalette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::views::kit;
        let ch = crate::theme::chrome(cx);
        let glyph = crate::icons::apple_intelligence(GLYPH, ch.accent, cx);
        let key = |k: &'static str, label: &'static str| {
            h_flex()
                .gap(px(5.))
                .child(div().px(px(5.)).rounded(px(4.)).border_1().border_color(ch.hairline).text_color(ch.text_muted).child(k))
                .child(label)
        };
        let turns: Vec<_> = self.turns.iter().enumerate().map(|(ix, turn)| self.render_turn(ix, turn, cx)).collect();
        v_flex()
            .id("ask-palette")
            .gap(px(12.))
            .text_size(px(13.))
            // The field has no padding of its own and is as tall as the glyph,
            // so the glyph sits the dialog's padding from both the top and the left.
            .child(
                h_flex()
                    .h(px(GLYPH))
                    .gap(px(12.))
                    .child(div().flex_none().child(glyph))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.input).appearance(false).px_0().py_0().h(px(GLYPH)).text_size(px(17.))),
                    )
                    .when(self.running, |d| d.child(Spinner::new().xsmall())),
            )
            .child(
                h_flex()
                    .justify_between()
                    .gap(px(12.))
                    .child(kit::hint(format!("Context: {}", self.context_label()), cx).min_w_0().truncate())
                    .child(kit::hint(self.model.clone(), cx).flex_none()),
            )
            .when(!turns.is_empty(), |d| {
                d.child(
                    kit::card(cx).child(
                        v_flex()
                            .id("ask-chat")
                            .track_scroll(&self.scroll)
                            .max_h(px(440.))
                            .overflow_y_scroll()
                            .p(px(14.))
                            .gap(px(12.))
                            .children(turns),
                    ),
                )
            })
            .child(
                h_flex()
                    .justify_between()
                    .text_size(px(11.))
                    .text_color(ch.text_faint)
                    .child(h_flex().gap(px(14.)).child(key("\u{21a9}", "Ask")).child(key("\u{21e7}\u{21a9}", "Find files")).child(key("esc", "Close")))
                    .when(self.running, |d| {
                        d.child(kit::button("ask-stop", "Stop", kit::Tone::Ghost, true, cx).on_click(cx.listener(|this, _, _, cx| {
                            this.stop(cx);
                            cx.notify();
                        })))
                    })
                    .when(!self.running && !self.turns.is_empty(), |d| {
                        d.child(
                            kit::button("ask-clear", "New Conversation", kit::Tone::Ghost, true, cx)
                                .on_click(cx.listener(|this, _, _, cx| this.clear(cx))),
                        )
                    }),
            )
    }
}

/// Toolbar button for a conversation that's out of sight: the AI symbol in a
/// flowing rainbow ring while the model works, then with a dot once the
/// answer is in. Clicking it reopens the palette.
pub fn toolbar_indicator(ws: &Workspace, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    let palette = ws.ask.as_ref()?.read(cx);
    let (running, unread) = (palette.running, palette.unread);
    if palette.open || !(running || unread) {
        return None;
    }
    let ch = crate::theme::chrome(cx);
    let glyph = crate::icons::apple_intelligence(17., ch.accent, cx);
    let tip = if running { "AI is answering\u{2026} (\u{2318}K)" } else { "AI answered \u{2014} click to read" };
    Some(
        div()
            .id("ask-indicator")
            .relative()
            .flex_none()
            .size(px(30.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(7.))
            .cursor_pointer()
            .hover(|s| s.bg(ch.hover_strong))
            .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx))
            .on_click(cx.listener(|this, _, window, cx| this.ai_ask(&AiAsk, window, cx)))
            .map(|d| {
                if !running {
                    return d.child(glyph).child(
                        div().absolute().top(px(4.)).right(px(4.)).size(px(8.)).rounded_full().bg(ch.accent).border_1().border_color(ch.toolbar_top),
                    );
                }
                // A soft halo behind the symbol, drifting through the Apple
                // Intelligence colors and slowly breathing.
                d.child(
                    div().absolute().size(px(16.)).rounded_full().with_animation(
                        "ask-indicator-halo",
                        Animation::new(std::time::Duration::from_millis(4200)).repeat(),
                        |el, t| {
                            let breathe = 0.5 - 0.5 * (t * std::f32::consts::TAU * 2.).cos();
                            let glow = |p: f32, a: f32, blur: f32| gpui_kit::BoxShadow {
                                color: crate::ai_glow::rainbow(t + p, a),
                                offset: gpui_kit::point(px(0.), px(0.)),
                                blur_radius: px(blur),
                                spread_radius: px(0.),
                                inset: false,
                            };
                            el.shadow(vec![glow(0., 0.6 + 0.3 * breathe, 7. + 4. * breathe), glow(0.5, 0.35 + 0.2 * breathe, 14. + 6. * breathe)])
                        },
                    ),
                )
                .child(div().relative().child(glyph))
            })
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::markdown_blocks;

    #[test]
    fn splits_markdown_into_blocks() {
        let text = "Intro line\nmore\n\n\n- a\n- b\n\n```\nfn x() {}\n\nfn y() {}\n```\n\nEnd";
        assert_eq!(markdown_blocks(text), ["Intro line\nmore", "- a\n- b", "```\nfn x() {}\n\nfn y() {}\n```", "End"]);
        assert!(markdown_blocks("").is_empty());
    }
}
