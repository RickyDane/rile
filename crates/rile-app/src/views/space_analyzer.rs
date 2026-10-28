//! The Space Analyzer page (`Location::SpaceAnalyzer`).
//!
//! Everything here renders from the cached scan in `Workspace::space`; no
//! filesystem work happens while rendering or navigating.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Sizable, h_flex,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::{
    AnyElement, Context, Div, ElementId, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, hsla, linear_color_stop, linear_gradient, prelude::FluentBuilder, px, relative,
};
use rile_services::space::{Category, Node, Scan, SuggestionKind};

use super::chrome::{Width, content_class};
use super::home::{PAGE_MAX_WIDTH, PAGE_TITLE_GAP, card, page_padding, page_title, page_title_top};
use crate::views::kit::PointerUnless;
use crate::art::{Art, art};
use crate::icons::icon;
use crate::theme::{Chrome, chrome};
use crate::workspace::Workspace;
use crate::workspace::space::{LARGEST_FILES, SpaceTab};

/// Bubbles drawn in the space map; smaller items share one "more" bubble.
const MAP_BUBBLES: usize = 36;
/// Rows listed per folder; the rest are summarized in one line.
const MAX_ROWS: usize = 250;
/// Cleanup rows shown per group until it is expanded.
const GROUP_PREVIEW: usize = 5;

fn size(bytes: u64) -> String {
    humansize::format_size(bytes, humansize::DECIMAL)
}

fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", count(n), if n == 1 { one } else { many })
}

fn ago(secs: u64) -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if secs == 0 {
        return "unknown date".into();
    }
    let d = now.saturating_sub(secs);
    match d {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", d / 60),
        3600..86_400 => format!("{} h ago", d / 3600),
        86_400..2_592_000 => format!("{} days ago", d / 86_400),
        2_592_000..31_536_000 => format!("{} months ago", d / 2_592_000),
        _ => format!("{} years ago", d / 31_536_000),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

fn text(value: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div().text_size(px(size)).text_color(color).child(value.into())
}

/// Category colours, tuned to read on both themes.
pub fn color(cat: Category, ch: &Chrome) -> Hsla {
    let l = if ch.dark { 0. } else { -0.12 };
    match cat {
        Category::Video => hsla(12. / 360., 0.82, 0.63 + l, 1.),
        Category::Images => hsla(40. / 360., 0.88, 0.58 + l, 1.),
        Category::Audio => hsla(282. / 360., 0.62, 0.70 + l, 1.),
        Category::Documents => hsla(212. / 360., 0.82, 0.66 + l, 1.),
        Category::Archives => hsla(338. / 360., 0.70, 0.66 + l, 1.),
        Category::Apps => hsla(170. / 360., 0.55, 0.52 + l, 1.),
        Category::Code => hsla(96. / 360., 0.45, 0.56 + l, 1.),
        Category::Other => hsla(220. / 360., 0.10, 0.62 + l, 1.),
    }
}

fn category_icon(node_is_dir: bool, cat: Category) -> IconName {
    if node_is_dir {
        return if cat == Category::Apps { IconName::AppWindowMac } else { IconName::Folder };
    }
    match cat {
        Category::Video => IconName::FileVideoCamera,
        Category::Images => IconName::FileImage,
        Category::Audio => IconName::FileMusic,
        Category::Documents => IconName::FileText,
        Category::Archives => IconName::FileArchive,
        Category::Apps => IconName::AppWindowMac,
        Category::Code => IconName::FileCode,
        Category::Other => IconName::File,
    }
}

fn badge(cat: Category, is_dir: bool, ch: &Chrome) -> Div {
    let c = color(cat, ch);
    div().size(px(30.)).flex_none().rounded(px(8.)).bg(c.opacity(0.14)).border_1().border_color(c.opacity(0.25))
        .flex().items_center().justify_center()
        .child(icon(category_icon(is_dir, cat)).size(px(15.)).text_color(c))
}

fn tip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut gpui_kit::App) -> gpui_kit::AnyView + 'static {
    let text = text.into();
    move |w, cx| Tooltip::new(text.clone()).build(w, cx)
}

/// A checkbox that doesn't also trigger the row it sits in.
fn select_box(id: impl Into<ElementId>, path: &Path, checked: bool, cx: &mut Context<Workspace>) -> impl IntoElement {
    let path = path.to_path_buf();
    div().flex_none().on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(
        Checkbox::new(id).checked(checked).pointer_unless(rile_services::space::is_protected(&path)).on_click(cx.listener(move |ws, _: &bool, _, cx| {
            cx.stop_propagation();
            ws.toggle_space_selection(path.clone(), cx);
        })),
    )
}

/// Reveal / trash buttons shown when hovering a row.
fn row_actions(id: usize, group: SharedString, path: &Path, cx: &mut Context<Workspace>) -> Div {
    let (reveal, trash) = (path.to_path_buf(), path.to_path_buf());
    h_flex().flex_none().gap_1().opacity(0.).group_hover(group, |s| s.opacity(1.))
        .child(Button::new(("space-reveal", id)).cursor_pointer().xsmall().ghost().icon(IconName::FolderOpen).tooltip("Show in Rile")
            .on_click(cx.listener(move |ws, _, w, cx| {
                cx.stop_propagation();
                ws.reveal_in_pane(reveal.clone(), w, cx);
            })))
        .when(!rile_services::space::is_protected(path), |d| d.child(Button::new(("space-trash", id)).cursor_pointer().xsmall().ghost().icon(IconName::Trash).tooltip(format!("Move to {}…", crate::platform_text::trash()))
            .on_click(cx.listener(move |ws, _, w, cx| {
                cx.stop_propagation();
                ws.trash_space_item(trash.clone(), w, cx);
            }))))
}

fn share_bar(share: f32, c: Hsla, ch: &Chrome) -> Div {
    div().h(px(3.)).w_full().rounded_full().bg(ch.hover_strong)
        .child(div().h_full().w(relative(share.clamp(0., 1.))).min_w(px(2.)).rounded_full().bg(c.opacity(0.8)))
}

pub fn render(ws: &mut Workspace, slot: usize, focus: Option<PathBuf>, window: &mut Window, cx: &mut Context<Workspace>) -> AnyElement {
    let ch = chrome(cx);
    let width = ws.slot_width(slot, window);
    let class = content_class(width);
    let (pad_x, _) = page_padding(class);
    let inner = width.min(PAGE_MAX_WIDTH) - pad_x * 2.;
    let viewport_h: f32 = window.viewport_size().height.into();

    // The results header is a slim row without a title, so the usual page
    // top padding would look much deeper than the side margins.
    let has_results = focus.as_ref().is_some_and(|f| ws.space.scan_for(f).is_some());
    let pad_top = if has_results { (pad_x - 20.).max(8.) } else { page_title_top(class) };
    let body = match &focus {
        None => start_page(ws, inner, class, &ch, cx),
        Some(focus) if ws.space.scan_for(focus).is_some() => {
            if ws.space.last_focus.as_ref() != Some(focus) {
                ws.space.last_focus = Some(focus.clone());
            }
            results(ws, focus, inner, viewport_h, &ch, cx)
        }
        Some(focus) if ws.space.scanning(focus) => scanning(ws, inner, &ch, cx),
        Some(focus) => match ws.space.error.clone() {
            Some(err) => error_state(&err, focus, &ch, cx),
            None => not_analyzed(focus, &ch, cx),
        },
    };
    let (selected, selected_bytes) = ws.space.selection();
    let bar = (!selected.is_empty()).then(|| action_bar(selected.len(), selected_bytes, ws.space.trashing, &ch, cx));
    div().relative().size_full().bg(ch.browser)
        .child(div().id(("space-page", slot)).debug_selector(|| "space-page".into()).size_full().overflow_y_scroll()
            .child(v_flex().w_full().max_w(px(PAGE_MAX_WIDTH)).mx_auto().px(px(pad_x)).pt(px(pad_top))
                .pb(px(if bar.is_some() { 96. } else { 28. })).gap(px(22.)).child(body)))
        .children(bar.map(|b| div().absolute().bottom(px(18.)).left_0().right_0().flex().justify_center().px(px(16.)).child(b)))
        .into_any_element()
}

// ----- start / progress / error states --------------------------------------

fn footnote(ch: &Chrome) -> AnyElement {
    h_flex().gap_2().text_color(ch.text_faint)
        .child(icon(IconName::Info).size(px(12.)))
        .child(text(format!("Analysis runs on {}. Nothing is removed without asking — cleaned up items go to the {}.", crate::platform_text::this_computer_lower(), crate::platform_text::trash()), 11., ch.text_faint))
        .into_any_element()
}

fn start_page(ws: &Workspace, inner: f32, class: Width, ch: &Chrome, cx: &mut Context<Workspace>) -> AnyElement {
    let wide = inner >= 720.;
    let mut bubbles = div().relative().w(px(170.)).h(px(130.)).flex_none();
    for (x, y, d, cat) in [(48., 14., 92., Category::Video), (6., 52., 46., Category::Images), (128., 4., 40., Category::Documents),
        (126., 72., 54., Category::Code), (28., 100., 28., Category::Audio), (104., 112., 18., Category::Archives)] {
        let c = color(cat, ch);
        bubbles = bubbles.child(div().absolute().left(px(x)).top(px(y)).size(px(d)).rounded_full().border_1().border_color(c.opacity(0.5))
            .bg(linear_gradient(145., linear_color_stop(c.opacity(0.35), 0.), linear_color_stop(c.opacity(0.10), 1.))));
    }
    let hero = card(ch).id("space-welcome").debug_selector(|| "space-welcome".into())
        .child(h_flex().gap(px(28.)).p(px(if wide { 28. } else { 20. })).items_center()
            .child(v_flex().flex_1().min_w_0().gap(px(10.))
                .child(text("See what’s taking up space.", 18., ch.text_strong).font_weight(FontWeight::SEMIBOLD))
                .child(text("Rile maps a folder in one pass, finds your largest files and suggests what can safely go — caches, build output, old installers. Browse the results instantly; nothing is rescanned while you explore.", 12., ch.text_muted).max_w(px(560.)))
                .child(h_flex().mt(px(6.)).gap_2().flex_wrap()
                    .child(Button::new("space-choose").cursor_pointer().primary().icon(IconName::FolderOpen).label("Choose a folder…")
                        .on_click(cx.listener(|ws, _, w, cx| ws.choose_space_root(w, cx))))
                    .when_some(dirs::home_dir(), |d, home| d.child(Button::new("space-home").cursor_pointer().outline().icon(IconName::House).label("Analyze Home")
                        .on_click(cx.listener(move |ws, _, w, cx| ws.analyze_folder(home.clone(), w, cx)))))))
            .when(wide, |d| d.child(bubbles)));

    // Quick targets: common folders and every mounted volume.
    // (icon, name, folder, detail line, disk usage)
    #[allow(clippy::type_complexity)]
    let mut targets: Vec<(IconName, String, PathBuf, Option<String>, Option<f32>)> = Vec::new();
    for (name, dir, ic) in [("Downloads", dirs::download_dir(), IconName::Download), ("Documents", dirs::document_dir(), IconName::FileText),
        ("Desktop", dirs::desktop_dir(), IconName::Monitor), ("Movies", dirs::video_dir(), IconName::Clapperboard)] {
        if let Some(dir) = dir.filter(|d| d.is_dir()) {
            targets.push((ic, name.into(), dir, None, None));
        }
    }
    for vol in &ws.volumes {
        let detail = format!("{} free of {}", size(vol.available), size(vol.total));
        targets.push((if vol.removable { IconName::Usb } else { IconName::HardDrive }, vol.name.clone(), vol.mount_point.clone(), Some(detail), Some(vol.used_fraction())));
    }
    let per_row = if inner >= 980. { 4 } else if inner >= 640. { 3 } else if inner >= 420. { 2 } else { 1 };
    let mut grid = v_flex().gap(px(10.));
    for (r, chunk) in targets.chunks(per_row).enumerate() {
        let mut row = h_flex().gap(px(10.));
        for (i, (ic, name, dir, detail, used)) in chunk.iter().cloned().enumerate() {
            let cached = ws.space.scan_for(&dir).is_some();
            let path_line = detail.unwrap_or_else(|| dir.display().to_string());
            let target = dir.clone();
            row = row.child(card(ch).id(("space-target", r * 16 + i)).flex_1().min_w_0().px(px(14.)).py(px(13.)).cursor_pointer()
                .hover(|s| s.border_color(ch.accent.opacity(0.5)))
                .on_click(cx.listener(move |ws, _, w, cx| ws.analyze_folder(target.clone(), w, cx)))
                .child(v_flex().gap(px(8.))
                    .child(h_flex().gap(px(10.))
                        .child(div().size(px(30.)).flex_none().rounded(px(8.)).bg(ch.accent.opacity(0.10)).flex().items_center().justify_center()
                            .child(icon(ic).size(px(15.)).text_color(ch.accent)))
                        .child(v_flex().min_w_0().flex_1().gap(px(2.))
                            .child(h_flex().gap_2().child(text(name, 12., ch.text_strong).font_weight(FontWeight::MEDIUM).truncate())
                                .when(cached, |d| d.child(text("Ready", 10., ch.success).px(px(5.)).rounded(px(4.)).bg(ch.success.opacity(0.12)))))
                            .child(text(path_line, 10., ch.text_faint).truncate())))
                    .when_some(used, |d, used| d.child(share_bar(used, if used > 0.9 { color(Category::Video, ch) } else { ch.accent }, ch)))));
        }
        for _ in chunk.len()..per_row {
            row = row.child(div().flex_1());
        }
        grid = grid.child(row);
    }

    let mut page = v_flex().gap(px(22.))
        .child(hero)
        .child(v_flex().gap(px(10.)).child(text("Quick start", 12., ch.text_muted).font_weight(FontWeight::SEMIBOLD)).child(grid));
    if !ws.space.scans.is_empty() {
        let mut recent = v_flex().p(px(6.));
        for (i, scan) in ws.space.scans.iter().enumerate() {
            let root = scan.root.clone();
            recent = recent.child(h_flex().id(("space-recent", i)).gap(px(12.)).px(px(10.)).py(px(9.)).rounded(px(8.)).cursor_pointer()
                .hover(|s| s.bg(ch.hover))
                .on_click(cx.listener(move |ws, _, w, cx| ws.space_navigate(root.clone(), w, cx)))
                .child(badge(scan.tree.dominant(), true, ch))
                .child(v_flex().flex_1().min_w_0().gap(px(2.))
                    .child(text(file_name(&scan.root), 12., ch.text_strong).truncate())
                    .child(text(format!("{} · analyzed {}", scan.root.display(), ago(scan.finished.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()))), 10., ch.text_faint).truncate()))
                .child(text(size(scan.tree.size), 12., ch.text_muted).flex_none()));
        }
        page = page.child(v_flex().gap(px(10.)).child(text("Recent analyses", 12., ch.text_muted).font_weight(FontWeight::SEMIBOLD)).child(card(ch).child(recent)));
    }
    v_flex()
        .child(page_title("Space Analyzer", class, ch).mb(px(PAGE_TITLE_GAP)))
        .child(page.child(footnote(ch)))
        .into_any_element()
}

fn scanning(ws: &Workspace, inner: f32, ch: &Chrome, cx: &mut Context<Workspace>) -> AnyElement {
    let Some(run) = ws.space.running.as_ref() else { return div().into_any_element() };
    let p = &run.progress;
    let load = |a: &std::sync::atomic::AtomicU64| a.load(std::sync::atomic::Ordering::Relaxed);
    let elapsed = run.started.elapsed().as_secs_f32();
    let current = p.current();
    let current = current.strip_prefix(&run.root).map(|r| r.display().to_string()).unwrap_or_default();
    let mut stats = h_flex().gap(px(10.)).flex_wrap();
    for (label, value) in [("Files", count(load(&p.files))), ("Folders", count(load(&p.dirs))), ("Size so far", size(load(&p.bytes))), ("Elapsed", format!("{elapsed:.1} s"))] {
        stats = stats.child(v_flex().flex_1().min_w(px(110.)).gap(px(4.)).px(px(14.)).py(px(11.)).rounded(px(9.)).bg(ch.hover)
            .child(text(label, 10., ch.text_faint))
            .child(text(value, 17., ch.text_strong).font_weight(FontWeight::SEMIBOLD)));
    }
    card(ch).id("space-progress").debug_selector(|| "space-progress".into())
        .w_full().max_w(px(680.)).mx_auto().mt(px(24.)).p(px(if inner > 600. { 30. } else { 20. }))
        .child(v_flex().gap(px(18.))
            .child(h_flex().gap(px(16.)).child(art(Art::Folder, 46.))
                .child(v_flex().flex_1().min_w_0().gap(px(3.))
                    .child(text("ANALYZING", 10., ch.accent).font_weight(FontWeight::SEMIBOLD))
                    .child(text(file_name(&run.root), 21., ch.text_strong).font_weight(FontWeight::SEMIBOLD).truncate())
                    .child(text(run.root.display().to_string(), 11., ch.text_faint).truncate()))
                .child(Button::new("space-cancel").cursor_pointer().outline().small().label("Cancel").on_click(cx.listener(|ws, _, w, cx| {
                    ws.cancel_space_scan(cx);
                    ws.open_location(rile_core::history::Location::SpaceAnalyzer(None), w, cx);
                }))))
            .child(crate::progress::activity_bar("space-scan-bar", None, 5., cx))
            .child(stats)
            .child(text(if current.is_empty() { "Reading…".to_string() } else { current }, 11., ch.text_faint).truncate())
            .child(text("Every folder is measured once. When it’s done you can explore any subfolder instantly.", 11., ch.text_muted)))
        .into_any_element()
}

fn error_state(message: &str, focus: &Path, ch: &Chrome, cx: &mut Context<Workspace>) -> AnyElement {
    let retry = focus.to_path_buf();
    card(ch).id("space-error").debug_selector(|| "space-error".into()).max_w(px(660.)).w_full().mx_auto().mt(px(24.)).p_8()
        .child(v_flex().gap(px(14.)).child(icon(IconName::TriangleAlert).size(px(28.)).text_color(ch.text_muted))
            .child(text("This folder couldn’t be analyzed.", 20., ch.text_strong).font_weight(FontWeight::SEMIBOLD))
            .child(text(focus.display().to_string(), 11., ch.text_faint).truncate())
            .child(text(message.to_string(), 12., ch.text_muted))
            .child(h_flex().gap_2().flex_wrap()
                .child(Button::new("space-retry").cursor_pointer().primary().label("Try again").icon(IconName::RotateCw)
                    .on_click(cx.listener(move |ws, _, w, cx| { ws.space.error = None; ws.analyze_folder(retry.clone(), w, cx); })))
                .child(Button::new("space-error-choose").cursor_pointer().outline().label("Choose another folder")
                    .on_click(cx.listener(|ws, _, w, cx| ws.choose_space_root(w, cx))))))
        .into_any_element()
}

fn not_analyzed(focus: &Path, ch: &Chrome, cx: &mut Context<Workspace>) -> AnyElement {
    let target = focus.to_path_buf();
    card(ch).id("space-stale").max_w(px(660.)).w_full().mx_auto().mt(px(24.)).p_8()
        .child(v_flex().gap(px(14.)).child(art(Art::Folder, 46.))
            .child(text(format!("{} isn’t analyzed yet", file_name(focus)), 20., ch.text_strong).font_weight(FontWeight::SEMIBOLD))
            .child(text(focus.display().to_string(), 11., ch.text_faint).truncate())
            .child(h_flex().gap_2()
                .child(Button::new("space-analyze-now").cursor_pointer().primary().icon(IconName::ChartPie).label("Analyze")
                    .on_click(cx.listener(move |ws, _, w, cx| ws.analyze_folder(target.clone(), w, cx))))
                .child(Button::new("space-stale-choose").cursor_pointer().outline().label("Choose another folder")
                    .on_click(cx.listener(|ws, _, w, cx| ws.choose_space_root(w, cx))))))
        .into_any_element()
}

fn action_bar(n: usize, bytes: u64, busy: bool, ch: &Chrome, cx: &mut Context<Workspace>) -> AnyElement {
    let (top, bottom) = ch.surface_colors();
    h_flex().id("space-action-bar").debug_selector(|| "space-action-bar".into()).gap(px(14.)).pl(px(16.)).pr(px(8.)).py(px(8.))
        .rounded(px(12.)).border_1().border_color(ch.card_border).shadow_lg()
        .bg(linear_gradient(180., linear_color_stop(top, 0.), linear_color_stop(bottom, 1.)))
        .child(icon(IconName::Check).size(px(15.)).text_color(ch.accent))
        .child(v_flex().gap(px(1.))
            .child(text(format!("{} selected", plural(n as u64, "item", "items")), 12., ch.text_strong).font_weight(FontWeight::SEMIBOLD))
            .child(text(format!("{} will be freed", size(bytes)), 10., ch.text_muted)))
        .child(Button::new("space-clear").cursor_pointer().small().ghost().label("Clear").on_click(cx.listener(|ws, _, _, cx| {
            ws.space.selected.clear();
            cx.notify();
        })))
        .child(Button::new("space-trash-selected").cursor_pointer().small().danger().icon(IconName::Trash).label(if busy { "Moving…".to_string() } else { format!("Move to {}…", crate::platform_text::trash()) })
            .pointer_unless(busy).on_click(cx.listener(|ws, _, w, cx| ws.trash_space_selection(w, cx))))
        .into_any_element()
}

// ----- results ----------------------------------------------------------------

fn results(ws: &mut Workspace, focus: &Path, inner: f32, viewport_h: f32, ch: &Chrome, cx: &mut Context<Workspace>) -> AnyElement {
    let tab = ws.space.tab;
    let largest = (tab == SpaceTab::Files).then(|| ws.space.largest(focus).to_vec());
    let ws = &*ws;
    let space = &ws.space;
    let Some(scan) = space.scan_for(focus) else { return div().into_any_element() };
    let Some(node) = scan.node(focus) else { return div().into_any_element() };
    let rescanning = space.running.as_ref().is_some_and(|r| scan.root.starts_with(&r.root) || r.root.starts_with(&scan.root));
    let reclaim: u64 = scan.suggestions_in(focus).map(|s| s.size).sum();

    let header = header(ws, scan, focus, rescanning, ch, cx);
    let summary = summary(ws, scan, node, focus, inner, ch);
    let mut tabs = crate::views::kit::segmented(cx);
    for (t, label) in [(SpaceTab::Overview, "Overview".to_string()), (SpaceTab::Files, "Largest files".to_string()),
        (SpaceTab::Cleanup, if reclaim > 0 { format!("Clean up · {}", size(reclaim)) } else { "Clean up".into() })] {
        tabs = tabs.child(crate::views::kit::segment(SharedString::from(format!("space-tab-{t:?}")), label, tab == t, cx)
            .on_click(cx.listener(move |ws, _, _, cx| { ws.space.tab = t; cx.notify(); })));
    }
    let content = match tab {
        SpaceTab::Overview => overview(ws, scan, node, focus, inner, viewport_h, ch, cx),
        SpaceTab::Files => files_tab(ws, focus, largest.unwrap_or_default(), ch, cx),
        SpaceTab::Cleanup => cleanup_tab(ws, scan, focus, reclaim, ch, cx),
    };
    v_flex().gap(px(18.)).child(header).child(summary).child(tabs).child(content).child(footnote(ch)).into_any_element()
}

/// One compact row: breadcrumbs on the left, scan facts and actions on the right.
fn header(ws: &Workspace, scan: &Scan, focus: &Path, rescanning: bool, ch: &Chrome, cx: &mut Context<Workspace>) -> AnyElement {
    // Breadcrumbs from the scan root down to the shown folder.
    let mut crumbs = h_flex().gap(px(2.)).min_w_0().flex_1().overflow_hidden()
        .child(icon(IconName::ChartPie).size(px(15.)).text_color(ch.accent).mr(px(4.)));
    let mut trail: Vec<PathBuf> = focus.ancestors().take_while(|p| p.starts_with(&scan.root)).map(Path::to_path_buf).collect();
    trail.reverse();
    let last = trail.len().saturating_sub(1);
    for (i, dir) in trail.into_iter().enumerate() {
        if i > 0 {
            crumbs = crumbs.child(icon(IconName::ChevronRight).size(px(12.)).text_color(ch.text_faint).flex_none());
        }
        let current = i == last;
        let (full, label) = (dir.display().to_string(), file_name(&dir));
        crumbs = crumbs.child(div().id(("space-crumb", i)).px(px(5.)).py(px(2.)).rounded(px(5.)).truncate()
            .when(current, |d| d.flex_none())
            .text_size(px(if current { 15. } else { 13. }))
            .text_color(if current { ch.text_strong } else { ch.text_muted })
            .when(current, |d| d.font_weight(FontWeight::SEMIBOLD))
            .tooltip(tip(full))
            .when(!current, |d| d.cursor_pointer().hover(|s| s.bg(ch.hover).text_color(ch.text_strong))
                .on_click(cx.listener(move |ws, _, w, cx| ws.space_navigate(dir.clone(), w, cx))))
            .child(label));
    }
    let rescan = focus.to_path_buf();
    let finished = scan.finished.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut facts = h_flex().gap(px(8.)).flex_none();
    if rescanning {
        let n = ws.space.running.as_ref().map_or(0, |r| r.progress.files.load(std::sync::atomic::Ordering::Relaxed));
        facts = facts.child(h_flex().gap(px(5.)).child(div().size(px(6.)).rounded_full().bg(ch.accent))
            .child(text(format!("Rescanning · {}", count(n)), 11., ch.accent)));
    } else {
        facts = facts.child(div().id("space-scan-facts")
            .tooltip(tip(format!("{} measured in {:.1} s", plural(scan.tree.files as u64, "file", "files"), scan.took.as_secs_f32())))
            .child(text(format!("Analyzed {}", ago(finished)), 11., ch.text_faint)));
    }
    if scan.unreadable > 0 {
        facts = facts.child(div().id("space-unreadable")
            .tooltip(tip(format!("{} couldn’t be read and aren’t counted. {}", plural(scan.unreadable, "folder", "folders"), if cfg!(target_os = "macos") { "On macOS, grant Rile Full Disk Access to include them." } else { "They are protected or belong to another user." })))
            .child(icon(IconName::TriangleAlert).size(px(13.)).text_color(color(Category::Images, ch))));
    }
    h_flex().id("space-header").debug_selector(|| "space-header".into()).gap(px(12.)).h(px(34.))
        .child(crumbs)
        .child(facts)
        .child(h_flex().gap(px(2.)).flex_none()
            .child(Button::new("space-rescan").cursor_pointer().small().ghost().icon(IconName::RotateCw).pointer_unless(rescanning)
                .tooltip("Rescan").on_click(cx.listener(move |ws, _, _, cx| ws.rescan_space(&rescan, cx))))
            .child(Button::new("space-change").cursor_pointer().small().ghost().icon(IconName::FolderOpen).tooltip("Analyze another folder…")
                .on_click(cx.listener(|ws, _, w, cx| ws.choose_space_root(w, cx)))))
        .into_any_element()
}

fn summary(ws: &Workspace, scan: &Scan, node: &Node, focus: &Path, inner: f32, ch: &Chrome) -> AnyElement {
    let total = node.size.max(1);
    // Category breakdown as one stacked bar.
    let mut cats: Vec<(Category, u64)> = match &node.breakdown {
        Some(b) => Category::ALL.iter().zip(b.iter()).map(|(c, v)| (*c, *v)).filter(|(_, v)| *v > 0).collect(),
        None => vec![(node.category(), node.size)],
    };
    cats.sort_by_key(|c| std::cmp::Reverse(c.1));
    let mut bar = h_flex().h(px(10.)).w_full().rounded_full().overflow_hidden().bg(ch.hover_strong);
    for (cat, v) in &cats {
        bar = bar.child(div().h_full().w(relative(*v as f32 / total as f32)).bg(color(*cat, ch)));
    }
    let mut legend = h_flex().gap(px(14.)).flex_wrap();
    for (cat, v) in cats.iter().take(6) {
        legend = legend.child(h_flex().gap(px(6.))
            .child(div().size(px(8.)).rounded_full().bg(color(*cat, ch)))
            .child(text(cat.label(), 11., ch.text_muted))
            .child(text(size(*v), 11., ch.text_faint)));
    }
    let folders = node.folders();
    let mut headline = v_flex().gap(px(4.)).w(px(if inner >= 900. { 260. } else { 220. })).flex_none()
        .child(text(size(node.size), 30., ch.text_strong).font_weight(FontWeight::SEMIBOLD))
        .child(text(format!("{} · {}", plural(node.files as u64, "file", "files"), plural(folders, "folder", "folders")), 11., ch.text_muted));
    if let Some(vol) = super::chrome::volume_for(ws, focus).filter(|v| v.total > 0) {
        let used = vol.total.saturating_sub(vol.available);
        let share = node.size as f32 / vol.total as f32;
        headline = headline.child(v_flex().mt(px(10.)).gap(px(6.))
            .child(h_flex().justify_between().gap_2()
                .child(text(vol.name.clone(), 10., ch.text_muted).truncate())
                .child(text(format!("{} free", size(vol.available)), 10., ch.text_faint)))
            .child(h_flex().h(px(5.)).w_full().rounded_full().overflow_hidden().bg(ch.hover_strong)
                .child(div().h_full().w(relative(share.min(1.))).bg(ch.accent))
                .child(div().h_full().w(relative((used as f32 / vol.total as f32 - share).max(0.))).bg(ch.text_faint.opacity(0.45))))
            .child(text(format!("{:.1}% of the disk", share * 100.), 10., ch.text_faint)));
    }
    let breakdown = v_flex().flex_1().min_w(px(240.)).gap(px(12.))
        .child(text("What’s in here", 11., ch.text_muted).font_weight(FontWeight::SEMIBOLD))
        .child(bar).child(legend);
    let row = h_flex().gap(px(28.)).flex_wrap().items_start().p(px(20.)).child(headline).child(breakdown);
    let _ = scan;
    card(ch).id("space-summary").debug_selector(|| "space-summary".into()).child(row).into_any_element()
}

// ----- overview: space map + contents -----------------------------------------

#[allow(clippy::too_many_arguments)]
fn overview(ws: &Workspace, scan: &Scan, node: &Node, focus: &Path, inner: f32, viewport_h: f32, ch: &Chrome, cx: &mut Context<Workspace>) -> AnyElement {
    if node.children.is_empty() || node.size == 0 {
        return card(ch).p_8().child(v_flex().items_center().gap_2()
            .child(icon(IconName::Folder).size(px(26.)).text_color(ch.text_faint))
            .child(text("This folder is empty", 13., ch.text_muted))).into_any_element();
    }
    let side_by_side = inner >= 900.;
    let height = (viewport_h - 420.).clamp(340., 620.);
    let map_w = if side_by_side { inner * 0.58 } else { inner };
    let map = space_map(ws, node, focus, map_w, height, ch, cx);
    let list = contents(ws, scan, node, focus, if side_by_side { Some(height) } else { None }, ch, cx);
    if side_by_side {
        h_flex().gap(px(14.)).items_start()
            .child(div().w(px(map_w)).flex_none().child(map))
            .child(div().flex_1().min_w_0().child(list)).into_any_element()
    } else {
        v_flex().gap(px(14.)).child(map).child(list).into_any_element()
    }
}

fn space_map(ws: &Workspace, node: &Node, focus: &Path, width: f32, height: f32, ch: &Chrome, cx: &mut Context<Workspace>) -> AnyElement {
    let shown: Vec<(usize, &Node)> = node.children.iter().enumerate().filter(|(_, c)| c.size > 0).take(MAP_BUBBLES).collect();
    let rest: u64 = node.children.iter().filter(|c| c.size > 0).skip(MAP_BUBBLES).map(|c| c.size).sum();
    let rest_n = node.children.iter().filter(|c| c.size > 0).count().saturating_sub(MAP_BUBBLES);
    let max = shown.first().map_or(1, |(_, c)| c.size).max(rest).max(1) as f64;
    let mut radii: Vec<f32> = shown.iter().map(|(_, c)| ((c.size as f64 / max).sqrt() * 100.) as f32).collect();
    if rest > 0 {
        radii.push(((rest as f64 / max).sqrt() * 100.) as f32);
    }
    // Keep tiny items visible and clickable.
    let floor = radii.first().copied().unwrap_or(1.) * 0.07;
    for r in &mut radii {
        *r = r.max(floor);
    }
    let centers = super::plugin_page::pack(&radii);
    let (min_x, min_y, max_x, max_y) = centers.iter().zip(&radii).fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |(a, b, c, d), (&(x, y), &r)| {
        (a.min(x - r), b.min(y - r), c.max(x + r), d.max(y + r))
    });
    let (area_w, area_h) = (width - 32., height - 70.);
    let scale = (area_w / (max_x - min_x)).min(area_h / (max_y - min_y));
    let (off_x, off_y) = ((area_w - (max_x - min_x) * scale) / 2., (area_h - (max_y - min_y) * scale) / 2.);
    let mut chart = div().relative().w(px(area_w)).h(px(area_h));
    for (k, (&(x, y), &r)) in centers.iter().zip(&radii).enumerate() {
        let d = 2. * r * scale;
        let left = off_x + (x - r - min_x) * scale;
        let top = off_y + (y - r - min_y) * scale;
        let Some(&(ix, child)) = shown.get(k) else {
            // The bubble that stands for everything too small to draw.
            chart = chart.child(div().id("space-bubble-rest").absolute().left(px(left)).top(px(top)).size(px(d)).rounded_full()
                .border_1().border_color(ch.text_faint.opacity(0.4)).bg(ch.hover)
                .tooltip(tip(format!("{rest_n} smaller items · {}", size(rest))))
                .flex().flex_col().items_center().justify_center()
                .when(d > 54., |el| el.child(text(format!("+{rest_n}"), 12., ch.text_muted).font_weight(FontWeight::SEMIBOLD))
                    .child(text(size(rest), 10., ch.text_faint))));
            continue;
        };
        let cat = child.category();
        let c = color(cat, ch);
        let path = focus.join(&*child.name);
        let selected = ws.space.is_selected(&path);
        let hovered = ws.space.hovered.as_ref() == Some(&path);
        let label = child.name.to_string();
        let tip_text = format!("{} · {} · {:.1}%{}", label, size(child.size), child.size as f64 / node.size.max(1) as f64 * 100.,
            if child.is_dir { " · click to open" } else { " · click to select" });
        let (hover_path, click_path, is_dir) = (path.clone(), path.clone(), child.is_dir);
        let mut bubble = div().id(("space-bubble", ix)).absolute().left(px(left)).top(px(top)).size(px(d)).rounded_full()
            .cursor_pointer().overflow_hidden().border_1()
            .border_color(if selected { ch.accent } else { c.opacity(if hovered { 0.95 } else { 0.55 }) })
            .when(selected || hovered, |el| el.border_2())
            .bg(linear_gradient(150., linear_color_stop(c.opacity(if hovered { 0.48 } else { 0.32 }), 0.), linear_color_stop(c.opacity(if hovered { 0.22 } else { 0.10 }), 1.)))
            .hover(|s| s.border_color(c))
            .tooltip(tip(tip_text))
            .on_hover(cx.listener(move |ws, over: &bool, _, cx| {
                let next = over.then(|| hover_path.clone());
                if (*over || ws.space.hovered.as_ref() == Some(&hover_path)) && ws.space.hovered != next {
                    ws.space.hovered = next;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |ws, _, w, cx| {
                if is_dir { ws.space_navigate(click_path.clone(), w, cx) } else { ws.toggle_space_selection(click_path.clone(), cx) }
            }))
            .flex().flex_col().items_center().justify_center().gap(px(2.)).px(px(d * 0.12));
        if d > 46. {
            bubble = bubble.when(d > 96., |el| el.child(icon(category_icon(child.is_dir, cat)).size(px(if d > 180. { 22. } else { 16. })).text_color(c)))
                .child(text(label, if d > 160. { 13. } else { 11. }, ch.text_strong).font_weight(FontWeight::SEMIBOLD).truncate().max_w(px(d * 0.78)))
                .when(d > 64., |el| el.child(text(size(child.size), 10., ch.text_muted)));
        }
        if selected {
            bubble = bubble.child(div().absolute().top(px(d * 0.12)).right(px(d * 0.12)).size(px(16.)).rounded_full().bg(ch.accent)
                .flex().items_center().justify_center().child(icon(IconName::Check).size(px(10.)).text_color(ch.browser)));
        }
        chart = chart.child(bubble);
    }
    // Going back up sits with the bubbles you drill into.
    let up = focus.to_path_buf();
    let at_root = ws.space.scan_for(focus).is_some_and(|s| s.root == focus);
    let parent_name = focus.parent().map(file_name);
    let up_tip = match (&parent_name, at_root) {
        (Some(p), true) => format!("Analyze {p}"),
        (Some(p), false) => format!("Back to {p}"),
        (None, _) => "Already at the top".into(),
    };
    card(ch).id("space-map").debug_selector(|| "space-map".into()).h(px(height)).p(px(16.)).flex().flex_col().gap(px(10.))
        .child(h_flex().justify_between().gap_2()
            .child(h_flex().gap(px(10.)).min_w_0()
                .child(Button::new("space-up").cursor_pointer().small().outline().icon(IconName::ArrowUp).tooltip(up_tip).pointer_unless(parent_name.is_none())
                    .on_click(cx.listener(move |ws, _, w, cx| ws.space_up(&up, w, cx))))
                .child(v_flex().gap(px(2.)).min_w_0()
                    .child(text(format!("Space map · {}", file_name(focus)), 12., ch.text_strong).font_weight(FontWeight::SEMIBOLD).truncate())
                    .child(text("Bubble area is size · click a folder to open it", 10., ch.text_faint))))
            .child(text(format!("{} items", count(node.children.len() as u64)), 10., ch.text_faint)))
        .child(chart)
        .into_any_element()
}

fn contents(ws: &Workspace, scan: &Scan, node: &Node, focus: &Path, height: Option<f32>, ch: &Chrome, cx: &mut Context<Workspace>) -> AnyElement {
    let total = node.size.max(1);
    let mut rows = v_flex().p(px(6.));
    for (ix, child) in node.children.iter().enumerate().take(MAX_ROWS) {
        let path = focus.join(&*child.name);
        let cat = child.category();
        let c = color(cat, ch);
        let selected = ws.space.is_selected(&path);
        let hovered = ws.space.hovered.as_ref() == Some(&path);
        let suggested = scan.suggestions.iter().find(|s| s.path == path);
        let detail = if child.is_dir {
            format!("{} · {}", plural(child.files as u64, "file", "files"), ago(child.modified_secs()))
        } else {
            format!("{} · {}", cat.label(), ago(child.modified_secs()))
        };
        let group: SharedString = format!("space-row-{ix}").into();
        let (click, hover_path, is_dir) = (path.clone(), path.clone(), child.is_dir);
        rows = rows.child(h_flex().id(("space-row", ix)).debug_selector(move || format!("space-row-{ix}")).group(group.clone())
            .gap(px(10.)).px(px(8.)).py(px(7.)).rounded(px(8.)).cursor_pointer()
            .when(hovered, |d| d.bg(ch.hover)).when(selected, |d| d.bg(ch.accent.opacity(0.08)))
            .hover(|s| s.bg(ch.hover))
            .on_hover(cx.listener(move |ws, over: &bool, _, cx| {
                let next = over.then(|| hover_path.clone());
                if (*over || ws.space.hovered.as_ref() == Some(&hover_path)) && ws.space.hovered != next {
                    ws.space.hovered = next;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |ws, _, w, cx| {
                if is_dir { ws.space_navigate(click.clone(), w, cx) } else { ws.toggle_space_selection(click.clone(), cx) }
            }))
            .child(select_box(("space-row-check", ix), &path, selected, cx))
            .child(badge(cat, child.is_dir, ch))
            .child(v_flex().flex_1().min_w_0().gap(px(3.))
                .child(h_flex().gap(px(6.)).min_w_0()
                    .child(text(child.name.to_string(), 12., ch.text_strong).truncate())
                    .when_some(suggested, |d, s| d.child(text(s.note.clone(), 9., ch.success).flex_none().px(px(5.)).rounded(px(4.)).bg(ch.success.opacity(0.12)))))
                .child(text(detail, 10., ch.text_faint).truncate())
                .child(share_bar(child.size as f32 / total as f32, c, ch)))
            .child(v_flex().items_end().flex_none().gap(px(1.))
                .child(text(size(child.size), 12., ch.text))
                .child(text(format!("{:.1}%", child.size as f64 / total as f64 * 100.), 10., ch.text_faint)))
            .child(row_actions(ix, group, &path, cx)));
    }
    if node.children.len() > MAX_ROWS {
        let rest: u64 = node.children.iter().skip(MAX_ROWS).map(|c| c.size).sum();
        rows = rows.child(text(format!("{} more items · {}", count((node.children.len() - MAX_ROWS) as u64), size(rest)), 11., ch.text_faint).px(px(12.)).py(px(10.)));
    }
    let list = div().id("space-contents-scroll").debug_selector(|| "space-contents-scroll".into()).flex_1().min_h_0()
        .when(height.is_some(), |d| d.overflow_y_scroll())
        .child(rows);
    card(ch).id("space-contents").debug_selector(|| "space-contents".into()).flex().flex_col().overflow_hidden()
        .when_some(height, |d, h| d.h(px(h)))
        // Contain the inner list's scrolling so it doesn't move the page.
        .when(height.is_some(), |d| d.on_scroll(|_, _, cx| cx.stop_propagation()))
        .child(h_flex().px(px(16.)).py(px(13.)).justify_between().border_b_1().border_color(ch.hairline)
            .child(text("Contents", 12., ch.text_strong).font_weight(FontWeight::SEMIBOLD))
            .child(text("Largest first", 10., ch.text_faint)))
        .child(list)
        .into_any_element()
}

// ----- largest files ----------------------------------------------------------

fn files_tab(ws: &Workspace, focus: &Path, files: Vec<rile_services::space::FileHit>, ch: &Chrome, cx: &mut Context<Workspace>) -> AnyElement {
    let filter = ws.space.file_filter;
    let mut chips = h_flex().gap(px(6.)).flex_wrap();
    let options = std::iter::once(None).chain(Category::ALL.iter().copied().map(Some));
    for (i, opt) in options.enumerate() {
        let active = filter == opt;
        let label = opt.map_or("All", Category::label);
        chips = chips.child(h_flex().id(("space-filter", i)).gap(px(6.)).px(px(10.)).py(px(4.)).rounded_full().cursor_pointer().border_1()
            .border_color(if active { ch.accent.opacity(0.6) } else { ch.hairline })
            .bg(if active { ch.accent.opacity(0.12) } else { gpui_kit::transparent_black() })
            .hover(|s| s.bg(ch.hover))
            .on_click(cx.listener(move |ws, _, _, cx| { ws.space.file_filter = opt; cx.notify(); }))
            .when_some(opt, |d, cat| d.child(div().size(px(7.)).rounded_full().bg(color(cat, ch))))
            .child(text(label, 11., if active { ch.text_strong } else { ch.text_muted })));
    }
    let total: u64 = files.iter().map(|f| f.size).sum();
    let mut rows = v_flex().p(px(6.));
    for (ix, f) in files.iter().enumerate() {
        let selected = ws.space.is_selected(&f.path);
        let group: SharedString = format!("space-file-{ix}").into();
        let rel = f.path.parent().and_then(|p| p.strip_prefix(focus).ok()).map(|p| p.display().to_string()).filter(|s| !s.is_empty());
        let toggle = f.path.clone();
        rows = rows.child(h_flex().id(("space-file", ix)).debug_selector(move || format!("space-file-{ix}")).group(group.clone())
            .gap(px(10.)).px(px(8.)).py(px(7.)).rounded(px(8.)).cursor_pointer()
            .when(selected, |d| d.bg(ch.accent.opacity(0.08))).hover(|s| s.bg(ch.hover))
            .on_click(cx.listener(move |ws, _, _, cx| ws.toggle_space_selection(toggle.clone(), cx)))
            .child(text(format!("{}", ix + 1), 10., ch.text_faint).w(px(22.)).flex_none())
            .child(select_box(("space-file-check", ix), &f.path, selected, cx))
            .child(badge(f.category, false, ch))
            .child(v_flex().flex_1().min_w_0().gap(px(2.))
                .child(text(file_name(&f.path), 12., ch.text_strong).truncate())
                .child(text(format!("{} · modified {}", rel.unwrap_or_else(|| "In this folder".into()), ago(f.modified)), 10., ch.text_faint).truncate()))
            .child(text(size(f.size), 12., ch.text).flex_none())
            .child(row_actions(10_000 + ix, group, &f.path, cx)));
    }
    if files.is_empty() {
        rows = rows.child(v_flex().items_center().gap_2().p_8()
            .child(icon(IconName::Search).size(px(22.)).text_color(ch.text_faint))
            .child(text("No files match this filter", 12., ch.text_muted)));
    }
    v_flex().gap(px(12.)).child(chips)
        .child(card(ch).id("space-files").debug_selector(|| "space-files".into()).overflow_hidden()
            .child(h_flex().px(px(16.)).py(px(13.)).justify_between().border_b_1().border_color(ch.hairline)
                .child(text(format!("Largest files in {}", file_name(focus)), 12., ch.text_strong).font_weight(FontWeight::SEMIBOLD))
                .child(text(format!("Top {} · {}", files.len().min(LARGEST_FILES), size(total)), 10., ch.text_faint)))
            .child(rows))
        .into_any_element()
}

// ----- cleanup ----------------------------------------------------------------

fn kind_icon(kind: SuggestionKind) -> IconName {
    match kind {
        SuggestionKind::Developer => IconName::FileCode,
        SuggestionKind::Caches => IconName::Archive,
        SuggestionKind::Installers => IconName::PackageOpen,
        SuggestionKind::Downloads => IconName::Download,
        SuggestionKind::Stale => IconName::Calendar,
        SuggestionKind::Logs => IconName::FileText,
    }
}

fn cleanup_tab(ws: &Workspace, scan: &Scan, focus: &Path, reclaim: u64, ch: &Chrome, cx: &mut Context<Workspace>) -> AnyElement {
    if reclaim == 0 {
        return card(ch).id("space-cleanup-empty").debug_selector(|| "space-cleanup".into()).p(px(36.))
            .child(v_flex().items_center().gap(px(10.))
                .child(div().size(px(44.)).rounded_full().bg(ch.success.opacity(0.14)).flex().items_center().justify_center()
                    .child(icon(IconName::Check).size(px(20.)).text_color(ch.success)))
                .child(text("Nothing obvious to clean up here", 14., ch.text_strong).font_weight(FontWeight::SEMIBOLD))
                .child(text("No caches, build output, installers or stale large files were found. Check Largest files for anything you no longer need.", 11., ch.text_muted).max_w(px(460.)).text_center()))
            .into_any_element();
    }
    let safe: Vec<PathBuf> = scan.suggestions_in(focus).filter(|s| s.kind.regenerable()).map(|s| s.path.clone()).collect();
    let safe_bytes: u64 = scan.suggestions_in(focus).filter(|s| s.kind.regenerable()).map(|s| s.size).sum();
    let hero = card(ch).id("space-cleanup").debug_selector(|| "space-cleanup".into())
        .child(h_flex().gap(px(20.)).p(px(20.)).flex_wrap().items_center()
            .child(div().size(px(48.)).flex_none().rounded(px(12.)).bg(ch.accent.opacity(0.12)).flex().items_center().justify_center()
                .child(icon(IconName::WandSparkles).size(px(22.)).text_color(ch.accent)))
            .child(v_flex().flex_1().min_w(px(220.)).gap(px(4.))
                .child(h_flex().items_baseline().gap(px(8.))
                    .child(text(size(reclaim), 24., ch.text_strong).font_weight(FontWeight::SEMIBOLD))
                    .child(text(format!("could be freed in {}", file_name(focus)), 12., ch.text_muted)))
                .child(text(format!("Suggestions come from names and locations. Review them, select what you don’t need, and move it to the {} — you can still restore it from there.", crate::platform_text::trash()), 11., ch.text_faint)))
            .when(!safe.is_empty(), |d| d.child(Button::new("space-select-safe").cursor_pointer().outline().icon(IconName::Check)
                .label(format!("Select regenerable · {}", size(safe_bytes)))
                .tooltip("Select caches, build output and logs — things apps and builds recreate")
                .on_click(cx.listener(move |ws, _, _, cx| {
                    for p in &safe {
                        ws.space.selected.insert(p.clone());
                    }
                    cx.notify();
                })))));
    let mut groups = v_flex().gap(px(12.));
    for kind in SuggestionKind::ALL {
        let items: Vec<_> = scan.suggestions_in(focus).filter(|s| s.kind == kind).collect();
        if items.is_empty() {
            continue;
        }
        let group_bytes: u64 = items.iter().map(|s| s.size).sum();
        let all = items.iter().all(|s| ws.space.selected.contains(&s.path));
        let expanded = ws.space.expanded.contains(&kind);
        let focus_buf = focus.to_path_buf();
        let header = h_flex().gap(px(12.)).px(px(16.)).py(px(13.)).border_b_1().border_color(ch.hairline)
            .child(Checkbox::new(SharedString::from(format!("space-group-{kind:?}"))).cursor_pointer().checked(all)
                .on_click(cx.listener(move |ws, _: &bool, _, cx| ws.toggle_space_group(kind, &focus_buf, cx))))
            .child(div().size(px(30.)).flex_none().rounded(px(8.)).bg(ch.hover).flex().items_center().justify_center()
                .child(icon(kind_icon(kind)).size(px(15.)).text_color(ch.text_muted)))
            .child(v_flex().flex_1().min_w_0().gap(px(2.))
                .child(h_flex().gap(px(8.)).flex_wrap()
                    .child(text(kind.title(), 12., ch.text_strong).font_weight(FontWeight::SEMIBOLD))
                    .child(if kind.regenerable() {
                        text("Safe to remove", 9., ch.success).px(px(6.)).py(px(1.)).rounded(px(4.)).bg(ch.success.opacity(0.12))
                    } else {
                        text("Review first", 9., color(Category::Images, ch)).px(px(6.)).py(px(1.)).rounded(px(4.)).bg(color(Category::Images, ch).opacity(0.14))
                    }))
                .child(text(kind.description(), 10., ch.text_faint)))
            .child(v_flex().items_end().flex_none()
                .child(text(size(group_bytes), 12., ch.text_strong).font_weight(FontWeight::MEDIUM))
                .child(text(plural(items.len() as u64, "item", "items"), 10., ch.text_faint)));
        let mut rows = v_flex().p(px(6.));
        for (i, s) in items.iter().enumerate().take(if expanded { usize::MAX } else { GROUP_PREVIEW }) {
            let id = (kind as usize) * 100_000 + i;
            let selected = ws.space.is_selected(&s.path);
            let group: SharedString = format!("space-sugg-{id}").into();
            let rel = s.path.parent().and_then(|p| p.strip_prefix(focus).ok()).map(|p| p.display().to_string()).filter(|r| !r.is_empty());
            let toggle = s.path.clone();
            let cat = if s.is_dir { Category::Code } else { Category::of_file(&file_name(&s.path)) };
            rows = rows.child(h_flex().id(("space-sugg", id)).group(group.clone()).gap(px(10.)).px(px(8.)).py(px(6.)).rounded(px(8.)).cursor_pointer()
                .when(selected, |d| d.bg(ch.accent.opacity(0.08))).hover(|st| st.bg(ch.hover))
                .on_click(cx.listener(move |ws, _, _, cx| ws.toggle_space_selection(toggle.clone(), cx)))
                .child(select_box(("space-sugg-check", id), &s.path, selected, cx))
                .child(icon(if s.is_dir { IconName::Folder } else { category_icon(false, cat) }).size(px(15.)).text_color(ch.text_muted).flex_none())
                .child(v_flex().flex_1().min_w_0().gap(px(1.))
                    .child(text(file_name(&s.path), 12., ch.text_strong).truncate())
                    .child(text(format!("{} · {}", s.note, rel.unwrap_or_else(|| "In this folder".into())), 10., ch.text_faint).truncate()))
                .child(text(size(s.size), 12., ch.text).flex_none())
                .child(row_actions(200_000 + id, group, &s.path, cx)));
        }
        if items.len() > GROUP_PREVIEW {
            let label = if expanded { "Show fewer".to_string() } else { format!("Show all {}", items.len()) };
            rows = rows.child(div().id(("space-expand", kind as usize)).px(px(12.)).py(px(8.)).cursor_pointer()
                .text_size(px(11.)).text_color(ch.accent).hover(|s| s.underline())
                .on_click(cx.listener(move |ws, _, _, cx| {
                    if !ws.space.expanded.remove(&kind) {
                        ws.space.expanded.insert(kind);
                    }
                    cx.notify();
                }))
                .child(label));
        }
        groups = groups.child(card(ch).id(("space-group", kind as usize)).overflow_hidden().child(header).child(rows));
    }
    v_flex().gap(px(14.)).child(hero).child(groups).into_any_element()
}
