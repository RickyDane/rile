//! Quick Look overlay (Space), the optional preview side panel and the
//! Properties dialog.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gpui_kit::component::{
    ActiveTheme, Disableable, Sizable, h_flex,
    button::{Button, ButtonVariants},
    spinner::Spinner,
    v_flex,
};
use gpui_kit::assets::IconName;
use gpui_kit::{
    AnyElement, AppContext, Context, InteractiveElement, IntoElement, MouseButton,
    ObjectFit, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    ScrollDelta, ScrollEvent, StyledImage, Task, Window, div, img, prelude::FluentBuilder, px, uniform_list,
};
use rile_services::preview::{self, Preview};
use rile_services::properties::{self, Properties};

use super::{Workspace, item_count};
use crate::actions::*;
use crate::icons::icon_for_path;

const MAX_TEXT: usize = 512 * 1024;

/// Explicitly allow filesystem images only for local Markdown documents.
/// GPUI's default Markdown resolver deliberately treats all links as URIs.
fn markdown_image_source(document: &std::path::Path, uri: &gpui_kit::SharedUri) -> gpui_kit::ImageSource {
    if let Ok(data) = data_url::DataUrl::process(uri.as_ref()) {
        let mime = data.mime_type();
        if let Some(format) = gpui_kit::ImageFormat::from_mime_type(&format!("{}/{}", mime.type_, mime.subtype)) {
            if let Ok((bytes, _)) = data.decode_to_vec() {
                return Arc::new(gpui_kit::Image::from_bytes(format, bytes)).into();
            }
        }
    }

    let resolved = url::Url::parse(uri.as_ref()).ok().or_else(|| {
        let document = std::path::absolute(document).ok()?;
        url::Url::from_file_path(document).ok()?.join(uri.as_ref()).ok()
    });
    if let Some(path) = resolved.and_then(|url| url.to_file_path().ok()) {
        return path.into();
    }
    uri.clone().into()
}

#[cfg(test)]
mod markdown_image_tests {
    use super::markdown_image_source;
    use gpui_kit::{ImageSource, Resource};
    use std::path::Path;

    #[test]
    fn local_images_resolve_relative_to_document() {
        let document = Path::new("/project/docs/README.md");
        for (link, expected) in [
            ("screenshots/native-list.png", "/project/docs/screenshots/native-list.png"),
            ("../images/screen%20shot.png", "/project/images/screen shot.png"),
            ("/images/photo.png", "/images/photo.png"),
            ("file:///images/screen%20shot.png", "/images/screen shot.png"),
            ("images/photo.png?raw=true#preview", "/project/docs/images/photo.png"),
        ] {
            let ImageSource::Resource(Resource::Path(path)) = markdown_image_source(document, &link.into()) else {
                panic!("expected a filesystem image for {link}");
            };
            assert_eq!(path.as_ref(), Path::new(expected));
        }
    }

    #[test]
    fn web_images_remain_uri_resources() {
        let link = "https://example.com/image.png";
        let ImageSource::Resource(Resource::Uri(uri)) = markdown_image_source(Path::new("/docs/README.md"), &link.into()) else {
            panic!("expected a web image");
        };
        assert_eq!(uri.as_ref(), link);
    }

    #[test]
    fn embedded_images_remain_decoded() {
        let link = "data:image/png;base64,iVBORw0KGgo=";
        assert!(matches!(
            markdown_image_source(Path::new("/docs/README.md"), &link.into()),
            ImageSource::Image(_)
        ));
    }
}

pub enum Loaded {
    Preview(Preview),
    /// A PDF: its page count; pages render progressively into
    /// [`PreviewState::pdf_pages`].
    Pdf { pages: usize },
    /// Text supplied by a plugin previewer.
    Plugin(String),
    Error(String),
}

/// Inspector details read alongside the preview.
pub struct Info {
    pub props: Option<Properties>,
    /// Pixel dimensions of images, read from the file header.
    pub dimensions: Option<(usize, usize)>,
}

/// What the preview surfaces show: the active pane's cursor item.
#[derive(Default)]
pub struct PreviewState {
    pub quicklook: bool,
    pub target: Option<PathBuf>,
    /// Connection `target` lives on (None = local): equal paths on different
    /// servers (or local vs remote) are different files.
    pub target_conn: Option<String>,
    pub loaded: Option<(PathBuf, Arc<Loaded>, Arc<Vec<SharedString>>)>,
    /// Connection `loaded` and `info` were read from.
    pub loaded_conn: Option<String>,
    /// Metadata of `target` for the inspector (local files only).
    pub info: Option<(PathBuf, Arc<Info>)>,
    /// Syntax colors per line of `loaded` (empty when not highlighted).
    pub highlights: Arc<Vec<Vec<(std::ops::Range<usize>, gpui_kit::HighlightStyle)>>>,
    /// Rendered pages of a PDF in `loaded`, in order.
    pub pdf_pages: Vec<Arc<gpui_kit::Image>>,
    pdf_zoom: PdfZoom,
    pdf_scroll: gpui_kit::ScrollHandle,
    pdf_drag: Option<gpui_kit::Point<gpui_kit::Pixels>>,
    _task: Option<Task<()>>,
    _pdf_task: Option<Task<()>>,
    pub(crate) properties_view: Option<gpui_kit::Entity<PropertiesView>>,
    pub(crate) properties_only: bool,
    properties_subtitle: Option<SharedString>,
    /// In-place text editor in the Quick Look card.
    pub(crate) editor: Option<super::text_edit::TextEditor>,
}

/// Pages rendered for the PDF viewer (the rest: open the file).
const MAX_PDF_PAGES: usize = 300;
const PDF_PAGE_PX: u32 = 1400;

/// Continuous zoom relative to the available viewer width.
struct PdfZoom {
    percent: f32,
}

impl Default for PdfZoom {
    fn default() -> Self {
        Self { percent: 100. }
    }
}

impl PdfZoom {
    fn percent(&self) -> f32 {
        self.percent
    }

    fn adjust(&mut self, steps: i8) {
        self.percent = (self.percent + f32::from(steps) * 25.).clamp(50., 300.);
    }

    fn scroll(&mut self, event: &ScrollEvent) -> bool {
        if !event.modifiers.control {
            return false;
        }
        let delta = match event.delta {
            ScrollDelta::Pixels(delta) => f32::from(delta.y) / 40.,
            ScrollDelta::Lines(delta) => delta.y,
        };
        self.percent = (self.percent + delta * 5.).clamp(50., 300.);
        true
    }
}

#[cfg(test)]
mod pdf_zoom_tests {
    use super::PdfZoom;

    #[test]
    fn zoom_steps_and_reset() {
        let mut zoom = PdfZoom::default();
        assert_eq!(zoom.percent(), 100.);
        zoom.adjust(1);
        assert_eq!(zoom.percent(), 125.);
        zoom.adjust(-1);
        assert_eq!(zoom.percent(), 100.);
        zoom.adjust(-1);
        assert_eq!(zoom.percent(), 75.);
        zoom = PdfZoom::default();
        assert_eq!(zoom.percent(), 100.);
    }

    #[test]
    fn zoom_is_bounded() {
        let mut zoom = PdfZoom::default();
        for _ in 0..20 {
            zoom.adjust(1);
        }
        assert_eq!(zoom.percent(), 300.);
        for _ in 0..20 {
            zoom.adjust(-1);
        }
        assert_eq!(zoom.percent(), 50.);
    }

    #[test]
    fn control_scroll_accumulates_precise_deltas() {
        use gpui_kit::{Modifiers, ScrollDelta, ScrollEvent, point, px};

        let mut zoom = PdfZoom::default();
        let mut event = ScrollEvent {
            delta: ScrollDelta::Pixels(point(px(0.), px(10.))),
            ..Default::default()
        };
        assert!(!zoom.scroll(&event));
        assert_eq!(zoom.percent(), 100.);
        event.modifiers = Modifiers { control: true, ..Modifiers::none() };
        for i in 1..=4 {
            assert!(zoom.scroll(&event));
            assert_eq!(zoom.percent(), 100. + i as f32 * 1.25);
        }
        assert_eq!(zoom.percent(), 105.);
        event.delta = ScrollDelta::Pixels(point(px(0.), px(-40.)));
        zoom.scroll(&event);
        assert_eq!(zoom.percent(), 100.);
        event.delta = ScrollDelta::Pixels(point(px(0.), px(0.1)));
        zoom.scroll(&event);
        assert!((zoom.percent() - 100.0125).abs() < 0.0001);
        event.delta = ScrollDelta::Lines(point(0., 1000.));
        zoom.scroll(&event);
        assert_eq!(zoom.percent(), 300.);
        event.delta = ScrollDelta::Lines(point(0., -1000.));
        zoom.scroll(&event);
        assert_eq!(zoom.percent(), 50.);
    }
}

/// Splits highlight spans over `text` into per-line runs.
fn line_highlights(text: &str, spans: &[rile_services::highlight::Span]) -> Vec<Vec<(std::ops::Range<usize>, gpui_kit::HighlightStyle)>> {
    let mut out = Vec::new();
    let mut offset = 0;
    let mut i = 0;
    for line in text.split('\n') {
        let (start, end) = (offset, offset + line.len());
        let mut runs = Vec::new();
        while i < spans.len() && spans[i].range.end <= start {
            i += 1;
        }
        let mut j = i;
        while j < spans.len() && spans[j].range.start < end {
            let sp = &spans[j];
            let (a, b) = (sp.range.start.max(start) - start, sp.range.end.min(end) - start);
            if a < b {
                let c = sp.color;
                runs.push((
                    a..b,
                    gpui_kit::HighlightStyle {
                        color: Some(gpui_kit::rgb(c).into()),
                        font_weight: sp.bold.then_some(gpui_kit::FontWeight::BOLD),
                        font_style: sp.italic.then_some(gpui_kit::FontStyle::Italic),
                        ..Default::default()
                    },
                ));
            }
            j += 1;
        }
        out.push(runs);
        offset = end + 1;
    }
    out
}

impl Workspace {
    fn preview_target(&self, cx: &gpui_kit::App) -> Option<PathBuf> {
        let pane = self.pane().read(cx);
        pane.cursor_entry()
            .filter(|e| pane.selected.contains(&e.path))
            .or_else(|| pane.selected_entries().first().copied())
            .map(|e| e.path.clone())
    }

    /// Re-targets previews when the cursor moves; loads in the background.
    pub fn sync_preview(&mut self, cx: &mut Context<Self>) {
        if !self.preview.quicklook && !self.settings.show_preview_panel && !self.inspector_overlay {
            return;
        }
        let target = self.preview_target(cx);
        let conn = self.pane().read(cx).conn();
        if target == self.preview.target && conn == self.preview.target_conn {
            return;
        }
        if self.preview.editor.as_ref().is_some_and(|e| Some(&e.path) != target.as_ref()) {
            self.finish_editing(true, cx);
            return;
        }
        self.preview.target = target.clone();
        self.preview.target_conn = conn.clone();
        self.preview.pdf_zoom = PdfZoom::default();
        self.preview.pdf_scroll = gpui_kit::ScrollHandle::new();
        self.preview.pdf_drag = None;
        self.preview.properties_view = None;
        self.preview.properties_subtitle = None;
        let Some(path) = target else {
            self.preview._task = None;
            return;
        };
        let plugins = self.plugins.host.clone();
        let dark = cx.theme().is_dark();
        let task = cx.background_spawn({
            let path = path.clone();
            let conn = conn.clone();
            async move {
                if let Some(conn) = conn {
                    return (remote_preview(&conn, &path), None);
                }
                let info = Info {
                    props: properties::basic(&path).ok(),
                    dimensions: imagesize::size(&path).ok().map(|s| (s.width, s.height)),
                };
                (load_local(&plugins, &path), Some(info))
            }
        });
        self.preview._pdf_task = None;
        self.preview.pdf_pages.clear();
        self.preview._task = Some(cx.spawn(async move |this, cx| {
            let (loaded, info) = task.await;
            // Split into lines and color code off the UI thread.
            let (loaded, lines, highlights) = cx
                .background_spawn(async move {
                    let (lines, highlights) = match &loaded {
                        Loaded::Preview(Preview::Text { text, language, .. }) => {
                            let text = text.replace('\t', "    ");
                            let spans = rile_services::highlight::highlight(&text, language.as_deref(), dark);
                            let lines: Vec<SharedString> = text.lines().map(|l| SharedString::from(l.to_string())).collect();
                            let hl = spans.map(|s| line_highlights(&text, &s)).unwrap_or_default();
                            (lines, hl)
                        }
                        Loaded::Plugin(text) => (text.lines().map(|l| SharedString::from(l.replace('\t', "    "))).collect(), Vec::new()),
                        _ => (Vec::new(), Vec::new()),
                    };
                    (loaded, lines, highlights)
                })
                .await;
            let pdf_pages = match &loaded {
                Loaded::Pdf { pages } => Some(*pages),
                _ => None,
            };
            this.update(cx, |this, cx| {
                if this.preview.target.as_ref() == Some(&path) && this.preview.target_conn == conn {
                    this.preview.loaded_conn = conn;
                    this.preview.info = info.map(|i| (path.clone(), Arc::new(i)));
                    this.preview.highlights = Arc::new(highlights);
                    this.preview.loaded = Some((path.clone(), Arc::new(loaded), Arc::new(lines)));
                    if let Some(pages) = pdf_pages {
                        this.render_pdf_pages(path, pages, cx);
                    }
                    cx.notify();
                }
            })
            .ok();
        }));
    }
}

impl Workspace {
    /// Renders the PDF's pages one after another into the viewer.
    fn render_pdf_pages(&mut self, path: PathBuf, pages: usize, cx: &mut Context<Self>) {
        let (tx, rx) = smol::channel::unbounded::<Arc<gpui_kit::Image>>();
        let worker_path = path.clone();
        let job = cx.background_spawn(async move {
            for page in 0..pages.min(MAX_PDF_PAGES) {
                let Ok(png) = rile_services::pdf::render_page(&worker_path, page, PDF_PAGE_PX) else { break };
                if tx.send(Arc::new(gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, png))).await.is_err() {
                    break;
                }
            }
        });
        self.preview._pdf_task = Some(cx.spawn(async move |this, cx| {
            let _job = job;
            while let Ok(page) = rx.recv().await {
                let alive = this.update(cx, |this, cx| {
                    if this.preview.target.as_ref() != Some(&path) {
                        return false;
                    }
                    this.preview.pdf_pages.push(page);
                    cx.notify();
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        }));
    }
}

fn load_local(plugins: &Option<Arc<std::sync::Mutex<rile_plugins::PluginHost>>>, path: &std::path::Path) -> Loaded {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        && let Ok(pages) = rile_services::pdf::page_count(path)
    {
        return Loaded::Pdf { pages };
    }
    if let Some(host) = plugins
        && let Some(text) = host.lock().ok().and_then(|mut h| h.preview(path))
    {
        return Loaded::Plugin(text);
    }
    if cfg!(target_os = "macos")
        && super::text_edit::is_rtf(path)
        && let Ok(text) = super::text_edit::read_text(path)
    {
        return Loaded::Preview(Preview::Text { text, truncated: false, language: None });
    }
    match preview::load(path, MAX_TEXT) {
        Ok(p) => Loaded::Preview(p),
        Err(e) => Loaded::Error(format!("{e:#}")),
    }
}

impl Workspace {
    fn set_previewing(&mut self, on: bool, cx: &mut Context<Self>) {
        self.preview.pdf_drag = None;
        self.preview.quicklook = on;
        if !on {
            // Closing keeps edits, like Quick Look markup does.
            self.finish_editing(true, cx);
            self.preview.properties_only = false;
            self.preview.properties_view = None;
            self.preview.properties_subtitle = None;
            self.preview.target = None.or(self.preview.target.take().filter(|_| self.settings.show_preview_panel));
        }
        for pane in &self.all_panes() {
            pane.update(cx, |p, cx| {
                p.previewing = on;
                cx.notify();
            });
        }
        self.sync_preview(cx);
        cx.notify();
    }

    pub(crate) fn toggle_quicklook(&mut self, _: &ToggleQuickLook, _: &mut Window, cx: &mut Context<Self>) {
        if self.preview.quicklook {
            self.set_previewing(false, cx);
        } else if self.preview_target(cx).is_some() {
            self.set_previewing(true, cx);
        }
    }

    /// Closing from a click: an open editor held focus, so give it back to the list.
    fn close_quicklook_ui(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editing = self.preview.editor.is_some();
        self.set_previewing(false, cx);
        if editing {
            let focus = self.pane().read(cx).focus_handle.clone();
            focus.focus(window, cx);
        }
    }

    pub(crate) fn close_preview(&mut self, _: &ClosePreview, _: &mut Window, cx: &mut Context<Self>) {
        self.set_previewing(false, cx);
    }

    pub(crate) fn toggle_preview_panel(&mut self, _: &TogglePreviewPanel, window: &mut Window, cx: &mut Context<Self>) {
        self.preview.target = None;
        if crate::views::chrome::width_class(window) <= crate::views::chrome::Width::Phone {
            // Narrow windows float the inspector temporarily; the saved
            // preference for wider windows stays untouched.
            self.inspector_overlay = !self.inspector_overlay;
        } else {
            self.settings.show_preview_panel = !self.settings.show_preview_panel;
            self.inspector_overlay = false;
            self.save_settings(window, cx);
        }
        self.sync_preview(cx);
        cx.notify();
    }

    fn render_preview_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(target) = &self.preview.target else {
            return center_text("No selection", cx);
        };
        let Some((path, loaded, lines)) =
            self.preview.loaded.as_ref().filter(|(p, ..)| p == target && self.preview.loaded_conn == self.preview.target_conn)
        else {
            return center_text("Loading…", cx);
        };
        match &**loaded {
            Loaded::Preview(Preview::Image(p)) => div()
                .size_full()
                .p_2()
                .flex()
                .items_center()
                .justify_center()
                .child(img(p.clone()).size_full().object_fit(ObjectFit::Contain))
                .into_any_element(),
            Loaded::Preview(Preview::Binary { .. }) => center_text("No preview available", cx),
            Loaded::Preview(Preview::Text { text, .. }) if super::text_edit::is_markdown(path) && !text.is_empty() => div()
                .id("preview-markdown")
                .size_full()
                .overflow_y_scroll()
                .px_6()
                .py_4()
                .text_sm()
                .child(gpui_kit::component::text::TextView::markdown(
                    gpui_kit::ElementId::Name(format!("md-{}", path.display()).into()),
                    SharedString::from(text.clone()),
                ).selectable(true).when(self.preview.loaded_conn.is_none(), |view| {
                    let document = path.clone();
                    view.image_source(move |uri| markdown_image_source(&document, uri))
                }))
                .into_any_element(),
            Loaded::Preview(Preview::Text { .. }) | Loaded::Plugin(_) if !lines.is_empty() => {
                let truncated = matches!(&**loaded, Loaded::Preview(Preview::Text { truncated: true, .. }));
                let lines = lines.clone();
                let highlights = self.preview.highlights.clone();
                let mono = theme.mono_font_family.clone();
                let gutter = (lines.len().to_string().len() as f32) * 8. + 12.;
                let language = match &**loaded {
                    Loaded::Preview(Preview::Text { language: Some(l), .. }) => rile_services::highlight::language_name(l),
                    _ => None,
                };
                v_flex()
                    .size_full()
                    .child(
                        uniform_list("preview-lines", lines.len(), move |range, _, cx| {
                            let muted = cx.theme().muted_foreground;
                            range
                                .map(|i| {
                                    h_flex()
                                        .h(px(18.))
                                        .px_2()
                                        .child(div().w(px(gutter)).flex_none().text_color(muted).child(format!("{}", i + 1)))
                                        .child(div().whitespace_nowrap().child(match highlights.get(i) {
                                            Some(runs) if !runs.is_empty() => {
                                                gpui_kit::StyledText::new(lines[i].clone()).with_highlights(runs.clone()).into_any_element()
                                            }
                                            _ => lines[i].clone().into_any_element(),
                                        }))
                                })
                                .collect()
                        })
                        .flex_1()
                        .font_family(mono)
                        .text_xs(),
                    )
                    .when(truncated || language.is_some(), |d| {
                        d.child(
                            h_flex()
                                .px_2()
                                .py_1()
                                .gap_2()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .children(language.map(|l| div().child(l)))
                                .when(truncated, |d| d.child("Preview truncated")),
                        )
                    })
                    .into_any_element()
            }
            Loaded::Pdf { pages } => {
                let rendered = self.preview.pdf_pages.clone();
                let pages = *pages;
                if rendered.is_empty() {
                    return center_text("Rendering PDF…", cx);
                }
                let more = pages > rendered.len();
                let zoom = self.preview.pdf_zoom.percent();
                let zoom_on_scroll = cx.listener(|this, event: &ScrollEvent, _, cx| {
                    if this.preview.pdf_zoom.scroll(event) {
                        cx.stop_propagation();
                        cx.notify();
                    }
                });
                let pan_on_move = cx.listener(|this, event: &gpui_kit::MouseMoveEvent, _, cx| {
                    let Some(previous) = this.preview.pdf_drag else { return };
                    if event.pressed_button != Some(MouseButton::Left) {
                        this.preview.pdf_drag = None;
                    } else {
                        let offset = this.preview.pdf_scroll.offset() + (event.position - previous);
                        this.preview.pdf_scroll.set_offset(offset);
                        this.preview.pdf_drag = Some(event.position);
                        cx.stop_propagation();
                    }
                    cx.notify();
                });
                let pan_on_up = cx.listener(|this, event: &gpui_kit::MouseUpEvent, _, cx| {
                    if event.button == MouseButton::Left && this.preview.pdf_drag.take().is_some() {
                        cx.stop_propagation();
                        cx.notify();
                    }
                });
                v_flex()
                    .size_full()
                    .child(
                        h_flex()
                            .flex_none()
                            .justify_center()
                            .gap_2()
                            .p_2()
                            .child(Button::new("pdf-zoom-out").ghost().small().label("−").tooltip("Zoom out")
                                .when(zoom > 50., |button| button.cursor_pointer())
                                .disabled(zoom <= 50.)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.preview.pdf_zoom.adjust(-1);
                                    cx.notify();
                                })))
                            .child(Button::new("pdf-zoom-reset").cursor_pointer().ghost().small().label(format!("{zoom:.0}%"))
                                .tooltip("Reset to fit width")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.preview.pdf_zoom = PdfZoom::default();
                                    cx.notify();
                                })))
                            .child(Button::new("pdf-zoom-in").ghost().small().label("+").tooltip("Zoom in")
                                .when(zoom < 300., |button| button.cursor_pointer())
                                .disabled(zoom >= 300.)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.preview.pdf_zoom.adjust(1);
                                    cx.notify();
                                }))),
                    )
                    .child(
                        div().relative().flex_1().min_h_0().w_full().child(
                        div()
                            .id("pdf-pages")
                            .debug_selector(|| "pdf-pages".into())
                            .size_full()
                            .overflow_scroll()
                            .track_scroll(&self.preview.pdf_scroll)
                            .cursor_grab()
                            .when(self.preview.pdf_drag.is_some(), |d| d.cursor_grabbing())
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &gpui_kit::MouseDownEvent, _, cx| {
                                this.preview.pdf_drag = Some(event.position);
                                cx.stop_propagation();
                                cx.notify();
                            }))
                            .bg(theme.muted)
                            .child(
                                v_flex()
                                    .w(gpui_kit::relative(zoom / 100.))
                                    .mx_auto()
                                    .gap(px(14.))
                                    .p(px(16.))
                                    .children(rendered.into_iter().enumerate().map(|(i, page)| {
                                        v_flex()
                                            .w_full()
                                            .flex_none()
                                            .gap(px(4.))
                                            .items_center()
                                            .child({
                                                let size = imagesize::blob_size(page.bytes()).expect("rendered PDF page dimensions");
                                                // A percentage image width otherwise retains its intrinsic
                                                // pixel height, leaving large blank areas between pages.
                                                div()
                                                    .debug_selector(move || format!("pdf-page-{i}"))
                                                    .w_full()
                                                    .aspect_ratio(size.width as f32 / size.height as f32)
                                                    .flex_none()
                                                    .shadow_md()
                                                    .child(img(page).size_full().object_fit(ObjectFit::Contain))
                                            })
                                            .child(div().text_xs().text_color(theme.muted_foreground).child(format!("{} / {pages}", i + 1)))
                                    }))
                                    .when(more, |d| {
                                        d.child(div().text_xs().text_color(theme.muted_foreground).child(if pages > MAX_PDF_PAGES {
                                            format!("Showing the first {MAX_PDF_PAGES} of {pages} pages — open the file to read the rest")
                                        } else {
                                            "Rendering more pages…".to_string()
                                        }))
                                    }),
                            ),
                        ).child(
                            // Capture before the scroll container's built-in bubble handler.
                            gpui_kit::canvas(|_, _, _| (), move |bounds, _, window, _| {
                                // Window-level handlers keep panning/releasing outside the viewer.
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
                                window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                                    if phase == gpui_kit::DispatchPhase::Capture
                                        && bounds.contains(&event.position)
                                        && event.modifiers.control
                                    {
                                        zoom_on_scroll(event, window, cx);
                                    }
                                });
                            }).absolute().inset_0().size_full(),
                        ),
                    )
                    .into_any_element()
            }
            Loaded::Preview(Preview::Directory { entries, total }) => v_flex()
                .id("preview-dir")
                .size_full()
                .p_3()
                .gap_1()
                .overflow_y_scroll()
                .text_sm()
                .child(div().text_xs().text_color(theme.muted_foreground).child(item_count(*total)))
                .children(entries.iter().map(|name| {
                    h_flex()
                        .gap_2()
                        .child(crate::icons::icon(icon_for_path(&path.join(name))).size_4().text_color(theme.muted_foreground))
                        .child(name.clone())
                }))
                .into_any_element(),
            Loaded::Preview(Preview::Unsupported(msg)) => center_text(msg.clone(), cx),
            Loaded::Error(e) => center_text(e.clone(), cx),
            _ => center_text("Empty file", cx),
        }
    }

    pub fn render_quicklook(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let name = self.preview.target.as_ref().map(|p| super::display_name(p)).unwrap_or_default();
        let target = self.preview.target.clone();
        let unavailable = (self.preview.properties_only && target.is_some()) || self.preview.loaded.as_ref().is_some_and(|(path, loaded, _)| {
            Some(path) == target.as_ref()
                && self.preview.loaded_conn == self.preview.target_conn
                && matches!(&**loaded, Loaded::Preview(Preview::Directory { .. } | Preview::Binary { .. } | Preview::Unsupported(_)) | Loaded::Error(_))
        });
        let body = if unavailable {
            let content = if self.preview.target_conn.is_some() {
                match self.remote_properties(cx) {
                    Some((_, rows)) => crate::views::kit::details(rows, cx).into_any_element(),
                    None => center_text("No selection", cx),
                }
            } else {
                let view = self.preview.properties_view.get_or_insert_with(|| {
                    cx.new(|cx| PropertiesView::new(vec![target.clone().expect("loaded preview has a target")], cx))
                });
                view.clone().into_any_element()
            };
            div()
                .id("quicklook-properties")
                .size_full()
                .overflow_hidden()
                .when(self.preview.target_conn.is_some(), |d| d.overflow_y_scroll().p_4())
                .child(content)
                .into_any_element()
        } else if let Some(editor) = self.render_editor(cx) {
            editor
        } else {
            self.render_preview_body(cx)
        };
        let editing = self.preview.editor.is_some();
        let dirty = self.preview.editor.as_ref().is_some_and(|e| e.dirty(cx));
        let editable = !editing
            && self.preview.target_conn.is_none()
            && target.as_deref().is_some_and(super::text_edit::is_editable)
            && self.preview.loaded.as_ref().is_some_and(|(p, l, _)| {
                Some(p) == target.as_ref() && matches!(&**l, Loaded::Preview(Preview::Text { truncated: false, .. }))
            });
        div()
            .id("quicklook")
            .occlude()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.overlay)
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, w, cx| {
                cx.stop_propagation();
                this.close_quicklook_ui(w, cx);
            }))
            .child(
                v_flex()
                    .id("quicklook-card")
                    .debug_selector(|| "quicklook-card".into())
                    .w(gpui_kit::relative(0.6))
                    .h(gpui_kit::relative(0.82))
                    .when(unavailable, |d| d.w(px(540.)).h(px(580.)))
                    .max_w(gpui_kit::relative(0.9))
                    .max_h(gpui_kit::relative(0.82))
                    .rounded_xl()
                    .overflow_hidden()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .shadow_2xl()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .when(unavailable, |d| {
                        d.child(
                            h_flex()
                                .p_4()
                                .gap_2()
                                .flex_none()
                                .items_start()
                                .child(div().flex_1().min_w_0().child(crate::views::kit::header(
                                    IconName::Info,
                                    "Properties".into(),
                                    Some(self.preview.properties_subtitle.clone().unwrap_or_else(|| name.clone().into())),
                                    cx,
                                )))
                                .child(
                                    Button::new("ql-properties-close").cursor_pointer()
                                        .ghost()
                                        .small()
                                        .icon(crate::icons::icon(IconName::X))
                                        .on_click(cx.listener(|this, _, w, cx| this.close_quicklook_ui(w, cx))),
                                ),
                        )
                    })
                    .when(!unavailable, |d| d.child(
                        h_flex()
                            .h(px(36.))
                            .px_3()
                            .gap_2()
                            .flex_none()
                            .border_b_1()
                            .border_color(theme.border)
                            .child(div().flex_1().truncate().text_sm().child(if dirty { SharedString::from(format!("{name} — Edited")) } else { name.clone().into() }))
                            .when(editing && dirty, |d| {
                                d.child(
                                    Button::new("ql-cancel").cursor_pointer()
                                        .small()
                                        .ghost()
                                        .label("Discard")
                                        .on_click(cx.listener(|this, _, w, cx| this.leave_editor(false, w, cx))),
                                )
                            })
                            .when(editable || editing, |d| d.child(mode_switch(editing, cx)))
                            .child(
                                Button::new("ql-close").cursor_pointer()
                                    .ghost()
                                    .small()
                                    .icon(crate::icons::icon(IconName::X))
                                    .on_click(cx.listener(|this, _, w, cx| this.close_quicklook_ui(w, cx))),
                            ),
                    ))
                    .child(div().px_3().py_1().text_xs().text_color(theme.muted_foreground).child(if editing { "⌘S save · Esc done" } else { "↑↓ browse · Space/Esc close" }))
                    .child(div().flex_1().min_h_0().child(body)),
            )
            .into_any_element()
    }

    fn remote_properties(&self, cx: &Context<Self>) -> Option<(SharedString, Vec<(&'static str, String)>)> {
        let conn = self.pane().read(cx).conn()?;
        let entry = self.pane().read(cx).cursor_entry().cloned()?;
        let server = crate::remote::info(&conn).map(|i| format!("{} ({})", i.display(), i.protocol.label())).unwrap_or_default();
        let size = if entry.is_dir { "—".to_string() } else { humansize::format_size(entry.size, humansize::DECIMAL) };
        let modified = entry
            .modified
            .map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "—".into());
        let rows = vec![
            ("Name", entry.name.to_string()),
            ("Path", entry.path.display().to_string()),
            ("Kind", entry.kind_label()),
            ("Size", size),
            ("Modified", modified),
            ("Server", server),
        ];
        Some((entry.name.to_string().into(), rows))
    }

    pub(crate) fn show_properties(&mut self, _: &ShowProperties, window: &mut Window, cx: &mut Context<Self>) {
        let mut paths = self.selection(cx);
        if paths.is_empty() {
            paths.extend(self.current_dir(cx));
        }
        self.open_properties(paths, window, cx);
    }

    fn open_properties(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let subtitle: SharedString = match paths.as_slice() {
            [one] => super::display_name(one).into(),
            many => item_count(many.len()).into(),
        };
        self.preview.properties_only = true;
        self.set_previewing(true, cx);
        if self.preview.target.is_none() {
            self.preview.target = paths.first().cloned();
        }
        self.preview.properties_subtitle = Some(subtitle);
        if self.preview.target_conn.is_none() {
            self.preview.properties_view = Some(cx.new(|cx| PropertiesView::new(paths, cx)));
        }
        self.pane().read(cx).focus_handle.clone().focus(window, cx);
        cx.notify();
    }
}

/// Preview / edit segmented switch (styled like the list's view-mode selector).
fn mode_switch(editing: bool, cx: &mut Context<Workspace>) -> AnyElement {
    let ch = crate::theme::chrome(cx);
    let seg = |id: &'static str, icon: IconName, active: bool, tip: &'static str| {
        div()
            .id(id)
            .w(px(28.))
            .h(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .cursor_pointer()
            .when(active, |d| d.bg(ch.segmented_active).shadow_xs())
            .when(!active, |d| d.hover(|s| s.bg(ch.hover_strong)))
            .child(crate::icons::icon(icon).size(px(14.)).text_color(if active { ch.text_strong } else { ch.icon_button }))
            .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx))
    };
    h_flex()
        .flex_none()
        .gap(px(2.))
        .p(px(2.))
        .rounded(px(7.))
        .bg(ch.segmented_bg)
        .border_1()
        .border_color(ch.hairline)
        .child(seg("ql-mode-preview", IconName::Eye, !editing, "Preview").on_click(cx.listener(move |this, _, w, cx| {
            if this.preview.editor.is_some() {
                this.leave_editor(true, w, cx);
            }
        })))
        .child(seg("ql-mode-edit", IconName::Pencil, editing, "Edit").on_click(cx.listener(move |this, _, w, cx| {
            if this.preview.editor.is_none() {
                this.start_editing(w, cx);
            }
        })))
        .into_any_element()
}

fn center_text(text: impl Into<SharedString>, cx: &mut Context<Workspace>) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p_4()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
        .into_any_element()
}

/// Properties dialog content: basic metadata immediately, recursive folder
/// totals streamed in from a background walk.
pub struct PropertiesView {
    paths: Vec<PathBuf>,
    props: Option<Result<Properties, String>>,
    totals: Option<(u64, u64, u64)>,
    totals_done: bool,
    /// Uncompressed content size when the single selected item is an archive.
    archive_uncompressed: Option<u64>,
    scroll: gpui_kit::ScrollHandle,
    has_more_below: bool,
    cancel: Arc<AtomicBool>,
    _tasks: Vec<Task<()>>,
}

impl Drop for PropertiesView {
    fn drop(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl PropertiesView {
    fn new(paths: Vec<PathBuf>, cx: &mut Context<Self>) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut tasks = Vec::new();
        let first = paths[0].clone();
        let basic = cx.background_spawn(async move { properties::basic(&first).map_err(|e| format!("{e:#}")) });
        tasks.push(cx.spawn(async move |this, cx| {
            let props = basic.await;
            this.update(cx, |this, cx| {
                this.props = Some(props);
                cx.notify();
            })
            .ok();
        }));

        let needs_walk = paths.len() > 1 || paths[0].is_dir();
        if needs_walk {
            let (tx, rx) = smol::channel::unbounded::<(u64, u64, u64, bool)>();
            let walk_paths = paths.clone();
            let walk_cancel = cancel.clone();
            cx.background_spawn(async move {
                let mut acc = (0u64, 0u64, 0u64);
                for p in &walk_paths {
                    let base = acc;
                    let result = if p.is_dir() {
                        properties::dir_totals(p, &walk_cancel, |b, f, d| {
                            tx.try_send((base.0 + b, base.1 + f, base.2 + d, false)).ok();
                        })
                    } else {
                        std::fs::symlink_metadata(p).map(|m| (m.len(), 1, 0)).map_err(Into::into)
                    };
                    if let Ok((b, f, d)) = result {
                        acc = (base.0 + b, base.1 + f, base.2 + d);
                    }
                }
                tx.try_send((acc.0, acc.1, acc.2, true)).ok();
            })
            .detach();
            tasks.push(cx.spawn(async move |this, cx| {
                while let Ok(mut msg) = rx.recv().await {
                    while let Ok(m) = rx.try_recv() {
                        msg = m;
                    }
                    let done = msg.3;
                    let alive = this.update(cx, |this, cx| {
                        this.totals = Some((msg.0, msg.1, msg.2));
                        this.totals_done = done;
                        cx.notify();
                    });
                    if alive.is_err() || done {
                        break;
                    }
                    cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                }
            }));
        } else {
            // A single file: its size is the total.
            let p = paths[0].clone();
            let size = std::fs::symlink_metadata(&p).map(|m| m.len()).unwrap_or(0);
            if rile_services::archive::detect(&p).is_some() {
                let arc_cancel = cancel.clone();
                let arc_path = p.clone();
                let job = cx.background_spawn(async move {
                    rile_services::archive::uncompressed_size(&arc_path, &arc_cancel).ok().flatten()
                });
                tasks.push(cx.spawn(async move |this, cx| {
                    if let Some(n) = job.await {
                        this.update(cx, |this, cx| {
                            this.archive_uncompressed = Some(n);
                            cx.notify();
                        })
                        .ok();
                    }
                }));
            }
            return Self {
                paths, props: None, totals: Some((size, 1, 0)), totals_done: true, archive_uncompressed: None,
                scroll: gpui_kit::ScrollHandle::new(), has_more_below: false, cancel, _tasks: tasks,
            };
        }
        Self {
            paths, props: None, totals: None, totals_done: false, archive_uncompressed: None,
            scroll: gpui_kit::ScrollHandle::new(), has_more_below: false, cancel, _tasks: tasks,
        }
    }
}

impl Render for PropertiesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::views::kit;
        let ch = crate::theme::chrome(cx);
        let fmt_time = |t: Option<std::time::SystemTime>| {
            t.map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%b %-d, %Y at %H:%M").to_string()).unwrap_or_else(|| "—".into())
        };
        let size_text = match self.totals {
            Some((b, _, _)) if !self.totals_done => {
                if b >= 1_000_000_000 {
                    format!("{:.2} GB", b as f64 / 1_000_000_000.)
                } else {
                    format!("{:.2} MB", b as f64 / 1_000_000.)
                }
            }
            Some((b, _, _)) => humansize::format_size(b, humansize::DECIMAL),
            None => "0.00 MB".into(),
        };
        let size_value = h_flex()
            .flex_wrap()
            .gap_2()
            .when(!self.totals_done, |d| d.child(Spinner::new().xsmall()))
            .child(
                div()
                    .id("properties-size")
                    .min_w_0()
                    .truncate()
                    .child(size_text)
                    .when_some(self.totals.filter(|_| self.totals_done), |d, (bytes, _, _)| {
                        let tip = format!("{bytes} bytes");
                        d.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                    }),
            )
            .when_some(
                self.totals.filter(|_| self.totals_done && (self.paths.len() > 1 || self.paths[0].is_dir())),
                |d, (_, files, folders)| d.child(div().min_w_0().child(format!("· {files} files, {folders} folders"))),
            )
            .when_some(
                self.archive_uncompressed.zip(self.totals).filter(|(u, _)| *u > 0),
                |d, (unc, (packed, _, _))| {
                    let pct = packed as f64 / unc as f64 * 100.;
                    let tip = format!("{} uncompressed", humansize::format_size(unc, humansize::DECIMAL));
                    d.child(
                        div()
                            .id("properties-ratio")
                            .min_w_0()
                            .child(format!("· {pct:.0}% of original"))
                            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)),
                    )
                },
            )
            .into_any_element();

        let col = v_flex().size_full().min_h_0().gap(px(14.));
        if self.paths.len() > 1 {
            return col.p_4().child(kit::details_with_values(
                vec![
                    ("Items", div().child(item_count(self.paths.len())).into_any_element()),
                    ("Location", div().child(self.paths[0].parent().map(|p| p.display().to_string()).unwrap_or_default()).into_any_element()),
                    ("Total size", size_value),
                ],
                cx,
            ));
        }
        match &self.props {
            None => col.p_4().child(kit::hint("Loading…", cx)),
            Some(Err(e)) => col.p_4().child(kit::notice(e.clone(), true, cx)),
            Some(Ok(p)) => {
                let entry = rile_core::Entry::from_path(p.path.clone());
                let art = match &entry {
                    Some(e) => crate::art::file_object(e, 56., true, true, cx),
                    None => crate::icons::icon(icon_for_path(&p.path)).size(px(40.)).text_color(ch.accent).into_any_element(),
                };
                let mut general = vec![
                    ("Location", p.path.parent().map(|x| x.display().to_string()).unwrap_or_default()),
                ];
                if let Some(a) = p.allocated {
                    general.push(("On disk", humansize::format_size(a, humansize::DECIMAL)));
                }
                if let Some(t) = &p.symlink_target {
                    general.push(("Link target", t.display().to_string()));
                }
                if let Some(m) = &p.mime {
                    general.push(("Type", m.clone()));
                }
                let mut access = Vec::new();
                if let Some(perm) = &p.permissions {
                    access.push(("Permissions", perm.clone()));
                }
                if p.owner.is_some() || p.group.is_some() {
                    access.push(("Owner", format!("{} : {}", p.owner.clone().unwrap_or_default(), p.group.clone().unwrap_or_default())));
                }
                access.push(("Read-only", if p.readonly { "Yes".into() } else { "No".into() }));
                let details = v_flex()
                    .id("properties-body")
                    .debug_selector(|| "properties-body".into())
                    .gap(px(14.))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p_4()
                    .child(
                    h_flex()
                        .gap(px(14.))
                        .child(div().size(px(60.)).flex_none().flex().items_center().justify_center().child(art))
                        .child(
                            v_flex()
                                .min_w_0()
                                .gap(px(4.))
                                .child(div().text_size(px(14.)).font_weight(gpui_kit::FontWeight::SEMIBOLD).text_color(ch.text_strong).child(p.name.clone()))
                                .child(div().text_size(px(12.)).text_color(ch.text_muted).child(p.kind_label.clone())),
                        ),
                )
                .child(v_flex().gap(px(8.)).child(kit::caption("GENERAL", cx)).child(kit::details_with_values({
                    let mut rows: Vec<_> = general.drain(..).map(|(k, v)| (k, div().child(v).into_any_element())).collect();
                    rows.insert(1, ("Size", size_value));
                    rows
                }, cx)))
                .child(
                    v_flex().gap(px(8.)).child(kit::caption("DATES", cx)).child(kit::details(
                        vec![("Created", fmt_time(p.created)), ("Modified", fmt_time(p.modified)), ("Accessed", fmt_time(p.accessed))],
                        cx,
                    )),
                )
                .child(v_flex().gap(px(8.)).child(kit::caption("ACCESS", cx)).child(kit::details(access, cx)));
                let view = cx.entity().downgrade();
                let scroll = self.scroll.clone();
                let has_more_below = self.has_more_below;
                col.gap_0().child(details).child(
                    kit::footer()
                        .debug_selector(|| "properties-footer".into())
                        .flex_none()
                        .relative()
                        .p_4()
                        .bg(cx.theme().popover)
                        .when(has_more_below, |d| {
                            d.shadow(vec![gpui_kit::BoxShadow {
                                color: gpui_kit::black().opacity(if ch.dark { 0.3 } else { 0.12 }),
                                offset: gpui_kit::point(px(0.), px(-4.)),
                                blur_radius: px(12.),
                                spread_radius: px(-4.),
                                inset: false,
                            }])
                        })
                        // Read the final scroll geometry after layout, including on resize.
                        .child(div()
                            .debug_selector(move || {
                                if has_more_below { "properties-footer-shadow" } else { "properties-footer-no-shadow" }.into()
                            })
                            .absolute()
                            .inset_0()
                            .child(gpui_kit::canvas(
                                move |_, _, cx| {
                                    let more = scroll.max_offset().y + scroll.offset().y > px(1.);
                                    if more != has_more_below {
                                        cx.defer(move |cx| {
                                            let _ = view.update(cx, |this, cx| {
                                                this.has_more_below = more;
                                                cx.notify();
                                            });
                                        });
                                    }
                                },
                                |_, _, _, _| {},
                            )
                            .size_full()))
                        .child(Button::new("prop-copy").cursor_pointer().outline().label("Copy Path").on_click({
                            let path = p.path.clone();
                            move |_, _, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(path.display().to_string()))
                        })),
                )
            }
        }
    }
}

#[cfg(test)]
mod properties_tests {
    use super::*;
    use gpui_kit::{TestAppContext, VisualTestContext, WindowOptions, size};

    #[gpui_kit::test]
    async fn properties_footer_shadow_disappears_when_content_fits(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("file.txt");
        std::fs::write(&path, "fixture").unwrap();
        let (window, _) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|cx| {
                    let mut view = PropertiesView::new(vec![path.clone()], cx);
                    view.props = Some(properties::basic(&path).map_err(|e| e.to_string()));
                    view
                })
            }).unwrap()
        });
        let mut vcx = VisualTestContext::from_window(window, cx);
        for (height, indicator) in [
            (1000., "properties-footer-no-shadow"),
            (300., "properties-footer-shadow"),
            (1000., "properties-footer-no-shadow"),
        ] {
            vcx.simulate_resize(size(px(540.), px(height)));
            vcx.run_until_parked();
            assert!(vcx.debug_bounds(indicator).is_some(), "wrong shadow at height {height}");
        }
    }
}

/// Preview for a file on a server: images are downloaded to a temp file,
/// everything else is sniffed from the first bytes.
fn remote_preview(conn: &str, path: &std::path::Path) -> Loaded {
    let run = || -> anyhow::Result<Loaded> {
        let remote_path = crate::remote::path_str(path);
        let session = crate::remote::session(conn)?;
        let mut fs = session.lock().map_err(|_| anyhow::anyhow!("connection busy"))?;
        let entry = fs.stat(&remote_path)?;
        if entry.is_dir {
            let mut names: Vec<String> = fs.list(&remote_path)?.into_iter().map(|e| e.name).collect();
            names.sort_by(|a, b| rile_core::fs_entry::natural_cmp(&a.to_lowercase(), &b.to_lowercase()));
            let total = names.len();
            names.truncate(200);
            return Ok(Loaded::Preview(Preview::Directory { entries: names, total }));
        }
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if preview::IMAGE_EXTENSIONS.contains(&ext.as_str()) && entry.size < 30 * 1024 * 1024 {
            let local = crate::remote::temp_path(conn, &remote_path);
            if let Some(dir) = local.parent() {
                std::fs::create_dir_all(dir)?;
            }
            fs.download(&remote_path, &local, &std::sync::atomic::AtomicBool::new(false), &mut |_, _| {})?;
            return Ok(Loaded::Preview(Preview::Image(local)));
        }
        let head = fs.read_head(&remote_path, MAX_TEXT)?;
        let sniff = &head[..head.len().min(8192)];
        Ok(Loaded::Preview(if sniff.contains(&0) {
            Preview::Binary { size: entry.size, hex: preview::hexdump(&head[..head.len().min(512)]) }
        } else {
            Preview::Text {
                text: String::from_utf8_lossy(&head).into_owned(),
                truncated: (head.len() as u64) < entry.size,
                language: Some(ext),
            }
        }))
    };
    run().unwrap_or_else(|e| Loaded::Error(format!("{e:#}")))
}
