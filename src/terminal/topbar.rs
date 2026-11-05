//! The boxed bar: the headline over the identity, the context gauge and the two-row clock.
use super::{App, text::*};
use crate::protocol::AgentView;
use ratatui::prelude::{Buffer, Color, Modifier, Rect};

/// `gauge` false leaves the context reading to the frame's own tape.
pub fn draw(app: &App, b: &mut Buffer, r: Rect, agent: Option<&AgentView>, gauge: bool, now: u64) {
    fill(b, r, style(WHITE, GLASS));
    if r.height < 4 || r.width < 6 {
        return;
    }
    let edge = style(CYAN, GLASS);
    let rule = "─".repeat(r.width as usize - 2);
    put(b, r.x, r.y, &format!("┌{rule}┐"), r.width, edge);
    put(b, r.x, r.bottom() - 1, &format!("└{rule}┘"), r.width, edge);
    for y in r.y + 1..r.bottom() - 1 {
        put(b, r.x, y, "│", 1, edge);
        put(b, r.right() - 1, y, "│", 1, edge);
    }
    let inner = Rect::new(r.x + 1, r.y + 1, r.width - 2, 2);
    let clock = big_clock(app, b, inner, now);
    // The headline and the identity share one left edge; the gauge stands three columns clear
    // of the clock and takes a quarter of what is left, never the words' last thirty columns.
    let x = r.x + 2;
    let open = clock.saturating_sub(x + 3);
    let gauge = match open {
        _ if !gauge => 0,
        60.. => (open / 4).clamp(20, 32),
        44.. => 14,
        _ => 0,
    };
    let right = clock.saturating_sub(3 + gauge);
    if gauge > 0 {
        let session = agent.map(|a| &a.session);
        super::gauge::bar(app, b, Rect::new(right, r.y + 1, gauge, 2), session, now);
    }
    let room = right.saturating_sub(x + 2 * u16::from(gauge > 0));
    let headline = agent.map_or("", |a| a.headline.as_str());
    let (headline, ink) = match headline {
        "" => ("No headline yet", GRAY_DIM),
        words => (words, MAGENTA),
    };
    let headline = clip(headline, room as usize);
    value(
        b,
        Rect::new(x, r.y + 1, room, 1),
        &[(&headline, ink, true)],
        lit(app, 1, now),
    );
    identity(app, b, Rect::new(x, r.y + 2, room, 1), agent, now);
}

// The identity row: the name whole, then the place (the repository and branch, cut from its
// middle, then left out) and the count of uncommitted files.
fn identity(app: &App, b: &mut Buffer, r: Rect, agent: Option<&AgentView>, now: u64) {
    let folder = app
        .cwd
        .file_name()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    // A new thread's composer: no session yet, the resident in place of the folder.
    let thread = app.resident.as_ref().filter(|_| app.focus.is_none());
    let folder = thread.map_or(folder.as_ref(), |r| r.as_str());
    let repo = agent.map_or(folder, |a| a.repo.as_deref().unwrap_or(&a.project));
    let read = app
        .folder
        .as_ref()
        .filter(|f| agent.is_some_and(|a| a.session == f.0));
    let branch = read.map_or_else(
        || agent.and_then(|a| a.branch.as_deref()).unwrap_or(""),
        |f| &f.1,
    );
    let changed = read.map_or_else(
        || agent.map_or(0, |a| a.changed_files.len() as u64),
        |f| f.2,
    );
    let changed = match changed {
        0 => String::new(),
        n => format!(" · {n} changed"),
    };
    let place = match branch {
        "" => format!(" · {repo}"),
        branch => format!(" · {repo} · {branch}"),
    };
    let name = agent.map_or(
        ["New thread", &app.brand.name][usize::from(thread.is_none())],
        |a| &a.name,
    );
    let room = r.width as usize;
    let name = clip(name, room.saturating_sub(width(&changed)).max(1));
    let left = room.saturating_sub(width(&name) + width(&changed));
    let place = match left {
        0..6 => String::new(),
        _ => clip(&place, left),
    };
    let words = [
        (&name[..], WHITE, true),
        (&place, GRAY, false),
        (&changed, GRAY, false),
    ];
    value(b, r, &words, lit(app, 0, now));
}

fn lit(app: &App, slot: usize, now: u64) -> bool {
    if let Some(at) = app.changed[slot].filter(|at| now < at.saturating_add(10_000)) {
        app.next.at(at.saturating_add(10_000));
        return true;
    }
    false
}

fn value(b: &mut Buffer, r: Rect, spans: &[(&str, Color, bool)], lit: bool) {
    let mut x = r.x;
    let ground = |tone| match tone {
        CYAN | GREEN | MAGENTA | AMBER | RED => tone,
        _ => rgb(0x39434b),
    };
    for &(words, tone, bold) in spans {
        let mut ink = if lit {
            style(GLASS, ground(tone))
        } else {
            style(tone, GLASS)
        };
        if bold {
            ink = ink.add_modifier(Modifier::BOLD);
        }
        x = put(b, x, r.y, words, r.right().saturating_sub(x), ink);
    }
    if lit && x > r.x {
        // Both place spans share the neutral ground; the headline has only one span.
        let margin = style(GLASS, ground(spans[0].1));
        put(b, r.x - 1, r.y, " ", 1, margin);
        put(b, x, r.y, " ", 1, margin);
    }
}

/// A time's local time of day (a reset's, a rebuild's), with the weekday unless it is today;
/// never a UTC day guess.
pub fn reset(app: &App, at: u64, now: u64) -> Option<String> {
    let (reset, today) = (
        zoned(at, app.zone.as_ref())?,
        zoned(now, app.zone.as_ref())?,
    );
    let format = if reset.date() == today.date() {
        "%-I:%M%P"
    } else {
        "%a %-I:%M%P"
    };
    Some(reset.strftime(format).to_string())
}

fn local(app: &App, at: u64, format: &str) -> Option<String> {
    Some(zoned(at, app.zone.as_ref())?.strftime(format).to_string())
}

// Loom's chrome/bigclock.rs font, encoded once in its workshop/octant.rs glyph table.
const OCTANT: [[&str; 2]; 10] = [
    ["𜵉𜵔𜵈", "𜴟𜴧▘"],
    ["𜴃▌ ", "𜴘𜴬 "],
    ["𜴁𜵳🯦", "𜴯𜴨𜴉"],
    ["𜵳𜶮🯦", "𜴧𜴧▘"],
    ["𜵑𜶛 ", "🮂𜴢𜺨"],
    ["𜴮𜴪𜴶", "𜴜𜴧▘"],
    ["𜷃𜶮 ", "𜴝𜴧▘"],
    ["🮂𜵔▘", "𜴡  "],
    ["𜶆𜶮🯦", "𜴝𜴧▘"],
    ["𜶆𜶮𜵈", "𜴘𜴐𜺨"],
];
const HALF: [[&str; 2]; 10] = [
    ["█▀█", "█▄█"],
    ["▄█ ", "▄█▄"],
    ["▀▀█", "█▄▄"],
    ["▀██", "▄▄█"],
    ["█ █", "▀▀█"],
    ["█▀▀", "▄▄█"],
    ["█▀▀", "█▄█"],
    ["▀▀█", "  █"],
    ["███", "█▄█"],
    ["█▀█", "▀▀█"],
];

fn big_clock(app: &App, b: &mut Buffer, r: Rect, now: u64) -> u16 {
    // For three seconds after Razor opens, the day of the month stands where the time will.
    let opened = app.presence.opened;
    let date = now < opened + 3000;
    let (digits, suffix) = if date {
        app.next.at(opened + 3000);
        ("%-d", [" %a", " %b"])
    } else {
        ("%-I:%M", ["   ", " %P"])
    };
    let (Some(digits), Some(top), Some(low)) = (
        local(app, now, digits),
        local(app, now, suffix[0]),
        local(app, now, suffix[1]),
    ) else {
        return r.right().saturating_sub(1);
    };
    let columns = digits
        .chars()
        .map(|c| if c == ':' { 1 } else { 3 })
        .sum::<u16>()
        + top.len() as u16;
    if r.width <= columns + 2 {
        return r.right().saturating_sub(1);
    }
    let left = r.right() - columns - 1;
    let octant = app.octant();
    let font = if octant { &OCTANT } else { &HALF };
    for row in 0..2 {
        let mut x = left;
        let y = r.y + row as u16;
        for ch in digits.chars() {
            let (words, tone) = match ch {
                '0'..='9' => (font[ch as usize - '0' as usize][row], WHITE),
                _ if octant => (["𜴉", "𜺨"][row], WHITE),
                _ => (["▄", "▀"][row], GRAY),
            };
            x = put(b, x, y, words, columns, style(tone, GLASS));
        }
        let words = [&top, &low][row].to_lowercase();
        put(b, x, y, &words, columns, style(GRAY, GLASS));
    }
    app.next.every(now, 60_000);
    left
}
