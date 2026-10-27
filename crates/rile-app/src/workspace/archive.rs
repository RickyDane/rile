//! Compressing to and extracting from archives (zip, 7z, tar and the
//! gzip / brotli / zstd streams), with a chosen compression level.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    WindowExt, h_flex,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
    notification::Notification,
    slider::{Slider, SliderEvent, SliderState},
    v_flex,
};
use gpui_kit::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription,
    WeakEntity, Window,
    div, px,
};
use rile_core::ops::{Progress, unique_destination};
use rile_services::archive::{self, ArchiveFormat, ArchiveProgress};

use super::{Workspace, display_name, item_count};
use crate::actions::*;

/// Archive progress in the shape the jobs panel and transfer window use.
fn to_progress(p: &ArchiveProgress) -> Progress {
    Progress {
        bytes_done: p.bytes_done,
        bytes_total: p.bytes_total,
        files_done: p.items_done,
        files_total: p.items_total,
        current: (!p.current.as_os_str().is_empty()).then(|| p.current.to_string_lossy().into()),
        file_bytes_done: p.file_bytes_done,
        file_bytes_total: p.file_bytes_total,
        ..Default::default()
    }
}

impl Workspace {
    pub(crate) fn compress(&mut self, _: &Compress, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane().read(cx).conn().is_some() {
            window.push_notification(Notification::info("Compressing isn't available on servers"), cx);
            return;
        }
        let paths = self.selection(cx);
        let Some(dir) = paths.first().and_then(|p| p.parent()).map(PathBuf::from) else { return };
        let ws = cx.entity().downgrade();
        let view = cx.new(|cx| CompressForm::new(ws, paths.clone(), dir, window, cx));
        let subtitle: SharedString = match paths.as_slice() {
            [one] => display_name(one).into(),
            many => item_count(many.len()).into(),
        };
        window.open_dialog(cx, move |dialog, _, cx| {
            crate::views::kit::dialog(dialog, IconName::Archive, "Compress", Some(subtitle.clone()), 520., cx).child(view.clone())
        });
    }

    pub(crate) fn extract(&mut self, _: &Extract, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane().read(cx).conn().is_some() {
            window.push_notification(Notification::info("Extracting isn't available on servers"), cx);
            return;
        }
        let archives: Vec<PathBuf> = self.selection(cx).into_iter().filter(|p| p.is_file() && archive::detect(p).is_some()).collect();
        if archives.is_empty() {
            window.push_notification(Notification::info("Select a zip, 7z, tar, gz, br or zst archive to extract"), cx);
            return;
        }
        for path in archives {
            let Some(dest) = path.parent().map(PathBuf::from) else { continue };
            let label = format!("Extracting {}", display_name(&path));
            self.start_op(label, window, cx, move |cancel, report| {
                let root = archive::extract(&path, &dest, cancel, |p| report(to_progress(p)))?;
                rile_services::rewards::record_event(rile_services::rewards::Event::Archive);
                Ok(vec![root])
            });
        }
    }

    pub(crate) fn start_compress(
        &mut self,
        inputs: Vec<PathBuf>,
        dest: PathBuf,
        format: ArchiveFormat,
        level: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = format!("Compressing {} to {}", item_count(inputs.len()), display_name(&dest));
        self.start_op(label, window, cx, move |cancel, report| {
            archive::compress(&inputs, &dest, format, level, cancel, |p| report(to_progress(p)))?;
            rile_services::rewards::record_event(rile_services::rewards::Event::Archive);
            Ok(vec![dest])
        });
    }
}

/// Whether `path` is an archive Rile can extract (for menus).
pub fn is_archive(path: &std::path::Path) -> bool {
    archive::detect(path).is_some()
}

/// Format, level and name of a new archive.
pub struct CompressForm {
    ws: WeakEntity<Workspace>,
    inputs: Arc<Vec<PathBuf>>,
    dir: PathBuf,
    format: ArchiveFormat,
    name: Entity<InputState>,
    level: Entity<SliderState>,
    level_value: u32,
    error: Option<SharedString>,
    _subs: Vec<Subscription>,
}

impl CompressForm {
    fn new(ws: WeakEntity<Workspace>, inputs: Vec<PathBuf>, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let format = ArchiveFormat::Zip;
        let name = cx.new(|cx| InputState::new(window, cx).default_value(archive::suggested_name(&inputs, format)));
        let level = cx.new(|_| Self::slider(format));
        let subs = vec![cx.subscribe_in(&level, window, |this: &mut Self, _, ev: &SliderEvent, _, cx| {
            let (SliderEvent::Change(v) | SliderEvent::Release(v)) = ev;
            this.level_value = v.start().round() as u32;
            cx.notify();
        })];
        Self {
            ws,
            inputs: Arc::new(inputs),
            dir,
            format,
            name,
            level,
            level_value: format.default_level(),
            error: None,
            _subs: subs,
        }
    }

    fn slider(format: ArchiveFormat) -> SliderState {
        let range = format.level_range().unwrap_or(0..=0);
        SliderState::new()
            .min(*range.start() as f32)
            .max((*range.end()).max(*range.start() + 1) as f32)
            .step(1.)
            .default_value(format.default_level() as f32)
    }

    /// Formats offered: single-stream ones only for one file.
    fn formats(&self) -> Vec<ArchiveFormat> {
        let single_file = matches!(self.inputs.as_slice(), [one] if one.is_file());
        ArchiveFormat::ALL.into_iter().filter(|f| f.supports_multiple() || single_file).collect()
    }

    fn set_format(&mut self, format: ArchiveFormat, window: &mut Window, cx: &mut Context<Self>) {
        let old = self.format;
        self.format = format;
        self.level_value = format.default_level();
        self.level = cx.new(|_| Self::slider(format));
        self._subs = vec![cx.subscribe_in(&self.level, window, |this: &mut Self, _, ev: &SliderEvent, _, cx| {
            let (SliderEvent::Change(v) | SliderEvent::Release(v)) = ev;
            this.level_value = v.start().round() as u32;
            cx.notify();
        })];
        // Keep a custom name, swapping only the extension.
        let current = self.name.read(cx).value().to_string();
        let stem = current.strip_suffix(&format!(".{}", old.extension())).unwrap_or(&current).to_string();
        self.name.update(cx, |i, cx| i.set_value(format!("{stem}.{}", format.extension()), window, cx));
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_string();
        if name.is_empty() || name.contains('/') {
            self.error = Some("Enter a file name".into());
            cx.notify();
            return;
        }
        let dest = unique_destination(&self.dir, &name);
        let (inputs, format, level) = ((*self.inputs).clone(), self.format, self.level_value);
        window.close_dialog(cx);
        self.ws.update(cx, |ws, cx| ws.start_compress(inputs, dest, format, level, window, cx)).ok();
    }
}

impl Render for CompressForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::views::kit;
        let ch = crate::theme::chrome(cx);
        let formats = self.formats();
        let chips = h_flex().flex_wrap().gap(px(6.)).children(formats.into_iter().map(|f| {
            let active = f == self.format;
            kit::segment(("fmt", f as usize), f.label(), active, cx)
                .border_1()
                .border_color(if active { ch.accent.opacity(0.5) } else { ch.hairline })
                .on_click(cx.listener(move |this, _, window, cx| this.set_format(f, window, cx)))
        }));
        let level = match self.format.level_range() {
            Some(range) => {
                let pos = if range.end() > range.start() {
                    (self.level_value - range.start()) as f32 / (range.end() - range.start()) as f32
                } else {
                    0.
                };
                let speed = match pos {
                    p if p < 0.2 => "Fastest",
                    p if p < 0.45 => "Fast",
                    p if p < 0.7 => "Balanced",
                    p if p < 0.9 => "Small",
                    _ => "Smallest (slowest)",
                };
                v_flex()
                    .gap(px(8.))
                    .child(
                        h_flex()
                            .justify_between()
                            .child(kit::caption("COMPRESSION LEVEL", cx))
                            .child(div().text_size(px(12.)).text_color(ch.text_muted).child(format!("{} · {speed}", self.level_value))),
                    )
                    .child(Slider::new(&self.level))
                    .child(
                        h_flex()
                            .justify_between()
                            .text_size(px(10.))
                            .text_color(ch.text_faint)
                            .child(format!("{} · fastest", range.start()))
                            .child(format!("best compression · {}", range.end())),
                    )
                    .into_any_element()
            }
            None => kit::hint("Plain tar bundles files without compressing them.", cx).into_any_element(),
        };
        v_flex()
            .gap(px(16.))
            .child(v_flex().gap(px(8.)).child(kit::caption("FORMAT", cx)).child(chips))
            .child(level)
            .child(v_flex().gap(px(8.)).child(kit::caption("SAVE AS", cx)).child(Input::new(&self.name)))
            .children(self.error.clone().map(|e| kit::notice(e, true, cx)))
            .child(
                kit::footer()
                    .child(Button::new("cmp-cancel").cursor_pointer().ghost().label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(
                        Button::new("cmp-go")
                            .cursor_pointer()
                            .primary()
                            .label("Compress")
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}
