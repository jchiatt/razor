//! The question band above the composer: whose ask it is when several wait, the question,
//! its numbered options, and how to answer in words or copy it.
use super::{
    App,
    text::*,
    view::{Action, Hit, PHONE},
};
use crate::protocol::{Ask, EntryId, SessionId};
use crate::terminal::bindings;
use ratatui::{buffer::Buffer, layout::Rect, style::Modifier, text::Span};

fn waiting(app: &App) -> usize {
    app.agents.values().map(|a| a.asks.len()).sum()
}

// Whether the band names the asker: the ask is another session's, or one of several.
fn named(app: &App, session: &SessionId) -> bool {
    app.focus.as_ref() != Some(session) || waiting(app) > 1
}

// The whole ask's height at `width`: the question, its options (two rows each below `PHONE`)
// and the keys, and a row naming the asker when the ask is another session's or one of several.
fn full(app: &App, ask: &Ask, width: u16) -> u16 {
    let options = ask.options.len().min(u16::MAX as usize) as u16;
    let tall = if width < PHONE { 2 } else { 1 };
    let question = wrapped(words(ask), width.saturating_sub(2 + CALL)).len() as u16;
    let rows = question
        .max(1)
        .saturating_add(1 + u16::from(named(app, &ask.id.0)));
    (options.saturating_mul(tall)).saturating_add(rows)
}

// The " YOUR CALL " label's columns; the question wraps in the columns after it.
const CALL: u16 = 11;

// A question with no words still says it is one.
fn words(ask: &Ask) -> &str {
    match ask.text.trim() {
        "" => "A question is waiting",
        _ => &ask.text,
    }
}

// The question's rows at `columns`: each one's words, as a range of its text.
fn wrapped(text: &str, columns: u16) -> Vec<std::ops::Range<usize>> {
    let mut rows = Vec::new();
    visit_wrap(&[Span::raw(text)], columns.into(), |parts, range, last| {
        if !last || !parts.is_empty() {
            let line = &text[range.clone()];
            let start = range.start + line.len() - line.trim_start().len();
            rows.push(start..range.start + line.trim_end().len());
        }
        true
    });
    rows
}

/// The band's height at `width` on a screen `height` rows tall: the whole ask, or one row when
/// that would take more than a third of the screen (at least three rows); an opened ask takes
/// up to half.
pub fn rows(app: &App, ask: &Ask, width: u16, height: u16) -> u16 {
    let full = full(app, ask, width);
    match app.ask_open.as_ref() == Some(&ask.id) {
        true => full.min((height / 2).max(3)),
        false if full > (height / 3).max(3) => 1,
        false => full,
    }
}

/// Draws the band; only an option shown in full is a click and number target.
pub fn draw(
    app: &App,
    b: &mut Buffer,
    r: Rect,
    (session, _, q): &(SessionId, EntryId, Ask),
) -> Vec<Hit> {
    let mut hits = Vec::new();
    if r.width < 3 || r.height == 0 {
        return hits;
    }
    fill(b, r, style(DATA, CYAN_BG));
    for y in r.y..r.bottom() {
        put(b, r.x, y, "│", 1, style(CYAN, CYAN_BG));
    }
    let rest = |x: u16| r.right().saturating_sub(x);
    let opened = app.ask_open.as_ref() == Some(&q.id) && r.height >= 3;
    if r.height < full(app, q, r.width) && !opened {
        return vec![folded(app, b, r, q)];
    }
    let name = app
        .agents
        .get(session)
        .map_or(session.as_str(), |a| &a.name);
    let mut y = r.y;
    if named(app, session) {
        asker(app, b, Rect { y, height: 1, ..r }, q, name);
        y += 1;
    }
    if y >= r.bottom() {
        return hits;
    }
    let x = put(b, r.x + 2, y, " YOUR CALL ", r.width - 2, bold(GLASS, CYAN));
    // The question wraps under itself; the options keep their rows and the last row it gets
    // ends in an ellipsis when more is left.
    let (text, tall) = (words(q), if r.width < PHONE { 2 } else { 1 });
    let lines = wrapped(text, rest(x));
    let spare = r.bottom().saturating_sub(y + 1);
    let options = (q.options.len() as u16).saturating_mul(tall);
    let rows = (lines.len() as u16)
        .min(spare.saturating_sub(options))
        .max(1);
    let link = address(text);
    for (dy, range) in (0..rows).zip(&lines) {
        let cut = dy + 1 == rows && lines.len() as u16 > rows;
        let end = if cut { text.len() } else { range.end };
        let words = clip(text[range.start..end].trim_end(), rest(x).into());
        let end = put(b, x, y + dy, &words, rest(x), bold(WHITE, CYAN_BG));
        // The ask's address, as far as it shows, is underlined and an OSC 8 link.
        if let Some((at, url)) = link
            && at < range.end
            && at + url.len() > range.start
        {
            let from = at.max(range.start);
            let start = x.saturating_add(width(&text[range.start..from]) as u16);
            let cells =
                (end.saturating_sub(start) as usize).min(width(&text[from..at + url.len()]));
            let cell = Rect::new(start, y + dy, cells as u16, 1);
            b.set_style(cell, Modifier::UNDERLINED);
            hits.extend((cells > 0).then(|| (cell, Action::Link(url.into()))));
        }
    }
    let shown = spare.saturating_sub(rows) / tall;
    options_rows(app, b, r, q, (y + rows, shown, tall), &mut hits);
    hits.extend(keys(app, b, r, q, name));
    hits
}

// Without room for the whole ask, one row: the question and how to answer it in words, which
// a click opens to as many options as half the screen holds.
fn folded(app: &App, b: &mut Buffer, r: Rect, q: &Ask) -> Hit {
    let rest = |x: u16| r.right().saturating_sub(x);
    let more = match waiting(app).saturating_sub(1) {
        0 => String::new(),
        more => format!(" · +{more}"),
    };
    let tag = format!("  {} options{more} · /answer ▸", q.options.len());
    let x = put(b, r.x + 2, r.y, "? ", 2, bold(CYAN, CYAN_BG));
    let room = rest(x).saturating_sub(width(&tag) as u16);
    let words = words(q).split_whitespace().collect::<Vec<_>>().join(" ");
    let words = clip(&words, room.into());
    let x = put(b, x, r.y, &words, room, bold(WHITE, CYAN_BG));
    put(b, x, r.y, &tag, rest(x), style(GRAY, CYAN_BG));
    (Rect::new(r.x, r.y, r.width, 1), Action::Ask(q.id.clone()))
}

// The asker's row in `r`: whose ask it is and which of how many wait, the name cut so the count
// always shows; `←→ next` only with more than one waiting, and never on a 40-column phone.
fn asker(app: &App, b: &mut Buffer, r: Rect, q: &Ask, name: &str) {
    let rest = |x: u16| r.right().saturating_sub(x);
    let count = waiting(app);
    let at = app.asks().iter().position(|a| a.id == q.id).unwrap_or(0) + 1;
    let next = match count > 1 && r.width >= 42 {
        true => " · <question.previous>/<question.next> next",
        false => "",
    };
    let count = bindings::text(app, &format!(" · {at} of {count}{next}"));
    let room = (r.width as usize).saturating_sub(2 + width(&count) + 5);
    let asks = format!("{} asks", clip(name, room));
    let x = put(b, r.x + 2, r.y, &asks, r.width - 2, bold(CYAN, CYAN_BG));
    put(b, x, r.y, &count, rest(x), style(GRAY, CYAN_BG));
}

// The options from the row after `y`, `shown` of them `tall` rows each: on a phone each is a
// two-row stripe, the grounds alternating, its label wrapped. A clipped label cannot sign an
// option the person has not seen in full: a click on it says so.
fn options_rows(
    app: &App,
    b: &mut Buffer,
    r: Rect,
    q: &Ask,
    (y, shown, tall): (u16, u16, u16),
    hits: &mut Vec<Hit>,
) {
    let rest = |x: u16| r.right().saturating_sub(x);
    for (n, option) in (0..).zip(&q.options).take(shown.into()) {
        let (y, mut dy, mut full) = (y + n * tall, 0, false);
        let striped = mix(YOU_BG, CYAN_BG, 0.5 * f64::from(n % 2));
        let ground = [CYAN_BG, striped][usize::from(tall > 1)];
        let stripe = Rect::new(r.x + 1, y, r.width - 1, tall);
        fill(b, stripe, style(DATA, ground));
        let number = format!(" {} ", bindings::hint(app, &format!("question.{}", n + 1)));
        let x = put(
            b,
            r.x + 2,
            y,
            &number,
            width(&number) as u16,
            bold(CYAN, YOU_BG),
        );
        let label = [Span::raw(option.as_str())];
        visit_wrap(&label, rest(x).into(), |_, range, last| {
            let words = match dy + 1 == tall {
                true => clip(option[range.start..].trim(), rest(x).into()),
                false => option[range].trim().to_owned(),
            };
            put(b, x, y + dy, &words, rest(x), bold(WHITE, ground));
            (dy, full) = (dy + 1, last);
            dy < tall && !last
        });
        let action =
            [Action::Cut, Action::Option(q.id.clone(), n.into())][usize::from(full)].clone();
        hits.push((Rect::new(r.x, y, r.width, tall), action));
    }
}

// The band's last row: while its answer is written in the editor, that it is; else how to
// answer in words, and the copy button, which takes the ask's address or its words.
fn keys(app: &App, b: &mut Buffer, r: Rect, q: &Ask, name: &str) -> Option<Hit> {
    let rest = |x: u16| r.right().saturating_sub(x);
    let y = r.bottom() - 1;
    if app.answering.as_ref() == Some(&q.id) {
        let words = format!(
            "answering {name} in words · <composer.send> sends to {name} only · <composer.back> leaves"
        );
        let words = bindings::text(app, &words);
        put(b, r.x + 2, y, &words, r.width - 2, bold(CYAN, CYAN_BG));
        return None;
    }
    let keys = match r.width >= 72 {
        true => "/answer words · <composer.send> to answer in words · ",
        false => "/answer words · ",
    };
    let keys = bindings::text(app, keys);
    let x = put(b, r.x + 2, y, &keys, r.width - 2, style(GRAY, CYAN_BG));
    put(b, x, y, "⎘ copy", rest(x), style(CYAN, CYAN_BG));
    let words = address(&q.text).map_or(&q.text[..], |(_, url)| url);
    (x + 6 <= r.right()).then(|| (Rect::new(x, y, 6, 1), Action::Copy(words.into())))
}

/// An ask's one web address and where it starts in its words, without trailing punctuation;
/// none when the words hold no address or several.
fn address(text: &str) -> Option<(usize, &str)> {
    let mut words = text.split_whitespace().filter(|w| w.contains("://"));
    let word = words.next().filter(|_| words.next().is_none())?;
    let url = word.trim_end_matches([';', ',', '.', ')']);
    url.starts_with("http")
        .then(|| (text.find(word).unwrap_or(0), url))
}
