//! Drive speed test popup (Overview → storage card → gauge button): runs
//! [`rile_services::speedtest`] on a background thread and shows a live
//! gauge plus per-phase throughput, IOPS and latency.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, WindowExt, h_flex, v_flex};
use gpui_kit::{
    AnyElement, InteractiveElement, StatefulInteractiveElement, AppContext, Bounds, Context, FontWeight, Hsla, IntoElement, ParentElement, PathBuilder, Pixels, Render,
    SharedString, Styled, Task, Window, canvas, div, point, prelude::FluentBuilder, px, relative,
};
use rile_core::places::Volume;
use rile_services::speedtest::{self, Config, Measure, Phase, Progress};

use super::Workspace;
use crate::theme::chrome;
use crate::views::kit;

/// Test file sizes offered in the footer.
const SIZES: [(&str, u64); 4] = [("256 MB", 256 << 20), ("1 GB", 1 << 30), ("4 GB", 4 << 30), ("16 GB", 16 << 30)];

enum Msg {
    Progress(Progress),
    Done(Phase, Measure),
    Finished(Result<speedtest::Report, String>),
}

#[derive(Clone, PartialEq)]
enum Status {
    Idle,
    Running,
    Done,
    Failed(SharedString),
}

pub struct SpeedTestView {
    volume: Volume,
    dir: PathBuf,
    status: Status,
    live: Option<Progress>,
    results: [Option<Measure>; 4],
    file_size: u64,
    /// Chosen test file size in bytes.
    size_choice: u64,
    uncached: bool,
    cancel: Arc<AtomicBool>,
    scroll: gpui_kit::ScrollHandle,
    has_more_below: bool,
    _task: Option<Task<()>>,
}

impl SpeedTestView {
    fn new(volume: Volume) -> Self {
        let dir = speedtest::test_dir(&volume.mount_point);
        Self {
            volume,
            dir,
            status: Status::Idle,
            live: None,
            results: [None; 4],
            file_size: 0,
            size_choice: 1 << 30,
            uncached: false,
            cancel: Arc::new(AtomicBool::new(false)),
            scroll: gpui_kit::ScrollHandle::new(),
            has_more_below: false,
            _task: None,
        }
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        self.status = Status::Running;
        self.live = None;
        self.results = [None; 4];
        let (tx, rx) = std::sync::mpsc::channel();
        let dir = self.dir.clone();
        let config = Config { file_size: self.size_choice, ..Config::default() };
        std::thread::spawn(move || {
            let p = tx.clone();
            let d = tx.clone();
            let res = speedtest::run(
                &dir,
                &config,
                &cancel,
                |prog| {
                    p.send(Msg::Progress(prog)).ok();
                },
                |phase, m| {
                    d.send(Msg::Done(phase, m)).ok();
                },
            );
            tx.send(Msg::Finished(res.map_err(|e| format!("{e:#}")))).ok();
        });
        self._task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(60)).await;
                let msgs: Vec<Msg> = rx.try_iter().collect();
                let mut finished = false;
                let alive = this
                    .update(cx, |this, cx| {
                        for msg in msgs {
                            match msg {
                                Msg::Progress(p) => this.live = Some(p),
                                Msg::Done(phase, m) => this.results[phase.index()] = Some(m),
                                Msg::Finished(Ok(report)) => {
                                    this.file_size = report.file_size;
                                    this.uncached = report.uncached;
                                    this.live = None;
                                    this.status = Status::Done;
                                    finished = true;
                                }
                                Msg::Finished(Err(e)) => {
                                    this.live = None;
                                    this.status = if e == "Cancelled" { Status::Idle } else { Status::Failed(e.into()) };
                                    finished = true;
                                }
                            }
                        }
                        cx.notify();
                    })
                    .is_ok();
                if finished || !alive {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        cx.notify();
    }

    /// Value the big gauge shows: live throughput, or the sequential read
    /// result once done.
    fn gauge_value(&self) -> (f64, &'static str) {
        if let Some(p) = self.live {
            return (p.mb_per_s, p.phase.label());
        }
        match self.results[Phase::SeqRead.index()] {
            Some(m) => (m.mb_per_s(), "Sequential read"),
            None => (0., "Ready"),
        }
    }
}


impl Drop for SpeedTestView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Maps MB/s to 0..=1 on a log scale from 1 MB/s to 10 GB/s.
fn gauge_fraction(mb_s: f64) -> f32 {
    if mb_s <= 1. { 0. } else { ((mb_s.log10()) / 4.).clamp(0., 1.) as f32 }
}

fn fmt_speed(mb_s: f64) -> String {
    if mb_s >= 1000. { format!("{:.2} GB/s", mb_s / 1000.) } else if mb_s >= 10. { format!("{mb_s:.0} MB/s") } else { format!("{mb_s:.1} MB/s") }
}

fn fmt_lat(d: Duration) -> String {
    let us = d.as_secs_f64() * 1e6;
    if us >= 1000. { format!("{:.2} ms", us / 1000.) } else { format!("{us:.0} µs") }
}

fn gauge(fraction: f32, track: Hsla, color: Hsla) -> AnyElement {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let w = f32::from(bounds.size.width);
            let h = f32::from(bounds.size.height);
            let r = (w / 2.).min(h) - 10.;
            let cx = f32::from(bounds.origin.x) + w / 2.;
            let cy = f32::from(bounds.origin.y) + h - 4.;
            let arc = |from: f32, to: f32, width: f32, color: Hsla, window: &mut Window| {
                if to <= from {
                    return;
                }
                let mut b = PathBuilder::stroke(px(width));
                let steps = 64;
                for i in 0..=steps {
                    let t = from + (to - from) * i as f32 / steps as f32;
                    let a = std::f32::consts::PI * (1. - t);
                    let p = point(px(cx + r * a.cos()), px(cy - r * a.sin()));
                    if i == 0 { b.move_to(p) } else { b.line_to(p) }
                }
                if let Ok(p) = b.build() {
                    window.paint_path(p, color);
                }
            };
            arc(0., 1., 12., track, window);
            arc(0., fraction.max(0.005), 12., color, window);
        },
    )
    .w_full()
    .h(px(120.))
    .into_any_element()
}

impl Render for SpeedTestView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ch = chrome(cx);
        let running = self.status == Status::Running;
        let (value, caption) = self.gauge_value();
        let fmt = |b: u64| humansize::format_size(b, humansize::DECIMAL);

        let readout = v_flex()
            .items_center()
            .child(gauge(gauge_fraction(value), ch.hover_strong, ch.accent))
            .child(
                div()
                    .mt(px(-34.))
                    .text_size(px(26.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ch.text)
                    .child(fmt_speed(value)),
            )
            .child(div().text_size(px(11.)).text_color(ch.text_faint).child(caption))
            .child(
                h_flex()
                    .w(px(248.))
                    .justify_between()
                    .text_size(px(10.))
                    .text_color(ch.text_faint)
                    .child("1 MB/s")
                    .child("10 GB/s"),
            );

        let rows = Phase::ALL.iter().map(|&phase| {
            let result = self.results[phase.index()];
            let live = self.live.filter(|p| p.phase == phase);
            let (speed, frac) = match (result, live) {
                (Some(m), _) => (fmt_speed(m.mb_per_s()), 1.),
                (None, Some(p)) => (fmt_speed(p.mb_per_s), p.fraction),
                _ => ("—".to_string(), 0.),
            };
            let detail = match result {
                Some(m) => format!(
                    "{:.0} IOPS · latency {} avg, {} min, {} max",
                    m.iops(),
                    fmt_lat(m.lat_avg),
                    fmt_lat(m.lat_min),
                    fmt_lat(m.lat_max)
                ),
                None if live.is_some() => "Running…".into(),
                None => "Waiting".into(),
            };
            v_flex()
                .gap(px(5.))
                .child(
                    h_flex()
                        .justify_between()
                        .text_size(px(12.))
                        .child(div().text_color(ch.text).child(phase.label()))
                        .child(div().font_weight(FontWeight::MEDIUM).text_color(ch.text).child(speed)),
                )
                .child(
                    div()
                        .h(px(5.))
                        .rounded(px(3.))
                        .bg(ch.hover_strong)
                        .overflow_hidden()
                        .child(div().h_full().w(relative(frac)).rounded(px(3.)).bg(if result.is_some() { ch.accent } else { ch.accent.opacity(0.6) })),
                )
                .child(div().text_size(px(10.)).text_color(ch.text_faint).child(detail))
        });

        let mut details = vec![
            ("Volume", format!("{} ({})", self.volume.name, self.volume.mount_point.display())),
            ("Test location", self.dir.display().to_string()),
            ("Capacity", format!("{} · {} free", fmt(self.volume.total), fmt(self.volume.available))),
            ("Type", if self.volume.removable { "External · removable".into() } else { "Internal".into() }),
        ];
        if self.status == Status::Done {
            details.push(("Test file", format!("{} · 8 MB blocks, 4 KB random", fmt(self.file_size))));
            details.push(("Cache", if self.uncached { "Bypassed (direct I/O)".into() } else { "OS cache may affect reads".into() }));
        }

        let body = v_flex()
            .id("speedtest-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .pb(px(14.))
            .gap(px(14.))
            .child(readout)
            .child(kit::card(cx).p(px(14.)).child(v_flex().gap(px(12.)).children(rows)))
            .child(kit::details(details, cx))
            .when_some(
                match &self.status {
                    Status::Failed(e) => Some(e.clone()),
                    _ => None,
                },
                |d, e| d.child(kit::notice(e, true, cx)),
            )
            .child(kit::hint(format!("Writes a temporary {} file to the drive (less if space is short) and deletes it afterwards.", SIZES.iter().find(|(_, b)| *b == self.size_choice).map_or("1 GB", |(l, _)| *l)), cx));

        // Room left for the body: the dialog keeps a margin around itself and
        // spends ~130px on its header and paddings.
        let margin = cx.theme().spacing_tokens().lg;
        let max_h = (window.viewport_size().height - margin * 2. - px(130.)).max(px(160.));
        let view = cx.entity().downgrade();
        let scroll = self.scroll.clone();
        let has_more_below = self.has_more_below;
        let footer = kit::footer()
            .debug_selector(|| "speedtest-footer".into())
            .flex_none()
            .relative()
            .mx(px(-16.))
            .p_4()
            .rounded_b(px(14.))
            .bg(ch.surface_colors().1)
            .when(has_more_below, |d| {
                d.shadow(vec![gpui_kit::BoxShadow {
                    color: gpui_kit::black().opacity(if ch.dark { 0.3 } else { 0.12 }),
                    offset: point(px(0.), px(-4.)),
                    blur_radius: px(12.),
                    spread_radius: px(-4.),
                    inset: false,
                }])
            })
            // Read the final scroll geometry after layout, including on resize.
            .child(
                div()
                    .debug_selector(move || if has_more_below { "speedtest-footer-shadow" } else { "speedtest-footer-no-shadow" }.into())
                    .absolute()
                    .inset_0()
                    .child(
                        canvas(
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
                        .size_full(),
                    ),
            )
            .justify_between()
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(div().text_size(px(11.)).text_color(ch.text_faint).child("Test file"))
                    .child(kit::segmented(cx).when(running, |d| d.opacity(0.5)).children(SIZES.iter().map(|&(label, bytes)| {
                        kit::segment(("speedtest-size", bytes as usize), label, self.size_choice == bytes, cx)
                            .when(running, |d| d.cursor_default())
                            .when(!running, |d| {
                                d.on_click(cx.listener(move |this, _, _, cx| {
                                    this.size_choice = bytes;
                                    cx.notify();
                                }))
                            })
                    }))),
            )
            .child(if running {
                let red = gpui_kit::hsla(0., 0.78, 0.55, 1.);
                div()
                    .id("speedtest-stop")
                    .h(px(30.))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .bg(red)
                    .shadow_sm()
                    .text_color(gpui_kit::white())
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .hover(|s| s.bg(gpui_kit::hsla(0., 0.82, 0.6, 1.)))
                    .active(|s| s.bg(gpui_kit::hsla(0., 0.72, 0.47, 1.)))
                    .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
                    .child(crate::icons::icon(IconName::CircleX).size(px(14.)))
                    .child("Stop Test")
                    .into_any_element()
            } else {
                Button::new("speedtest-start")
                    .cursor_pointer()
                    .primary()
                    .icon(crate::icons::icon(if self.status == Status::Idle { IconName::Play } else { IconName::RotateCw }))
                    .label(if self.status == Status::Idle { "Start Test" } else { "Run Again" })
                    .on_click(cx.listener(|this, _, _, cx| this.start(cx)))
                    .into_any_element()
            });

        v_flex().max_h(max_h).child(body).child(footer)
    }
}

impl Workspace {
    /// Opens the speed test popup for `volume` and starts the test.
    pub fn open_speedtest(&mut self, volume: Volume, window: &mut Window, cx: &mut Context<Self>) {
        let subtitle: SharedString = volume.name.clone().into();
        let view = cx.new(|cx| {
            let mut v = SpeedTestView::new(volume);
            v.start(cx);
            v
        });
        window.open_dialog(cx, move |dialog, _, cx| {
            kit::dialog(dialog, IconName::Gauge, "Drive Speed Test", Some(subtitle.clone()), 520., cx).pb_0().child(view.clone())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{TestAppContext, VisualTestContext, WindowOptions, size};

    #[gpui_kit::test]
    async fn footer_shadow_only_when_body_scrolls(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let volume = Volume { name: "Test".into(), mount_point: tmp.path().into(), total: 1 << 40, available: 1 << 39, removable: true };
        let (window, _) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| cx.new(|_| SpeedTestView::new(volume))).unwrap()
        });
        let mut vcx = VisualTestContext::from_window(window, cx);
        for (height, indicator) in
            [(1400., "speedtest-footer-no-shadow"), (450., "speedtest-footer-shadow"), (1400., "speedtest-footer-no-shadow")]
        {
            vcx.simulate_resize(size(px(520.), px(height)));
            vcx.run_until_parked();
            assert!(vcx.debug_bounds(indicator).is_some(), "wrong shadow at height {height}");
        }
    }
}
