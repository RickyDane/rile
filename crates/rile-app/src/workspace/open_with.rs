//! Opening files with a chosen application: per-type default programs,
//! the "Open With" submenu and the application chooser.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::{
    WindowExt, h_flex,
    input::{Input, InputEvent, InputState},
    menu::{PopupMenu, PopupMenuItem},
    notification::Notification,
    v_flex,
};
use gpui_kit::assets::IconName;
use gpui_kit::{
    App, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, Window, div, prelude::FluentBuilder, px, uniform_list,
};
use rile_services::open_with::{self, AppInfo};

use super::Workspace;
use crate::views::WorkspaceHandle;

/// Opens a local file the way the user asked: with the default program
/// chosen in Settings for its type, else the system's default.
pub fn open_file(path: &Path, cx: &mut App) {
    // Deferred: callers (e.g. the Enter action) may run inside the workspace's
    // update, and reading the workspace there would panic (re-entrancy).
    let path = path.to_path_buf();
    cx.defer(move |cx| open_file_now(&path, cx));
}

fn open_file_now(path: &Path, cx: &mut App) {
    let app = cx
        .try_global::<WorkspaceHandle>()
        .and_then(|h| h.0.upgrade())
        .and_then(|ws| ws.read(cx).settings.default_app_for(path).cloned());
    match app {
        Some(app) => open_in(vec![path.to_path_buf()], app, cx),
        None => cx.open_with_system(path),
    }
}

/// Opens `paths` with the application bundle at `app` (in the background;
/// failures show a notification).
pub fn open_in(paths: Vec<PathBuf>, app: PathBuf, cx: &mut App) {
    let task = cx.background_spawn(async move { open_with::open_with(&paths, &app).map_err(|e| format!("{e:#}")) });
    cx.spawn(async move |cx| {
        if let Err(e) = task.await {
            cx.update(|cx| {
                if let Some(window) = cx.active_window() {
                    window.update(cx, |_, window, cx| window.push_notification(Notification::error(e), cx)).ok();
                }
            });
        }
    })
    .detach();
}

/// Adds an "Open With" submenu for `paths`: the applications that can open
/// the first one (default first), then "Other…" for any installed app.
pub fn open_with_submenu(menu: PopupMenu, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<PopupMenu>) -> PopupMenu {
    let Some(first) = paths.first() else { return menu };
    let apps = open_with::apps_for(first);
    let paths = Arc::new(paths);
    menu.submenu_with_icon(Some(crate::icons::menu_icon(IconName::AppWindow)), "Open With", window, cx, move |mut menu, _, _| {
        for (ix, app) in apps.iter().enumerate() {
            let label = if ix == 0 { format!("{} (default)", app.name) } else { app.name.clone() };
            let (paths, app_path) = (paths.clone(), app.path.clone());
            menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| open_in((*paths).clone(), app_path.clone(), cx)));
            if ix == 0 && apps.len() > 1 {
                menu = menu.separator();
            }
        }
        if !apps.is_empty() {
            menu = menu.separator();
        }
        let paths = paths.clone();
        menu.item(PopupMenuItem::new("Other…").on_click(move |_, window, cx| {
            let paths = (*paths).clone();
            choose_app(
                "Open With",
                format!("Choose an app to open {}", describe(&paths)),
                window,
                cx,
                move |app, _, cx| open_in(paths.clone(), app, cx),
            );
        }))
    })
}

fn describe(paths: &[PathBuf]) -> String {
    match paths {
        [one] => format!("\"{}\"", super::display_name(one)),
        many => super::item_count(many.len()),
    }
}

/// Shows the application chooser; `on_pick` receives the chosen bundle.
pub fn choose_app(
    title: &'static str,
    subtitle: String,
    window: &mut Window,
    cx: &mut App,
    on_pick: impl Fn(PathBuf, &mut Window, &mut App) + 'static,
) {
    let on_pick: Arc<dyn Fn(PathBuf, &mut Window, &mut App)> = Arc::new(on_pick);
    let view = cx.new(|cx| AppChooser::new(on_pick, window, cx));
    let input = view.read(cx).query.clone();
    let first = std::cell::Cell::new(true);
    let subtitle: SharedString = subtitle.into();
    window.open_dialog(cx, move |dialog, window, cx| {
        if first.replace(false) {
            input.update(cx, |i, cx| i.focus(window, cx));
        }
        crate::views::kit::dialog(dialog, IconName::AppWindow, title, Some(subtitle.clone()), 460., cx).child(view.clone())
    });
}

/// Searchable list of installed applications.
pub struct AppChooser {
    query: Entity<InputState>,
    apps: Option<Arc<Vec<AppInfo>>>,
    on_pick: Arc<dyn Fn(PathBuf, &mut Window, &mut App)>,
    _task: Task<()>,
    _sub: Subscription,
}

impl AppChooser {
    fn new(on_pick: Arc<dyn Fn(PathBuf, &mut Window, &mut App)>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search applications"));
        let sub = cx.subscribe_in(&query, window, |this: &mut Self, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::Change => cx.notify(),
            InputEvent::PressEnter { .. } => {
                if let Some(app) = this.filtered(cx).first().cloned() {
                    this.pick(app.path, window, cx);
                }
            }
            _ => {}
        });
        let load = cx.background_spawn(async { open_with::all_applications() });
        let task = cx.spawn(async move |this, cx| {
            let apps = load.await;
            this.update(cx, |this, cx| {
                this.apps = Some(Arc::new(apps));
                cx.notify();
            })
            .ok();
        });
        Self { query, apps: None, on_pick, _task: task, _sub: sub }
    }

    fn filtered(&self, cx: &App) -> Vec<AppInfo> {
        let q = self.query.read(cx).value().trim().to_lowercase();
        self.apps
            .iter()
            .flat_map(|a| a.iter())
            .filter(|a| q.is_empty() || a.name.to_lowercase().contains(&q))
            .cloned()
            .collect()
    }

    fn pick(&mut self, app: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        window.close_dialog(cx);
        (self.on_pick)(app, window, cx);
    }
}

impl Render for AppChooser {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ch = crate::theme::chrome(cx);
        let apps = Arc::new(self.filtered(cx));
        let loading = self.apps.is_none();
        let count = apps.len();
        v_flex()
            .gap(px(12.))
            .child(Input::new(&self.query).prefix(crate::icons::icon(IconName::Search).size(px(14.)).text_color(ch.text_faint)))
            .child(
                div()
                    .h(px(320.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(ch.hairline)
                    .when(loading, |d| d.child(crate::views::kit::hint("Loading applications…", cx)).p(px(12.)))
                    .when(!loading && count == 0, |d| d.child(crate::views::kit::hint("No matching applications", cx)).p(px(12.)))
                    .when(count > 0, |d| {
                        d.child(
                            uniform_list(
                                "app-list",
                                count,
                                cx.processor(move |_this, range: std::ops::Range<usize>, _, cx| {
                                    range
                                        .map(|ix| {
                                            let app = &apps[ix];
                                            let path = app.path.clone();
                                            h_flex()
                                                .id(("app", ix))
                                                .h(px(34.))
                                                .px(px(10.))
                                                .gap(px(10.))
                                                .rounded(px(6.))
                                                .cursor_pointer()
                                                .hover(|s| s.bg(ch.hover_strong))
                                                .child(crate::icons::app_icon(&app.path, 22., ch.text_muted, cx))
                                                .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).text_color(ch.text).child(app.name.clone()))
                                                .on_click(cx.listener(move |this, _, window, cx| this.pick(path.clone(), window, cx)))
                                        })
                                        .collect()
                                }),
                            )
                            .size_full()
                            .p(px(4.)),
                        )
                    }),
            )
    }
}

impl Workspace {
    /// Sets (or with `None` clears) the default program for `key`.
    pub fn set_default_app(&mut self, key: String, app: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        match app {
            Some(app) => self.settings.default_apps.insert(key, app),
            None => self.settings.default_apps.remove(&key),
        };
        self.save_settings(window, cx);
        cx.notify();
    }
}
