//! The photo library: every image on this Mac (Spotlight plus the Photos
//! library's originals), newest first and grouped by month, with search by
//! name or by content through smart tags. While it's open, photos are
//! described on device in the background (when enabled in Settings).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Sizable, h_flex,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    v_flex,
};
use gpui_kit::{
    AnyElement, AppContext, Context, Entity, FontWeight, InteractiveElement, IntoElement, ListAlignment, ListState, MouseButton,
    ObjectFit, ParentElement, Pixels, Point, Render, ScrollDelta, SharedString, Size, StatefulInteractiveElement, Styled, StyledImage, Subscription, Task,
    WeakEntity, Window, div, img, list, prelude::FluentBuilder, px, relative,
};
use rile_core::history::Location;
use rile_services::tags::{self, TagKind};

use crate::theme::chrome;
use crate::workspace::Workspace;

const GAP: f32 = 4.;
const HEADER_HEIGHT: f32 = 46.;
const MIN_TILE: f32 = 132.;

#[derive(Default)]
struct PhotoPan {
    offset: Point<Pixels>,
    drag: Option<Point<Pixels>>,
    viewport: Size<Pixels>,
    image: Size<f32>,
}

impl PhotoPan {
    fn constrain(&mut self, scale: f32) {
        let width = f32::from(self.viewport.width);
        let height = f32::from(self.viewport.height);
        if self.image.width <= 0. || self.image.height <= 0. {
            self.offset = Point::default();
            return;
        }
        // ObjectFit::Contain leaves letterboxing: only the actual image's
        // overflow can be panned, not the enlarged element's empty margins.
        let fit = (width / self.image.width).min(height / self.image.height);
        let x = px(((self.image.width * fit * scale - width) / 2.).max(0.));
        let y = px(((self.image.height * fit * scale - height) / 2.).max(0.));
        self.offset.x = self.offset.x.clamp(-x, x);
        self.offset.y = self.offset.y.clamp(-y, y);
        if scale <= 1. {
            self.drag = None;
        }
    }

    fn move_to(&mut self, position: Point<Pixels>, pressed_left: bool, scale: f32) -> bool {
        let Some(previous) = self.drag else { return false };
        if pressed_left {
            self.offset += position - previous;
            self.drag = Some(position);
            self.constrain(scale);
        } else {
            // Recover if the mouse was released outside the window.
            self.drag = None;
        }
        true
    }
}

struct PhotoZoom {
    scale: f32,
    target: f32,
    last_frame: Option<Instant>,
    pan: PhotoPan,
}

impl Default for PhotoZoom {
    fn default() -> Self {
        Self { scale: 1., target: 1., last_frame: None, pan: PhotoPan::default() }
    }
}

impl PhotoZoom {
    fn scroll(&mut self, control: bool, delta: ScrollDelta) -> bool {
        if !control {
            return false;
        }
        let delta = f32::from(delta.pixel_delta(px(24.)).y);
        if !delta.is_finite() || delta == 0. {
            return false;
        }
        // Exponential steps feel the same at every magnification. Keep fit as
        // the minimum, and normalize mouse-wheel lines and trackpad pixels.
        self.target = (self.target * (delta * 0.004).exp()).clamp(1., 8.);
        true
    }

    fn advance(&mut self, elapsed: Duration) -> bool {
        // Time-based easing stays consistent across different refresh rates.
        self.scale += (self.target - self.scale) * (1. - (-elapsed.as_secs_f32() * 20.).exp());
        self.pan.constrain(self.scale);
        if (self.target - self.scale).abs() < 0.001 {
            self.scale = self.target;
            self.pan.constrain(self.scale);
            false
        } else {
            true
        }
    }
}

#[derive(Clone)]
pub struct Photo {
    pub path: PathBuf,
    pub taken: SystemTime,
    pub modified: Option<SystemTime>,
    pub size: u64,
}

enum Row {
    /// "October 2026", number of photos in the month.
    Header(SharedString, usize),
    /// Indices into `shown`.
    Tiles(Vec<usize>),
}

pub struct GalleryView {
    ws: WeakEntity<Workspace>,
    photos: Vec<Photo>,
    /// Indices into `photos` matching the search, in display order.
    shown: Vec<usize>,
    rows: Arc<Vec<Row>>,
    list: ListState,
    cols: usize,
    tile: f32,
    width: f32,
    scanning: bool,
    query: Entity<InputState>,
    /// The search that's applied (set on Enter), lowercase.
    applied_query: String,
    /// Photo index (into `shown`) open in the viewer.
    viewer: Option<usize>,
    zoom: PhotoZoom,
    _scan: Option<Task<()>>,
    _sub: Subscription,
}

impl GalleryView {
    pub fn new(ws: WeakEntity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search photos: \"beach at sunset\", \"receipts\", a name…"));
        let sub = cx.subscribe_in(&query, window, |this: &mut Self, _, ev: &InputEvent, _, cx| {
            // Search on Enter; clearing the field shows everything again.
            let text = this.query.read(cx).value().trim().to_lowercase();
            let apply = match ev {
                InputEvent::PressEnter { .. } => true,
                InputEvent::Change => text.is_empty() && !this.applied_query.is_empty(),
                _ => false,
            };
            if apply {
                this.applied_query = text;
                this.refilter(cx);
            }
        });
        let mut this = Self {
            ws,
            photos: Vec::new(),
            shown: Vec::new(),
            rows: Arc::default(),
            list: ListState::new(0, ListAlignment::Top, px(600.)),
            cols: 4,
            tile: 160.,
            width: 0.,
            scanning: false,
            query,
            applied_query: String::new(),
            viewer: None,
            zoom: PhotoZoom::default(),
            _scan: None,
            _sub: sub,
        };
        this.rescan(cx);
        this
    }

    /// Finds every image again (Spotlight + Photos library).
    pub fn rescan(&mut self, cx: &mut Context<Self>) {
        self.scanning = true;
        // `shown`, `rows` and the viewer index into `photos`: reset together.
        self.photos.clear();
        self.shown.clear();
        self.viewer = None;
        self.relayout(cx);
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = smol::channel::unbounded::<Vec<Photo>>();
        let worker_cancel = cancel.clone();
        let job = cx.background_spawn(async move {
            rile_services::gallery::find_images(&worker_cancel, |batch| {
                let photos: Vec<Photo> = batch
                    .into_iter()
                    .filter_map(|path| {
                        // Capture dates (EXIF) come in a second pass; file
                        // dates keep the first results instant.
                        let meta = std::fs::metadata(&path).ok()?;
                        let taken = meta.created().ok().or_else(|| meta.modified().ok()).unwrap_or(SystemTime::UNIX_EPOCH);
                        Some(Photo { path, taken, modified: meta.modified().ok(), size: meta.len() })
                    })
                    .collect();
                tx.try_send(photos).ok();
            });
        });
        self._scan = Some(cx.spawn(async move |this, cx| {
            let _cancel = CancelOnDrop(cancel);
            let _job = job;
            while let Ok(mut batch) = rx.recv().await {
                while let Ok(more) = rx.try_recv() {
                    batch.extend(more);
                }
                if this
                    .update(cx, |this, cx| {
                        this.photos.extend(batch);
                        this.photos.sort_by(|a, b| b.taken.cmp(&a.taken));
                        this.refilter(cx);
                    })
                    .is_err()
                {
                    return;
                }
                cx.background_executor().timer(Duration::from_millis(150)).await;
            }
            // Second pass: capture dates from the image headers.
            let Ok(paths) = this.update(cx, |this, cx| {
                this.scanning = false;
                cx.notify();
                this.photos.iter().map(|p| p.path.clone()).collect::<Vec<_>>()
            }) else {
                return;
            };
            let dates = cx
                .background_spawn(async move {
                    paths
                        .into_iter()
                        .filter_map(|p| {
                            let taken = rile_services::gallery::image_meta(&p)?.taken?;
                            Some((p, taken))
                        })
                        .collect::<HashMap<PathBuf, SystemTime>>()
                })
                .await;
            this.update(cx, |this, cx| {
                for photo in &mut this.photos {
                    if let Some(t) = dates.get(&photo.path) {
                        photo.taken = *t;
                    }
                }
                this.photos.sort_by(|a, b| b.taken.cmp(&a.taken));
                this.refilter(cx);
            })
            .ok();
        }));
        cx.notify();
    }

    /// Applies the search: content matches (smart tags) first, then names.
    fn refilter(&mut self, cx: &mut Context<Self>) {
        let q = self.applied_query.clone();
        self.shown = if q.is_empty() {
            (0..self.photos.len()).collect()
        } else {
            let by_path: HashMap<&std::path::Path, usize> = self.photos.iter().enumerate().map(|(i, p)| (p.path.as_path(), i)).collect();
            let mut out: Vec<usize> = tags::search_kind(&q, Some(TagKind::Image), 0)
                .into_iter()
                .filter_map(|(p, _)| by_path.get(p.as_path()).copied())
                .collect();
            let mut seen: std::collections::HashSet<usize> = out.iter().copied().collect();
            for (i, p) in self.photos.iter().enumerate() {
                if !seen.contains(&i) && p.path.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains(&q)) {
                    out.push(i);
                    seen.insert(i);
                }
            }
            out
        };
        self.viewer = None;
        self.relayout(cx);
    }

    /// Groups the shown photos by month into rows of `cols` tiles.
    fn relayout(&mut self, cx: &mut Context<Self>) {
        self.rows = Arc::new(photo_rows(&self.photos, &self.shown, self.cols, !self.applied_query.is_empty()));
        self.list.reset(self.rows.len());
        cx.notify();
    }

    /// Side padding, matching the other full-page views.
    fn pad_x(&self) -> f32 {
        super::home::page_padding(super::chrome::content_class(self.width)).0
    }

    /// Adapts the grid to the slot's width.
    pub fn set_width(&mut self, width: f32, cx: &mut Context<Self>) {
        if (self.width - width).abs() < 1. {
            return;
        }
        self.width = width;
        let inner = (width - self.pad_x() * 2.).max(MIN_TILE);
        let cols = ((inner + GAP) / (MIN_TILE + GAP)).floor().max(1.) as usize;
        self.tile = ((inner - GAP * (cols as f32 - 1.)) / cols as f32).floor();
        if cols != self.cols {
            self.cols = cols;
            self.relayout(cx);
        } else {
            cx.notify();
        }
    }

    fn open_viewer(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.viewer = Some(ix);
        self.zoom = PhotoZoom::default();
        cx.notify();
    }

    fn step_viewer(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(ix) = self.viewer {
            let next = (ix as isize + delta).clamp(0, self.shown.len() as isize - 1) as usize;
            if next != ix {
                self.zoom = PhotoZoom::default();
            }
            self.viewer = Some(next);
            cx.notify();
        }
    }

    fn render_row(&mut self, row_ix: usize, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let ch = chrome(cx);
        let rows = self.rows.clone();
        match rows.get(row_ix) {
            Some(Row::Header(label, count)) => h_flex()
                .h(px(HEADER_HEIGHT))
                .px(px(self.pad_x()))
                .pt(px(14.))
                .items_baseline()
                .gap(px(10.))
                .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).text_color(ch.text_strong).child(label.clone()))
                .child(div().text_size(px(11.)).text_color(ch.text_faint).child(format!("{count} photo{}", if *count == 1 { "" } else { "s" })))
                .into_any_element(),
            Some(Row::Tiles(ixs)) => {
                let tile = self.tile;
                let tiles: Vec<AnyElement> = ixs
                    .iter()
                    .filter_map(|&shown_ix| {
                        let photo = self.shown.get(shown_ix).and_then(|&i| self.photos.get(i))?.clone();
                        let thumb = crate::art::thumbnail(&photo.path, photo.modified, photo.size, tile, cx);
                        div()
                            .id(("photo", shown_ix))
                            .size(px(tile))
                            .flex_none()
                            .rounded(px(6.))
                            .overflow_hidden()
                            .cursor_pointer()
                            .bg(ch.hover_strong)
                            .hover(|s| s.opacity(0.85))
                            .map(|d| match thumb {
                                Some(image) => d.child(img(image).size_full().object_fit(ObjectFit::Cover)),
                                // Still decoding: shimmer like the skeleton.
                                None => d.child(shimmer(div().size_full(), ("photo-shimmer", shown_ix), shown_ix, skeleton_tint(&ch))),
                            })
                            .on_click(cx.listener(move |this, _, _, cx| this.open_viewer(shown_ix, cx)))
                            .into_any_element()
                            .into()
                    })
                    .collect();
                h_flex().px(px(self.pad_x())).pb(px(GAP)).gap(px(GAP)).children(tiles).into_any_element()
            }
            None => div().into_any_element(),
        }
    }

    fn render_viewer(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let ch = chrome(cx);
        let Some(photo) = self.shown.get(ix).and_then(|&i| self.photos.get(i)).cloned() else { return div().into_any_element() };
        let now = Instant::now();
        let elapsed = self.zoom.last_frame.map(|last| now.duration_since(last)).unwrap_or(Duration::ZERO);
        if self.zoom.advance(elapsed) {
            self.zoom.last_frame = Some(now);
            cx.on_next_frame(window, |_, _, cx| cx.notify());
        } else {
            self.zoom.last_frame = None;
        }
        let image = crate::art::preview_image(&photo.path, photo.modified, photo.size, 1024., cx);
        self.zoom.pan.image = image.as_ref().map(|image| {
            let size = image.size(0);
            gpui_kit::size(size.width.0 as f32, size.height.0 as f32)
        }).unwrap_or_default();
        self.zoom.pan.constrain(self.zoom.scale);
        let entity = cx.entity();
        let pan_on_move = cx.listener(|this, event: &gpui_kit::MouseMoveEvent, _, cx| {
            if this.zoom.pan.move_to(event.position, event.pressed_button == Some(MouseButton::Left), this.zoom.scale) {
                cx.stop_propagation();
                cx.notify();
            }
        });
        let pan_on_up = cx.listener(|this, event: &gpui_kit::MouseUpEvent, _, cx| {
            if event.button == MouseButton::Left && this.zoom.pan.drag.take().is_some() {
                cx.stop_propagation();
                cx.notify();
            }
        });
        let record = tags::get(&photo.path);
        let name = photo.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let taken = chrono::DateTime::<chrono::Local>::from(photo.taken).format("%A, %B %-d, %Y at %H:%M").to_string();
        let folder = photo.path.parent().map(|p| p.display().to_string()).unwrap_or_default();
        let (count, path) = (self.shown.len(), photo.path.clone());
        let nav = |id: &'static str, icon: IconName, delta: isize, cx: &mut Context<Self>| {
            div()
                .id(id)
                .size(px(38.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .bg(gpui_kit::black().opacity(0.35))
                .hover(|s| s.bg(gpui_kit::black().opacity(0.55)))
                .child(crate::icons::icon(icon).size(px(18.)).text_color(gpui_kit::white()))
                .on_click(cx.listener(move |this, _, _, cx| this.step_viewer(delta, cx)))
        };
        let details = v_flex()
            .id("photo-details")
            .w(px(280.))
            .flex_none()
            .h_full()
            .min_w_0()
            .overflow_y_scroll()
            .gap(px(14.))
            .p(px(18.))
            .bg(ch.inspector)
            .border_l_1()
            .border_color(ch.hairline)
            .child(div().w_full().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(ch.text_strong).child(name))
            .child(div().text_size(px(11.)).text_color(ch.text_muted).child(taken))
            .child(div().w_full().text_size(px(10.)).text_color(ch.text_faint).child(format!("{} · {folder}", humansize::format_size(photo.size, humansize::DECIMAL))))
            .child(match &record {
                Some(r) => v_flex()
                    .gap(px(8.))
                    .when(!r.description.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(ch.text).child(r.description.clone())))
                    .child(h_flex().flex_wrap().gap(px(5.)).children(r.tags.iter().take(14).map(|t| {
                        div().px(px(7.)).py(px(2.)).rounded_full().bg(ch.accent_soft).text_size(px(10.)).text_color(ch.accent).child(t.clone())
                    })))
                    .when(!r.text.trim().is_empty(), |d| {
                        d.child(crate::views::kit::caption("TEXT IN IMAGE", cx)).child(
                            div().text_size(px(11.)).text_color(ch.text_muted).child(r.text.chars().take(400).collect::<String>()),
                        )
                    })
                    .into_any_element(),
                None => crate::views::kit::hint("Not described yet. Describing finds photos by what's in them.", cx).into_any_element(),
            })
            .child(div().flex_1())
            .child(
                v_flex()
                    .gap(px(8.))
                    .pt(px(14.))
                    .border_t_1()
                    .border_color(ch.hairline)
                    .child(Button::new("ph-open").cursor_pointer().w_full().primary().icon(IconName::ExternalLink).label("Open").on_click({
                        let path = path.clone();
                        move |_, _, cx| crate::workspace::open_with::open_file(&path, cx)
                    }))
                    .child(Button::new("ph-reveal").cursor_pointer().w_full().outline().icon(IconName::FolderOpen).label("Show in Folder").on_click(cx.listener({
                        let path = path.clone();
                        move |this, _, window, cx| {
                            let Some(ws) = this.ws.upgrade() else { return };
                            let (dir, file) = (path.parent().map(PathBuf::from), path.clone());
                            if let Some(dir) = dir {
                                ws.update(cx, |ws, cx| {
                                    ws.pane().update(cx, |p, _| p.select_after_reload(vec![file]));
                                    ws.open_location(Location::Dir(dir), window, cx);
                                });
                            }
                        }
                    })))
                    .when(record.is_none() && cfg!(any(target_os = "macos", windows)), |d| {
                        d.child(Button::new("ph-tag").cursor_pointer().w_full().ghost().child(ai_label("Describe Photo", ch.accent, cx)).tooltip("Describe this photo on device so search finds it by what's in it").on_click(cx.listener({
                            let path = path.clone();
                            move |_, _, _, cx| {
                                let path = path.clone();
                                let job = cx.background_spawn(async move {
                                    crate::workspace::tag_file(&path, false).ok();
                                });
                                cx.spawn(async move |this, cx| {
                                    job.await;
                                    this.update(cx, |_, cx| cx.notify()).ok();
                                })
                                .detach();
                            }
                        })))
                    }),
            );
        h_flex()
            .id("photo-viewer")
            .absolute()
            .inset_0()
            .overflow_hidden()
            .occlude()
            .bg(gpui_kit::black().opacity(0.92))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id("photo-stage")
                    .flex_1()
                    // Never let the photo's own size push the details panel
                    // out: the stage takes what's left and the photo fits in it.
                    .min_w_0()
                    .min_h_0()
                    .h_full()
                    .relative()
                    .overflow_hidden()
                    .on_scroll(cx.listener(|this, event: &gpui_kit::ScrollEvent, _, cx| {
                        if this.zoom.scroll(event.modifiers.control, event.delta) {
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .id("photo-pan-viewport")
                            .absolute()
                            .top(px(56.))
                            .bottom(px(40.))
                            .left(px(64.))
                            .right(px(64.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .overflow_hidden()
                            .when(image.is_some() && self.zoom.scale > 1., |d| d.cursor_grab())
                            .when(self.zoom.pan.drag.is_some(), |d| d.cursor_grabbing())
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &gpui_kit::MouseDownEvent, _, cx| {
                                if this.zoom.scale > 1. && this.zoom.pan.image.width > 0. {
                                    this.zoom.pan.drag = Some(event.position);
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                            }))
                            .child(match image {
                                Some(image) => img(image)
                                    .w(relative(self.zoom.scale))
                                    .h(relative(self.zoom.scale))
                                    .flex_none()
                                    .relative()
                                    .left(self.zoom.pan.offset.x)
                                    .top(self.zoom.pan.offset.y)
                                    .object_fit(ObjectFit::Contain)
                                    .into_any_element(),
                                None => div().text_color(gpui_kit::white().opacity(0.6)).child("Loading…").into_any_element(),
                            })
                            .child(gpui_kit::canvas(
                                move |bounds, _, cx| {
                                    entity.update(cx, |this, cx| {
                                        if this.zoom.pan.viewport != bounds.size {
                                            this.zoom.pan.viewport = bounds.size;
                                            this.zoom.pan.constrain(this.zoom.scale);
                                            cx.notify();
                                        }
                                    });
                                },
                                move |_, _, window, _| {
                                    // Keep dragging and releasing even beyond the image viewport.
                                    window.on_mouse_event(move |event: &gpui_kit::MouseMoveEvent, phase, window, cx| {
                                        if phase == gpui_kit::DispatchPhase::Capture {
                                            pan_on_move(event, window, cx);
                                        }
                                    });
                                    window.on_mouse_event(move |event: &gpui_kit::MouseUpEvent, phase, window, cx| {
                                        if phase == gpui_kit::DispatchPhase::Capture {
                                            pan_on_up(event, window, cx);
                                        }
                                    });
                                },
                            ).absolute().inset_0()),
                    )
                    .child(
                        div().absolute().top_0().bottom_0().left(px(14.)).flex().items_center()
                            .child(nav("ph-prev", IconName::ChevronLeft, -1, cx)),
                    )
                    .child(
                        div().absolute().top_0().bottom_0().right(px(14.)).flex().items_center()
                            .child(nav("ph-next", IconName::ChevronRight, 1, cx)),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(14.))
                            .left(px(14.))
                            .child(nav("ph-close", IconName::X, 0, cx).on_click(cx.listener(|this, _, _, cx| {
                                this.viewer = None;
                                cx.notify();
                            }))),
                    )
                    .child(
                        div()
                            .absolute()
                            .bottom(px(12.))
                            .left(px(14.))
                            .text_size(px(11.))
                            .text_color(gpui_kit::white().opacity(0.6))
                            .child(format!("{} of {count} · {:.0}% · Ctrl+scroll zoom · Drag to pan · ←/→ browse · Esc close", ix + 1, self.zoom.scale * 100.)),
                    ),
            )
            .child(details)
            .into_any_element()
    }
}

impl GalleryView {
    /// Placeholder grid shown until the first photos arrive: two month
    /// headers with rows of shimmering tiles, sized like the real grid.
    fn skeleton(&self, base: gpui_kit::Hsla) -> AnyElement {
        let (cols, tile, pad_x) = (self.cols.max(1), self.tile, self.pad_x());
        let mut n = 0;
        let mut section = |rows: usize, label_w: f32| {
            let header = h_flex()
                .h(px(HEADER_HEIGHT))
                .px(px(self.pad_x()))
                .pt(px(14.))
                .gap(px(10.))
                .items_center()
                .child(shimmer(div().w(px(label_w)).h(px(16.)).rounded(px(4.)), ("sk-head", n), n, base))
                .child(shimmer(div().w(px(64.)).h(px(11.)).rounded(px(4.)), ("sk-count", n), n + 1, base));
            let rows: Vec<AnyElement> = (0..rows)
                .map(|_| {
                    h_flex()
                        .px(px(pad_x))
                        .pb(px(GAP))
                        .gap(px(GAP))
                        .children((0..cols).map(|_| {
                            n += 1;
                            shimmer(div().size(px(tile)).flex_none().rounded(px(6.)), ("sk-tile", n), n, base)
                        }))
                        .into_any_element()
                })
                .collect();
            v_flex().child(header).children(rows)
        };
        v_flex()
            .id("gallery-skeleton")
            .flex_1()
            .overflow_hidden()
            .child(section(2, 132.))
            .child(section(3, 104.))
            .into_any_element()
    }
}

/// Tile indices are positions in `shown` in both browsing and search mode.
fn photo_rows(photos: &[Photo], shown: &[usize], cols: usize, searching: bool) -> Vec<Row> {
    let mut rows = Vec::new();
    if searching {
        // Tiles index into `shown`, not `photos`. Keep relevance order.
        for chunk in (0..shown.len()).collect::<Vec<_>>().chunks(cols.max(1)) {
            rows.push(Row::Tiles(chunk.to_vec()));
        }
    } else {
        let mut i = 0;
        while i < shown.len() {
            let month = month_of(photos[shown[i]].taken);
            let start = i;
            while i < shown.len() && month_of(photos[shown[i]].taken) == month {
                i += 1;
            }
            rows.push(Row::Header(month.into(), i - start));
            for chunk in (start..i).collect::<Vec<_>>().chunks(cols.max(1)) {
                rows.push(Row::Tiles(chunk.to_vec()));
            }
        }
    }
    rows
}

/// Color of loading placeholders: clearly visible on the browser
/// background in both themes.
fn skeleton_tint(ch: &crate::theme::Chrome) -> gpui_kit::Hsla {
    ch.text.opacity(if ch.dark { 0.13 } else { 0.10 })
}

/// A placeholder block that softly pulses; `phase` staggers neighbours so
/// the pulse ripples across the grid.
fn shimmer(el: gpui_kit::Div, id: impl Into<gpui_kit::ElementId>, phase: usize, base: gpui_kit::Hsla) -> gpui_kit::AnimationElement<gpui_kit::Div> {
    use gpui_kit::{Animation, AnimationExt};
    let offset = (phase % 12) as f32 / 12.;
    el.bg(base).with_animation(id, Animation::new(Duration::from_millis(1600)).repeat(), move |el, t| {
        let wave = ((t + offset) * std::f32::consts::TAU).sin() * 0.5 + 0.5;
        el.opacity(0.3 + 0.7 * wave)
    })
}

fn month_of(t: SystemTime) -> String {
    if t == SystemTime::UNIX_EPOCH {
        return "Undated".into();
    }
    chrono::DateTime::<chrono::Local>::from(t).format("%B %Y").to_string()
}

struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

impl Render for GalleryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ch = chrome(cx);
        let total = self.photos.len();
        let shown = self.shown.len();
        let searching = !self.applied_query.is_empty();
        let described = tags::count_kind(TagKind::Image);
        let ai_glyph = crate::icons::apple_intelligence(15., ch.accent, cx);
        // Describing runs as a workspace background job (see the jobs panel);
        // mirror its progress here.
        let job = self.ws.upgrade().and_then(|ws| {
            let ws = ws.read(cx);
            let id = ws.photo_job?;
            ws.ops.iter().find(|o| o.id == id).map(|o| (id, crate::jobs::op_status(o), o.progress.files_done, o.progress.files_total))
        });
        let describing = job.is_some();
        let status: AnyElement = match job {
            Some((id, st, done, todo)) => h_flex()
                .gap(px(8.))
                .text_size(px(11.))
                .text_color(ch.text_muted)
                .child(div().w(px(110.)).child(crate::progress::activity_bar("gallery-tagging", st.overall, 4., cx)))
                .child(if todo > 0 { format!("Describing photos {done}/{todo}") } else { "Looking for photos to describe…".to_string() })
                .child(
                    Button::new("gallery-tag-stop")
                        .cursor_pointer()
                        .xsmall()
                        .ghost()
                        .label("Pause")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(ws) = this.ws.upgrade() {
                                ws.update(cx, |ws, cx| {
                                    ws.cancel_op(id);
                                    cx.notify();
                                });
                            }
                        })),
                )
                .into_any_element(),
            None => h_flex()
                .gap(px(8.))
                .child(div().text_size(px(11.)).text_color(ch.text_faint).child(if self.scanning {
                    format!("Finding photos… {total}")
                } else if searching {
                    format!("{shown} of {total} photos")
                } else {
                    format!("{total} photos · {described} described")
                }))
                .when(!self.scanning && cfg!(any(target_os = "macos", windows)) && described < total, |d| {
                    d.child(
                        Button::new("gallery-describe")
                            .cursor_pointer()
                            .small()
                            .outline()
                            .child(ai_label("Describe All", ch.accent, cx))
                            .tooltip("Describe every photo on device so search finds them by what's in them")
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(ws) = this.ws.upgrade() {
                                    ws.update(cx, |ws, cx| ws.start_photo_job(true, window, cx));
                                }
                            })),
                    )
                })
                .into_any_element(),
        };
        let class = super::chrome::content_class(self.width);
        let header = v_flex()
            .px(px(self.pad_x()))
            .pt(px(super::home::page_title_top(class)))
            .pb(px(8.))
            .gap(px(super::home::PAGE_TITLE_GAP))
            .child(
                h_flex()
                    .gap(px(12.))
                    .child(super::home::page_title("Photos", class, &ch).flex_1())
                    .child(status)
                    .when(!describing && !self.scanning, |d| {
                        d.child(
                            Button::new("gallery-rescan")
                                .cursor_pointer()
                                .xsmall()
                                .ghost()
                                .icon(crate::icons::icon(IconName::RotateCw))
                                .tooltip("Look for new photos")
                                .on_click(cx.listener(|this, _, _, cx| this.rescan(cx))),
                        )
                    }),
            )
            .child(
                h_flex()
                    .h(px(34.))
                    .px(px(10.))
                    .gap(px(8.))
                    .rounded(px(9.))
                    .border_1()
                    .border_color(ch.search_border)
                    .bg(ch.search_bg)
                    .child(ai_glyph)
                    .child(div().flex_1().child(Input::new(&self.query).appearance(false).small().cleanable(true))),
            );
        let empty = shown == 0 && !self.scanning;
        let loading = shown == 0 && self.scanning;
        let viewer = self.viewer.map(|ix| self.render_viewer(ix, window, cx));
        v_flex()
            .id("gallery")
            .key_context("Gallery")
            .size_full()
            .relative()
            .bg(ch.browser)
            .child(header)
            .when(empty, |d| {
                d.child(
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .gap(px(8.))
                        .child(crate::icons::icon(IconName::Images).size(px(34.)).text_color(ch.text_faint))
                        .child(div().text_size(px(13.)).text_color(ch.text_muted).child(if searching {
                            "No photos match — try other words, or let them be described first"
                        } else {
                            if cfg!(windows) { "No photos found. Windows Search indexes your pictures; check that indexing is on." } else { "No photos found. Spotlight indexes your pictures; check that it's on." }
                        })),
                )
            })
            .when(loading, |d| d.child(self.skeleton(skeleton_tint(&ch))))
            .when(!empty && !loading, |d| {
                d.child(list(self.list.clone(), cx.processor(|this, ix: usize, window, cx| this.render_row(ix, window, cx))).flex_1().pb(px(24.)))
            })
            .children(viewer)
    }
}

/// The gallery for a pane slot, created on first use and sized to the slot.
pub fn render(ws: &mut Workspace, slot: usize, window: &mut Window, cx: &mut Context<Workspace>) -> AnyElement {
    let gallery = match &ws.gallery {
        Some(g) => g.clone(),
        None => {
            let weak = cx.entity().downgrade();
            let g = cx.new(|cx| GalleryView::new(weak, window, cx));
            ws.gallery = Some(g.clone());
            g
        }
    };
    let width = ws.slot_width(slot, window);
    gallery.update(cx, |g, cx| g.set_width(width, cx));
    let focus = ws.panes[slot].read(cx).focus_handle.clone();
    let keys = gallery.downgrade();
    div()
        .id(("gallery-slot", slot))
        .track_focus(&focus)
        .size_full()
        // The slot holds focus, so the photo viewer's keys arrive here.
        .on_key_down(move |ev: &gpui_kit::KeyDownEvent, _, cx| {
            keys.update(cx, |this, cx| {
                if this.viewer.is_none() {
                    return;
                }
                match ev.keystroke.key.as_str() {
                    "left" => this.step_viewer(-1, cx),
                    "right" => this.step_viewer(1, cx),
                    "escape" | "space" => {
                        this.viewer = None;
                        cx.notify();
                    }
                    _ => return,
                }
                cx.stop_propagation();
            })
            .ok();
        })
        .capture_any_mouse_down(cx.listener(move |this, _, w, cx| {
            if this.active_pane != slot {
                this.set_active_pane(slot, w, cx);
            }
        }))
        .child(gallery)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pan_fixture() -> PhotoPan {
        PhotoPan {
            viewport: gpui_kit::size(px(400.), px(300.)),
            image: gpui_kit::size(800., 600.),
            ..PhotoPan::default()
        }
    }

    #[test]
    fn dragging_moves_with_mouse_and_stops_after_release() {
        let mut pan = pan_fixture();
        let start = gpui_kit::point(px(100.), px(100.));
        let moved = gpui_kit::point(px(140.), px(125.));
        assert!(!pan.move_to(moved, true, 2.));
        pan.drag = Some(start);
        assert!(pan.move_to(moved, true, 2.));
        assert_eq!(pan.offset, gpui_kit::point(px(40.), px(25.)));
        assert_eq!(pan.drag, Some(moved));
        assert!(pan.move_to(start, false, 2.));
        assert_eq!(pan.drag, None);
        assert!(!pan.move_to(start, true, 2.));
        assert_eq!(pan.offset, gpui_kit::point(px(40.), px(25.)));
    }

    #[test]
    fn dragging_clamps_at_edges_and_reverses_immediately() {
        let mut pan = pan_fixture();
        pan.drag = Some(Point::default());
        pan.move_to(gpui_kit::point(px(1000.), px(-1000.)), true, 2.);
        assert_eq!(pan.offset, gpui_kit::point(px(200.), px(-150.)));
        pan.move_to(gpui_kit::point(px(990.), px(-990.)), true, 2.);
        assert_eq!(pan.offset, gpui_kit::point(px(190.), px(-140.)));
    }

    #[test]
    fn pan_limits_account_for_letterboxing_on_both_axes() {
        let mut pan = pan_fixture();
        pan.image = gpui_kit::size(1200., 300.);
        pan.offset = gpui_kit::point(px(1000.), px(1000.));
        pan.constrain(2.);
        assert_eq!(pan.offset, gpui_kit::point(px(200.), px(0.)));
        pan.image = gpui_kit::size(300., 1200.);
        pan.offset = gpui_kit::point(px(-1000.), px(-1000.));
        pan.constrain(2.);
        assert_eq!(pan.offset, gpui_kit::point(px(0.), px(-150.)));
    }

    #[test]
    fn pan_recenters_on_zoom_out_and_reclamps_after_resize() {
        let mut zoom = PhotoZoom { scale: 2., target: 1., pan: pan_fixture(), ..PhotoZoom::default() };
        zoom.pan.offset = gpui_kit::point(px(200.), px(150.));
        zoom.pan.drag = Some(Point::default());
        zoom.advance(Duration::from_secs(1));
        assert_eq!(zoom.pan.offset, Point::default());
        assert_eq!(zoom.pan.drag, None);
        zoom.pan.offset = gpui_kit::point(px(200.), px(150.));
        zoom.pan.viewport = gpui_kit::size(px(200.), px(150.));
        zoom.pan.constrain(2.);
        assert_eq!(zoom.pan.offset, gpui_kit::point(px(100.), px(75.)));
    }

    #[test]
    fn zoom_requires_control_and_vertical_scroll() {
        let mut zoom = PhotoZoom::default();
        let vertical = ScrollDelta::Lines(gpui_kit::point(0., 1.));
        assert!(!zoom.scroll(false, vertical));
        assert!(!zoom.scroll(true, ScrollDelta::Lines(gpui_kit::point(1., 0.))));
        assert!(!zoom.scroll(true, ScrollDelta::Lines(gpui_kit::point(0., f32::NAN))));
        assert_eq!(zoom.target, 1.);
        assert!(zoom.scroll(true, vertical));
        assert!(zoom.target > 1.);
        assert_eq!(zoom.scale, 1., "scroll should set a target, not jump to it");
    }

    #[test]
    fn zoom_normalizes_scroll_units_and_clamps_to_fit_and_eight_times() {
        let mut lines = PhotoZoom::default();
        let mut pixels = PhotoZoom::default();
        lines.scroll(true, ScrollDelta::Lines(gpui_kit::point(0., 1.)));
        pixels.scroll(true, ScrollDelta::Pixels(gpui_kit::point(px(0.), px(24.))));
        assert_eq!(lines.target, pixels.target);
        lines.scroll(true, ScrollDelta::Lines(gpui_kit::point(0., -1.)));
        assert!((lines.target - 1.).abs() < 0.0001);
        lines.scroll(true, ScrollDelta::Lines(gpui_kit::point(0., 10000.)));
        assert_eq!(lines.target, 8.);
        lines.scroll(true, ScrollDelta::Lines(gpui_kit::point(0., -10000.)));
        assert_eq!(lines.target, 1.);
    }

    #[test]
    fn zoom_eases_in_both_directions_and_stops_at_target() {
        let mut zoom = PhotoZoom { target: 4., ..PhotoZoom::default() };
        assert!(zoom.advance(Duration::from_millis(16)));
        assert!(zoom.scale > 1. && zoom.scale < 4.);
        assert!(!zoom.advance(Duration::from_secs(1)));
        assert_eq!(zoom.scale, 4.);
        zoom.target = 1.;
        assert!(zoom.advance(Duration::from_millis(16)));
        assert!(zoom.scale > 1. && zoom.scale < 4.);
        assert!(!zoom.advance(Duration::from_secs(1)));
        assert_eq!(zoom.scale, 1.);
    }

    #[test]
    fn zoom_easing_is_independent_of_frame_rate() {
        let mut fast = PhotoZoom { target: 4., ..PhotoZoom::default() };
        let mut slow = PhotoZoom { target: 4., ..PhotoZoom::default() };
        for _ in 0..10 {
            fast.advance(Duration::from_millis(10));
        }
        slow.advance(Duration::from_millis(100));
        assert!((fast.scale - slow.scale).abs() < 0.0001);
    }

    #[test]
    fn search_rows_resolve_filtered_positions_in_relevance_order() {
        // Sparse, reordered matches: photo indices cannot be used as positions
        // in the much shorter filtered list (including a single OCR match).
        for shown in [vec![7, 2, 5], vec![7], vec![]] {
            for cols in [0, 1, 2, 6] {
                let rows = photo_rows(&[], &shown, cols, true);
                let mut displayed = Vec::new();
                for row in rows {
                    let Row::Tiles(indices) = row else { panic!("search must not have month headers") };
                    assert!(indices.len() <= cols.max(1));
                    for ix in indices {
                        displayed.push(*shown.get(ix).expect("tile must index into filtered results"));
                    }
                }
                assert_eq!(displayed, shown);
            }
        }
    }
}

/// An AI action's button content: the Apple Intelligence symbol and a label.
fn ai_label<V: 'static>(label: &'static str, color: gpui_kit::Hsla, cx: &mut Context<V>) -> impl IntoElement {
    h_flex().gap(px(6.)).child(crate::icons::apple_intelligence(16., color, cx)).child(label)
}
