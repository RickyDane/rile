//! Window-wide, queued trophy celebrations. No focus changes or modal dialogs.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{Animation, AnimationExt, AnyElement, AppContext, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Styled, Task, div, px};
use rile_services::rewards::{self, Event, Progress, Unlock};

use super::Workspace;

const DISPLAY: Duration = Duration::from_millis(4200);

#[derive(Default)]
pub struct RewardState {
    pub progress: Option<Progress>,
    pending: VecDeque<Unlock>,
    active: Option<(Unlock, Instant)>,
    serial: usize,
    pub(super) task: Option<Task<()>>,
}

impl RewardState {
    pub(crate) fn enqueue(&mut self, unlocks: impl IntoIterator<Item = Unlock>) -> bool {
        self.pending.extend(unlocks);
        self.advance(Instant::now())
    }

    fn advance(&mut self, now: Instant) -> bool {
        let had_active = self.active.is_some();
        if self.active.as_ref().is_some_and(|(_, start)| now.duration_since(*start) >= DISPLAY) {
            self.active = None;
        } else if self.active.is_some() {
            return false;
        }
        if self.pending.is_empty() { return had_active; }
        self.active = self.pending.pop_front().map(|badge| (badge, now));
        self.serial += 1;
        true
    }
}

impl Workspace {
    pub(super) fn reward_event(&self, event: Event, cx: &mut Context<Self>) {
        cx.background_spawn(async move { rewards::record_event(event) }).detach();
    }

    pub(super) fn watch_rewards(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let mut next_poll = Instant::now();
            loop {
                let update = if Instant::now() >= next_poll {
                    next_poll = Instant::now() + Duration::from_secs(3);
                    Some(cx.background_spawn(async { rewards::poll(rewards::snapshot()) }).await)
                } else { None };
                if this.update(cx, |this, cx| {
                    let mut changed = false;
                    if let Some((progress, unlocks)) = update {
                        changed = this.rewards.progress != Some(progress);
                        this.rewards.progress = Some(progress);
                        changed |= this.rewards.enqueue(unlocks);
                    }
                    if this.rewards.advance(Instant::now()) || changed { cx.notify(); }
                }).is_err() { break; }
                cx.background_executor().timer(Duration::from_millis(250)).await;
            }
        })
    }
}

pub fn celebration(ws: &Workspace, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    let (badge, _) = ws.rewards.active.as_ref()?;
    let ch = crate::theme::chrome(cx);
    let serial = ws.rewards.serial;
    let accent = ch.accent;
    Some(
        h_flex().id("reward-celebration").absolute().right(px(18.)).bottom(px(48.)).max_w(px(340.)).p(px(16.))
            .debug_selector(|| "reward-celebration".into())
            .gap(px(13.)).rounded(px(13.)).border_1().border_color(accent.opacity(0.5))
            .bg(ch.surface_colors().0).shadow_2xl()
            .child(
                div().size(px(48.)).relative().flex().items_center().justify_center()
                    .rounded_full().bg(accent.opacity(0.12))
                    .children((0..8).map(move |i| {
                        let angle = i as f32 * std::f32::consts::TAU / 8.;
                        div().absolute().size(px(4.)).rounded_full().bg(accent)
                            .with_animation(("reward-spark", serial * 8 + i), Animation::new(Duration::from_millis(1100)), move |el, t| {
                                let radius = 12. + 23. * t;
                                el.left(px(22. + angle.cos() * radius)).top(px(22. + angle.sin() * radius)).opacity(1. - t)
                            })
                    }))
                    .child(crate::icons::icon(IconName::Star).text_color(accent)
                        .with_animation(("reward-star", serial), Animation::new(Duration::from_millis(700)), |el, t| {
                            let pop = (t * std::f32::consts::PI).sin();
                            el.size(px(26. + 10. * pop))
                        })),
            )
            .child(v_flex().gap(px(4.)).min_w_0()
                .child(div().text_size(px(9.)).font_weight(FontWeight::SEMIBOLD).text_color(accent).child("BADGE UNLOCKED"))
                .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(ch.text_strong).child(badge.name.clone()))
                .child(div().text_size(px(10.)).text_color(ch.text_muted).child(badge.detail.clone())))
            .with_animation(("reward-unlock", serial), Animation::new(DISPLAY), |el, t| {
                let enter = (t / 0.09).min(1.);
                let exit = ((1. - t) / 0.12).min(1.);
                el.opacity(enter.min(exit)).bottom(px(48. - 14. * (1. - enter).powi(3)))
            })
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn celebrations_queue_and_expire_in_order() {
        let mut state = RewardState::default();
        for name in ["First", "Second"] {
            state.pending.push_back(Unlock { id: name.into(), name: name.into(), detail: "earned".into() });
        }
        let now = Instant::now();
        state.advance(now);
        assert_eq!(state.active.as_ref().unwrap().0.name, "First");
        assert!(!state.advance(now + Duration::from_secs(1)));
        state.advance(now + DISPLAY);
        assert_eq!(state.active.as_ref().unwrap().0.name, "Second");
        state.advance(now + DISPLAY * 2);
        assert!(state.active.is_none());
        assert!(state.pending.is_empty());
        assert!(!state.advance(now + DISPLAY * 3));
    }
}
