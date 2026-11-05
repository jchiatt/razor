//! The shortcode strip: while a `:code` is typed, one row of the emoji whose shortcodes match;
//! Tab or Enter takes the first, Tab again the next, Esc hides it, and a typed closing `:`
//! turns a whole code into its emoji at once.
use super::super::{
    App,
    text::*,
    view::{Action, Hit},
};
use super::emoji::{LIT, glyph, record};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{buffer::Buffer, layout::Rect};
use std::{cell::Cell, ops::Range};

/// The strip's memory: the cycle under way, the draft Esc hid it on, and whether it has shown
/// since the draft was last empty, which keeps its row so the editor does not jump.
#[derive(Default)]
pub struct Codes {
    cycle: Option<Cycle>,
    hidden: Option<String>,
    held: Cell<bool>,
}

// The matches, the one now in the draft (past the last: the typed code again), its range,
// and the draft it left.
struct Cycle {
    found: Found,
    index: usize,
    range: Range<usize>,
    after: String,
}

// The `:code` before the cursor, its letters, and at most five emoji with the code that matched.
#[derive(Clone)]
struct Found {
    range: Range<usize>,
    query: String,
    options: Vec<(usize, &'static str)>,
}

fn code_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-')
}

// The colon and the code's first byte, when a `:` starts the run of code characters before
// `cursor` and follows whitespace or the draft's start.
fn code_before(text: &str, cursor: usize) -> Option<(usize, usize)> {
    let start = text[..cursor].trim_end_matches(code_char).len();
    let colon = start
        .checked_sub(1)
        .filter(|&c| text.as_bytes()[c] == b':')?;
    text[..colon]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace)
        .then_some((colon, start))
}

fn found(app: &App) -> Option<Found> {
    let (text, cursor) = (&app.composer.text, app.composer.cursor());
    if text.starts_with('/') || text[cursor..].starts_with(char::is_alphanumeric) {
        return None;
    }
    let (colon, start) = code_before(text, cursor)?;
    let query = text[start..cursor].to_lowercase();
    if !query.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    // Codes starting with the letters (an exact one first, then the more used, then shorter),
    // then codes containing them.
    let rank = |g: &str| {
        app.used
            .recent
            .iter()
            .position(|r| r == g)
            .unwrap_or(usize::MAX)
    };
    let mut ranked: Vec<(bool, bool, usize, usize, usize, &'static str)> = Vec::new();
    for (i, e) in app.emoji.emoji.iter().enumerate() {
        let prefix = e.codes.iter().find(|c| c.starts_with(&query));
        let code = prefix.or_else(|| e.codes.iter().find(|c| c.contains(&query[..])));
        if let Some(&code) = code {
            let len = if prefix.is_some() { code.len() } else { 0 };
            ranked.push((prefix.is_none(), code != query, rank(e.glyph), len, i, code));
        }
    }
    ranked.sort();
    let options: Vec<_> = ranked.into_iter().take(5).map(|r| (r.4, r.5)).collect();
    let range = colon..cursor;
    (!options.is_empty()).then_some(Found {
        range,
        query,
        options,
    })
}

fn current(app: &App) -> Option<(Found, Option<&Cycle>)> {
    if app.grid.is_some() {
        return None;
    }
    let (text, cursor, codes) = (&app.composer.text, app.composer.cursor(), &app.codes);
    let live = |c: &&Cycle| &c.after == text && cursor == c.range.end;
    let cycling = codes.cycle.as_ref().filter(live);
    let found = match &cycling {
        Some(c) => Some(c.found.clone()),
        None if codes.hidden.as_ref() == Some(text) => None,
        None => found(app),
    }?;
    Some((found, cycling))
}

/// The `:code` being typed and the range it spans, for the grid to open searching for it.
pub fn typed(app: &App) -> Option<(String, Range<usize>)> {
    current(app).map(|(f, _)| (f.query, f.range))
}

/// One row while a `:code` has matches, and after, until the draft is empty; none while the
/// grid is open.
pub fn rows(app: &App) -> u16 {
    let shown = current(app).is_some();
    let held = &app.codes.held;
    held.set(!app.composer.text.is_empty() && (held.get() || shown));
    u16::from(app.grid.is_none() && (shown || held.get()))
}

/// Tab and Enter take the first match (Tab again the next), Esc hides the strip, and a typed
/// `:` closes a whole code into its emoji. False leaves the key to the editor.
pub fn key(app: &mut App, k: KeyEvent) -> bool {
    if k.code == KeyCode::Char(':') && close(app) {
        return true;
    }
    let Some((_, cycling)) = current(app) else {
        return false;
    };
    match k.code {
        KeyCode::Tab => take(app, None),
        KeyCode::Enter if cycling.is_none() => take(app, None),
        KeyCode::Esc => {
            (app.codes.hidden, app.codes.cycle) = (Some(app.composer.text.clone()), None);
            true
        }
        _ => false,
    }
}

/// A click on match `n` puts it in the draft in place of the code.
pub fn click(app: &mut App, n: usize) {
    take(app, Some(n));
}

fn take(app: &mut App, index: Option<usize>) -> bool {
    let Some((found, cycling)) = current(app) else {
        return false;
    };
    let range = cycling.map_or(&found.range, |c| &c.range).clone();
    let index =
        index.unwrap_or_else(|| cycling.map_or(0, |c| (c.index + 1) % (found.options.len() + 1)));
    let option = found.options.get(index).copied();
    let with = match option {
        Some((i, _)) => glyph(&app.emoji, &app.used.tones, i).to_owned(),
        None => format!(":{}", found.query),
    };
    let start = range.start;
    app.composer.swap(range, &with);
    let after = app.composer.text.clone();
    app.codes.cycle = Some(Cycle {
        found,
        index,
        range: start..start + with.len(),
        after,
    });
    app.edited();
    if let Some((i, _)) = option {
        record(app, app.emoji.emoji[i].glyph, None);
    }
    true
}

// A typed `:` after a whole known code: the code becomes its emoji.
fn close(app: &mut App) -> bool {
    let (text, cursor) = (&app.composer.text, app.composer.cursor());
    let Some((colon, start)) = code_before(text, cursor).filter(|(_, s)| *s < cursor) else {
        return false;
    };
    let code = text[start..cursor].to_lowercase();
    let Some(&i) = app
        .emoji
        .codes
        .get(code.as_str())
        .filter(|_| !text.starts_with('/'))
    else {
        return false;
    };
    app.composer
        .swap(colon..cursor, glyph(&app.emoji, &app.used.tones, i));
    app.codes.cycle = None;
    app.edited();
    record(app, app.emoji.emoji[i].glyph, None);
    true
}

/// Draws the strip: the keys, each match as a chip (the one Tab takes lit), and `esc hides`
/// where it fits; a held row with nothing to show is blank.
pub fn draw(app: &App, b: &mut Buffer, r: Rect, hits: &mut Vec<Hit>) {
    let Some((found, cycling)) = current(app).filter(|_| r.height > 0) else {
        return;
    };
    fill(b, r, style(DATA, CYAN_BG));
    let keys = "Emoji › ";
    let room = r.width as i64 - 2 - width(keys) as i64;
    let x = put(b, r.x, r.y, "│", 1, style(CYAN, CYAN_BG)) + 1;
    let mut x = put(b, x, r.y, keys, r.width, bold(CYAN, CYAN_BG));
    let (lit, mut used) = (cycling.map_or(0, |c| c.index), 0);
    for (n, &(i, code)) in found.options.iter().enumerate() {
        let text = glyph(&app.emoji, &app.used.tones, i);
        let need = (width(text) + code.len() + 6) as i64;
        if n > 0 && used + need > room {
            break;
        }
        let label = match n == 0 && need > room {
            true => clip(
                &format!(":{code}:"),
                (room - width(text) as i64 - 4).max(1) as usize,
            ),
            false => format!(":{code}:"),
        };
        let bg = if n == lit { LIT } else { BUTTON };
        let at = x;
        x = put(
            b,
            x,
            r.y,
            &format!(" {text} "),
            r.right() - x,
            style(WHITE, bg),
        );
        let ink = match n == lit {
            true => bold(WHITE, bg),
            false => style(DATA, bg),
        };
        x = put(b, x, r.y, &format!("{label} "), r.right() - x, ink) + 1;
        hits.push((Rect::new(at, r.y, x - 1 - at, 1), Action::Code(n)));
        used += (width(text) + width(&label) + 4) as i64;
    }
}
