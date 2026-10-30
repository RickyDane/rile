//! The Overview start page: live storage, the contents of the user's
//! places and recent changes — computed on demand from the top level of
//! each place, plus remembered discovery and trophy progress.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use gpui_kit::assets::IconName;
use gpui_kit::component::{Sizable, h_flex, spinner::Spinner, v_flex};
use gpui_kit::{
    AnyElement, Bounds, Context, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, PathBuilder,
    AppContext, Pixels, StatefulInteractiveElement, Styled, Task, Window, canvas, div, linear_color_stop, linear_gradient, point,
    prelude::FluentBuilder, px, relative, rgba,
};
use rile_core::history::Location;
use rile_core::places::{Place, PlaceIcon};
use rile_core::{Entry, list_dir};

use crate::art::{Art, art};
use crate::icons::{icon_for_entry, icon_for_place};
use crate::theme::{Chrome, chrome, usage_color};
use crate::views::chrome::{Width, content_class};
use crate::workspace::Workspace;

/// Entries read per place; keeps huge folders (e.g. Downloads) cheap.
const MAX_PER_PLACE: usize = 20_000;
const DAYS: usize = 7;

pub struct Group {
    pub name: &'static str,
    pub color: u32,
    pub count: usize,
}

/// Statistics over the top level of the user's places.
pub struct OverviewStats {
    pub items: usize,
    pub files: usize,
    pub folders: usize,
    pub places: usize,
    /// (files, folders, folders seen) Rile remembers from browsing.
    pub discovered: (usize, usize, usize),
    pub rewards: rile_services::rewards::Progress,
    pub groups: Vec<Group>,
    /// Items modified on each of the last seven days (oldest first).
    pub activity: [usize; DAYS],
    /// Every scanned entry, newest first (search + recent files).
    pub entries: Vec<Entry>,
}

#[derive(Default)]
pub struct OverviewState {
    pub loading: bool,
    pub stats: Option<Arc<OverviewStats>>,
    _task: Option<Task<()>>,
}

fn group_of(entry: &Entry) -> usize {
    match entry.extension().as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "svg" | "bmp" | "tif" | "tiff" | "ico" | "avif" | "raw" => 0,
        "rs" | "js" | "ts" | "tsx" | "jsx" | "py" | "go" | "c" | "h" | "cpp" | "hpp" | "java" | "kt" | "swift" | "rb"
        | "php" | "sh" | "zsh" | "json" | "toml" | "yaml" | "yml" | "html" | "css" | "scss" | "lua" | "sql" | "xml"
        | "zig" => 1,
        "txt" | "md" | "pdf" | "doc" | "docx" | "rtf" | "pages" | "csv" | "xls" | "xlsx" | "numbers" | "key"
        | "ppt" | "pptx" | "odt" | "epub" => 2,
        "mp4" | "mov" | "mkv" | "avi" | "webm" | "m4v" | "mp3" | "wav" | "flac" | "aac" | "m4a" | "ogg" | "aiff" => 3,
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" | "dmg" | "iso" | "zst" | "pkg" => 4,
        _ => 5,
    }
}

pub fn compute(places: &[Place], now: SystemTime) -> OverviewStats {
    let mut entries: Vec<Entry> = Vec::new();
    let mut scanned = 0;
    // The home folder's top level is mostly the other places; skip it.
    for place in places.iter().filter(|p| p.icon != PlaceIcon::Home) {
        if let Ok(mut list) = list_dir(&place.path, false) {
            rile_services::index::record(&place.path, &list);
            list.truncate(MAX_PER_PLACE);
            entries.extend(list);
            scanned += 1;
        }
    }
    let mut groups = vec![
        Group { name: "Images", color: 0x64adf4ff, count: 0 },
        Group { name: "Code", color: 0x9d8de0ff, count: 0 },
        Group { name: "Documents", color: 0x65b8a6ff, count: 0 },
        Group { name: "Audio & video", color: 0xd98ca8ff, count: 0 },
        Group { name: "Archives", color: 0xd9a46dff, count: 0 },
        Group { name: "Other", color: 0x7c8594ff, count: 0 },
    ];
    let mut activity = [0usize; DAYS];
    let today = chrono::DateTime::<chrono::Local>::from(now).date_naive();
    for e in &entries {
        if !e.is_dir {
            groups[group_of(e)].count += 1;
        }
        if let Some(t) = e.modified {
            let day = chrono::DateTime::<chrono::Local>::from(t).date_naive();
            let ago = (today - day).num_days();
            if (0..DAYS as i64).contains(&ago) {
                activity[DAYS - 1 - ago as usize] += 1;
            }
        }
    }
    let files = entries.iter().filter(|e| !e.is_dir).count();
    entries.sort_by(|a, b| b.modified.cmp(&a.modified));
    let (seen, _) = rile_services::index::stats();
    let (known_files, known_folders) = rile_services::index::counts();
    let (images, documents) = rile_services::tags::reward_counts();
    let rewards = rile_services::rewards::Progress {
        files: known_files,
        folders: seen,
        images,
        documents,
        ..Default::default()
    };
    OverviewStats {
        rewards,
        discovered: (known_files, known_folders, seen),
        items: entries.len(),
        files,
        folders: entries.len() - files,
        places: scanned,
        groups,
        activity,
        entries,
    }
}

impl Workspace {
    /// Recomputes the Overview statistics in the background (and volumes).
    pub fn refresh_overview(&mut self, cx: &mut Context<Self>) {
        self.refresh_volumes(cx);
        if self.overview.loading {
            return;
        }
        self.overview.loading = true;
        let places = self.places.clone();
        let task = cx.background_spawn(async move {
            let mut stats = compute(&places, SystemTime::now());
            stats.rewards = rile_services::rewards::record(stats.rewards);
            stats
        });
        self.overview._task = Some(cx.spawn(async move |this, cx| {
            let stats = task.await;
            this.update(cx, |this, cx| {
                this.overview.loading = false;
                this.overview.stats = Some(Arc::new(stats));
                cx.notify();
            })
            .ok();
        }));
    }

    /// Opens the folder containing `path` with `path` selected.
    pub fn reveal_in_pane(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(parent) = path.parent().map(PathBuf::from) else { return };
        self.pane().update(cx, |p, cx| {
            p.select_after_reload(vec![path]);
            p.navigate(Location::Dir(parent), cx);
            p.focus(window, cx);
        });
        cx.notify();
    }
}

/// How the Overview arranges itself in a slot `width` pixels wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverviewLayout {
    pub class: Width,
    pub metric_cols: usize,
    /// Chart and bottom panels one above the other instead of side by side.
    pub stacked: bool,
    pub storage_cols: usize,
    /// Columns of the Devices cards, capped at three for wider cards.
    pub device_cols: usize,
}

pub fn overview_layout(width: f32) -> OverviewLayout {
    OverviewLayout {
        class: content_class(width),
        metric_cols: if width >= 900. { 5 } else if width >= 640. { 3 } else if width >= 440. { 2 } else { 1 },
        stacked: width < 860.,
        storage_cols: if width >= 600. { 2 } else { 1 },
        device_cols: if width >= 640. { 3 } else if width >= 420. { 2 } else { 1 },
    }
}

pub(super) const PAGE_MAX_WIDTH: f32 = 1390.;

/// Space between a page title and the content below it.
pub(super) const PAGE_TITLE_GAP: f32 = 16.;

/// Top padding above a page title.
pub(super) fn page_title_top(width: Width) -> f32 {
    page_padding(width).1.min(24.)
}

pub(super) fn page_padding(width: Width) -> (f32, f32) {
    match width {
        Width::Phone => (16., 25.),
        Width::Narrow | Width::Compact => (25., 27.),
        Width::Regular => (38., 34.),
        Width::Wide => (38., 42.),
    }
}

pub(super) fn page_title(title: impl Into<gpui_kit::SharedString>, width: Width, ch: &Chrome) -> gpui_kit::Div {
    h_flex()
        .text_size(px(if width <= Width::Phone { 22. } else { 24. }))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ch.text_strong)
        .child(title.into())
        .child(div().text_color(ch.accent).child("."))
}

pub(super) fn card(ch: &Chrome) -> gpui_kit::Div {
    div()
        .rounded(px(12.))
        .border_1()
        .border_color(ch.card_border)
        // A faint accent-tinted sheen from the top left, fading into the plain card.
        .bg(linear_gradient(150., linear_color_stop(ch.accent.opacity(0.055), 0.), linear_color_stop(ch.card_bottom, 0.75)))
}

fn panel_header(title: &'static str, subtitle: String, trailing: Option<AnyElement>, ch: &Chrome) -> AnyElement {
    h_flex()
        .px(px(21.))
        .pt(px(20.))
        .gap(px(12.))
        .justify_between()
        .child(
            v_flex()
                .gap(px(6.))
                .min_w_0()
                .child(div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(ch.text).child(title))
                .child(div().text_size(px(11.)).text_color(ch.text_faint).child(subtitle)),
        )
        .children(trailing)
        .into_any_element()
}

/// Rows of `per_row` equally wide cells.
fn grid(cells: Vec<AnyElement>, per_row: usize, gap: f32) -> AnyElement {
    let mut rows = Vec::new();
    let mut iter = cells.into_iter().peekable();
    while iter.peek().is_some() {
        let row: Vec<AnyElement> = iter.by_ref().take(per_row).collect();
        let missing = per_row - row.len();
        rows.push(
            h_flex()
                .gap(px(gap))
                .items_stretch()
                .children(row.into_iter().map(|c| div().flex_1().min_w_0().flex().child(c)))
                .children((0..missing).map(|_| div().flex_1()))
                .into_any_element(),
        );
    }
    v_flex().gap(px(gap)).children(rows).into_any_element()
}

pub fn render(ws: &mut Workspace, slot: usize, window: &mut Window, cx: &mut Context<Workspace>) -> AnyElement {
    let ch = chrome(cx);
    let layout = overview_layout(ws.slot_width(slot, window));
    let width = layout.class;
    if ws.overview.stats.is_none() && !ws.overview.loading {
        ws.refresh_overview(cx);
    }
    let stats = ws.overview.stats.clone();
    let query = ws.panes[slot].read(cx).filter.trim().to_lowercase();
    let searching = !query.is_empty();
    let stacked = layout.stacked;

    let (pad_x, _) = page_padding(width);

    // The slot's pane is not drawn here, so the page holds its focus: window
    // shortcuts (⌘F, ⌘T, ⌘L…) keep working while file shortcuts don't apply.
    let focus = ws.panes[slot].read(cx).focus_handle.clone();
    let page = v_flex()
        .id(("overview", slot))
        .track_focus(&focus)
        .size_full()
        .overflow_y_scroll()
        .bg(ch.browser)
        .capture_any_mouse_down(cx.listener(move |this, _, w, cx| {
            if this.active_pane != slot {
                this.set_active_pane(slot, w, cx);
            }
        }))
        .child(
            v_flex()
                .w_full()
                .max_w(px(PAGE_MAX_WIDTH))
                .mx_auto()
                .px(px(pad_x))
                .pt(px(page_title_top(width)))
                .pb(px(22.))
                .child(
                    page_title("Home", width, &ch).mb(px(PAGE_TITLE_GAP)),
                )
                .when(!searching, |d| {
                    d.child(metrics(ws, stats.as_deref(), width, layout.metric_cols, cx))
                        .child(storage(ws, width, layout.device_cols, cx))
                        .child(charts(stats.as_deref(), stacked, width, cx))
                })
                .child(bottom(ws, stats.as_deref(), &query, stacked, width, cx))
                .child(
                    h_flex()
                        .mt(px(24.))
                        .gap(px(7.))
                        .when(!stacked, |d| d.justify_center())
                        .text_size(px(11.))
                        .text_color(ch.text_faint)
                        .child(crate::icons::icon(IconName::Info).size(px(12.)))
                        .child(
                            "Live from this device. Counts cover the top level of your places; \"Discovered\" counts what Rile remembers from folders you opened. Rile never crawls your disk.",
                        ),
                )
                .when(!searching, |d| d.child(trophies(ws.rewards.progress.or_else(|| stats.as_ref().map(|s| s.rewards)), layout.storage_cols, Some(crate::icons::apple_intelligence(20., ch.accent, cx)), &ch))),
        );
    page.into_any_element()
}

pub(super) fn metric(label: impl Into<gpui_kit::SharedString>, value: String, unit: Option<String>, detail: String, art_kind: Art, width: Width, ch: &Chrome) -> AnyElement {
    let label = label.into();
    let tooltip = format!("{label}: {value}{} — {detail}", unit.as_ref().map(|u| format!(" {u}")).unwrap_or_default());
    let icon = match width {
        Width::Phone => 40.,
        Width::Compact | Width::Narrow => 48.,
        _ => 56.,
    };
    div()
        .id(label.clone())
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx))
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(8.))
        .py(px(8.))
        .child(art(art_kind, icon))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(0.))
                .child(div().truncate().text_size(px(11.)).line_height(px(16.)).text_color(ch.text_muted).child(label))
                .child(
                    h_flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .text_size(px(match width {
                                    Width::Wide | Width::Regular => 20.,
                                    _ => 19.,
                                }))
                                .line_height(px(24.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ch.text_strong)
                                .child(value),
                        )
                        .children(unit.map(|u| div().text_size(px(12.)).line_height(px(16.)).text_color(ch.text_muted).child(u))),
                )
                .child(div().truncate().text_size(px(11.)).line_height(px(16.)).text_color(ch.text_faint).child(detail)),
        )
        .into_any_element()
}

fn split_size(bytes: u64) -> (String, Option<String>) {
    let s = humansize::format_size(bytes, humansize::DECIMAL);
    match s.split_once(' ') {
        Some((v, u)) => (v.to_string(), Some(u.to_string())),
        None => (s, None),
    }
}

/// `ai_glyph` stands in for the AI track's icon (the Apple Intelligence symbol).
fn trophies(progress: Option<rile_services::rewards::Progress>, cols: usize, mut ai_glyph: Option<AnyElement>, ch: &Chrome) -> AnyElement {
    let tracks = progress.unwrap_or_default().tracks();
    let earned: usize = tracks.iter().map(|t| t.earned()).sum();
    let total: usize = tracks.iter().filter(|t| t.unavailable.is_none()).map(|t| t.targets.len()).sum();
    let upcoming: usize = tracks.iter().filter(|t| t.unavailable.is_some()).map(|t| t.targets.len()).sum();
    let cells = tracks.iter().map(|track| {
        let color = if track.earned() > 0 { ch.accent } else { ch.text_faint };
        let symbol = match track.id {
            "files" => IconName::File,
            "folders" | "organized" => IconName::FolderOpen,
            "images" | "photo-percent" => IconName::Images,
            "documents" => IconName::FileText,
            "searches" => IconName::Search,
            "ai-found" => IconName::Sparkles,
            "servers" | "dual-transfers" => IconName::Globe,
            "archives" => IconName::Archive,
            "space" | "space-scans" => IconName::ChartPie,
            _ => IconName::Star,
        };
        card(ch)
            .w_full()
            .p(px(17.))
            .child(
                h_flex().gap(px(10.))
                    .child(div().rounded(px(9.)).p(px(9.)).bg(color.opacity(0.1))
                        .child(match if track.id == "ai-found" { ai_glyph.take() } else { None } {
                            Some(glyph) => div().size(px(20.)).when(track.earned() == 0, |d| d.opacity(0.5)).child(glyph).into_any_element(),
                            None => crate::icons::icon(symbol).size(px(20.)).text_color(color).into_any_element(),
                        }))
                    .child(v_flex().gap(px(4.))
                        .child(div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(ch.text).child(track.name))
                        .child(div().text_size(px(11.)).text_color(ch.text_faint).child(track.action)))
                    .child(div().ml_auto().text_size(px(12.)).text_color(color).child(format!("{} / {}", track.earned(), track.targets.len()))),
            )
            .child(div().mt(px(14.)).text_size(px(12.)).text_color(ch.text_muted).child(
                if let Some(reason) = track.unavailable { reason.into() }
                else if progress.is_none() { "Reading progress…".into() }
                else if let Some(next) = track.next() { format!("{} / {} · next badge", track.label(track.count), track.label(next)) }
                else { format!("{} · all badges earned!", track.label(track.count)) }
            ))
            .child(div().mt(px(8.)).h(px(4.)).rounded(px(2.)).overflow_hidden().bg(ch.text_faint.opacity(0.12))
                .child(div().h_full().w(relative(track.fraction())).bg(ch.accent).rounded(px(2.))))
            .child(h_flex().mt(px(12.)).gap(px(6.)).flex_wrap().children(track.targets.iter().map(|&target| {
                let unlocked = progress.is_some() && track.unavailable.is_none() && track.count >= target;
                h_flex().gap(px(4.)).px(px(7.)).py(px(5.)).rounded(px(6.)).border_1()
                    .border_color(if unlocked { ch.accent.opacity(0.35) } else { ch.card_border })
                    .bg(if unlocked { ch.accent.opacity(0.09) } else { ch.text_faint.opacity(0.03) })
                    .text_size(px(11.)).text_color(if unlocked { ch.accent } else { ch.text_faint })
                    .child(crate::icons::icon(if unlocked { IconName::Check } else { IconName::Star }).size(px(11.)))
                    .child(track.label(target))
            })))
            .into_any_element()
    }).collect();
    v_flex().mt(px(24.)).gap(px(13.))
        .child(h_flex().gap(px(9.)).flex_wrap()
            .child(crate::icons::icon(IconName::Star).size(px(17.)).text_color(ch.accent))
            .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(ch.text).child("Trophy cabinet"))
            .child(div().ml_auto().text_size(px(12.)).text_color(ch.text_faint).child(
                if progress.is_some() { format!("{earned} of {total} badges earned · {upcoming} coming soon") } else { "Loading badges…".into() }
            )))
        .child(grid(cells, cols, 12.))
        .child(div().text_size(px(11.)).text_color(ch.text_faint).child(
            "Explore, tag, collect. Counts and coverage show your personal best; earned badges stay unlocked. Photo coverage uses remembered photos, not every image on your disk. Coming-soon badges don't count toward your available total.",
        ))
        .into_any_element()
}

fn metrics(ws: &Workspace, stats: Option<&OverviewStats>, width: Width, cols: usize, cx: &mut Context<Workspace>) -> AnyElement {
    let ch = chrome(cx);
    let used: u64 = ws.volumes.iter().map(|v| v.total.saturating_sub(v.available)).sum();
    let total: u64 = ws.volumes.iter().map(|v| v.total).sum();
    let disks = ws.volumes.len();
    let (value, unit) = split_size(used);
    let free = total.saturating_sub(used);
    let (free_value, free_unit) = split_size(free);
    let free_pct = if total == 0 { 0 } else { (free as u128 * 100 / total as u128) as u64 };
    let pending = || "Reading…".to_string();
    let cells = vec![
        metric(
            "Storage used",
            value,
            unit,
            format!("of {} across {disks} disk{}", humansize::format_size(total, humansize::DECIMAL), if disks == 1 { "" } else { "s" }),
            Art::Storage,
            width,
            &ch,
        ),
        metric(
            "Files in your places",
            stats.map(|s| s.files.to_string()).unwrap_or_else(pending),
            None,
            stats.map(|s| format!("top level of {} places · live count", s.places)).unwrap_or_default(),
            Art::Files,
            width,
            &ch,
        ),
        metric(
            "Folders",
            stats.map(|s| s.folders.to_string()).unwrap_or_else(pending),
            None,
            "at the top of your places".into(),
            Art::Folder,
            width,
            &ch,
        ),
        metric(
            "Discovered",
            stats.map(|s| (s.discovered.0 + s.discovered.1).to_string()).unwrap_or_else(pending),
            None,
            stats
                .map(|s| {
                    if ws.settings.index_discovered {
                        format!("{} files · {} folders remembered", s.discovered.0, s.discovered.1)
                    } else {
                        "remembering is off (Settings → Search)".into()
                    }
                })
                .unwrap_or_default(),
            Art::Discovered,
            width,
            &ch,
        ),
        metric(
            "Free space",
            free_value,
            free_unit,
            if total == 0 { "no disks found".into() } else { format!("{free_pct}% available{}", if free_pct < 10 { " · running low" } else { "" }) },
            Art::FreeSpace(crate::art::FreeSpaceLevel::from_capacity(free, total)),
            width,
            &ch,
        ),
    ];
    div().mb(px(24.)).child(grid(cells, cols, if width <= Width::Compact { 10. } else { 14. })).into_any_element()
}

fn charts(stats: Option<&OverviewStats>, stacked: bool, width: Width, cx: &mut Context<Workspace>) -> AnyElement {
    let ch = chrome(cx);
    let activity = stats.map(|s| s.activity).unwrap_or_default();
    let changed: usize = activity.iter().sum();
    let today = chrono::Local::now().date_naive();
    let labels: Vec<String> = (0..DAYS)
        .map(|i| (today - chrono::Days::new((DAYS - 1 - i) as u64)).format("%b %-d").to_string())
        .collect();
    // Running total across the week: how the changes accumulated.
    let mut running = 0;
    let values: [usize; DAYS] = activity.map(|n| {
        running += n;
        running
    });
    let max = changed.max(1);

    let line_color: Hsla = if ch.dark { rgba(0x7ebcf4ff).into() } else { ch.accent };
    let grid_color = ch.hairline;
    let fill_top = ch.accent.opacity(0.30);
    let fill_bottom = ch.accent.opacity(0.0);
    let halo = ch.accent.opacity(0.18);
    let end_fill: Hsla = if ch.dark { rgba(0xe2f3ffff).into() } else { gpui_kit::white().into() };
    let plot = canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let (x0, y0) = (bounds.origin.x, bounds.origin.y);
            let (w, h) = (bounds.size.width, bounds.size.height);
            let top = px(8.);
            let base = h - px(2.);
            for frac in [0., 0.5, 1.] {
                let y = y0 + top + (base - top) * frac;
                let mut b = PathBuilder::stroke(px(1.)).dash_array(&[px(2.), px(5.)]);
                b.move_to(point(x0, y));
                b.line_to(point(x0 + w, y));
                if let Ok(p) = b.build() {
                    window.paint_path(p, grid_color);
                }
            }
            let pts: Vec<_> = values
                .iter()
                .enumerate()
                .map(|(i, &v)| {
                    let x = x0 + w * (i as f32 / (DAYS - 1) as f32);
                    let y = y0 + top + (base - top) * (1. - v as f32 / max as f32);
                    point(x, y)
                })
                .collect();
            let curve = |b: &mut PathBuilder| {
                b.move_to(pts[0]);
                for i in 1..pts.len() {
                    let (a, c) = (pts[i - 1], pts[i]);
                    let mx = (a.x + c.x) / 2.;
                    b.cubic_bezier_to(c, point(mx, a.y), point(mx, c.y));
                }
            };
            let mut area = PathBuilder::fill();
            curve(&mut area);
            area.line_to(point(x0 + w, y0 + base));
            area.line_to(point(x0, y0 + base));
            area.close();
            if let Ok(p) = area.build() {
                window.paint_path(p, linear_gradient(180., linear_color_stop(fill_top, 0.), linear_color_stop(fill_bottom, 1.)));
            }
            let mut line = PathBuilder::stroke(px(2.3));
            curve(&mut line);
            if let Ok(p) = line.build() {
                window.paint_path(p, line_color);
            }
            let end = *pts.last().unwrap();
            let circle = |r: f32, style: PathBuilder| {
                let mut b = style;
                b.move_to(point(end.x + px(r), end.y));
                b.arc_to(point(px(r), px(r)), px(0.), false, true, point(end.x - px(r), end.y));
                b.arc_to(point(px(r), px(r)), px(0.), false, true, point(end.x + px(r), end.y));
                b.close();
                b.build().ok()
            };
            if let Some(p) = circle(8., PathBuilder::fill()) {
                window.paint_path(p, halo);
            }
            if let Some(p) = circle(3.5, PathBuilder::fill()) {
                window.paint_path(p, end_fill);
            }
            if let Some(p) = circle(3.5, PathBuilder::stroke(px(2.))) {
                window.paint_path(p, line_color);
            }
        },
    )
    .size_full();

    let y_labels = v_flex()
        .h(px(150.))
        .pt(px(2.))
        .justify_between()
        .flex_none()
        .w(px(22.))
        .text_size(px(11.))
        .text_color(ch.text_faint)
        .children([max, max / 2, 0].map(|v| div().text_right().child(v.to_string())));

    let activity_panel = card(&ch)
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .child(panel_header(
            "Recent activity",
            "Changes in your places, accumulated · last 7 days".into(),
            Some(
                div()
                    .flex_none()
                    .px(px(9.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(ch.hairline)
                    .text_size(px(11.))
                    .text_color(ch.text_muted)
                    .child("7 days")
                    .into_any_element(),
            ),
            &ch,
        ))
        .child(
            h_flex()
                .mx(px(23.))
                .mt(px(18.))
                .gap(px(10.))
                .child(div().text_size(px(20.)).font_weight(FontWeight::MEDIUM).text_color(ch.text_strong).child(if stats.is_some() { changed.to_string() } else { "—".into() }))
                .child(
                    v_flex()
                        .gap(px(4.))
                        .child(div().text_size(px(11.)).text_color(ch.text_muted).child("items modified"))
                        .child(div().text_size(px(10.)).text_color(ch.text_faint).child("from file dates · no history kept")),
                ),
        )
        .child(
            h_flex()
                .mx(px(16.))
                .mt(px(14.))
                .gap(px(10.))
                .items_start()
                .child(y_labels)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().h(px(150.)).relative().child(plot).when(stats.is_some() && changed == 0, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(12.))
                                    .text_color(ch.text_faint)
                                    .child("Nothing changed in the last 7 days"),
                            )
                        }))
                        .child(
                            h_flex()
                                .mt(px(8.))
                                .mb(px(14.))
                                .justify_between()
                                .text_size(px(11.))
                                .text_color(ch.text_faint)
                                .children(labels.into_iter().enumerate().filter(|(i, _)| width > Width::Phone || i % 2 == 0).map(|(_, l)| div().child(l))),
                        ),
                ),
        );

    let groups: Vec<(&'static str, u32, usize)> = stats.map(|s| s.groups.iter().map(|g| (g.name, g.color, g.count)).collect()).unwrap_or_default();
    let files = stats.map(|s| s.files).unwrap_or(0);
    let donut_size = match width {
        Width::Compact | Width::Phone => 108.,
        Width::Narrow => 128.,
        _ => 138.,
    };
    let track = ch.hairline;
    let segs: Vec<(Hsla, f32)> = groups.iter().filter(|g| g.2 > 0).map(|g| (rgba(g.1).into(), g.2 as f32 / files.max(1) as f32)).collect();
    let ring = canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let c = bounds.center();
            let stroke = 11.;
            let r = f32::from(bounds.size.width) / 2. - stroke / 2. - 1.;
            let at = |a: f32| point(c.x + px(r * a.cos()), c.y + px(r * a.sin()));
            let arc = |a0: f32, a1: f32| {
                let mut b = PathBuilder::stroke(px(stroke));
                b.move_to(at(a0));
                // Split long arcs so each piece stays under 180°.
                let steps = ((a1 - a0) / std::f32::consts::PI).ceil().max(1.) as usize;
                for s in 1..=steps {
                    let a = a0 + (a1 - a0) * s as f32 / steps as f32;
                    b.arc_to(point(px(r), px(r)), px(0.), false, true, at(a));
                }
                b.build().ok()
            };
            let start = -std::f32::consts::FRAC_PI_2;
            if let Some(p) = arc(start, start + std::f32::consts::TAU - 0.0001) {
                window.paint_path(p, track);
            }
            let gap = if segs.len() > 1 { 2.5 / r } else { 0. };
            let mut a = start;
            for (color, frac) in &segs {
                let len = frac * std::f32::consts::TAU;
                if len - gap > 0.01
                    && let Some(p) = arc(a, a + len - gap)
                {
                    window.paint_path(p, *color);
                }
                a += len;
            }
        },
    )
    .absolute()
    .inset_0();
    let donut = div()
        .relative()
        .size(px(donut_size))
        .flex_none()
        .child(ring)
        .child(
            v_flex()
                .absolute()
                .inset_0()
                .items_center()
                .justify_center()
                .child(div().text_size(px(22.)).font_weight(FontWeight::MEDIUM).text_color(ch.text_strong).child(files.to_string()))
                .child(div().mt(px(3.)).text_size(px(11.)).text_color(ch.text_faint).child("files")),
        );
    let legend = v_flex().flex_1().max_w(px(if stacked { 360. } else { 170. })).gap(px(11.)).children(groups.iter().map(|(name, color, count)| {
        h_flex()
            .gap(px(8.))
            .text_size(px(12.))
            .text_color(ch.text_muted)
            .child(div().size(px(7.)).rounded_full().bg(rgba(*color)))
            .child(div().flex_1().child(*name))
            .child(div().font_weight(FontWeight::MEDIUM).text_color(ch.text).child(count.to_string()))
    }));
    let types_panel = card(&ch)
        .when(!stacked, |d| d.w(relative(0.4)).flex_none())
        .min_w_0()
        .overflow_hidden()
        .child(panel_header(
            "A closer look",
            "Files in your places, by kind".into(),
            Some(crate::icons::icon(IconName::LayoutGrid).size(px(15.)).text_color(ch.text_faint).into_any_element()),
            &ch,
        ))
        .child(
            h_flex()
                .when(!stacked, |d| d.justify_center())
                .gap(px(if width <= Width::Compact { 17. } else { 27. }))
                .px(px(22.))
                .pt(px(if stacked { 25. } else { 32. }))
                .pb(px(28.))
                .child(donut)
                .child(legend),
        );

    let row = if stacked { v_flex().gap(px(14.)) } else { h_flex().gap(px(18.)).items_stretch() };
    row.mb(px(26.)).child(activity_panel).child(types_panel).into_any_element()
}

fn storage(ws: &Workspace, width: Width, cols: usize, cx: &mut Context<Workspace>) -> AnyElement {
    let ch = chrome(cx);
    const COLORS: [u32; 4] = [0x5daaffff, 0xa190e8ff, 0x65b8a6ff, 0xd9a46dff];
    let mut cards: Vec<AnyElement> = Vec::new();
    // Internal disks show this machine's OS (once read off-thread) instead of "Internal".
    let internal = crate::workspace::nearby::cached_facts().map(|f| f.os).filter(|os| !os.is_empty()).unwrap_or_else(|| "Internal".into());
    for (ix, vol) in ws.volumes.iter().enumerate() {
        let mount = vol.mount_point.clone();
        let desc = if vol.removable {
            "External · removable".to_string()
        } else if vol.mount_point == std::path::Path::new("/") {
            format!("{internal} · startup disk")
        } else {
            internal.clone()
        };
        let speedtest = {
            let vol = vol.clone();
            div()
                .id(("speedtest", ix))
                .flex_none()
                .size(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .cursor_pointer()
                .text_color(ch.text_muted)
                .hover(|s| s.bg(ch.hover_strong).text_color(ch.accent))
                .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Test drive speed").build(window, cx))
                .on_click(cx.listener(move |this, _, w, cx| {
                    cx.stop_propagation();
                    this.open_speedtest(vol.clone(), w, cx);
                }))
                .child(crate::icons::icon(IconName::Gauge).size(px(13.)))
                .into_any_element()
        };
        let open = cx.listener(move |this, _, w, cx| this.open_path(mount.clone(), w, cx));
        cards.push(source_card(
            ("storage-card", ix),
            SourceCard {
                art: if vol.removable { Art::ExternalDrive } else { crate::art::this_computer() },
                name: vol.name.clone(),
                subtitle: format!("{desc} · {}", vol.mount_point.display()),
                // This machine's own disks: always there, no status to show.
                status: None,
                connecting: false,
                footer: (String::new(), String::new()),
                usage: Some((vol.total.saturating_sub(vol.available), vol.total)),
                empty_note: "",
                color: Some(rgba(COLORS[ix % COLORS.len()]).into()),
                action: Some(speedtest),
            },
            width,
            &ch,
            open,
        ));
    }
    // Saved remote sources (servers, shares, cloud drives), connected or not.
    // Rile peers are listed below with the nearby devices instead.
    if ws.settings.remote_enabled {
        let remotes: Vec<_> = ws.connections.iter().filter(|c| c.protocol != rile_remote::Protocol::Peer).cloned().collect();
        for (ix, conn) in remotes.into_iter().enumerate() {
            let connected = crate::remote::is_connected(&conn.id);
            let connecting = ws.is_source_loading(&conn.id);
            let name = if conn.name.is_empty() { conn.display() } else { conn.name.clone() };
            let art_kind = match conn.protocol {
                rile_remote::Protocol::GoogleDrive => Art::GoogleDrive,
                rile_remote::Protocol::OneDrive => Art::OneDrive,
                _ => Art::NetworkShare,
            };
            let kind = if conn.protocol.is_cloud() { "Cloud drive" } else { "Network share" };
            let detail = conn.initial_path.clone().filter(|p| !p.is_empty()).unwrap_or_else(|| "Home folder".into());
            let status = if connecting && connected { ("Loading details…", true) } else if connecting { ("Connecting…", false) } else if connected { ("Connected", true) } else { ("Not connected", false) };
            let read = ws.remote_capacity.get(&conn.id);
            let usage = read.copied().flatten().map(|c| (c.used, c.total));
            let empty_note = match (connected, read) {
                (false, _) if connecting => "Waiting for connection…",
                (false, _) => "Connect to see capacity",
                (true, None) => "Reading capacity…",
                (true, Some(_)) => "Capacity not reported",
            };
            let id = conn.id.clone();
            let open = cx.listener(move |this, _, w, cx| this.open_connection(&id, w, cx));
            cards.push(source_card(
                ("remote-card", ix),
                SourceCard { art: art_kind, name, subtitle: format!("{kind} · {}", conn.display()), status: Some(status), connecting, footer: (conn.protocol.label().to_string(), detail), usage, empty_note, color: None, action: None },
                width,
                &ch,
                open,
            ));
        }
    }
    // Nearby Rile devices: paired ones (online or not) and discovered ones.
    if ws.settings.nearby.enabled {
        for (ix, row) in ws.nearby.rows().into_iter().enumerate() {
            let online = row.online.is_some();
            let ready = online && row.paired && !row.forgot_us;
            let connecting = ws.is_source_loading(&rile_remote::peer::conn_id(&row.id));
            let status = match (row.paired, online) {
                _ if connecting && ready => ("Loading details…", true),
                _ if connecting => ("Connecting…", false),
                _ if ready => ("Connected", true),
                (false, _) => ("Not paired", false),
                (true, true) => ("Needs pairing", false),
                (true, false) => ("Offline", false),
            };
            let usage = row.details.as_ref().map(|d| (d.storage_used, d.storage_total)).filter(|(_, t)| *t > 0);
            let art_kind = match row.kind {
                rile_remote::peer::DeviceKind::Server => Art::NearbyServer,
                rile_remote::peer::DeviceKind::Desktop => Art::Desktop,
                rile_remote::peer::DeviceKind::Laptop => Art::NearbyComputer,
            };
            // OS first so it survives truncation of long CPU model names.
            let subtitle = [row.os.as_str(), row.model.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            let footer_detail = row.online.as_ref().map(|(host, _)| host.clone()).unwrap_or_else(|| "Last seen on this network".into());
            let id = row.id.clone();
            let open = cx.listener(move |this, _, w, cx| this.open_device(&id, None, w, cx));
            cards.push(source_card(
                ("nearby-card", ix),
                SourceCard {
                    art: art_kind,
                    name: row.name.clone(),
                    subtitle: if subtitle.is_empty() { "Nearby device".into() } else { subtitle },
                    status: Some(status),
                    connecting,
                    footer: ("Rile".into(), footer_detail),
                    usage,
                    empty_note: if ready { "Capacity not reported" } else { "Shown when connected" },
                    color: None,
                    action: None,
                },
                width,
                &ch,
                open,
            ));
        }
    }
    let body = if cards.is_empty() {
        div().text_size(px(12.)).text_color(ch.text_faint).child("Reading disks…").into_any_element()
    } else {
        grid(cards, cols, if width <= Width::Narrow { 10. } else { 12. })
    };
    v_flex()
        .mb(px(26.))
        .child(
            h_flex()
                .justify_between()
                .mb(px(13.))
                .child(div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(ch.text).child("Devices"))
                .child(div().text_size(px(11.)).text_color(ch.text_faint).child("Disks and connected sources")),
        )
        .child(body)
        .into_any_element()
}

struct SourceCard {
    art: Art,
    name: String,
    subtitle: String,
    /// Status label and whether it is live (green) or idle (faint); `None`
    /// for this machine's own disks.
    status: Option<(&'static str, bool)>,
    connecting: bool,
    /// Without usage: kind/protocol label and where it points.
    footer: (String, String),
    /// (used, total) bytes when the source reports capacity.
    usage: Option<(u64, u64)>,
    /// Shown next to the footer when `usage` is `None`.
    empty_note: &'static str,
    /// Usage bar color; teal when `None`.
    color: Option<Hsla>,
    /// A small button before the chevron (e.g. the drive speed test).
    action: Option<AnyElement>,
}

/// A compact card in the Devices section of Home: a disk, remote source or
/// nearby device. Art beside name and subtitle; full-width usage bar and two stat lines.
fn source_card(
    id: (&'static str, usize),
    s: SourceCard,
    width: Width,
    ch: &Chrome,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    let fmt = |b: u64| humansize::format_size(b, humansize::DECIMAL);
    let live = s.status.is_none_or(|(_, live)| live);
    let color = s.color.unwrap_or_else(|| rgba(0x65b8a6ff).into());
    let frac = s.usage.map(|(used, total)| (used as f32 / total.max(1) as f32).clamp(0., 1.)).unwrap_or(0.);
    let color = usage_color(frac, color);
    let fmt_used = s.usage.map(|(used, _)| fmt(used));
    let status_el = s.status.map(|(status, live)| {
        let status_color = if s.connecting { ch.accent } else if live { ch.success } else { ch.text_faint };
        h_flex()
            .flex_none()
            .gap(px(5.))
            .text_color(status_color)
            .when(s.connecting, |d| d.child(Spinner::new().xsmall()))
            .when(!s.connecting, |d| d.child(div().size(px(4.)).rounded_full().bg(status_color)))
            .child(status)
    });
    // Line 1: used · % (or protocol · path) and status. Line 2: free and total (or the note).
    let (line1_left, line2_left, line2_right): (AnyElement, AnyElement, AnyElement) = match s.usage {
        Some((used, total)) => (
            h_flex()
                .min_w_0()
                .gap(px(4.))
                .child(div().font_weight(FontWeight::MEDIUM).text_color(ch.text_muted).child(fmt_used.unwrap_or_default()))
                .child(div().truncate().child(format!("used · {:.0}%", frac * 100.)))
                .into_any_element(),
            div().truncate().child(format!("{} free", fmt(total.saturating_sub(used)))).into_any_element(),
            div().flex_none().child(format!("{} total", fmt(total))).into_any_element(),
        ),
        None => {
            let (kind, detail) = s.footer;
            (
                h_flex()
                    .min_w_0()
                    .gap(px(4.))
                    .child(div().flex_none().font_weight(FontWeight::MEDIUM).text_color(ch.text_muted).child(kind))
                    .child(div().min_w_0().truncate().child(format!("· {detail}")))
                    .into_any_element(),
                div().truncate().child(s.empty_note).into_any_element(),
                div().into_any_element(),
            )
        }
    };
    card(ch)
        .id(id)
        .w_full()
        .min_w_0()
        .relative()
        .overflow_hidden()
        .px(px(13.))
        .py(px(12.))
        .when(live, |d| {
            d.cursor_pointer()
                .hover(|st| st.border_color(ch.accent.opacity(0.3)))
                .on_click(on_click)
        })
        .when(!live, |d| d.opacity(0.3))
        .when(!live && !s.connecting, |d| {
            let stripe = ch.text.opacity(0.10);
            d.child(canvas(
                |_, _, _| {},
                move |bounds: Bounds<Pixels>, _, window, _| {
                    let width = f32::from(bounds.size.width);
                    let height = f32::from(bounds.size.height);
                    let mut x = 0.;
                    while x < width + height {
                        let y_start = (x - width).max(0.);
                        let y_end = x.min(height);
                        let mut line = PathBuilder::stroke(px(1.));
                        line.move_to(point(bounds.origin.x + px(x - y_start), bounds.origin.y + px(y_start)));
                        line.line_to(point(bounds.origin.x + px(x - y_end), bounds.origin.y + px(y_end)));
                        if let Ok(path) = line.build() {
                            window.paint_path(path, stripe);
                        }
                        x += 18.;
                    }
                },
            ).absolute().inset_0())
        })
        .child(
            h_flex()
                .gap(px(10.))
                .child(art(s.art, if width <= Width::Phone { 46. } else { 56. }))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(
                            h_flex()
                                .gap(px(6.))
                                .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(ch.text).child(s.name))
                                .children(s.action),
                        )
                        .child(div().truncate().text_size(px(10.)).text_color(ch.text_faint).child(s.subtitle)),
                ),
        )
        .child(
            div()
                .mt(px(10.))
                .h(px(5.))
                .rounded(px(3.))
                .bg(ch.hover_strong)
                .overflow_hidden()
                .when(s.usage.is_some(), |d| {
                    d.child(div().h_full().w(relative(frac)).rounded(px(3.)).bg(linear_gradient(
                        90.,
                        linear_color_stop(color.opacity(0.65), 0.),
                        linear_color_stop(color, 1.),
                    )))
                }),
        )
        .child(
            v_flex()
                .mt(px(7.))
                .gap(px(3.))
                .text_size(px(10.))
                .text_color(ch.text_faint)
                .child(h_flex().gap(px(8.)).justify_between().child(line1_left).children(status_el))
                .child(h_flex().gap(px(8.)).justify_between().child(line2_left).child(line2_right)),
        )
        .into_any_element()
}

fn bottom(ws: &Workspace, stats: Option<&OverviewStats>, query: &str, stacked: bool, width: Width, cx: &mut Context<Workspace>) -> AnyElement {
    let ch = chrome(cx);
    let searching = !query.is_empty();
    let rows: Vec<Entry> = match stats {
        Some(s) if searching => s.entries.iter().filter(|e| e.name_lower.contains(query)).take(50).cloned().collect(),
        Some(s) => s.entries.iter().filter(|e| !e.is_dir).take(5).cloned().collect(),
        None => Vec::new(),
    };
    let place_of = |e: &Entry| e.path.parent().map(crate::workspace::display_name).unwrap_or_default();
    let recent_rows: Vec<AnyElement> = rows
        .into_iter()
        .enumerate()
        .map(|(ix, e)| {
            let path = e.path.clone();
            let is_dir = e.is_dir;
            h_flex()
                .id(("recent", ix))
                .gap(px(11.))
                .px(px(11.))
                .py(px(10.))
                .rounded(px(7.))
                .cursor_pointer()
                .hover(|s| s.bg(ch.hover_strong))
                .on_click(cx.listener(move |this, _, w, cx| {
                    if is_dir {
                        this.open_path(path.clone(), w, cx)
                    } else {
                        this.reveal_in_pane(path.clone(), w, cx)
                    }
                }))
                .child(
                    div()
                        .w(px(27.))
                        .h(px(29.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .bg(ch.accent_soft)
                        .child(crate::icons::icon(icon_for_entry(&e)).size(px(16.)).text_color(ch.accent)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(5.))
                        .child(div().truncate().text_size(px(12.)).text_color(ch.text).child(e.name.to_string()))
                        .child(div().truncate().text_size(px(10.)).text_color(ch.text_faint).child(place_of(&e))),
                )
                .child(div().min_w(px(42.)).text_right().text_size(px(11.)).text_color(ch.text_muted).child(crate::views::list::format_size(&e)))
                .when(width > Width::Compact || width == Width::Narrow, |d| {
                    d.child(
                        div()
                            .ml(px(9.))
                            .min_w(px(70.))
                            .text_right()
                            .text_size(px(10.))
                            .text_color(ch.text_faint)
                            .child(e.modified.map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%b %-d, %Y").to_string()).unwrap_or_default()),
                    )
                })
                .child(crate::icons::icon(IconName::ChevronRight).size(px(10.)).text_color(ch.text_faint))
                .into_any_element()
        })
        .collect();
    let empty = recent_rows.is_empty();
    let recent = card(&ch)
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .child(panel_header(
            if searching { "Search results" } else { "Recently modified" },
            if searching { format!("Matching “{query}” at the top of your places") } else { "Pick up where you left off".into() },
            None,
            &ch,
        ))
        .child(
            v_flex()
                .px(px(10.))
                .pt(px(14.))
                .pb(px(10.))
                .children(recent_rows)
                .when(empty, |d| {
                    d.child(div().px(px(13.)).py(px(23.)).text_size(px(12.)).text_color(ch.text_faint).child(match (stats.is_some(), searching) {
                        (false, _) => "Reading your places…".to_string(),
                        (true, true) => "Nothing at the top of your places matches. Open a folder and press ⏎ in search to look through its subfolders.".into(),
                        (true, false) => "No files at the top of your places yet.".into(),
                    }))
                }),
        );

    let wanted = [PlaceIcon::Downloads, PlaceIcon::Documents, PlaceIcon::Desktop, PlaceIcon::Home];
    let mut shortcuts: Vec<Place> = wanted.iter().filter_map(|w| ws.places.iter().find(|p| p.icon == *w).cloned()).collect();
    for fav in ws.settings.favorites.iter().take(4usize.saturating_sub(shortcuts.len())) {
        shortcuts.insert(0, Place { name: crate::workspace::display_name(fav), path: fav.clone(), icon: PlaceIcon::Folder });
    }
    shortcuts.truncate(4);
    let shortcut_rows = shortcuts.into_iter().enumerate().map(|(ix, place)| {
        let path = place.path.clone();
        h_flex()
            .id(("shortcut", ix))
            .mx(px(11.))
            .mt(px(8.))
            .px(px(11.))
            .py(px(10.))
            .gap(px(11.))
            .rounded(px(7.))
            .cursor_pointer()
            .hover(|s| s.bg(ch.hover_strong))
            .text_size(px(12.))
            .text_color(ch.text)
            .on_click(cx.listener(move |this, _, w, cx| this.open_path(path.clone(), w, cx)))
            .child(crate::icons::icon(icon_for_place(place.icon)).size(px(17.)).text_color(ch.accent))
            .child(div().flex_1().truncate().child(place.name))
            .child(crate::icons::icon(IconName::ChevronRight).size(px(11.)).text_color(ch.text_faint))
    });
    let shortcuts_panel = card(&ch)
        .when(!stacked, |d| d.w(relative(0.4)).flex_none())
        .min_w_0()
        .overflow_hidden()
        .child(panel_header(
            "Jump back in",
            "Your everyday places".into(),
            Some(crate::icons::icon(IconName::Folder).size(px(15.)).text_color(ch.text_faint).into_any_element()),
            &ch,
        ))
        .child(div().h(px(6.)))
        .children(shortcut_rows)
        .child(div().mx(px(22.)).my(px(18.)).text_size(px(10.)).text_color(ch.text_faint).child("Browse directly. No indexing required."));

    let row = if stacked { v_flex().gap(px(14.)) } else { h_flex().gap(px(18.)).items_start() };
    row.child(recent).when(!searching, |d| d.child(shortcuts_panel)).into_any_element()
}
