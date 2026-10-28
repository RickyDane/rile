//! Background jobs panel, opened from the sidebar footer: running copies,
//! moves, transfers and downloads with progress, speed and cancel, plus the
//! last few that finished.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::{Sizable, button::{Button, ButtonVariants}, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, px, rgba,
};

use crate::theme::chrome;
use crate::workspace::{RunningOp, Workspace};

fn fmt_bytes(b: u64) -> String {
    humansize::format_size(b, humansize::DECIMAL)
}

fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 { format!("{s}s") } else if s < 3600 { format!("{}m {}s", s / 60, s % 60) } else { format!("{}h {}m", s / 3600, (s % 3600) / 60) }
}

/// Numbers shown for a running op, shared by the jobs panel and the
/// transfer window.
pub struct OpStatus {
    /// Overall progress, None while unknown.
    pub overall: Option<f32>,
    /// The file being copied right now and its progress.
    pub file: Option<(String, f32, String)>,
    /// "3 of 12 items · 1.2 GB of 4 GB"
    pub totals: String,
    /// "48 MB/s · 2m 10s left"
    pub rate: String,
    /// Just one file moves: one bar says it all.
    pub single: bool,
}

pub fn op_status(op: &RunningOp) -> OpStatus {
    let p = &op.progress;
    let file_frac = p.file_fraction();
    let overall = if p.files_total == 0 && p.items_total > 1 {
        // Byte totals are per item (server transfers): combine with the item count.
        Some(((p.items_done as f32 + file_frac.unwrap_or(0.)) / p.items_total as f32).clamp(0., 1.))
    } else if p.bytes_total > 0 || p.items_total > 0 || p.files_total > 0 {
        Some(p.fraction().clamp(0., 1.))
    } else {
        None
    };
    let file = p.current.as_ref().zip(file_frac).map(|(name, f)| {
        let size = if p.file_bytes_total > 0 {
            format!("{} of {}", fmt_bytes(p.file_bytes_done), fmt_bytes(p.file_bytes_total))
        } else {
            String::new()
        };
        (name.to_string(), f.clamp(0., 1.), size)
    });
    let mut totals = Vec::new();
    if p.files_total > 0 {
        totals.push(format!("{} of {} items", p.files_done.min(p.files_total), p.files_total));
    } else if p.items_total > 0 {
        totals.push(format!("{} of {} items", p.items_done, p.items_total));
    }
    if p.bytes_total > 0 && (p.files_total > 0 || p.items_total <= 1) {
        totals.push(format!("{} of {}", fmt_bytes(p.bytes_done), fmt_bytes(p.bytes_total)));
    }
    let elapsed = op.started.elapsed();
    let speed = (op.speed > 0. && elapsed.as_secs_f64() > 0.5).then_some(op.speed);
    let left = p.bytes_total.saturating_sub(p.bytes_done);
    let mut eta = speed.filter(|_| left > 0).map(|s| Duration::from_secs_f64(left as f64 / s));
    let mut rate = Vec::new();
    if let Some(s) = speed {
        rate.push(format!("{}/s", fmt_bytes(s as u64)));
    } else if p.bytes_total == 0 && p.files_done > 0 && p.files_total > p.files_done && elapsed.as_secs() >= 3 {
        // Item-based work (e.g. describing photos): pace by items.
        let per_min = p.files_done as f64 / elapsed.as_secs_f64() * 60.;
        rate.push(format!("{per_min:.0}/min"));
        eta = Some(Duration::from_secs_f64((p.files_total - p.files_done) as f64 * 60. / per_min.max(0.01)));
    }
    match eta {
        Some(eta) => rate.push(format!("{} left", fmt_duration(eta))),
        None => rate.push(format!("{} elapsed", fmt_duration(elapsed))),
    }
    let single = p.files_total <= 1 && p.items_total <= 1;
    OpStatus { overall, file, totals: totals.join(" · "), rate: rate.join(" · "), single }
}

/// Floating panel anchored above the sidebar footer.
pub fn render_panel(ws: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) -> AnyElement {
    let ch = chrome(cx);
    let running: Vec<AnyElement> = ws
        .ops
        .iter()
        .map(|op| {
            let id = op.id;
            let st = op_status(op);
            let detail = [st.totals.clone(), st.rate.clone()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            v_flex()
                .gap(px(7.))
                .px(px(14.))
                .py(px(11.))
                .border_t_1()
                .border_color(ch.hairline)
                .child(
                    h_flex()
                        .gap(px(10.))
                        .child(crate::icons::icon(IconName::ArrowDownUp).size(px(15.)).text_color(ch.accent))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(ch.text).child(op.label.clone()))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(ch.text_muted)
                                .child(st.overall.map(|f| format!("{:.0}%", f * 100.)).unwrap_or_else(|| "…".into())),
                        )
                        .child(
                            Button::new(("job-cancel", id)).cursor_pointer()
                                .ghost()
                                .xsmall()
                                .icon(crate::icons::icon(IconName::X))
                                .tooltip("Cancel")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.cancel_op(id);
                                    cx.notify();
                                })),
                        ),
                )
                .when_some(st.file.clone(), |d, (name, _, _)| {
                    d.child(div().truncate().text_size(px(10.)).text_color(ch.text_muted).child(name))
                })
                .child(crate::progress::activity_bar(("job-progress", id), st.overall, 5., cx))
                .child(div().text_size(px(10.)).text_color(ch.text_faint).child(detail))
                .when(op.backgrounded && op.progress.bytes_total > 0, |d| {
                    d.child(
                        div()
                            .id(("job-show", id))
                            .text_size(px(10.))
                            .text_color(ch.accent)
                            .cursor_pointer()
                            .child("Show transfer window")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(op) = this.ops.iter_mut().find(|o| o.id == id) {
                                    op.backgrounded = false;
                                }
                                this.jobs_open = false;
                                cx.notify();
                            })),
                    )
                })
                .into_any_element()
        })
        .collect();

    let finished: Vec<AnyElement> = ws
        .finished_jobs
        .iter()
        .enumerate()
        .map(|(ix, job)| {
            let (icon, color, status): (IconName, gpui_kit::Hsla, String) = match &job.error {
                None => (IconName::Check, ch.success, "Done".into()),
                Some(e) if e.as_ref() == "Cancelled" => (IconName::X, ch.text_faint, "Cancelled".into()),
                Some(e) => (IconName::CircleX, rgba(0xf07178ff).into(), e.to_string()),
            };
            let ago = job.finished.elapsed();
            h_flex()
                .id(("job-done", ix))
                .gap(px(10.))
                .px(px(14.))
                .py(px(8.))
                .border_t_1()
                .border_color(ch.hairline)
                .child(crate::icons::icon(icon).size(px(14.)).text_color(color))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(div().truncate().text_size(px(12.)).text_color(ch.text_muted).child(job.label.clone()))
                        .child(div().truncate().text_size(px(10.)).text_color(ch.text_faint).child(status)),
                )
                .child(div().text_size(px(10.)).text_color(ch.text_faint).child(if ago.as_secs() < 60 { "just now".to_string() } else { format!("{} ago", fmt_duration(ago)) }))
                .into_any_element()
        })
        .collect();

    let empty = running.is_empty();
    let has_finished = !finished.is_empty();
    crate::views::kit::surface(v_flex(), cx)
        .id("jobs-panel")
        .occlude()
        .w(px(340.))
        .max_h(px(420.))
        .overflow_hidden()
        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
            this.jobs_open = false;
            this.jobs_closed_at = Some(std::time::Instant::now());
            cx.notify();
        }))
        .child(
            h_flex()
                .px(px(14.))
                .py(px(12.))
                .gap(px(8.))
                .child(div().flex_1().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(ch.text_strong).child("Background jobs"))
                .child(div().text_size(px(11.)).text_color(ch.text_faint).child(if empty { "Idle".to_string() } else { format!("{} running", ws.ops.len()) })),
        )
        .child(
            v_flex()
                .id("jobs-list")
                .overflow_y_scroll()
                .children(running)
                .when(empty, |d| {
                    d.child(
                        v_flex()
                            .items_center()
                            .gap(px(6.))
                            .px(px(14.))
                            .py(px(22.))
                            .border_t_1()
                            .border_color(ch.hairline)
                            .child(crate::icons::icon(IconName::Check).size(px(20.)).text_color(ch.text_faint))
                            .child(div().text_size(px(12.)).text_color(ch.text_muted).child("Nothing running"))
                            .child(div().text_size(px(10.)).text_color(ch.text_faint).child("File transfers and AI model downloads show up here.")),
                    )
                })
                .when(has_finished, |d| {
                    d.child(
                        div()
                            .px(px(14.))
                            .pt(px(10.))
                            .pb(px(4.))
                            .border_t_1()
                            .border_color(ch.hairline)
                            .text_size(px(10.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ch.text_faint)
                            .child("RECENTLY FINISHED"),
                    )
                    .children(finished)
                }),
        )
        .into_any_element()
}

/// The centered transfer window: every running byte transfer with a bar
/// for the current file and one for the whole operation. "Run in
/// Background" hides it into the jobs panel.
pub fn render_transfer_window(ws: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    let ch = chrome(cx);
    let shown: Vec<&RunningOp> = ws.ops.iter().filter(|o| o.in_transfer_window()).collect();
    if shown.is_empty() {
        return None;
    }
    let count = shown.len();
    let rows: Vec<AnyElement> = shown
        .into_iter()
        .enumerate()
        .map(|(ix, op)| {
            let id = op.id;
            let st = op_status(op);
            let line = |left: String, right: String| {
                h_flex()
                    .gap(px(10.))
                    .text_size(px(11.))
                    .child(div().flex_1().min_w_0().truncate().text_color(ch.text_muted).child(left))
                    .child(div().flex_none().text_color(ch.text_faint).child(right))
            };
            v_flex()
                .gap(px(8.))
                .px(px(20.))
                .py(px(16.))
                .when(ix > 0, |d| d.border_t_1().border_color(ch.hairline))
                .child(
                    h_flex()
                        .gap(px(10.))
                        .child(crate::icons::icon(IconName::ArrowDownUp).size(px(16.)).text_color(ch.accent))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ch.text_strong)
                                .child(op.label.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(ch.text_muted)
                                .child(st.overall.map(|f| format!("{:.0}%", f * 100.)).unwrap_or_else(|| "…".into())),
                        )
                        .child(
                            Button::new(("transfer-cancel", id)).cursor_pointer()
                                .ghost()
                                .xsmall()
                                .icon(crate::icons::icon(IconName::X))
                                .tooltip("Cancel")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.cancel_op(id);
                                    cx.notify();
                                })),
                        ),
                )
                .map(|d| match (st.single, st.file.clone()) {
                    // One file: its name and size over a single bar, speed below.
                    (true, file) => {
                        let (name, size) = file.map(|(n, _, s)| (n, s)).unwrap_or_else(|| ("Preparing…".into(), String::new()));
                        d.child(line(name, size))
                            .child(crate::progress::activity_bar(("transfer-all", id), st.overall, 6., cx))
                            .child(line(String::new(), st.rate.clone()))
                    }
                    (false, file) => d
                        .when_some(file, |d, (name, frac, size)| {
                            d.child(line(name, size)).child(crate::progress::activity_bar(("transfer-file", id), Some(frac), 4., cx))
                        })
                        .child(div().h(px(2.)))
                        .child(line(if st.totals.is_empty() { "Overall".into() } else { st.totals.clone() }, st.rate.clone()))
                        .child(crate::progress::activity_bar(("transfer-all", id), st.overall, 6., cx)),
                })
                .into_any_element()
        })
        .collect();
    let panel = crate::views::kit::surface(v_flex(), cx)
        .id("transfer-window")
        .occlude()
        .w(px(460.))
        .max_w(gpui_kit::relative(0.92))
        .max_h(gpui_kit::relative(0.8))
        .overflow_hidden()
        .shadow_2xl()
        .child(
            h_flex()
                .px(px(20.))
                .pt(px(16.))
                .pb(px(4.))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ch.text_strong)
                        .child(if count == 1 { "Transferring".to_string() } else { format!("{count} transfers") }),
                ),
        )
        .child(v_flex().id("transfer-list").overflow_y_scroll().children(rows))
        .child(
            h_flex()
                .px(px(20.))
                .py(px(12.))
                .gap(px(8.))
                .justify_end()
                .border_t_1()
                .border_color(ch.hairline)
                .child(
                    Button::new("transfer-cancel-all").cursor_pointer()
                        .outline()
                        .small()
                        .label(if count == 1 { "Cancel" } else { "Cancel All" })
                        .on_click(cx.listener(|this, _, _, cx| {
                            let ids: Vec<usize> = this.ops.iter().filter(|o| o.in_transfer_window()).map(|o| o.id).collect();
                            for id in ids {
                                this.cancel_op(id);
                            }
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("transfer-background").cursor_pointer()
                        .primary()
                        .small()
                        .label("Run in Background")
                        .on_click(cx.listener(|this, _, _, cx| {
                            for op in this.ops.iter_mut().filter(|o| o.in_transfer_window()) {
                                op.backgrounded = true;
                            }
                            cx.notify();
                        })),
                ),
        );
    Some(
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .child(panel)
            .into_any_element(),
    )
}
