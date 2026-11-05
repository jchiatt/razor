//! The palette and the cell-level drawing every surface shares: measured Unicode widths,
//! clipping, greedy word wrap, and number and name formats.
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use std::borrow::Cow;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub(super) const GLASS: Color = rgb(0x05080b);
pub(super) const BEZEL: Color = rgb(0x1d2227);
pub(super) const BEZEL_HI: Color = rgb(0x2b3238);
pub(super) const RECESS: Color = rgb(0x0b1015);
pub(super) const DATA: Color = rgb(0xdfe6ea);
pub(super) const WHITE: Color = rgb(0xeef2f4);
pub(super) const GRAY: Color = rgb(0x7d8993);
pub(super) const GRAY_DIM: Color = rgb(0x434d56);
pub(super) const CYAN: Color = rgb(0x3fd9f2);
pub(super) const GREEN: Color = rgb(0x3ee56d);
pub(super) const MAGENTA: Color = rgb(0xff5bdc);
pub(super) const AMBER: Color = rgb(0xffb319);
pub(super) const RED: Color = rgb(0xff3d32);
pub(super) const VIOLET: Color = rgb(0xcd8bff);
pub(super) const AGENT_BG: Color = rgb(0x1a1130);
pub(super) const YOU_BG: Color = rgb(0x123440);
pub(super) const CYAN_BG: Color = rgb(0x05202a);
pub(super) const RED_BG: Color = rgb(0x2a0b09);
pub(super) const AMBER_BG: Color = rgb(0x2a1e05);
pub(super) const CHIP: Color = rgb(0x1d4a58);
/// Lamplight: the one colour skills own.
pub(super) const LAMP: Color = rgb(0xecd48c);
pub(super) const BUTTON: Color = rgb(0x0c3a48);
pub(super) const STRIP: Color = rgb(0x16202a);
pub(super) const VIOLET_DIM: Color = rgb(0x704d9a);
/// The agent's violet lit toward white: a live thought's spinner and newest letters.
pub(super) const VIOLET_HI: Color = rgb(0xe6b9ff);
pub(super) const ADDED_BG: Color = rgb(0x07240f);

pub(super) const fn rgb(n: u32) -> Color {
    Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8)
}
/// `a` moved toward `b` by `t` clamped to 0..1, channel by channel, rounded.
pub(super) fn mix(a: Color, b: Color, t: f64) -> Color {
    let t = t.clamp(0., 1.);
    let rgb = |c| match c {
        Color::Rgb(r, g, b) => [r, g, b].map(f64::from),
        _ => [0.; 3],
    };
    let ([r, g, b], [x, y, z]) = (rgb(a), rgb(b));
    let at = |from: f64, to: f64| (from + (to - from) * t).round() as u8;
    Color::Rgb(at(r, x), at(g, y), at(b, z))
}

pub(super) fn style(fg: Color, bg: Color) -> Style {
    Style::default().fg(fg).bg(bg)
}
pub(super) fn bold(fg: Color, bg: Color) -> Style {
    style(fg, bg).add_modifier(Modifier::BOLD)
}
/// The `s` a count other than one takes.
pub(super) fn plural<N: PartialEq + From<u8>>(n: N) -> &'static str {
    if n == N::from(1) { "" } else { "s" }
}
/// Terminal cells of `s`; control characters take none.
pub(super) fn width(s: &str) -> usize {
    s.graphemes(true)
        .filter(|g| !g.chars().any(char::is_control))
        .map(UnicodeWidthStr::width)
        .sum()
}

/// Writes whole graphemes from `x` within `max` cells and the buffer; returns the next column.
pub(super) fn put(b: &mut Buffer, x: u16, y: u16, s: &str, max: u16, style: Style) -> u16 {
    if y < b.area.y || y >= b.area.bottom() {
        return x;
    }
    let right = x.saturating_add(max).min(b.area.right());
    let mut at = x;
    for g in s.graphemes(true) {
        let w = g.width() as u16;
        if g.chars().any(char::is_control) {
            continue;
        } else if at.saturating_add(w) > right {
            break;
        } else if w == 0 || at < b.area.x {
            continue;
        }
        b[(at, y)].set_symbol(g).set_style(style);
        for dx in 1..w {
            b[(at + dx, y)].reset();
            b[(at + dx, y)].set_style(style);
        }
        at += w;
    }
    at
}

pub(super) fn fill(b: &mut Buffer, r: Rect, style: Style) {
    for y in r.y..r.bottom().min(b.area.bottom()) {
        for x in r.x..r.right().min(b.area.right()) {
            b[(x, y)].reset();
            b[(x, y)].set_style(style);
        }
    }
}

/// Draws a line's spans from `r.x` over `base`, clipped at `r`'s right edge.
pub(super) fn line(b: &mut Buffer, r: Rect, y: u16, l: &Line<'_>, base: Style) {
    let mut x = r.x;
    for s in &l.spans {
        let style = base.patch(l.style).patch(s.style);
        x = put(b, x, y, &s.content, r.right().saturating_sub(x), style);
    }
}

/// A scrollbar down column `x` from row `y`: `height` rows showing the rows from `top` of `total`.
/// The thumb is counted in half cells and is never less than one; nothing shows while all fits.
pub(super) fn scrollbar(
    b: &mut Buffer,
    x: u16,
    y: u16,
    height: usize,
    total: usize,
    top: usize,
) -> bool {
    let Some(range) = total.checked_sub(height).filter(|r| *r > 0 && height > 0) else {
        return false;
    };
    let track = height * 2;
    let size = (track * height / total).max(1);
    // Rounded half up, as the scrollbox does.
    let start = (2 * top.min(range) * (track - size) + range) / (2 * range);
    for half in (start & !1..start + size).step_by(2) {
        let cover = (start + size).min(half + 2) - start.max(half);
        let glyph = match (cover, start <= half) {
            (2, _) => "█",
            (_, true) => "▀",
            _ => "▄",
        };
        put(
            b,
            x,
            y + (half / 2) as u16,
            glyph,
            1,
            style(GRAY_DIM, GLASS),
        );
    }
    true
}

/// Words in `ink` on the glass.
pub(super) fn span(text: impl Into<Cow<'static, str>>, ink: Color) -> Span<'static> {
    Span::styled(text, style(ink, GLASS))
}

/// `── LABEL ──────── hint`, `columns` wide: the label `GRAY`, the rest `GRAY_DIM`.
pub(super) fn rule(label: &str, hint: Option<&str>, columns: usize) -> Line<'static> {
    let head = format!("── {label} ");
    let tail = hint.map_or(String::new(), |h| format!(" {h}"));
    let dashes = columns.saturating_sub(width(&head) + width(&tail)).max(1);
    let rest = format!("{}{tail}", "─".repeat(dashes));
    Line::from(vec![span(head, GRAY), span(rest, GRAY_DIM)])
}

/// A description's first sentence on one line, without its full stop (one dot off `Hmm..`).
pub(super) fn sentence(description: &str) -> String {
    let flat = description.split_whitespace().collect::<Vec<_>>().join(" ");
    let end = flat.char_indices().skip(1).find_map(|(i, c)| {
        let end = i + c.len_utf8();
        let ends = matches!(c, '.' | '!' | '?')
            && flat[end..].chars().next().is_none_or(char::is_whitespace);
        ends.then_some(end)
    });
    let first = &flat[..end.unwrap_or(flat.len())];
    first.strip_suffix('.').unwrap_or(first).to_owned()
}

/// `s` within `columns` cells, ending in `…` when cut.
pub(super) fn clip(s: &str, columns: usize) -> String {
    if width(s) <= columns {
        return s.to_owned();
    } else if columns == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut n = 0;
    for g in s.graphemes(true).take_while(|g| {
        n += g.width();
        n < columns
    }) {
        out.push_str(g);
    }
    out.push('…');
    out
}

/// `spans` within `columns` cells, the span that does not fit clipped with `…`.
pub(super) fn clip_spans(spans: Vec<Span<'static>>, columns: usize) -> Vec<Span<'static>> {
    let mut used = 0;
    let mut out = Vec::new();
    for s in spans {
        let n = width(&s.content);
        if used + n > columns {
            if columns > used {
                out.push(Span::styled(clip(&s.content, columns - used), s.style));
            }
            break;
        }
        used += n;
        out.push(s);
    }
    out
}

/// `s` within `columns` cells, cut from its middle with `…` so both its ends stay.
pub(super) fn clip_middle(s: &str, columns: usize) -> String {
    if width(s) <= columns || columns < 3 {
        return clip(s, columns);
    }
    let mut n = 0;
    let tail = s.graphemes(true).rev().take_while(|g| {
        n += g.width();
        n <= (columns - 1) / 2
    });
    let tail: String = tail.collect::<Vec<_>>().into_iter().rev().collect();
    format!("{}{tail}", clip(s, columns - width(&tail)))
}

// OpenTUI's greedy word wrap breaks after these marks, not after an em dash or emoji, and around
// every CJK character.
fn break_after(g: &str) -> bool {
    let mark =
        |c: char| c.is_whitespace() || "-/\\.,;:!?()[]{}\u{00ad}\u{2010}、。！？".contains(c);
    g.chars().last().is_some_and(mark) || cjk(g)
}

fn cjk(g: &str) -> bool {
    g.chars()
        .last()
        .is_some_and(|c| ('\u{2e80}'..='\u{9fff}').contains(&c))
}

/// One grapheme of a wrapped row, with its source byte range.
#[derive(Clone, Copy)]
pub(super) struct Part<'a> {
    pub(super) text: &'a str,
    pub(super) style: Style,
    pub(super) at: usize,
    pub(super) end: usize,
}

/// Visits wrapped rows one at a time with their source byte ranges (spaces and newlines
/// included); the last visit is flagged and may be empty, so an editor can place its cursor.
/// Returning false stops without laying out the rest.
pub(super) fn visit_wrap(
    spans: &[Span<'_>],
    columns: usize,
    mut visit: impl FnMut(&[Part<'_>], std::ops::Range<usize>, bool) -> bool,
) {
    let columns = columns.max(1);
    let mut row: Vec<Part<'_>> = Vec::new();
    let (mut used, mut at, mut start, mut split, mut wrapped) = (0, 0, 0, None, false);
    for span in spans {
        for source in span.content.graphemes(true) {
            let end = at + source.len();
            if source == "\n" {
                if !visit(&row, start..end, false) {
                    return;
                }
                row.clear();
                (used, split, wrapped, start, at) = (0, None, false, end, end);
                continue;
            }
            let g = if source == "\t" { "  " } else { source };
            if cjk(g) && !row.is_empty() {
                split = Some(row.len());
            }
            if used + g.width() > columns && !row.is_empty() {
                let tail = split.map_or_else(Vec::new, |n| row.split_off(n));
                let next = tail.first().map_or(at, |part| part.at);
                while row.last().is_some_and(|part| part.text.trim().is_empty()) {
                    row.pop();
                }
                if !visit(&row, start..next, false) {
                    return;
                }
                (row, start, split, wrapped) = (tail, next, None, true);
                used = row.iter().map(|part| width(part.text)).sum();
            }
            if !g.chars().any(char::is_control) && !(wrapped && used == 0 && g.trim().is_empty()) {
                row.push(Part {
                    text: g,
                    style: span.style,
                    at,
                    end,
                });
                used += g.width();
                if break_after(g) {
                    split = Some(row.len());
                }
            }
            at = end;
        }
    }
    visit(&row, start..at, true);
}

pub(super) fn wrap(spans: Vec<Span<'static>>, columns: usize) -> Vec<Line<'static>> {
    let mut rows = Vec::new();
    visit_wrap(&spans, columns, |parts, _, last| {
        if !last || !parts.is_empty() {
            rows.push(to_line(parts));
        }
        true
    });
    rows
}

pub(super) fn to_line(parts: &[Part<'_>]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for part in parts {
        match spans.last_mut().filter(|last| last.style == part.style) {
            Some(last) => last.content.to_mut().push_str(part.text),
            None => spans.push(Span::styled(part.text.to_owned(), part.style)),
        }
    }
    Line::from(spans)
}

pub(super) fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Wall milliseconds as a time in `zone`; none without a zone, or past what a time can hold.
pub(super) fn zoned(at: u64, zone: Option<&jiff::tz::TimeZone>) -> Option<jiff::Zoned> {
    let t = jiff::Timestamp::from_millisecond(i64::try_from(at).ok()?).ok()?;
    Some(t.to_zoned(zone?.clone()))
}

/// `3:09pm` in `zone`; empty without one.
pub(super) fn clock(at: u64, zone: Option<&jiff::tz::TimeZone>) -> String {
    zoned(at, zone).map_or_else(String::new, |t| t.strftime("%-I:%M%P").to_string())
}

pub(super) fn kilo(n: u64) -> String {
    match n {
        1_000_000.. => format!("{:.1}M", n as f64 / 1_000_000.).replace(".0M", "M"),
        1000.. => format!("{}k", (n + 500) / 1000),
        _ => n.to_string(),
    }
}

/// Compact durations: `812ms`, `4.2s` under ten seconds, `41s`, `2m 10s`.
pub(super) fn dur(ms: u64) -> String {
    match ms {
        ..1000 => format!("{ms}ms"),
        1000..10_000 => format!("{:.1}s", ms as f64 / 1000.),
        _ => match (ms + 500) / 1000 {
            s @ ..60 => format!("{s}s"),
            s => format!("{}m {:02}s", s / 60, s % 60),
        },
    }
}

pub(super) fn timer(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        3600.. => format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60),
        _ => format!("{}:{:02}", s / 60, s % 60),
    }
}

pub(super) fn effort(value: crate::protocol::Effort) -> &'static str {
    use crate::protocol::Effort::*;
    match value {
        Low => "low",
        Medium => "medium",
        High => "high",
        Xhigh => "xhigh",
        Max => "max",
    }
}

/// A model's display name: `claude-opus-5-5` is `Opus 5.5`.
pub(super) fn model(s: &str) -> String {
    if s.is_empty() {
        return "model unknown".into();
    }
    let s = s.strip_prefix("claude-").unwrap_or(s);
    let s = s.split('[').next().unwrap_or(s);
    let mut out = s
        .replace("-5-5", " 5.5")
        .replace("-4-5", " 4.5")
        .replace('-', " ");
    if let Some(c) = out.get_mut(..1) {
        c.make_ascii_uppercase();
    }
    out
}

/// OSC, CSI and terminal strings are data, never terminal instructions or words in the
/// transcript; an unfinished sequence from a live delta is consumed too.
pub(super) fn screen_safe(src: &str) -> Cow<'_, str> {
    if !src
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Cow::Borrowed(src);
    }
    let mut out = String::with_capacity(src.len().min(64 << 10));
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        let command = match c {
            '\x1b' => chars.next(),
            '\u{9b}' => Some('['),
            '\u{90}' => Some('P'),
            '\u{9d}' => Some(']'),
            '\u{9e}' => Some('^'),
            '\u{9f}' => Some('_'),
            _ if !c.is_control() || matches!(c, '\n' | '\r' | '\t') => {
                out.push(c);
                continue;
            }
            _ => continue,
        };
        match command {
            Some('[') => {
                chars.by_ref().find(|c| ('@'..='~').contains(c));
            }
            Some(']' | 'P' | '^' | '_') => {
                while let Some(c) = chars.next() {
                    if matches!(c, '\x07' | '\u{9c}')
                        || c == '\x1b' && chars.next_if_eq(&'\\').is_some()
                    {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    Cow::Owned(out)
}
