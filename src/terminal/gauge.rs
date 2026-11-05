//! The context gauge: a vertical tape left of the transcript with its scale, trend and turns
//! left before a rebuild, folded to eight columns on a click; the split's one-row bar; and the
//! top bar's two-row bar.
use super::motion::{ease_out, on, progress};
use super::{App, text::*};
use crate::protocol::{AgentView, Body, SessionId};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier},
};

const RISE: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

// Points per turn over the last five steps of recorded turn ends, not a live estimate: a rebuild
// or an unmeasured turn ends the series.
fn trend(app: &App, id: &SessionId, window: u64) -> Option<f64> {
    let entries = app.transcripts.get(id)?.entries.values().rev();
    let ends = entries.take_while(|e| !matches!(e.body, Body::Rebuild { .. }));
    let mut points = ends.filter_map(|e| match e.body {
        Body::TurnEnd { context, .. } => Some(context),
        _ => None,
    });
    let last = points.next()??;
    let (step, first) = points.take(5).map_while(|point| point).enumerate().last()?;
    let steps = step + 1;
    if window == 0 || first == 0 || last == 0 {
        return None;
    }
    Some(((last as f64 - first as f64) / window as f64 * 1000. / steps as f64).round() / 10.)
}

fn tone(pct: f64, threshold: f64) -> Color {
    match pct {
        _ if pct >= threshold => RED,
        _ if pct >= 70. => AMBER,
        _ => WHITE,
    }
}

// A percentage fills `h` rows with an eighth-cell at its upper edge: the eighths, the edge row
// and the top lit row. The scale uses the same edge.
fn fill_at(pct: f64, h: usize) -> (usize, usize, usize) {
    let value = pct.clamp(0., 100.) / 100. * h as f64;
    let whole = value.floor() as usize;
    let partial = ((value - whole as f64) * 8.).round() as usize;
    let row = h.saturating_sub(whole + 1);
    let top = if partial > 0 {
        row
    } else {
        h.saturating_sub(whole)
    };
    (partial, row, top.min(h.saturating_sub(1)))
}

/// The row's context as a percent of its window; `None` when unmeasured.
pub(super) fn share(row: &AgentView) -> Option<f64> {
    let (c, w) = row.context.zip(row.window.filter(|w| *w > 0))?;
    Some((c as f64 / w as f64 * 100.).clamp(0., 100.))
}

/// The tide a new status row starts: from the share its gauge shows at `now`, so a move during a
/// tide continues it without a jump. `None` on the first reading, or when the share did not move.
pub(super) fn tide(app: &App, row: &AgentView, now: u64) -> Option<(SessionId, f64, u64)> {
    let old = app.agents.get(&row.session).and_then(share)?;
    let from = shown(app, &row.session, now).0?;
    (share(row)? != old).then(|| (row.session.clone(), from, now))
}

// `id`'s share as its gauge shows it at `now`, rounded, and how far its tide has run (1 at rest):
// rising over 400 ms or falling over 600 ms from the tide's share, eased out, with motion on.
pub(super) fn shown(app: &App, id: &SessionId, now: u64) -> (Option<f64>, f64) {
    let share = app.agents.get(id).and_then(share);
    let (level, k) = match (&app.tide, share) {
        (Some((tide, from, at)), Some(to)) if tide == id && on(app) => {
            let k = progress(now, *at, if to >= *from { 400 } else { 600 });
            (Some(from + (to - from) * ease_out(k)), k)
        }
        _ => (share, 1.),
    };
    if k < 1. {
        app.next.at(now + 83);
    }
    (level.map(f64::round), k)
}

/// The tape's columns: 17, or 8 folded.
pub(super) fn columns(app: &App) -> u16 {
    if app.gauge_folded { 8 } else { 17 }
}

pub(super) fn context(app: &App, id: Option<&SessionId>) -> Option<(u64, u64)> {
    match id {
        Some(id) => {
            let agent = app.agents.get(id)?;
            agent.context.zip(agent.window.filter(|w| *w > 0))
        }
        None => Some((0, *app.models.as_ref()?.windows.get(app.choice().0)?)),
    }
}

/// The tape is present at zero before a session; an unmeasured session says so. A move of
/// the reading fills or drains like a tide, its top cell white and cooling.
pub(super) fn draw(app: &App, b: &mut Buffer, r: Rect, id: Option<&SessionId>, now: u64) {
    if r.width < 7 || r.height == 0 {
        return;
    }
    let agent = id.and_then(|id| app.agents.get(id));
    let context = context(app, id);
    let (pct, k) = id.map_or((Some(0.), 1.), |id| shown(app, id, now));
    let threshold = agent.map_or(90., |a| a.threshold * 100.).clamp(0., 100.);
    let trend = id.zip(context).and_then(|(id, (_, w))| trend(app, id, w));
    let turns = pct.zip(trend).filter(|(p, t)| *p < threshold && *t > 0.);
    let turns = turns.map(|(p, t)| ((threshold - p) / t).round().max(1.) as u64);
    let folded = app.gauge_folded;
    let tokens = !folded && r.height >= 6;
    let show_turns = !folded && r.height >= 10 && turns.is_some();
    let h = r.height as usize
        - usize::from(!folded)
        - usize::from(tokens) * 2
        - usize::from(show_turns) * 2;
    if !folded {
        put(b, r.x, r.y, " CONTEXT", r.width - 1, style(GRAY, GLASS));
    }
    if h > 0 {
        let body = Rect::new(r.x, r.y + u16::from(!folded), r.width, h as u16);
        meter(b, body, (pct, 1. - k), threshold, trend.filter(|_| !folded));
        scale(b, body, pct, threshold, folded);
    }
    if let (true, Some(turns)) = (show_turns, turns) {
        let (words, ink) = (
            format!(" ~{turns} turn{} to", plural(turns)),
            style(MAGENTA, GLASS),
        );
        put(b, r.x, r.bottom() - 2, &words, r.width - 1, ink);
        put(b, r.x, r.bottom() - 1, "   rebuild", r.width - 1, ink);
    }
    if tokens {
        let y = r.bottom() - 1 - u16::from(show_turns) * 2;
        match context {
            Some((c, w)) => {
                let (c, w) = (format!(" {}", kilo(c)), format!(" of {}", kilo(w)));
                let x = put(b, r.x, y, &c, r.width - 1, style(WHITE, GLASS));
                put(
                    b,
                    x,
                    y,
                    &w,
                    r.right().saturating_sub(x + 1),
                    style(GRAY, GLASS),
                );
            }
            None => _ = put(b, r.x, y, " not measured", r.width - 1, style(GRAY, GLASS)),
        }
    }
    for y in r.y..r.bottom() {
        put(b, r.right() - 1, y, "│", 1, style(BEZEL_HI, GLASS));
    }
}

/// One row: the context line with its 70% and threshold marks, the trend's reach over five
/// turns (`▲`, without the tape's words) and the percentage; it fills like the tape.
pub(super) fn horizontal(app: &App, b: &mut Buffer, r: Rect, id: Option<&SessionId>, now: u64) {
    if r.height == 0 || r.width < 20 {
        return;
    }
    let agent = id.and_then(|id| app.agents.get(id));
    let (pct, k) = id.map_or((Some(0.), 1.), |id| shown(app, id, now));
    let threshold = agent.map_or(90., |a| a.threshold * 100.).clamp(0., 100.);
    let window = context(app, id).map(|(_, w)| w);
    let trend = id.zip(window).and_then(|(id, w)| trend(app, id, w));
    // Folded by a tap: no word, the lines' numbers beside their marks, no trend.
    let folded = app.gauge_folded;
    let cells = r.width.saturating_sub([14, 6][usize::from(folded)]) as usize;
    let at = |p: f64| (p / 100. * cells.saturating_sub(1) as f64).round() as usize;
    let reach = pct.zip(trend).filter(|(_, t)| *t > 0. && !folded);
    let reach = reach.map(|(p, t)| at((p + t * 5.).min(100.)));
    let word = if folded { "" } else { "CONTEXT " };
    let x = put(b, r.x + 1, r.y, word, r.width - 1, style(GRAY, GLASS));
    for i in 0..cells {
        let empty = pct.is_none_or(|p| p <= 0. || i > at(p));
        let (mark, tone) = match i {
            _ if reach == Some(i) && empty => ("▲", MAGENTA),
            _ if i == at(threshold) => ("┃", RED),
            _ if i == at(70.) => ("┃", AMBER),
            _ if empty && reach.is_some_and(|reach| i < reach) => ("─", MAGENTA),
            _ if empty => ("─", GRAY_DIM),
            _ if i >= at(threshold) => ("━", RED),
            _ if i >= at(70.) => ("━", AMBER),
            _ => ("━", GREEN),
        };
        let lit = pct.is_some_and(|p| i == at(p)) && !empty;
        let tone = mix(tone, WHITE, if lit { 1. - k } else { 0. });
        put(b, x + i as u16, r.y, mark, 1, style(tone, GLASS));
    }
    for (line, ink, limit) in [(70., AMBER, at(threshold)), (threshold, RED, cells + 1)] {
        let digits = format!("{line:.0}");
        let (cell, ink) = (at(line), style(ink, GLASS));
        if folded && cell + 2 + digits.len() <= limit {
            put(b, x + cell as u16 + 1, r.y, &digits, 3, ink);
        }
    }
    // The percentage in its zone's colour.
    let words = match (pct, folded) {
        (Some(p), true) => format!("{p:>4.0}"),
        (None, true) => "   —".into(),
        (Some(p), false) => format!(" {p:.0}%"),
        (None, false) => " —".into(),
    };
    let zone = pct.map_or(WHITE, |p| tone(p, threshold));
    put(b, x + cells as u16, r.y, &words, 5, style(zone, GLASS));
}

/// The bar's two rows: the tape on its side, filled left to right in its zones with an eighth
/// at the edge, which cools from white as a tide settles; the 70% and threshold lines marked on
/// the empty track; beside it the percentage over the tokens.
pub(super) fn bar(app: &App, b: &mut Buffer, r: Rect, id: Option<&SessionId>, now: u64) {
    const EIGHTHS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let agent = id.and_then(|id| app.agents.get(id));
    let (pct, k) = id.map_or((Some(0.), 1.), |id| shown(app, id, now));
    let threshold = agent.map_or(90., |a| a.threshold * 100.).clamp(0., 100.);
    let (top, low) = match (pct, context(app, id)) {
        (Some(p), Some((c, w))) => (format!("{p:.0}%"), format!("{}/{}", kilo(c), kilo(w))),
        _ => ("—".into(), String::new()),
    };
    let words = width(&top).max(width(&low)) as u16;
    let cells = r.width.saturating_sub(words + 1) as usize;
    let at = |p: f64| (p / 100. * cells as f64) as usize;
    let value = pct.unwrap_or(0.).clamp(0., 100.) / 100. * cells as f64;
    let (whole, eighth) = (value as usize, ((value.fract()) * 8.).round() as usize);
    let (whole, eighth) = if eighth == 8 {
        (whole + 1, 0)
    } else {
        (whole, eighth)
    };
    let edge = if eighth > 0 {
        whole
    } else {
        whole.saturating_sub(1)
    };
    for i in 0..cells {
        let zone = match i {
            _ if i >= at(threshold) => RED,
            _ if i >= at(70.) => AMBER,
            _ => GREEN,
        };
        let hot = pct.is_some_and(|p| p > 0.) && i == edge;
        let ink = mix(zone, WHITE, if hot { 1. - k } else { 0. });
        let (glyph, ink) = match i {
            _ if i < whole => ("█", ink),
            _ if i == whole && eighth > 0 => (EIGHTHS[eighth], ink),
            _ if i == at(threshold) => ("▏", RED),
            _ if i == at(70.) => ("▏", AMBER),
            _ => (" ", GRAY_DIM),
        };
        for y in r.y..r.bottom() {
            put(b, r.x + i as u16, y, glyph, 1, style(ink, BEZEL));
        }
    }
    let x = r.x + cells as u16 + 1;
    let ink = pct.map_or(GRAY, |p| tone(p, threshold));
    put(b, x, r.y, &top, words, bold(ink, GLASS));
    put(b, x, r.y + 1, &low, words, style(GRAY, GLASS));
}

// The meter column, its top lit cell `heat` of the way to white, and beside it the trend's reach
// over five turns.
fn meter(b: &mut Buffer, r: Rect, level: (Option<f64>, f64), threshold: f64, trend: Option<f64>) {
    let (pct, heat) = level;
    let h = r.height as usize;
    let (partial, edge, top) = fill_at(pct.unwrap_or(0.), h);
    let reach = pct.zip(trend).filter(|(_, t)| *t > 0.);
    let tip = reach.map_or(top, |(p, t)| fill_at(p + t * 5., h).2);
    let (hard, soft) = (fill_at(threshold, h).2, fill_at(70., h).2);
    for row in 0..h {
        let y = r.y + row as u16;
        let zone = match row {
            _ if row <= hard => RED,
            _ if row <= soft => AMBER,
            _ => GREEN,
        };
        let (glyph, tone) = match pct {
            Some(p) if p >= 100. || row > edge => ("█", zone),
            Some(_) if row == edge && partial > 0 => (RISE[partial], zone),
            _ => ("│", GRAY_DIM),
        };
        let hot = row == top && glyph != "│";
        let tone = mix(tone, WHITE, if hot { heat } else { 0. });
        put(b, r.x + 4, y, glyph, 1, style(tone, GLASS));
        if pct.is_some() && row < top && row >= tip {
            let arrow = if row == tip { "▲" } else { "│" };
            put(b, r.x + 5, y, arrow, 1, style(MAGENTA, GLASS));
        }
    }
}

// The numbers left of the meter and the marks right of it. Open: a label every 10 or 20 points,
// the current percentage and the rebuild line; folded: the ends, the current and the threshold.
fn scale(b: &mut Buffer, r: Rect, pct: Option<f64>, threshold: f64, folded: bool) {
    let h = r.height as usize;
    let (current, hard) = (pct.map(|p| fill_at(p, h).2), fill_at(threshold, h).2);
    let ink = pct.map_or(WHITE, |p| tone(p, threshold));
    let mut labels = std::collections::BTreeMap::new();
    for n in (0..=100).rev().step_by(if h >= 20 { 10 } else { 20 }) {
        labels.entry(fill_at(n as f64, h).2).or_insert(n);
    }
    for row in 0..h {
        let (y, here) = (r.y + row as u16, current == Some(row));
        if folded {
            let (number, tone) = match () {
                _ if here => (format!("{:>3}", pct.unwrap_or(0.) as u64), ink),
                _ if row == hard => (format!("{threshold:>3}"), RED),
                _ if row == 0 => ("100".into(), GRAY),
                _ if row + 1 == h && pct.is_some() => ("  0".into(), GRAY),
                _ if row + 1 == h => ("  —".into(), GRAY),
                _ => ("   ".into(), GRAY),
            };
            let look = style(tone, GLASS);
            let bold = look.add_modifier(Modifier::BOLD);
            put(b, r.x, y, &number, 3, if here { bold } else { look });
            let mark = match () {
                _ if row == hard => "◁",
                _ if here => "◀",
                _ => " ",
            };
            put(b, r.x + 6, y, mark, 1, look);
            continue;
        }
        let label = labels.get(&row);
        let number = label.map_or("   ".into(), |n| format!("{n:>3}"));
        let (tick, tone) = if label.is_some() {
            ("┤", WHITE)
        } else {
            ("│", GRAY)
        };
        put(b, r.x, y, &number, 3, style(tone, GLASS));
        put(b, r.x + 3, y, tick, 1, style(GRAY, GLASS));
        let rest = r.width.saturating_sub(8);
        match pct {
            Some(pct) if here => {
                put(b, r.x + 6, y, "◀", 1, style(ink, GLASS));
                let bold = bold(ink, GLASS);
                put(b, r.x + 7, y, &format!("{}%", pct as u64), rest, bold);
            }
            _ if row == hard => _ = put(b, r.x + 6, y, "◁REBUILD", rest + 1, style(RED, GLASS)),
            _ => {}
        }
    }
}
