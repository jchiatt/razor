//! A question to the person as the transcript draws it: two rows while
//! the band shows it (Left and Right move the band, and these rows with it), the read-only card
//! with its numbered options while it waits behind another, and the road taken once an input
//! answers it.
use super::{Row, Transcript};
use crate::protocol::{Body, Entry, Input, Origin};
use crate::terminal::{motion, text::*, tools::Next, view::PHONE};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

// The waiting card unrolls over this long from its call's time, and an answer lights the road
// taken over this long from its own.
const ARRIVE: u64 = 520;
const ANSWER: u64 = 900;

type Segs = Vec<Span<'static>>;

/// One ask as the transcript knows it: its words and options, and where it stands.
pub(super) struct Asked<'a> {
    text: &'a str,
    options: Vec<&'a str>,
    state: State<'a>,
    at: u64,
}
enum State<'a> {
    First,
    Waiting,
    // The answer's words, and when its input was recorded.
    Answered(&'a str, u64),
}
// What a card row is, for its thread and the air under the header.
#[derive(Clone, Copy, PartialEq)]
enum Part {
    Header,
    Asker,
    Asked,
    Other,
}

impl Transcript {
    /// `e` as a question to the person: open in the session's status row, or answered by a
    /// later input; none for any other call, which draws as its tool row.
    pub(super) fn asked<'a>(&'a self, e: &'a Entry) -> Option<Asked<'a>> {
        let Body::Call { name, input, .. } = &e.body else {
            return None;
        };
        // Razor's own question, its card drawn from the call's decoded input.
        if crate::record::ask(name, input).is_none() || input["type"] != "question" {
            return None;
        }
        let answer = self.entries.range(e.id..).find_map(|(_, a)| match &a.body {
            Body::Input(Input {
                from: Origin::Answer { ask },
                text,
                ..
            }) if *ask == e.id => Some((text.as_str(), a.at)),
            _ => None,
        });
        let state = match (answer, self.asks.contains(&e.id)) {
            (Some((text, at)), _) => State::Answered(text, at),
            (None, true) if self.band == Some(e.id) => State::First,
            (None, true) => State::Waiting,
            (None, false) => return None,
        };
        let options = input["options"].as_array().into_iter().flatten();
        Some(Asked {
            text: input["text"].as_str().unwrap_or_default(),
            options: options.filter_map(|o| o.as_str()).collect(),
            state,
            at: e.at,
        })
    }
}

/// When a card next looks different by itself: the next paced frame while it unrolls, or
/// while its answer's light runs.
pub(super) fn next(a: &Asked, now: u64, motion: bool) -> Next {
    let look = match (motion, &a.state) {
        (true, State::Waiting) if now.saturating_sub(a.at) < ARRIVE => now + 1,
        (true, State::Answered(_, at)) if now.saturating_sub(*at) < ANSWER => now + 1,
        _ => u64::MAX,
    };
    Next {
        look,
        ..Next::NEVER
    }
}

/// The ask's rows at `columns` for the model `name` asking, at `now`; without motion a card is
/// drawn whole at once.
pub(super) fn rows(a: &Asked, name: &str, columns: usize, now: u64, motion: bool) -> Vec<Row> {
    let now = if motion { now } else { u64::MAX };
    let room = columns.saturating_sub(2);
    let lines = match a.state {
        State::First => record(a, name, room),
        _ => card(a, name, room, now),
    };
    let row = |l: Segs| {
        Row::plain(Line::from(
            [vec![Span::raw("  ")], clip_spans(l, room)].concat(),
        ))
    };
    lines.into_iter().map(row).collect()
}

// While the band offers the options, the transcript keeps who asked and what waits.
fn record(a: &Asked, name: &str, room: usize) -> Vec<Segs> {
    let right = clip_spans(vec![ink(" waiting", CYAN)], room);
    let head = vec![tab(" "), plain(" "), ink(name, DATA), ink(" asks", GRAY)];
    let mut first = paint(head, room - w(&right), None);
    first.extend(right);
    let dim = mix(CYAN, GLASS, 0.45);
    let text = clip(a.text, room.saturating_sub(2));
    let second = vec![ink("◆ ", dim), ink(text, DATA)];
    vec![first, second]
}

// The card beside its thread, a column of its ground each side, and a row of air under the
// header while it waits; a phone card of three options that passes twelve rows drops the
// columns of ground and the air.
fn card(a: &Asked, name: &str, room: usize, now: u64) -> Vec<Segs> {
    let open = !matches!(a.state, State::Answered(..));
    let budget = match room < usize::from(PHONE) && a.options.len() == 3 {
        true => 12,
        false => usize::MAX,
    };
    let mut lines = body(a, name, room.saturating_sub(4), now);
    let padded = lines.len() <= budget;
    if !padded {
        lines = body(a, name, room.saturating_sub(2), now);
    }
    let air = open && lines.len() < budget;
    let inner = room.saturating_sub(2);
    let mut out = Vec::new();
    let mut prev: Option<(Part, &Option<Segs>)> = None;
    for (i, (part, line)) in lines.iter().enumerate() {
        let header = |p: Part| matches!(p, Part::Header | Part::Asker);
        if let Some((_, shown)) = prev.filter(|(p, _)| air && header(*p))
            && !header(*part)
        {
            let mut gap = thread(i, Part::Other, open);
            if let (Some(_), Some(g)) = (line, shown.as_ref().and_then(|l| ground(l))) {
                gap.push(Span::styled(" ".repeat(inner), style(DATA, g)));
            }
            out.push(gap);
        }
        let mut row = thread(i, *part, open);
        let body = line.clone().unwrap_or_default();
        match ground(&body).filter(|_| padded) {
            Some(g) => {
                let fill = inner.saturating_sub(w(&body) + 1);
                row.push(Span::styled(" ", style(DATA, g)));
                row.extend(body);
                row.push(Span::styled(" ".repeat(fill), style(DATA, g)));
            }
            None if padded => row.extend([plain(" ")].into_iter().chain(body)),
            None => row.extend(body),
        }
        out.push(row);
        prev = Some((*part, line));
    }
    out
}

// The thread beside a card: the loom stops for the person, then carries on green.
fn thread(i: usize, part: Part, open: bool) -> Segs {
    match (open, part) {
        (true, Part::Header) if i == 0 => vec![bold("◇", CYAN), plain(" ")],
        (true, _) => vec![ink("┆", mix(CYAN, GLASS, 0.35)), plain(" ")],
        (false, Part::Asked) => vec![ink("├─", GREEN)],
        (false, _) => vec![ink("│", GREEN), plain(" ")],
    }
}

// The card `cw` wide: the road taken once answered, else the open card unrolling from the
// top, its leading row a step brighter, each row faded in from the glass. `None` is a row the
// card has not reached yet.
fn body(a: &Asked, name: &str, cw: usize, now: u64) -> Vec<(Part, Option<Segs>)> {
    if let State::Answered(answer, at) = a.state {
        let p = motion::progress(now, at, ANSWER);
        let mut rows = folded(a, name, answer, cw);
        // How long it waited, at the right of the road while the room holds it.
        let waited = crate::terminal::status::elapsed(at.saturating_sub(a.at));
        let meta = format!("waited {waited}");
        if let Some((_, road)) = rows.get_mut(1)
            && w(road) + width(&meta) + 2 <= cw
        {
            let pad = " ".repeat(cw - w(road) - width(&meta));
            road.extend([plain(pad), ink(meta, GRAY)]);
        }
        let rows = rows.into_iter().enumerate();
        let lit = |(i, (part, s)): (usize, (Part, Segs))| {
            let (road, s) = (w(&s) + 2, paint(s, cw, None));
            (part, Some(if p < 1. { lit(i, s, p, road) } else { s }))
        };
        return rows.map(lit).collect();
    }
    let rows = open(a, name, cw);
    let total = rows.len() as f64 + 1.0;
    let front = match now.checked_sub(a.at).filter(|t| *t < ARRIVE) {
        Some(t) => (1.0 - (1.0 - t as f64 / ARRIVE as f64).powi(3)) * total,
        None => total,
    };
    let row = |(i, (part, segs)): (usize, (Part, Segs))| {
        let k = (front - i as f64).clamp(0.0, 1.0);
        let ground = mix(CYAN_BG, YOU_BG, 1.0 - k);
        let faded = paint(segs, cw, Some(ground)).into_iter().map(|s| {
            let fg = s.style.fg.map(|fg| mix(ground, fg, k));
            let bg = s.style.bg.map(|bg| mix(GLASS, bg, k));
            Span::styled(s.content, Style { fg, bg, ..s.style })
        });
        (part, (k > 0.0).then(|| faded.collect()))
    };
    rows.into_iter().enumerate().map(row).collect()
}

// Row `i` of the road taken at `p` of its answer's light: on the road, the light's head eases
// along its `road` cells over 0.7, the cells ahead dim and those within two of it lifted toward
// white; the roads not taken surface from 0.4; who asked stays as it was.
fn lit(i: usize, segs: Segs, p: f64, road: usize) -> Segs {
    let head = motion::ease_out(p / 0.7) * road as f64;
    let mut cells = Vec::new();
    for s in segs {
        for ch in s.content.chars() {
            let (fg, d) = (s.style.fg.unwrap_or(DATA), cells.len() as f64 - head);
            let fg = match i {
                0 => fg,
                1 if d > 2. => mix(GLASS, fg, 0.3),
                1 => mix(fg, WHITE, 0.7 * (1. - d.abs() / 2.).max(0.)),
                _ => mix(GLASS, fg, motion::ease_out((p - 0.4) / 0.6)),
            };
            cells.push(Span::styled(ch.to_string(), s.style.fg(fg)));
        }
    }
    cells
}

// The open card, read only: the tab and who asks, the question, the numbered options, and
// that they are read only here.
fn open(a: &Asked, name: &str, cw: usize) -> Vec<(Part, Segs)> {
    let wide = cw >= usize::from(PHONE);
    let pad = cw.saturating_sub(1).min(if wide { 2 } else { 1 });
    let label = pad + 3 + if wide { 2 } else { 1 };
    let indent = |n: usize| plain(" ".repeat(n));
    let tab = tab(&" ".repeat(pad));
    let asker = |verb: bool| {
        [vec![bold(name, WHITE)], vec![ink(" asks", GRAY)]][..1 + usize::from(verb)].concat()
    };
    let room = cw.saturating_sub(w(std::slice::from_ref(&tab)) + 1);
    let mut rows = match [asker(true), asker(false)]
        .into_iter()
        .find(|l| wide || w(l) <= room)
    {
        Some(l) => vec![(Part::Header, [vec![tab, plain(" ")], l].concat())],
        None => vec![
            (Part::Header, vec![tab]),
            (Part::Asker, [vec![indent(pad)], asker(true)].concat()),
        ],
    };
    let words = cw.saturating_sub(pad + if wide { 2 } else { 1 });
    for l in words_wrapped(a.text, words) {
        rows.push((Part::Other, vec![indent(pad), bold(l, WHITE)]));
    }
    if wide {
        rows.push((Part::Other, Vec::new()));
    }
    for (n, option) in a.options.iter().enumerate() {
        let cap = Span::styled(
            format!(" {} ", n + 1),
            style(CYAN, YOU_BG).add_modifier(Modifier::BOLD),
        );
        let words = ink(clip(option, cw.saturating_sub(label + 1)), WHITE);
        rows.push((
            Part::Other,
            vec![indent(pad), cap, indent(label - pad - 3), words],
        ));
    }
    let foot = if wide {
        "options · read only"
    } else {
        "read only"
    };
    if wide {
        rows.push((Part::Other, Vec::new()));
    }
    rows.push((Part::Other, vec![indent(pad), ink(foot, GRAY)]));
    rows
}

// The road taken, read back, then the roads not taken as dim stubs; an answer in words that
// matches no option is said back as written.
fn folded(a: &Asked, name: &str, answer: &str, cw: usize) -> Vec<(Part, Segs)> {
    let (wide, dim) = (cw >= usize::from(PHONE), mix(CYAN, GLASS, 0.45));
    let Some(picked) = a.options.iter().position(|o| *o == answer) else {
        let said = clip(&format!("you said {answer}"), cw);
        let asked = vec![
            ink("◆ ", dim),
            ink(clip(a.text, cw.saturating_sub(2)), DATA),
        ];
        return vec![(Part::Asked, asked), (Part::Other, vec![ink(said, CYAN)])];
    };
    let by = if wide { " asked  " } else { "  " };
    let mut asked = vec![ink("◆", dim), plain(" "), ink(name, GRAY), ink(by, GRAY)];
    asked.push(ink(clip(a.text, cw.saturating_sub(w(&asked) + 1)), DATA));
    let road = |said: bool| {
        let mut l = vec![ink(if wide { "┝━━▸ " } else { "┝━▸ " }, CYAN)];
        l.extend(said.then(|| ink("you chose ", CYAN)));
        l.extend([ink(format!("{} · ", picked + 1), CYAN), bold(answer, CYAN)]);
        l
    };
    let mut rows = vec![(Part::Asked, asked)];
    rows.push((
        Part::Other,
        Some(road(true))
            .filter(|r| w(r) + 2 <= cw)
            .unwrap_or_else(|| road(false)),
    ));
    let lead = if wide { "╰╌╌╌ " } else { "╰╌╌ " };
    let room = cw.saturating_sub(width(lead)).max(1);
    let mut line: Segs = Vec::new();
    let mut stubs = Vec::new();
    for (n, option) in a.options.iter().enumerate().filter(|(n, _)| *n != picked) {
        let piece = [
            bold(format!("{} ", n + 1), GRAY_DIM),
            ink(clip(option, room.saturating_sub(2)), GRAY_DIM),
        ];
        if !line.is_empty() && w(&line) + 3 + w(&piece) > room {
            stubs.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(plain("   "));
        }
        line.extend(piece);
    }
    stubs.extend(Some(line).filter(|l| !l.is_empty()));
    if stubs.is_empty() {
        rows[1].1[0] = ink(if wide { "╰━━▸ " } else { "╰━▸ " }, CYAN);
    }
    for (n, stub) in stubs.into_iter().enumerate() {
        let lead = if n == 0 {
            lead.into()
        } else {
            " ".repeat(width(lead))
        };
        rows.push((Part::Other, [vec![ink(lead, GRAY_DIM)], stub].concat()));
    }
    rows
}

// Whose move it is, on the cyan tab.
fn tab(margin: &str) -> Span<'static> {
    let look = style(GLASS, CYAN).add_modifier(Modifier::BOLD);
    Span::styled(format!("{margin}YOUR CALL{margin}"), look)
}

// Words wrapped at spaces to `room`, a word longer than a row clipped.
fn words_wrapped(text: &str, room: usize) -> Vec<String> {
    let room = room.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace().map(|word| clip(word, room)) {
        if !line.is_empty() && width(&line) + 1 + width(&word) > room {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&word);
    }
    lines.extend(Some(line).filter(|l| !l.is_empty()));
    lines
}

// The line cut and padded to `cw` columns, on `ground` where it has none of its own.
fn paint(line: Segs, cw: usize, ground: Option<Color>) -> Segs {
    let mut line = clip_spans(line, cw);
    let fill = cw.saturating_sub(w(&line));
    line.push(plain(" ".repeat(fill)));
    for s in line.iter_mut().filter(|s| s.style.bg.is_none()) {
        s.style.bg = ground;
    }
    line
}

fn ground(line: &[Span<'_>]) -> Option<Color> {
    line.iter().rev().find_map(|s| s.style.bg)
}
fn w(line: &[Span<'_>]) -> usize {
    line.iter().map(|s| width(&s.content)).sum()
}
fn plain(text: impl Into<String>) -> Span<'static> {
    Span::raw(text.into())
}
fn ink(text: impl Into<String>, fg: Color) -> Span<'static> {
    Span::styled(text.into(), Style::default().fg(fg))
}
fn bold(text: impl Into<String>, fg: Color) -> Span<'static> {
    Span::styled(
        text.into(),
        Style::default().fg(fg).add_modifier(Modifier::BOLD),
    )
}
