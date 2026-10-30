//! Two-finger trackpad swipes through a pane's history. The page follows
//! the fingers sideways with the previous (or next) page attached beside
//! it, like a strip of pages; letting go past the threshold slides the
//! neighbour in, otherwise the strip springs back.

use std::time::{Duration, Instant};

use gpui_kit::component::v_flex;
use gpui_kit::{
    AnyElement, App, AppContext, BoxShadow, Bounds, Context, DispatchPhase, Entity, Hitbox, HitboxBehavior, InteractiveElement,
    IntoElement, ParentElement, Pixels, ScrollEvent, Styled, Subscription, Task, TouchPhase, Window, div, hsla, point, px,
};

use super::Workspace;
use crate::pane::Pane;
use rile_core::history::Location;
use rile_core::settings::ViewKind;

/// Travel before a gesture is judged horizontal or vertical.
const ENGAGE: f32 = 10.;
/// Fraction of the slot width that commits on release.
const COMMIT: f32 = 0.22;
/// A quick flick commits from this far, at [`FLICK_SPEED`] px/ms or more.
const FLICK_MIN: f32 = 40.;
const FLICK_SPEED: f32 = 0.6;
/// How far the page gives when there is no page in that direction.
const RUBBER: f32 = 56.;
const FINISH: Duration = Duration::from_millis(220);
const SETTLE: Duration = Duration::from_millis(220);
/// Longest the neighbour stays over the pane while it loads the new folder.
const COVER: Duration = Duration::from_millis(600);
/// Longest a trackpad's momentum scrolling runs after the fingers lift.
const MOMENTUM: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct Swipe {
    gesture: Option<Gesture>,
    /// Content offset while the fingers drag it.
    offset: f32,
    anim: Option<Anim>,
    peek: Option<Peek>,
    /// After navigating, the neighbour stays on top until the pane has
    /// loaded the same page, so the swap doesn't flash an empty listing.
    covering: Option<Instant>,
    _commit: Option<Task<()>>,
    /// Set when a sideways swipe lifts: the trackpad's momentum events that
    /// follow would otherwise scroll whatever list is under the pointer.
    momentum: Option<Instant>,
}

struct Gesture {
    /// Raw finger travel since the gesture started.
    dx: f32,
    dy: f32,
    /// `None` until decided; `Some(false)` leaves the gesture to scrolling.
    horizontal: Option<bool>,
    /// Horizontal speed in px/ms, smoothed.
    velocity: f32,
    last: Instant,
}

/// The page next to the current one: where back (or forward) leads.
struct Peek {
    back: bool,
    location: Location,
    /// A throwaway pane listing the folder (none for built-in pages).
    pane: Option<Entity<Pane>>,
    _observe: Option<Subscription>,
}

#[derive(Clone, Copy)]
struct Anim {
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
}

impl Anim {
    fn new(from: f32, to: f32, duration: Duration) -> Self {
        Self { from, to, start: Instant::now(), duration }
    }

    fn progress(&self) -> f32 {
        (self.start.elapsed().as_secs_f32() / self.duration.as_secs_f32()).min(1.)
    }

    fn value(&self) -> f32 {
        let t = 1. - (1. - self.progress()).powi(3);
        self.from + (self.to - self.from) * t
    }
}

/// Resistance past the end of the history: approaches [`RUBBER`].
fn rubber(dx: f32) -> f32 {
    dx.signum() * RUBBER * (1. - (-dx.abs() / (RUBBER * 3.)).exp())
}

impl Workspace {
    /// Where slot `ix` goes back (`true`) or forward (`false`) to, if anywhere.
    fn swipe_target(&self, ix: usize, back: bool, cx: &App) -> Option<Location> {
        let pane = self.panes[ix].read(cx);
        if !back {
            pane.history.forward_target().cloned()
        } else if pane.search.is_some() {
            // Back from search results shows the folder itself.
            Some(pane.location.clone())
        } else {
            pane.history.back_target().cloned()
        }
    }

    /// Makes sure the page in the swipe's direction is ready beside it.
    fn swipe_peek(&mut self, ix: usize, back: bool, cx: &mut Context<Self>) -> bool {
        if self.swipe[ix].peek.as_ref().is_some_and(|p| p.back == back) {
            return true;
        }
        let Some(location) = self.swipe_target(ix, back, cx) else { return false };
        let (pane, observe) = if matches!(location, Location::Dir(_) | Location::Remote { .. }) {
            let p = self.panes[ix].read(cx);
            let (hidden, key, order, view) = (p.show_hidden, p.sort_key, p.sort_order, p.view);
            let target = location.clone();
            let pane = cx.new(|cx| Pane::new(target, hidden, key, order, view, cx));
            let observe = cx.observe(&pane, |_, _, cx| cx.notify());
            (Some(pane), Some(observe))
        } else {
            (None, None)
        };
        self.swipe[ix].peek = Some(Peek { back, location, pane, _observe: observe });
        true
    }

    /// Feeds a scroll event over slot `ix`; returns whether the swipe took it.
    fn swipe_scroll(&mut self, ix: usize, ev: &ScrollEvent, width: f32, cx: &mut Context<Self>) -> bool {
        if !ev.delta.precise() {
            return false;
        }
        let pane = self.panes[ix].read(cx);
        // Miller columns scroll sideways themselves.
        if pane.view == ViewKind::Columns && matches!(pane.location, Location::Dir(_) | Location::Remote { .. }) {
            return false;
        }
        let delta = ev.delta.pixel_delta(px(20.));
        let (dx, dy): (f32, f32) = (delta.x.into(), delta.y.into());
        let s = &mut self.swipe[ix];
        match ev.touch_phase {
            TouchPhase::Started => {
                s.momentum = None;
                if s._commit.is_some() || s.covering.is_some() {
                    return false;
                }
                // Catch a page that is still settling back where it is.
                let caught = s.anim.take().map_or(0., |a| a.value());
                s.offset = caught;
                s.gesture = Some(Gesture {
                    dx: caught,
                    dy: 0.,
                    horizontal: (caught != 0.).then_some(true),
                    velocity: 0.,
                    last: Instant::now(),
                });
                caught != 0.
            }
            TouchPhase::Moved => {
                let Some(g) = s.gesture.as_mut() else {
                    // Momentum carries no phase of its own; swallow it until
                    // the next touch (or it has surely run out).
                    return s.momentum.is_some_and(|t| t.elapsed() < MOMENTUM);
                };
                g.dx += dx;
                g.dy += dy;
                if g.horizontal.is_none() && (g.dx.abs() >= ENGAGE || g.dy.abs() >= ENGAGE) {
                    g.horizontal = Some(g.dx.abs() > g.dy.abs() * 2.);
                }
                if g.horizontal != Some(true) {
                    return false;
                }
                let ms = g.last.elapsed().as_secs_f32() * 1000.;
                if ms > 0. {
                    g.velocity = g.velocity * 0.6 + dx / ms.max(4.) * 0.4;
                }
                g.last = Instant::now();
                let raw = g.dx;
                let offset = if raw != 0. && self.swipe_peek(ix, raw > 0., cx) { raw.clamp(-width, width) } else { rubber(raw) };
                self.swipe[ix].offset = offset;
                cx.notify();
                true
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                let Some(g) = s.gesture.take() else { return false };
                if g.horizontal != Some(true) {
                    return false;
                }
                s.momentum = Some(Instant::now());
                let offset = s.offset;
                let back = offset > 0.;
                let flick = offset.abs() >= FLICK_MIN && g.velocity.signum() == offset.signum() && g.velocity.abs() >= FLICK_SPEED;
                let commit = ev.touch_phase == TouchPhase::Ended
                    && offset != 0.
                    && s.peek.as_ref().is_some_and(|p| p.back == back)
                    && (offset.abs() >= width * COMMIT || flick);
                if commit {
                    self.swipe_commit(ix, back, width, cx);
                } else {
                    let s = &mut self.swipe[ix];
                    s.anim = Some(Anim::new(offset, 0., SETTLE));
                    s.offset = 0.;
                }
                cx.notify();
                true
            }
        }
    }

    /// Slides the neighbour fully in, then navigates the pane there.
    fn swipe_commit(&mut self, ix: usize, back: bool, width: f32, cx: &mut Context<Self>) {
        let dir = if back { 1. } else { -1. };
        let s = &mut self.swipe[ix];
        let from = s.offset;
        // Finish at the speed the fingers left off, but never drag on.
        let remaining = 1. - from.abs() / width;
        let duration = FINISH.mul_f32(remaining.clamp(0.4, 1.));
        s.anim = Some(Anim::new(from, dir * width, duration));
        s.offset = 0.;
        s._commit = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(duration).await;
            this.update(cx, |ws, cx| {
                let Some(pane) = ws.panes.get(ix).cloned() else { return };
                pane.update(cx, |p, cx| if back { p.go_back(cx) } else { p.go_forward(cx) });
                let s = &mut ws.swipe[ix];
                s.anim = None;
                s.covering = Some(Instant::now());
                s._commit = None;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Current offset of slot `ix`'s page. Keeps frames coming while an
    /// animation runs and drops the neighbour once it is no longer needed.
    fn swipe_frame(&mut self, ix: usize, window: &mut Window, cx: &App) -> f32 {
        let loading = self.panes[ix].read(cx).loading;
        let s = &mut self.swipe[ix];
        if let Some(since) = s.covering {
            if loading && since.elapsed() < COVER {
                window.request_animation_frame();
            } else {
                s.covering = None;
                s.peek = None;
            }
            return 0.;
        }
        let Some(anim) = s.anim else {
            if s.gesture.is_none() && s._commit.is_none() {
                s.peek = None;
            }
            return s.offset;
        };
        let value = anim.value();
        if anim.progress() < 1. {
            window.request_animation_frame();
        } else if anim.to == 0. {
            s.anim = None;
            s.peek = None;
        }
        value
    }

    /// Wraps slot `ix`'s page so it follows swipes, with its neighbour beside it.
    pub(super) fn render_swipe(&mut self, ix: usize, content: AnyElement, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let offset = self.swipe_frame(ix, window, cx);
        let width = self.slot_width(ix, window).max(1.);
        let covering = self.swipe[ix].covering.is_some();
        let ws = cx.entity();
        let page = v_flex().flex_1().min_h_0().w_full().relative().left(px(offset)).child(content);
        let peek = self.swipe[ix].peek.as_ref().map(|p| (p.back, p.location.clone(), p.pane.clone()));
        let peek = peek.and_then(|(back, location, pane)| {
            // The neighbour sits flush against the page's edge.
            let x = if covering { 0. } else if back { offset - width } else { offset + width };
            if !covering && (offset == 0. || (offset > 0.) != back) {
                return None;
            }
            let pane = pane.unwrap_or_else(|| self.panes[ix].clone());
            let page = self.render_page(pane, &location, ix, window, cx);
            let ch = crate::theme::chrome(cx);
            Some(
                v_flex()
                    .id(("swipe-peek", ix))
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .w_full()
                    .left(px(x))
                    .bg(ch.browser)
                    .shadow(vec![BoxShadow {
                        color: hsla(0., 0., 0., if ch.dark { 0.45 } else { 0.18 }),
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(18.),
                        spread_radius: px(0.),
                        inset: false,
                    }])
                    .child(page),
            )
        });
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(page)
            .children(peek)
            // Listens before the pages' scrollers so a sideways swipe
            // doesn't also scroll a list.
            .child(
                gpui_kit::canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |bounds: Bounds<Pixels>, hitbox: Hitbox, window, _| {
                        let width: f32 = bounds.size.width.into();
                        window.on_mouse_event(move |ev: &ScrollEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture
                                && hitbox.should_handle_scroll(window)
                                && ws.update(cx, |ws, cx| ws.swipe_scroll(ix, ev, width, cx))
                            {
                                cx.stop_propagation();
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
            .into_any_element()
    }
}
