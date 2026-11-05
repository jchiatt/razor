//! The child card's edge: a light lapping it at the child's pace and breathing while it thinks,
//! a stitch per call along the top, the edge fraying amber on a failure and pulled in on a good
//! end. Only the light integrates between frames.
use crate::{
    protocol::{AgentView, Body, Entry, Outcome, SessionId, State},
    terminal::{
        motion::ease_in_out,
        text::*,
        transcript::{Row, Transcript},
    },
};
use ratatui::{style::Color, text::Span};
use unicode_segmentation::UnicodeSegmentation;

/// The edge's own colour: the cells a card draws in it are the ones the rim repaints.
pub(super) const RIM: Color = rgb(0x4c3762);

/// A finished child's edge: the failure's fray drawn over 1.4 s, else the success's flash
/// fading over 420 ms; settled at once with motion off.
pub(in crate::terminal) fn ending(
    ended: Option<(u64, bool)>,
    now: u64,
    motion: bool,
) -> (f64, f64) {
    let Some((at, fail)) = ended else {
        return (0., 0.);
    };
    let since = if motion {
        now.saturating_sub(at) as f64
    } else {
        f64::MAX
    };
    match fail {
        true => ((since / 1400.).min(1.), 0.),
        false => (0., 1. - (since / 420.).min(1.)),
    }
}

/// The edge lights of at most 64 cards, oldest first; reduced motion never reads this memory.
#[derive(Default)]
pub(in crate::terminal) struct Lights(Vec<(SessionId, Light)>);
struct Light {
    at: u64,
    pos: f64,
    speed: f64,
    think: f64,
    dim: f64,
    phase: f64,
}
/// Where the light is on the edge, how much it breathes, and how far the edge has dimmed.
#[derive(Default)]
pub(in crate::terminal) struct Pulse {
    pos: f64,
    breath: f64,
    pub(in crate::terminal) dim: f64,
}
impl Lights {
    /// The light of `agent`'s card at `now`, eased toward the child's pace; with motion off,
    /// only the dimming of a quiet child, read straight from its times.
    pub(in crate::terminal) fn pulse(
        &mut self,
        agent: &AgentView,
        t: Option<&Transcript>,
        now: u64,
        motion: bool,
    ) -> Pulse {
        let (speed, think, dim) = pace(agent, t, now);
        if !motion {
            return Pulse {
                dim,
                ..Pulse::default()
            };
        }
        let id = &agent.session;
        if !self.0.iter().any(|(key, _)| key == id) {
            if self.0.len() == 64 {
                self.0.remove(0);
            }
            let light = Light {
                at: now,
                pos: 0.,
                speed,
                think,
                dim,
                phase: 0.,
            };
            self.0.push((id.clone(), light));
        }
        let Some((_, p)) = self.0.iter_mut().find(|(key, _)| key == id) else {
            return Pulse::default();
        };
        let dt = now.saturating_sub(p.at).min(250) as f64;
        p.speed += (speed - p.speed) * (1. - (-dt / 450.).exp());
        p.think += (think - p.think) * (1. - (-dt / 700.).exp());
        p.dim += (dim - p.dim) * (1. - (-dt / 1200.).exp());
        p.pos += p.speed * dt / 1000.;
        p.phase = (p.phase + dt / 2600. * std::f64::consts::TAU) % std::f64::consts::TAU;
        p.at = now;
        Pulse {
            pos: p.pos,
            breath: p.think * (0.5 - 0.5 * p.phase.cos()),
            dim: p.dim,
        }
    }
}

/// The light breathes whenever the child works with no call in flight, at any pace.
fn pace(agent: &AgentView, t: Option<&Transcript>, now: u64) -> (f64, f64, f64) {
    let busy = t.is_some_and(|t| {
        t.entries
            .values()
            .rev()
            .take_while(|e| now.saturating_sub(e.at) < 8000)
            .filter(|e| matches!(e.body, Body::Call { .. }))
            .take(3)
            .count()
            == 3
    });
    let running = !agent.calls.is_empty() || t.is_some_and(|t| t.running_since().is_some());
    let think = f64::from(u8::from(agent.state == State::Working && !running));
    let quiet = now.saturating_sub(agent.last_activity);
    let (speed, dim) = match () {
        _ if busy => (46., 0.),
        _ if running => (20., 0.),
        _ if quiet < 20_000 => (12., 0.),
        _ => (8., 1.),
    };
    (speed, think, dim)
}

/// One card's edge as drawn at `now`.
#[derive(Default)]
pub(in crate::terminal) struct Rim<'a> {
    /// The column of the card's anchor pipe, and the card's width.
    pub(in crate::terminal) left: usize,
    pub(in crate::terminal) columns: usize,
    pub(in crate::terminal) now: u64,
    pub(in crate::terminal) motion: bool,
    /// The child's transcript: one stitch per call.
    pub(in crate::terminal) t: Option<&'a Transcript>,
    /// How far the card has unrolled, 0 to 1.
    pub(in crate::terminal) open: f64,
    pub(in crate::terminal) pulse: Pulse,
    /// How far a failure has frayed the edge, the good end's flash, and how far the edge has
    /// been pulled in from the right, each 0 to 1.
    pub(in crate::terminal) fray: f64,
    pub(in crate::terminal) flash: f64,
    pub(in crate::terminal) pull: f64,
}
impl Rim<'_> {
    /// Repaints the edge cells (`RIM`) of `rows`, top to bottom: the light, the fray, the reach
    /// while it unrolls, the pull, and the top's stitches.
    pub(super) fn paint(&self, rows: &mut [Row]) {
        let (left, right) = (self.left, self.columns.saturating_sub(1));
        let span = right.saturating_sub(left);
        let side = rows.len().saturating_sub(2);
        let len = (span * 2 + side * 2 + 2) as f64;
        let reach = left as f64 + (self.open / 0.35).min(1.) * span as f64;
        let pulled = right.saturating_sub(
            (ease_in_out(self.pull) * span.saturating_sub(3) as f64).round() as usize,
        );
        let last = rows.len().saturating_sub(1);
        for (y, row) in rows.iter_mut().enumerate() {
            let mut x: usize = 0;
            let mut cells = Vec::new();
            for s in &row.words.spans {
                if s.style.fg != Some(RIM)
                    && (y > 0 && y < last || self.open == 1. && self.pull == 0.)
                {
                    x += width(&s.content);
                    cells.push(s.clone());
                    continue;
                }
                for g in s.content.graphemes(true) {
                    let i = if y == 0 {
                        x.saturating_sub(left)
                    } else if y == last {
                        span + 1 + side + right.saturating_sub(x)
                    } else if x <= left {
                        2 * span + 2 + 2 * side - y
                    } else {
                        span + y
                    };
                    let rim = s.style.fg == Some(RIM);
                    let mut cell = Span::styled(g.to_owned(), s.style);
                    if x >= left && (x as f64 > reach || x > pulled) {
                        cell = Span::raw(" ");
                    } else if rim {
                        cell = self.cell(g, i as f64, len, span as f64);
                    }
                    if (y == 0 || y == last) && x == pulled && pulled < right {
                        cell = Span::styled(
                            if y == 0 { "╮" } else { "╯" },
                            style(self.colour(i as f64, len, span as f64), GLASS),
                        );
                    }
                    x += width(g);
                    cells.push(cell);
                }
            }
            if y == 0 {
                self.stitches(&mut cells, len);
            }
            row.words.spans = cells;
        }
    }
    fn stitches(&self, cells: &mut [Span<'static>], len: f64) {
        let mut run = 0..0;
        let mut start = 0;
        for (i, cell) in cells.iter().enumerate() {
            if cell.content != "─" {
                start = i + 1;
            } else if i + 1 - start > run.len() {
                run = start..i + 1;
            }
        }
        let fits = run.len() / 2;
        let calls: Vec<_> = self
            .t
            .into_iter()
            .flat_map(|t| t.entries.values())
            .filter(|e| matches!(e.body, Body::Call { .. }))
            .rev()
            .take(fits + 1)
            .collect();
        let shown = calls.len().min(fits);
        for (k, call) in calls.iter().take(shown).enumerate() {
            let x = run.end - 1 - k * 2;
            let old = shown - k - 1;
            let fade = if calls.len() > fits && old < 3 {
                0.35 + 0.22 * old as f64
            } else {
                1.
            };
            let base = mix(GLASS, self.stitch(call), fade * (1. - 0.5 * self.pulse.dim));
            let flashed = mix(base, GREEN, self.flash);
            let column: usize = cells[..x].iter().map(|s| width(&s.content)).sum();
            let light = self.lit(column.saturating_sub(self.left) as f64, len);
            cells[x] = Span::styled("━", style(mix(flashed, rgb(0xffffff), 0.45 * light), GLASS));
        }
    }
    fn lit(&self, i: f64, len: f64) -> f64 {
        if !self.motion {
            return 0.;
        }
        let k = (self.pulse.pos - i).rem_euclid(len.max(1.));
        if k < 10. {
            (1. - k / 10.).powi(2)
        } else if len - k < 2. {
            0.35 * (1. - (len - k) / 2.)
        } else {
            0.
        }
    }
    fn amount(&self, i: f64, len: f64, span: f64) -> f64 {
        let d = (i - span).abs();
        (self.fray * 1.7 - d.min(len - d) / (len / 2.).max(1.)).clamp(0., 1.)
    }
    /// The edge's colour at `i` cells along it, clockwise from the top left, of `len`.
    pub(in crate::terminal) fn colour(&self, i: f64, len: f64, span: f64) -> Color {
        let hi = mix(VIOLET, rgb(0xffffff), 0.62);
        let edge = mix(mix(GLASS, VIOLET, 0.85), hi, 0.42 * self.pulse.breath);
        let base = mix(GLASS, edge, 1. - 0.58 * self.pulse.dim);
        let lit = mix(base, hi, (self.lit(i, len) + 0.8 * self.flash).min(1.));
        mix(
            GLASS,
            mix(lit, AMBER, self.amount(i, len, span)),
            (self.open / 0.35).min(1.),
        )
    }
    /// The edge's glyph `g` at `i` of `len`, frayed and in its colour.
    pub(in crate::terminal) fn cell(&self, g: &str, i: f64, len: f64, span: f64) -> Span<'static> {
        let a = self.amount(i, len, span);
        // Multiply as doubles before truncating to JavaScript's u32.
        let x = (i * 374_761_393. + 7. * 668_265_263.).rem_euclid(4294967296.) as u32;
        let x = ((x ^ (x >> 13)) as i32 as f64 * 1_274_126_177.).rem_euclid(4294967296.) as u32;
        let r = ((x ^ (x >> 16)) % 10_000) as f64 / 10_000.;
        let glyph = if r < 0.14 * a {
            " "
        } else if r < 0.62 * a {
            match (g, r < 0.34 * a) {
                ("─", true) => "┄",
                ("─", false) => "╌",
                ("│", true) => "┆",
                ("│", false) => "╎",
                _ => g,
            }
        } else {
            g
        };
        Span::styled(glyph.to_owned(), style(self.colour(i, len, span), GLASS))
    }
    fn stitch(&self, call: &Entry) -> Color {
        let Body::Call { name, .. } = &call.body else {
            return GRAY;
        };
        let result = self.t.and_then(|t| t.result(call));
        let base = match result.map(|e| &e.body) {
            Some(Body::Result {
                outcome: Outcome::Error | Outcome::Unknown,
                ..
            }) => AMBER,
            _ => match super::kind(name) {
                "edit" | "write" | "apply_patch" => MAGENTA,
                "shell" => GREEN,
                "agent" => VIOLET,
                _ => GRAY,
            },
        };
        if !self.motion {
            return base;
        }
        let age = self.now.saturating_sub(result.map_or(call.at, |e| e.at));
        if result.is_none() {
            mix(
                mix(GLASS, base, 0.55),
                mix(base, rgb(0xffffff), 0.3),
                0.5 - 0.5 * (self.now as f64 / 900. * std::f64::consts::TAU).cos(),
            )
        } else {
            mix(
                base,
                rgb(0xffffff),
                0.75 * (1. - (age as f64 / 600.).min(1.)),
            )
        }
    }
}
