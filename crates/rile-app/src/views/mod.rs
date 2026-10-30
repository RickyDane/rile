pub mod backup_settings;
pub mod backups;
pub mod devices;
pub mod chrome;
pub mod columns;
pub mod grid;
pub mod gallery;
pub mod home;
pub mod kit;
pub mod list;
pub mod ai_settings;
pub mod plugin_page;
pub mod space_analyzer;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::component::{ActiveTheme, menu::{ContextMenuExt, PopupMenu, PopupMenuItem}, v_flex, h_flex};
use gpui_kit::{
    AnyElement, App, AppContext, Context, Div, ExternalPaths, Global, InteractiveElement, IntoElement,
    DispatchPhase, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, NavigationDirection, ParentElement, Render, SharedString, Stateful,
    StatefulInteractiveElement, Styled, WeakEntity, Window, canvas, div, prelude::FluentBuilder, px,
};
use rile_core::Entry;
use rile_core::settings::ViewKind;

use crate::actions::*;
use gpui_kit::assets::IconName;
use crate::pane::{DraggedFiles, Pane, PaneEvent};
use crate::workspace::Workspace;

/// Lets context menus built inside panes reach the workspace (plugins etc.).
pub struct WorkspaceHandle(pub WeakEntity<Workspace>);
impl Global for WorkspaceHandle {}

/// Display preferences the workspace publishes for all panes.
#[derive(Clone, Copy)]
pub struct ViewPrefs {
    pub row_height: f32,
    pub thumbnails: bool,
    pub icon_size: f32,
}
impl Global for ViewPrefs {}

pub fn prefs(cx: &App) -> ViewPrefs {
    cx.try_global::<ViewPrefs>().copied().unwrap_or(ViewPrefs { row_height: 26., thumbnails: true, icon_size: 64. })
}

thread_local! {
    static TYPEAHEAD: std::cell::RefCell<(String, Option<Instant>)> = const { std::cell::RefCell::new((String::new(), None)) };
}

impl Render for Pane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.view {
            ViewKind::List => list::render(self, window, cx),
            ViewKind::Grid => grid::render(self, window, cx),
            ViewKind::Columns => columns::render(self, window, cx),
        };
        frame(self, body, cx)
    }
}

/// Shared pane chrome: key context, selection actions, background drop
/// target, viewport measuring and the context menu.
fn frame(pane: &mut Pane, body: AnyElement, cx: &mut Context<Pane>) -> AnyElement {
    let theme = cx.theme().clone();
    let focus = pane.focus_handle.clone();
    let entity = cx.entity();
    let weak = entity.downgrade();
    let dir = pane.dir_path();
    let key_context = if pane.previewing { "FileList previewing" } else { "FileList" };

    let status: Option<AnyElement> = if let Some(s) = &pane.search {
        Some(
            h_flex()
                .px_3()
                .h(px(26.))
                .gap_2()
                .flex_none()
                .bg(theme.accent)
                .text_xs()
                .text_color(theme.accent_foreground)
                .child(div().flex_1().truncate().child(s.label.clone()))
                .child(match (&s.error, s.running, s.stopped) {
                    (Some(e), _, _) => SharedString::from(format!("Error: {e}")),
                    (None, true, _) => format!("Searching… {} found, {} scanned · Esc to stop", pane.total_count(), s.scanned).into(),
                    (None, false, true) => format!("Stopped · {} found, {} scanned", pane.total_count(), s.scanned).into(),
                    (None, false, false) => format!("{} found", pane.total_count()).into(),
                })
                .when(s.running, |d| {
                    d.child(
                        div()
                            .id("stop-search")
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.list_hover))
                            .child("Stop")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.stop_search(cx);
                            })),
                    )
                })
                .child(
                    div()
                        .id("close-search")
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.list_hover))
                        .child("✕")
                        .on_click(cx.listener(|this, _, _, cx| this.clear_search(cx))),
                )
                .into_any_element(),
        )
    } else if !pane.filter.is_empty() {
        Some(
            h_flex()
                .px_3()
                .h(px(24.))
                .flex_none()
                .bg(theme.accent)
                .text_xs()
                .text_color(theme.accent_foreground)
                .child(format!("Filter: \"{}\" — {} of {} (Esc to clear)", pane.filter, pane.entries().len(), pane.total_count()))
                .into_any_element(),
        )
    } else {
        None
    };

    v_flex()
        .id("pane")
        .size_full()
        .relative()
        .bg(crate::theme::chrome(cx).browser)
        .track_focus(&pane.focus_handle)
        .key_context(key_context)
        .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_vertical(1, false, cx)))
        .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.move_vertical(-1, false, cx)))
        .on_action(cx.listener(|this, _: &ExtendNext, _, cx| this.move_vertical(1, true, cx)))
        .on_action(cx.listener(|this, _: &ExtendPrev, _, cx| this.move_vertical(-1, true, cx)))
        .on_action(cx.listener(|this, _: &SelectLeft, _, cx| match this.view {
            ViewKind::Grid => this.move_cursor(-1, false, cx),
            ViewKind::Columns => this.go_up(cx),
            ViewKind::List => {}
        }))
        .on_action(cx.listener(|this, _: &SelectRight, _, cx| match this.view {
            ViewKind::Grid => this.move_cursor(1, false, cx),
            ViewKind::Columns => {
                if let Some(e) = this.cursor_entry().cloned()
                    && e.is_dir
                {
                    this.open_entry(&e, cx);
                    this.cursor_to(0, false, cx);
                }
            }
            ViewKind::List => {}
        }))
        .on_action(cx.listener(|this, _: &SelectFirst, _, cx| this.cursor_to(0, false, cx)))
        .on_action(cx.listener(|this, _: &SelectLast, _, cx| this.cursor_to(usize::MAX, false, cx)))
        .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.select_all(cx)))
        .on_action(cx.listener(|this, _: &ClearSelection, _, cx| this.clear_selection(cx)))
        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
            let ks = &ev.keystroke;
            let m = &ks.modifiers;
            // AltGr arrives as Ctrl+Alt on Windows; it types characters.
            let alt_gr = cfg!(windows) && m.control && m.alt && ks.key_char.is_some();
            if (m.control || m.platform || m.alt || m.function) && !alt_gr {
                return;
            }
            let Some(ch) = ks.key_char.as_deref() else { return };
            if ch.chars().count() != 1 || ch == " " || ch.chars().any(char::is_control) {
                return;
            }
            // Type-ahead: letters typed within 1s accumulate into a prefix.
            let prefix = TYPEAHEAD.with(|t| {
                let mut t = t.borrow_mut();
                if t.1.is_none_or(|at| at.elapsed() > Duration::from_millis(1000)) {
                    t.0.clear();
                }
                t.0.push_str(ch);
                t.1 = Some(Instant::now());
                t.0.clone()
            });
            this.jump_to_prefix(&prefix, cx);
            cx.stop_propagation();
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| this.activate(window, cx)),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(|this, _: &MouseDownEvent, window, cx| this.activate(window, cx)),
        )
        // Mouse thumb buttons navigate this pane's history.
        .on_mouse_down(
            MouseButton::Navigate(NavigationDirection::Back),
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                this.activate(window, cx);
                this.go_back(cx);
            }),
        )
        .on_mouse_down(
            MouseButton::Navigate(NavigationDirection::Forward),
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                this.activate(window, cx);
                this.go_forward(cx);
            }),
        )
        .children(status)
        .when(pane.loading || pane.search.as_ref().is_some_and(|s| s.running), |d| {
            d.child(div().absolute().top_0().left_0().right_0().h(px(2.)).child(crate::progress::activity_bar("pane-busy", None, 2., cx)))
        })
        .child(
            div()
                .id("pane-body")
                .flex_1()
                .min_h_0()
                .relative()
                .child(
                    // Records the viewport width for grid layout.
                    canvas(
                        {
                            let entity = entity.clone();
                            move |bounds, _, cx| {
                                let w: f32 = bounds.size.width.into();
                                entity.update(cx, |p, cx| {
                                    p.viewport_origin = bounds.origin;
                                    if (p.viewport_width - w).abs() > 1. {
                                        p.viewport_width = w;
                                        cx.notify();
                                    }
                                });
                            }
                        },
                        {
                            // While rubber-banding, follow the mouse anywhere in the window.
                            let entity = entity.clone();
                            let banding = pane.rubber.is_some();
                            let row_height = list::row_pitch(cx);
                            move |_, _, window, _| {
                                if !banding {
                                    return;
                                }
                                let e = entity.clone();
                                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                                    if phase == DispatchPhase::Bubble {
                                        e.update(cx, |p, cx| {
                                            if ev.pressed_button == Some(MouseButton::Left) {
                                                p.rubber_update(ev.position, row_height, cx)
                                            } else {
                                                p.rubber_end(cx)
                                            }
                                        });
                                    }
                                });
                                let e = entity.clone();
                                window.on_mouse_event(move |ev: &MouseUpEvent, phase, _, cx| {
                                    if phase == DispatchPhase::Bubble && ev.button == MouseButton::Left {
                                        e.update(cx, |p, cx| p.rubber_end(cx));
                                    }
                                });
                            }
                        },
                    )
                    .absolute()
                    .size_full(),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                        this.activate(window, cx);
                        let additive = ev.modifiers.shift || ev.modifiers.secondary(cx);
                        if !additive {
                            // Blank clicks only deselect; do not run Escape's
                            // search/filter reset behavior.
                            this.deselect(cx);
                        }
                        // Columns view has no free space to band in.
                        if this.view != ViewKind::Columns {
                            this.rubber_start(ev.position, additive, cx);
                        }
                    }),
                )
                .when_some(dir.clone(), |d, dir| drop_target_quiet(d, dir, cx))
                .child(body)
                .when_some(pane.rubber.as_ref().map(|r| r.rect()), |d, rect| {
                    let o = pane.viewport_origin;
                    d.child(
                        div()
                            .absolute()
                            .left(rect.left() - o.x)
                            .top(rect.top() - o.y)
                            .w(rect.size.width)
                            .h(rect.size.height)
                            .border_1()
                            .border_color(theme.primary.opacity(0.8))
                            .bg(theme.primary.opacity(0.12))
                            .rounded_sm(),
                    )
                })
                .when(pane.ai_searching(), |d| d.child(crate::ai_glow::bottom_glow("ai-glow")))
                .context_menu(move |menu, window, cx| build_context_menu(menu, &weak, &focus, window, cx)),
        )
        .into_any_element()
}

fn build_context_menu(
    menu: PopupMenu,
    pane: &WeakEntity<Pane>,
    focus: &gpui_kit::FocusHandle,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    use crate::icons::menu_icon;
    let Some(pane) = pane.upgrade() else { return menu };
    let selection = pane.read(cx).selected_paths();
    let remote = pane.read(cx).conn().is_some();
    let ai = cfg!(feature = "ai")
        && cx.try_global::<WorkspaceHandle>().and_then(|h| h.0.upgrade()).is_some_and(|ws| ws.read(cx).settings.ai_enabled);
    let menu = menu.action_context(focus.clone());
    if selection.is_empty() {
        return menu
            .menu_with_icon("New Folder", menu_icon(IconName::FolderPlus), Box::new(NewFolder))
            .menu_with_icon("New File", menu_icon(IconName::FilePlus), Box::new(NewFile))
            .menu_with_icon("Paste", menu_icon(IconName::ClipboardPaste), Box::new(Paste))
            .separator()
            .menu_with_icon("Refresh", menu_icon(IconName::RotateCw), Box::new(Refresh))
            .menu_with_icon("Show / Hide Hidden Files", menu_icon(IconName::Eye), Box::new(ToggleHidden))
            .menu_with_icon("Search in Folder…", menu_icon(IconName::Search), Box::new(AdvancedSearch))
            .when(!remote, |m| {
                m.menu_with_icon("Open in Terminal", menu_icon(IconName::SquareTerminal), Box::new(OpenTerminal))
                    .menu_with_icon("Compare Folders…", menu_icon(IconName::GitCompareArrows), Box::new(CompareFolders))
            })
            .when(ai && !remote, |m| m.menu_with_icon("Summarize Folder with AI", crate::icons::ai_icon(), Box::new(AiSummarize)))
            .menu_with_icon("Folder Properties", menu_icon(IconName::Info), Box::new(ShowProperties));
    }
    let any_dir = selection.iter().any(|p| p.is_dir());
    let all_files = !remote && selection.iter().all(|p| p.is_file());
    let archives = !remote && selection.iter().any(|p| crate::workspace::archive::is_archive(p));
    let mut menu = menu
        .menu_with_icon("Open", menu_icon(IconName::ExternalLink), Box::new(OpenSelected));
    if all_files {
        menu = crate::workspace::open_with::open_with_submenu(menu, selection.clone(), window, cx);
    }
    let danger = cx.theme().danger;
    let mut menu = menu
        .menu_with_icon(crate::platform_text::preview(), menu_icon(IconName::Eye), Box::new(ToggleQuickLook))
        .menu_with_icon("Open in Other Pane", menu_icon(IconName::Columns2), Box::new(OpenInOtherPane))
        .separator()
        .menu_with_icon(if cfg!(target_os = "macos") { "Move to Trash" } else { "Delete" }, menu_icon(IconName::Trash).text_color(danger), Box::new(MoveToTrash))
        .menu_with_icon("Delete Permanently…", menu_icon(IconName::CircleX).text_color(danger), Box::new(DeletePermanently))
        .separator()
        .menu_with_icon("Cut", menu_icon(IconName::Scissors), Box::new(Cut))
        .menu_with_icon("Copy", menu_icon(IconName::Copy), Box::new(Copy))
        .menu_with_icon("Paste", menu_icon(IconName::ClipboardPaste), Box::new(Paste))
        .menu_with_icon("Duplicate", menu_icon(IconName::Files), Box::new(Duplicate))
        .menu_with_icon("Copy Path", menu_icon(IconName::Link), Box::new(CopyPath))
        .separator()
        .menu_with_icon(if selection.len() > 1 && !remote { format!("Rename {} Items…", selection.len()) } else { "Rename…".into() }, menu_icon(IconName::Pencil), Box::new(Rename))
        .menu_with_icon("Add to / Remove from Favorites", menu_icon(IconName::Star), Box::new(ToggleFavorite));
    if !remote {
        let dirs = selection.iter().filter(|p| p.is_dir()).count();
        menu = menu.separator().menu_with_icon("Add to Shelf", menu_icon(IconName::PackagePlus), Box::new(AddToShelf));
        if dirs == 1 && selection.len() == 1 {
            menu = menu.menu_with_icon("Open in Terminal", menu_icon(IconName::SquareTerminal), Box::new(OpenTerminal));
        }
        if dirs >= 1 && dirs == selection.len() && dirs <= 2 {
            menu = menu.menu_with_icon(if dirs == 2 { "Compare These Folders…" } else { "Compare with…" }, menu_icon(IconName::GitCompareArrows), Box::new(CompareFolders));
        }
        menu = menu.separator().menu_with_icon("Compress…", menu_icon(IconName::Archive), Box::new(Compress));
        if selection.iter().any(|p| crate::archive_fs::is_browsable(p)) {
            menu = menu.menu_with_icon("Browse Archive", menu_icon(IconName::FolderOpen), Box::new(BrowseArchive));
        }
        if archives {
            menu = menu.menu_with_icon("Extract Here", menu_icon(IconName::PackageOpen), Box::new(Extract));
        }
        menu = nearby_items(menu, &selection, window, cx);
    }
    if ai && !remote {
        menu = menu
            .separator()
            .menu_with_icon(if any_dir { "Summarize Contents with AI" } else { "Summarize with AI" }, crate::icons::ai_icon(), Box::new(AiSummarize))
            .menu_with_icon("Describe & Tag with AI", crate::icons::ai_icon(), Box::new(SmartTag));
    }

    // Plugin-contributed commands.
    if let Some(ws) = cx.try_global::<WorkspaceHandle>().and_then(|h| h.0.upgrade()) {
        let dir = pane.read(cx).current_dir().map(|p| p.to_path_buf());
        let commands = ws.read(cx).plugin_commands(dir.clone(), selection.clone());
        if !commands.is_empty() {
            menu = menu.separator().label("Plugins");
            for (plugin_id, cmd_id, label) in commands {
                let ws = ws.downgrade();
                let (dir, selection) = (dir.clone(), selection.clone());
                menu = menu.item(PopupMenuItem::new(label).icon(menu_icon(IconName::Puzzle)).on_click(move |_, window, cx| {
                    ws.update(cx, |ws, cx| {
                        ws.run_plugin_command(&plugin_id, &cmd_id, dir.clone(), selection.clone(), window, cx)
                    })
                    .ok();
                }));
            }
        }
    }

    menu.separator()
        .menu_with_icon("Properties", menu_icon(IconName::Info), Box::new(ShowProperties))
}

/// "Send to Device" (paired devices online) and "Share with Nearby Devices"
/// for a single folder.
fn nearby_items(mut menu: PopupMenu, selection: &[std::path::PathBuf], window: &mut Window, cx: &mut Context<PopupMenu>) -> PopupMenu {
    use crate::icons::menu_icon;
    let Some(ws) = cx.try_global::<WorkspaceHandle>().and_then(|h| h.0.upgrade()) else { return menu };
    let (enabled, devices, shared) = {
        let w = ws.read(cx);
        let devices: Vec<(String, String)> = w
            .nearby
            .rows()
            .into_iter()
            .filter(|r| r.paired && r.online.is_some() && !r.forgot_us)
            .map(|r| (r.id, r.name))
            .collect();
        let shared = match selection {
            [one] if one.is_dir() => Some(w.is_shared(one)),
            _ => None,
        };
        (w.settings.nearby.enabled, devices, shared)
    };
    if !enabled {
        return menu;
    }
    if !devices.is_empty() {
        let paths = std::sync::Arc::new(selection.to_vec());
        let weak = ws.downgrade();
        menu = menu.submenu_with_icon(Some(menu_icon(IconName::Send)), "Send to Device", window, cx, move |mut menu, _, _| {
            for (id, name) in &devices {
                let (id, paths, weak) = (id.clone(), paths.clone(), weak.clone());
                menu = menu.item(PopupMenuItem::new(name.clone()).on_click(move |_, window, cx| {
                    weak.update(cx, |ws, cx| ws.send_to_device(&id, (*paths).clone(), window, cx)).ok();
                }));
            }
            menu
        });
    }
    if let (Some(shared), Some(dir)) = (shared, selection.first().cloned()) {
        let weak = ws.downgrade();
        let label = if shared { "Stop Sharing with Nearby Devices" } else { "Share with Nearby Devices" };
        menu = menu.item(PopupMenuItem::new(label).icon(menu_icon(IconName::Share2)).on_click(move |_, window, cx| {
            let dir = dir.clone();
            weak.update(cx, |ws, cx| {
                if shared { ws.unshare_folder(&dir, window, cx) } else { ws.share_folder(dir, window, cx) }
            })
            .ok();
        }));
    }
    menu
}

/// Shared mouse, drag and drop behaviour for an entry element (row/tile).
pub fn entry_interactions(
    el: Stateful<Div>,
    ix: usize,
    entry: &Entry,
    // Passed in: reading the pane here would re-enter it mid-render.
    pane_conn: Option<String>,
    cx: &mut Context<Pane>,
) -> Stateful<Div> {
    let dragged = DraggedFiles {
        source: cx.entity().downgrade(),
        conn: pane_conn,
        path: entry.path.clone(),
        set: None,
    };
    let name = entry.name.clone();
    let entry_icon = crate::icons::icon_for_entry(entry);
    let el = el
        .debug_selector(|| format!("entry-{name}"))
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                this.activate(window, cx);
                if ev.click_count >= 2 {
                    this.open_index(ix, cx);
                } else if !(this.entries().get(ix).is_some_and(|e| this.selected.contains(&e.path))
                    && !ev.modifiers.shift
                    && !ev.modifiers.secondary(cx))
                {
                    // Keep a multi-selection intact so it can be dragged.
                    this.click(ix, ev.modifiers, cx);
                } else {
                    this.cursor = Some(ix);
                    cx.notify();
                }
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.activate(window, cx);
                this.ensure_selected(ix, cx);
            }),
        )
        .on_drag(dragged, move |d, offset, _, cx| {
            let count = d.resolve(cx).len();
            let icon = if count > 1 { IconName::Files } else { entry_icon.clone() };
            cx.new(|_| DragPreview { icon, offset })
        })
        // macOS and Windows use our native handoff at the workspace boundary.
        .when(!cfg!(any(target_os = "macos", windows)), |el| {
            el.external_drag_payload(|d: &DraggedFiles, _, cx| d.external_payload(cx))
        });
    if entry.is_dir {
        drop_target(el, entry.path.clone(), cx)
    } else {
        el
    }
}

/// Makes an element accept internal file drags and external (OS) drops into `dest`.
pub fn drop_target<E: InteractiveElement + StatefulInteractiveElement>(el: E, dest: PathBuf, cx: &mut Context<Pane>) -> E {
    let hl = cx.theme().drop_target;
    drop_target_inner(el.drag_over::<DraggedFiles>(move |s, _, _, _| s.bg(hl)), dest, cx)
}

/// Like [`drop_target`], but in-app drags don't tint it (used for the whole
/// item list, where a full-area highlight is just noise).
pub fn drop_target_quiet<E: InteractiveElement + StatefulInteractiveElement>(el: E, dest: PathBuf, cx: &mut Context<Pane>) -> E {
    drop_target_inner(el, dest, cx)
}

fn drop_target_inner<E: InteractiveElement + StatefulInteractiveElement>(el: E, dest: PathBuf, cx: &mut Context<Pane>) -> E {
    let hl = cx.theme().drop_target;
    let dest2 = dest.clone();
    el.drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(hl))
        .on_drop(cx.listener(move |this, d: &DraggedFiles, window, cx| {
            let dest_conn = this.conn();
            let same_fs = d.conn == dest_conn;
            let paths: Vec<PathBuf> = d
                .resolve_in(this, cx)
                .into_iter()
                .filter(|p| !same_fs || (*p != dest && !dest.starts_with(p)))
                .collect();
            if paths.is_empty() || (same_fs && paths.iter().all(|p| p.parent() == Some(dest.as_path()))) {
                return;
            }
            let copy = Pane::drop_is_copy(&paths, d.conn.as_deref(), &dest, dest_conn.as_deref(), window);
            cx.emit(PaneEvent::Drop { paths, src_conn: d.conn.clone(), dest: dest.clone(), dest_conn, copy });
            cx.stop_propagation();
        }))
        .on_drop(cx.listener(move |this, ext: &ExternalPaths, _, cx| {
            cx.emit(PaneEvent::DropExternal { paths: ext.paths().to_vec(), dest: dest2.clone(), dest_conn: this.conn() });
            cx.stop_propagation();
        }))
}

pub struct DragPreview {
    pub icon: IconName,
    /// Where the cursor grabbed the source element; GPUI anchors the preview
    /// at `cursor - offset`, so padding by it pins the preview to the cursor.
    pub offset: gpui_kit::Point<gpui_kit::Pixels>,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .pl(self.offset.x)
            .pt(self.offset.y)
            .child(crate::icons::icon(self.icon.clone()).size_6().text_color(theme.foreground))
    }
}

pub fn placeholder(text: SharedString, cx: &App) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}

/// Message for an empty pane, or None if there is content to render.
pub fn empty_message(pane: &Pane) -> Option<SharedString> {
    if let Some(err) = &pane.error {
        return Some(err.clone());
    }
    if !pane.entries().is_empty() || pane.loading {
        return None;
    }
    Some(match (&pane.search, pane.filter.is_empty()) {
        (Some(s), _) if s.running => "Searching…".into(),
        (Some(_), _) => "No results".into(),
        (None, false) => "No matches".into(),
        (None, true) => "Folder is empty".into(),
    })
}
