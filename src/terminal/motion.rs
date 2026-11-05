//! The frame clock: a draw that puts something moving on screen names the instant its cells next
//! change, the earliest stands, and the loop sleeps until then, paced. Everything that moves is a
//! function of record times, `now` and the motion switch, so nothing here remembers a frame.
use std::cell::Cell;
use tokio::time::{Duration, Instant};

/// A moving frame comes at most twelve times a second.
pub(super) const FRAME: Duration = Duration::from_millis(83);
/// While a frame moves between a child's card and the split, thirty a second.
pub(super) const MOVE: Duration = Duration::from_millis(33);

/// The earliest wall instant, in ms, at which the screen just drawn looks different by itself.
/// A draw starts it empty; shared by reference, so drawers holding `&App` can ask.
#[derive(Default)]
pub(super) struct Wake(Cell<Option<u64>>);
impl Wake {
    /// Asks for a frame at wall `at` ms; one already past asks for the next paced frame.
    pub(super) fn at(&self, at: u64) {
        self.0.set(Some(self.0.get().map_or(at, |old| old.min(at))));
    }
    /// Asks for the first multiple of `step` ms after `now`.
    pub(super) fn every(&self, now: u64, step: u64) {
        self.at(now - now % step + step);
    }
    /// The earliest ask, emptied: a draw calls it first, so a thing that stops moving stops asking.
    pub(super) fn take(&self) -> Option<u64> {
        self.0.take()
    }
    pub(super) fn get(&self) -> Option<u64> {
        self.0.get()
    }
}

/// When the person left the terminal and came back, from its focus reports, and when Razor
/// opened: wall ms. A terminal that never reports focus stays focused.
#[derive(Clone, Copy, Default, PartialEq)]
pub(super) struct Presence {
    pub(super) focused: bool,
    /// The last focus loss and the last focus gain; `back` is before `left` while away.
    pub(super) left: u64,
    pub(super) back: u64,
    pub(super) opened: u64,
}
impl Presence {
    /// Focused, opened at `now`.
    pub(super) fn new(now: u64) -> Self {
        Self {
            focused: true,
            opened: now,
            ..Self::default()
        }
    }
    /// A focus report: losing focus stamps `left`, gaining it stamps `back`; a repeat changes
    /// nothing.
    pub(super) fn report(&mut self, focused: bool, now: u64) {
        if focused != self.focused {
            *if focused {
                &mut self.back
            } else {
                &mut self.left
            } = now;
            self.focused = focused;
        }
    }
    /// Whether something at `at` happened unseen: after the person left and before they came
    /// back, or at any time since they left while they are still away.
    pub(super) fn missed(&self, at: u64) -> bool {
        at >= self.left && (!self.focused || at < self.back)
    }
}

/// Whether motion plays: `ui.motion` is on and the terminal has focus. Every motion reader asks
/// this one switch; otherwise the settled form draws.
pub(super) fn on(app: &super::App) -> bool {
    app.ui.motion && app.presence.focused
}

/// When the loop next draws for an ask at wall `at`: then, but never before `last` plus the pace
/// after a draw that took `cost`.
pub(super) fn deadline(at: u64, now: u64, last: Instant, cost: Duration, moving: bool) -> Instant {
    let asked = Instant::now() + Duration::from_millis(at.saturating_sub(now));
    asked.max(last + pace(cost, moving))
}

/// `FRAME`, or `MOVE` while a frame is `moving`, or after a draw over half of that, twice its cost
/// in 25 ms steps, at most 250 ms: half of each frame stays free for keys.
pub(super) fn pace(cost: Duration, moving: bool) -> Duration {
    let floor = [FRAME, MOVE][usize::from(moving)];
    let twice = cost.as_millis() as u64 * 2;
    match twice > floor.as_millis() as u64 {
        true => Duration::from_millis(twice.div_ceil(25).saturating_mul(25).min(250)),
        false => floor,
    }
}

/// How far `at` is into a span of `ms` from `start`, clamped to 0..1.
pub(super) fn progress(now: u64, start: u64, ms: u64) -> f64 {
    (now.saturating_sub(start) as f64 / ms as f64).clamp(0., 1.)
}

/// How far a motion of `ms` from `start` has played: its `progress`, or whole with motion off.
pub(super) fn played(motion: bool, now: u64, start: u64, ms: u64) -> f64 {
    progress(if motion { now } else { u64::MAX }, start, ms)
}

/// `1 - (1 - t)³`, with `t` clamped to 0..1.
pub(super) fn ease_out(t: f64) -> f64 {
    1. - (1. - t.clamp(0., 1.)).powi(3)
}

/// The cubic ease in and out, with `t` clamped to 0..1.
pub(super) fn ease_in_out(t: f64) -> f64 {
    let t = t.clamp(0., 1.);
    match t < 0.5 {
        true => 4. * t * t * t,
        false => 1. - (-2. * t + 2.).powi(3) / 2.,
    }
}
