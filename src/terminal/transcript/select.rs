//! A drag over the drawn transcript, or a double or triple click: the words it covers lit on
//! their own ground, copied as the rows wrote them (a wrapped row run on into the next), and
//! offered to the editor as a quote.
use super::{Row, Transcript};
use crate::terminal::{App, clipboard, markdown::Kind, motion, text::*, view::Action};
use ratatui::{buffer::Buffer, layout::Rect, style::Color};
use unicode_segmentation::UnicodeSegmentation;

// The selection ground keeps every ink legible, unlike a reversed cell.
const LIT: Color = rgb(0x144f68);

/// A press in the transcript: where it went down, where the pointer is, whether it has moved
/// off that cell, and the click it is if it never does.
pub(in crate::terminal) struct Selection {
    anchor: (u16, u16),
    at: (u16, u16),
    dragged: bool,
    click: Option<Action>,
    // When its words reached the clipboard, and the words: it stays lit with the quote chip
    // until the next key, press or scroll, or until the rows under it move.
    copied: Option<(u64, String)>,
    // The transcript's first drawn row when it was made.
    top: usize,
}

// The covered words of one row: its screen row, the columns lit, and the words there.
struct Stretch {
    y: u16,
    lit: (u16, u16),
    text: String,
}

impl Transcript {
    // Drawn row `i` and its words after its lead; none on a frame row, which a copy leaves out.
    fn text(&self, i: usize) -> Option<(&Row, String)> {
        let row = self.rows.get(i).filter(|row| row.kind != Kind::Frame)?;
        let text: String = row.words.spans.iter().map(|s| s.content.as_ref()).collect();
        let mut used = 0;
        let skipped = text.graphemes(true).skip_while(|g| {
            used += width(g);
            used <= usize::from(row.lead)
        });
        Some((row, skipped.collect::<String>().trim_end().to_owned()))
    }

    // The cells of the word under `at`, as the pair `words` reads: the run of non-blank cells
    // there, quotes, brackets and backticks trimmed, and a trailing `.`, `,`, `;` or `:`.
    fn word(&self, at: (u16, u16)) -> Option<((u16, u16), (u16, u16))> {
        let (r, top) = self.area?;
        let (row, text) = self.text(usize::from(at.1.checked_sub(top)?))?;
        let (open, close) = (
            |c| "\"'`([{<“‘".contains(c),
            |c| "\"'`)]}>”’.,;:".contains(c),
        );
        let mut x = r.x + row.lead;
        for piece in text.split(' ') {
            let (core, w) = (piece.trim_start_matches(open), width(piece) as u16);
            if (x..x + w).contains(&at.0) {
                let from = x + width(&piece[..piece.len() - core.len()]) as u16;
                let core = core.trim_end_matches(close);
                return (!core.is_empty())
                    .then(|| ((from, at.1), (from + width(core) as u16 - 1, at.1)));
            }
            x += w + 1;
        }
        None
    }

    // The cells of the paragraph under `at`: the rows above and below with its lead and ground,
    // up to a blank row; in a code block, the whole block, its blank lines with it.
    fn paragraph(&self, at: (u16, u16)) -> Option<((u16, u16), (u16, u16))> {
        let (r, top) = self.area?;
        let i = usize::from(at.1.checked_sub(top)?);
        let code = |row: &Row| matches!(row.kind, Kind::Code(_));
        let (row, _) = self
            .text(i)
            .filter(|(row, text)| !text.is_empty() || code(row))?;
        let same = |j: &usize| {
            let next = self
                .text(*j)
                .filter(|(n, _)| n.lead == row.lead && n.bg == row.bg);
            next.is_some_and(|(n, text)| if code(row) { code(n) } else { !text.is_empty() })
        };
        let first = (0..i).rev().take_while(same).last().unwrap_or(i) as u16;
        let last = (i + 1..self.rows.len())
            .take_while(same)
            .last()
            .unwrap_or(i) as u16;
        Some(((r.x, top + first), (r.right(), top + last)))
    }

    // The rows between `a` and `b` in reading order: each row's covered words, `None` for a
    // blank row inside the drag, and whether the row runs on into the next.
    fn stretches(&self, a: (u16, u16), b: (u16, u16)) -> Vec<(Option<Stretch>, bool)> {
        let Some((r, top)) = self.area else {
            return Vec::new();
        };
        let ((x0, y0), (x1, y1)) = match (a.1, a.0) <= (b.1, b.0) {
            true => (a, b),
            false => (b, a),
        };
        let words = |i| self.text(i);
        let mut out = Vec::new();
        for y in y0.max(top)..=y1.min(r.bottom().saturating_sub(1)) {
            let i = usize::from(y - top);
            let Some((row, text)) = words(i) else {
                continue;
            };
            // The wrap broke here when the next row's first word would not have fit after it.
            let next = words(i + 1).filter(|(n, _)| n.lead == row.lead && n.bg == row.bg);
            let first = next
                .as_ref()
                .and_then(|(_, t)| t.split(' ').next())
                .unwrap_or("");
            let room = usize::from(r.width.saturating_sub(2 + row.lead));
            let wraps = !matches!(row.kind, Kind::Cells(_) | Kind::Code(_));
            let soft = wraps && !first.is_empty() && width(&text) + 1 + width(first) > room;
            let start = r.x + row.lead;
            let from = if y == y0 { x0.max(start) } else { start };
            let to = if y == y1 { x1 + 1 } else { u16::MAX };
            let end = start + width(&text) as u16;
            if text.is_empty() {
                out.push((None, false));
            } else if from < end.min(to) {
                // Each grapheme with its column in the row.
                let cells = text.graphemes(true).scan(row.lead, |at, g| {
                    let x = *at;
                    *at += width(g) as u16;
                    Some((x, g))
                });
                let covered = cells.filter(|g| (from..to.min(end)).contains(&(r.x + g.0)));
                let text = read(&row.kind, covered);
                let lit = (from, to.min(end + u16::from(soft && to > end)));
                out.push((Some(Stretch { y, lit, text }), soft && to > end));
            }
        }
        out
    }

    /// The words between two screen cells, as written: rows the wrap broke joined by a space,
    /// other rows by a newline, blank rows at either end left off. Only drawn rows are read.
    fn words(&self, a: (u16, u16), b: (u16, u16)) -> Option<String> {
        let mut out = String::new();
        let mut on = None;
        for (stretch, soft) in self.stretches(a, b) {
            match on {
                Some(true) => out.push(' '),
                Some(false) => out.push('\n'),
                None => {}
            }
            out.push_str(stretch.as_ref().map_or("", |s| s.text.as_str()));
            on = Some(soft);
        }
        let out = out.trim_matches('\n');
        (!out.trim().is_empty()).then(|| out.to_owned())
    }
}

// The words a copy takes from a row's covered cells, each with its column: a border parts a
// table's cells with ` | ` and ends a card's body, and a tab's two cells are the tab again.
fn read<'a>(kind: &Kind, cells: impl Iterator<Item = (u16, &'a str)>) -> String {
    match kind {
        Kind::Cells(bars) => {
            let marked = cells.map(|(x, g)| if bars.contains(&x) { "\u{1f}" } else { g });
            let marked: String = marked.collect();
            let parts: Vec<&str> = marked.split('\u{1f}').map(str::trim).collect();
            // What lies before the first border and after the last is no cell.
            let parts = match parts.as_slice() {
                ["", inside @ .., ""] | ["", inside @ ..] | [inside @ .., ""] => inside,
                all => all,
            };
            parts.join(" | ")
        }
        Kind::Code(tabs) => cells
            .filter(|(x, _)| !tabs.contains(&x.wrapping_sub(1)))
            .map(|(x, g)| if tabs.contains(&x) { "\t" } else { g })
            .collect(),
        _ => cells.map(|(_, g)| g).collect(),
    }
}

/// A left press at `at`: starts a selection when it lands on the drawn transcript, keeping the
/// row's click for the release. False leaves the press to the hit under it. Any press but the
/// quote chip's drops the selection shown.
pub(in crate::terminal) fn press(app: &mut App, at: (u16, u16), click: Option<Action>) -> bool {
    if matches!(click, Some(Action::Quote)) {
        return false;
    }
    app.selection = None;
    let inside = |t: &Transcript| t.area.is_some_and(|(r, _)| r.contains(at.into()));
    let pane = app.status_area.is_some_and(|r| r.contains(at.into()));
    let row = click.as_ref().is_none_or(|a| {
        matches!(
            a,
            Action::Call(..) | Action::CopyText(..) | Action::Child(_)
        )
    });
    let Some(t) = app.transcript().filter(|t| !pane && row && inside(t)) else {
        return false;
    };
    let (dragged, copied, top) = (false, None, t.first_row);
    app.selection = Some(Selection {
        anchor: at,
        at,
        dragged,
        click,
        copied,
        top,
    });
    true
}

/// The pointer moved with the button down.
pub(in crate::terminal) fn drag(app: &mut App, at: (u16, u16)) {
    if let Some(s) = app.selection.as_mut() {
        s.dragged |= s.anchor != at;
        s.at = at;
    }
}

/// The button came up. A drag copies its words. A press that never moved is its click; on a row
/// without one, a second click on the same cell within 400 ms copies the word there, and a third
/// the rows of its paragraph, up to a blank row. The copy stays lit with its quote chip.
pub(in crate::terminal) fn release(app: &mut App, at: (u16, u16), now: u64) -> Option<Action> {
    drag(app, at);
    let mut s = app.selection.take()?;
    let clicks = match app.clicks {
        (last, cell, n) if cell == at && now < last + 400 => n.saturating_add(1),
        _ => 1,
    };
    app.clicks = (now, at, clicks);
    let t = app.transcript()?;
    if !s.dragged {
        let span = match (&s.click, clicks) {
            (Some(_), _) | (None, 1) => return s.click,
            (None, 2) => t.word(at),
            _ => t.paragraph(at),
        };
        (s.anchor, s.at) = span?;
    }
    let Some(words) = t.words(s.anchor, s.at) else {
        app.say("Those cells hold no text of their own to copy.");
        return None;
    };
    clipboard::copy(app, Some(words.clone()));
    s.copied = Some((now, words));
    app.selection = Some(s);
    None
}

/// Lights the selection. A copied one starts brighter and settles over 330 ms while motion
/// plays, and carries the quote chip after its last row's words; rows that moved drop it.
pub(in crate::terminal) fn paint(app: &mut App, b: &mut Buffer, now: u64) {
    // A press alone lights nothing: only a drag, or a word or paragraph a click copied.
    let lit = |s: &&Selection| s.dragged || s.copied.is_some();
    let (Some(s), Some(t)) = (app.selection.as_ref().filter(lit), app.transcript()) else {
        return;
    };
    if s.copied.is_some() && t.first_row != s.top {
        return app.selection = None;
    }
    let flash = s.copied.as_ref().filter(|_| motion::on(app));
    let k = flash.map_or(0., |(at, _)| {
        1. - motion::ease_out(motion::progress(now, *at, 330))
    });
    let (light, mut last) = (mix(LIT, CYAN, 0.6 * k), None);
    for stretch in t.stretches(s.anchor, s.at).into_iter().flat_map(|(s, _)| s) {
        for x in stretch.lit.0..stretch.lit.1 {
            if let Some(cell) = b.cell_mut((x, stretch.y)) {
                cell.set_bg(light);
            }
        }
        last = Some(stretch.y);
    }
    // After the last row's words when they leave room, else over the end of its ground.
    let (r, top) = t.area.unwrap_or_default();
    let chip = last.filter(|_| s.copied.is_some()).map(|y| {
        let words = t.text(usize::from(y - top));
        let end = words.map_or(r.x, |(row, text)| r.x + row.lead + width(&text) as u16);
        let x = end.min(r.right().saturating_sub(12)) + 1;
        put(b, x, y, " ❝ quote ", 9, style(CYAN, BEZEL_HI));
        (Rect::new(x, y, 9, 1), Action::Quote)
    });
    app.hits.extend(chip);
    if k > 0. {
        app.next.at(now);
    }
}

/// The quote chip: the copied words into the editor, each line after `> `, the cursor on the
/// line below them, after a blank line when the editor has words.
pub(in crate::terminal) fn quote(app: &mut App) {
    let Some((_, words)) = app.selection.take().and_then(|s| s.copied) else {
        return;
    };
    let lead = ["\n\n", ""][usize::from(app.composer.text.is_empty())];
    let quoted: String = words.lines().map(|l| format!("> {l}\n")).collect();
    if app.composer.append(&format!("{lead}{quoted}")) {
        app.edited();
    }
}
