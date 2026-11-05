//! The grid's card: the search row, the grid rows under each tab's name with the tone row
//! floating beside its emoji, the name row and the tabs, drawn into the band.
use super::{BUBBLE, CELL, Grid, Hit, INK, LIT, Layout, Line, TABS, TONE_NAMES, keep};
use crate::terminal::{
    App,
    data::{Emoji, Table},
    text::*,
    view::{Action, Hit as Target},
};
use ratatui::{buffer::Buffer, layout::Rect, style::Color};

// One element of a card row: `w` columns on `bg`, its words clipped to fit, the rest blank.
struct Pen<'b> {
    b: &'b mut Buffer,
    x: u16,
    y: u16,
}
impl Pen<'_> {
    fn cell(&mut self, w: usize, bg: Color, parts: &[(&str, Color, bool)]) -> Rect {
        let at = Rect::new(self.x, self.y, w as u16, 1);
        fill(self.b, at, style(INK, bg));
        let mut x = self.x;
        for (text, fg, strong) in parts {
            let look = if *strong {
                bold(*fg, bg)
            } else {
                style(*fg, bg)
            };
            x = put(self.b, x, self.y, text, at.right().saturating_sub(x), look);
        }
        self.x += w as u16;
        at
    }
    // ` 😀 `, or `▏😀▕` lit, padded to the cell whatever the glyph's width.
    fn emoji(&mut self, glyph: &str, bg: Color, lit: bool) -> Rect {
        let pad = " ".repeat((CELL - 2).saturating_sub(width(glyph)));
        let (l, r) = if lit { ("▏", "▕") } else { (" ", " ") };
        self.cell(
            CELL,
            bg,
            &[
                (l, CYAN, false),
                (glyph, INK, false),
                (&pad, INK, false),
                (r, CYAN, false),
            ],
        )
    }
}

/// Draws the card flush right in `r`: the search, the grid with each tab's name, the selected
/// emoji's name or tones, and the tabs, each cell four columns wide and at most twelve across.
pub fn draw(app: &App, b: &mut Buffer, r: Rect, hits: &mut Vec<Target>) {
    let (Some(g), t) = (&app.grid, &app.emoji) else {
        return;
    };
    let width = r.width as usize;
    let cols = (width.saturating_sub(3) / CELL).clamp(1, 12);
    let fit = cols * CELL + 3;
    let panel = if width < fit || width - fit < CELL {
        width
    } else {
        fit
    };
    let rows = r.height as usize;
    let shown = rows.saturating_sub(3).max(1);
    g.size.set((cols, shown));
    let (l, inner) = (g.layout(t), panel.saturating_sub(3));
    let top = keep(g.top, l.row_of(g.at), shown, l.rows);
    let card = Rect::new(r.right() - panel as u16, r.y, panel as u16, r.height);
    fill(b, r, style(DATA, GLASS));
    fill(b, card, style(INK, RECESS));
    for y in card.y..card.bottom() {
        put(b, card.x, y, "│", 1, style(CYAN, RECESS));
    }
    let mut hit = |area: Rect, h: Hit| hits.push((area, Action::Grid(h)));
    hit(card, Hit::Card);
    let x = card.x + 2;
    search_row(g, &l, top, inner, &mut Pen { b, x, y: card.y }, &mut hit);
    let bubble = bubble(g, t, &l, top, shown, cols);
    for row in 0..shown.min(rows.saturating_sub(3)) {
        let y = card.y + 1 + row as u16;
        grid_row(
            g,
            t,
            &l,
            (top + row, row, inner),
            bubble,
            &mut Pen { b, x, y },
            &mut hit,
        );
    }
    let y = card.bottom() - 2;
    name_row(
        g,
        t,
        inner,
        bubble.is_some(),
        &mut Pen { b, x, y },
        &mut hit,
    );
    let lit = l.line(top).and_then(|line| match line {
        Line::Label(tab) => Some(tab),
        Line::Cells(s, ..) => l.sections[s].tab,
    });
    let mut p = Pen {
        b,
        x,
        y: card.bottom() - 1,
    };
    for (n, (icon, _)) in TABS.iter().enumerate() {
        let on = g.search.is_empty() && lit == Some(n);
        hit(
            p.emoji(icon, if on { LIT } else { RECESS }, on),
            Hit::Tab(n),
        );
    }
}

fn search_row(
    g: &Grid,
    l: &Layout,
    top: usize,
    inner: usize,
    p: &mut Pen,
    hit: &mut impl FnMut(Rect, Hit),
) {
    let searching = !g.search.is_empty();
    let label = match (searching, l.line(top)) {
        (true, _) if l.total == 0 => "none".into(),
        (true, _) => format!("{} found", l.total),
        (false, Some(Line::Label(_)) | None) => String::new(),
        (false, Some(Line::Cells(s, ..))) => l.sections[s]
            .tab
            .map_or(String::new(), |n| TABS[n].1.to_uppercase()),
    };
    let room = inner.saturating_sub(6);
    let fits = |text: &str| width(text) + width(&label) + 3 <= room;
    let text = match searching {
        true => clip(&g.search, room.saturating_sub(1).max(1)),
        false => ["search emoji · type to filter", "search emoji", "search"]
            .into_iter()
            .find(|t| fits(t))
            .unwrap_or("search")
            .into(),
    };
    let shown = if fits(&text) {
        format!("{label} ")
    } else {
        String::new()
    };
    p.cell(3, BEZEL, &[(" ⌕ ", GRAY, false)]);
    let ink = if searching { WHITE } else { GRAY };
    p.cell(
        inner.saturating_sub(6 + width(&shown)),
        BEZEL,
        &[(&text, ink, false), ("▏", CYAN, false)],
    );
    p.cell(
        width(&shown),
        BEZEL,
        &[(&shown, if searching { GRAY } else { CYAN }, false)],
    );
    hit(p.cell(3, BEZEL, &[(" ✕ ", GRAY, false)]), Hit::Close);
}

// Where the tone row floats: its first row shown, the cell it starts at, and the emoji's column
// and side; none when it is closed or no three rows are free on either side.
fn bubble(
    g: &Grid,
    t: &Table,
    l: &Layout,
    top: usize,
    shown: usize,
    cols: usize,
) -> Option<(usize, usize, usize, bool)> {
    g.toning?;
    g.selected(t)?;
    let row = l
        .row_of(g.at)
        .checked_sub(top)
        .filter(|&r| r < shown && cols >= BUBBLE)?;
    let Some(Line::Cells(_, start, _)) = l.line(top + row) else {
        return None;
    };
    let (col, below) = (g.at - start, row + 3 < shown);
    (below || row >= 3).then(|| {
        (
            if below { row + 1 } else { row - 3 },
            col.saturating_sub(3).min(cols - BUBBLE),
            col,
            below,
        )
    })
}

fn grid_row(
    g: &Grid,
    t: &Table,
    l: &Layout,
    (r, row, inner): (usize, usize, usize),
    bubble: Option<(usize, usize, usize, bool)>,
    p: &mut Pen,
    hit: &mut impl FnMut(Rect, Hit),
) {
    let covered = bubble.filter(|b| row >= b.0 && row < b.0 + 3);
    let start = p.x;
    match l.line(r) {
        Some(Line::Label(n)) if covered.is_none() => {
            let name = TABS[n].1.to_uppercase();
            let rule = "─".repeat(inner.saturating_sub(width(&name) + 2));
            p.cell(
                inner,
                RECESS,
                &[
                    (&format!(" {name} "), GRAY, false),
                    (&rule, GRAY_DIM, false),
                ],
            );
        }
        Some(Line::Cells(s, first, len)) => {
            let items: Vec<usize> = g
                .items(t, l.sections[s].tab)
                .skip(first - l.sections[s].start)
                .take(len)
                .collect();
            for (c, &i) in items.iter().enumerate() {
                let on = first + c == g.at;
                hit(
                    p.emoji(g.glyph(t, i), if on { LIT } else { RECESS }, on),
                    Hit::Cell(first + c),
                );
            }
        }
        None if row == 0 && l.total == 0 => {
            p.cell(
                inner,
                RECESS,
                &[(&format!(" no emoji for “{}”", g.search.trim()), GRAY, false)],
            );
        }
        _ => {}
    }
    if let Some(b) = covered {
        p.x = start + (b.1 * CELL) as u16;
        tone_bubble(g, t, row, b, p, hit);
    }
}

// One of the three rows of the tone row floating at `(top, left, col, below)`: its edge with the
// arrow toward the emoji, or the six tones.
fn tone_bubble(
    g: &Grid,
    t: &Table,
    row: usize,
    (top, left, col, below): (usize, usize, usize, bool),
    p: &mut Pen,
    hit: &mut impl FnMut(Rect, Hit),
) {
    let w = BUBBLE * CELL;
    if row - top == 1 {
        let choices = choices(&t.emoji[g.selected(t).unwrap_or_default()]);
        p.cell(2, BEZEL, &[("│ ", CYAN, false)]);
        for n in 0..6u8 {
            let on = Some(n) == g.toning;
            let glyph = choices.get(usize::from(n)).copied().unwrap_or("");
            hit(
                p.emoji(glyph, if on { LIT } else { BEZEL }, on),
                Hit::Tone(n),
            );
        }
        p.cell(2, BEZEL, &[(" │", CYAN, false)]);
        return;
    }
    let first = row == top;
    let arrow = (col - left) * CELL + 1;
    let (a, z) = if first {
        ('╭', '╮')
    } else {
        ('╰', '╯')
    };
    let mid = match (first == below, below) {
        (true, true) => '▲',
        (true, false) => '▼',
        (false, _) => '─',
    };
    let rule = |n| "─".repeat(n);
    let edge = format!(
        "{a}{}{mid}{}{z}",
        rule(arrow - 1),
        rule(w.saturating_sub(arrow + 2))
    );
    p.cell(w, RECESS, &[(&edge, CYAN, false)]);
}

// An emoji's base glyph, then its five tones when it has them.
fn choices(e: &Emoji) -> Vec<&'static str> {
    std::iter::once(e.glyph)
        .chain(e.tones.into_iter().flatten())
        .collect()
}

// The selected emoji's name and first shortcode beside the tone button; with the tones open,
// the tone's name and keys (and the six tones themselves when no bubble floats).
fn name_row(
    g: &Grid,
    t: &Table,
    inner: usize,
    popped: bool,
    p: &mut Pen,
    hit: &mut impl FnMut(Rect, Hit),
) {
    let i = g.selected(t);
    if let (Some(tone), Some(i)) = (g.toning, i) {
        return tone_row(&t.emoji[i], usize::from(tone), inner, popped, p, hit);
    }
    let label = match (g.armed, inner >= 48) {
        (true, true) => " ▾ tap an emoji ",
        (true, false) => " ▾ tap one ",
        (false, true) => " ▾ skin tones ",
        (false, false) => " ▾ tones ",
    };
    let button = if g.armed { LIT } else { BEZEL_HI };
    let Some(i) = i else {
        p.cell(
            inner.saturating_sub(width(label)),
            BEZEL,
            &[(" No emoji selected", GRAY, false)],
        );
        return hit(
            p.cell(width(label), button, &[(label, WHITE, false)]),
            Hit::ToneMode,
        );
    };
    let e = &t.emoji[i];
    let head = format!(" {} ", g.glyph(t, i));
    let code = e.codes.first().map_or(String::new(), |c| format!(" :{c}:"));

    let room = inner as i64 - width(&head) as i64 - width(label) as i64 - 1;
    let name = clip(e.name, room.max(4) as usize);
    let coded = !code.is_empty() && (width(&name) + width(&code)) as i64 <= room;

    let mut parts = vec![(head.as_str(), WHITE, false), (name.as_str(), WHITE, true)];
    if coded {
        parts.push((&code, CYAN, false));
    }
    p.cell(inner.saturating_sub(width(label)), BEZEL, &parts);
    hit(
        p.cell(width(label), button, &[(label, WHITE, false)]),
        Hit::ToneMode,
    );
}

// The name row with the tones open: the tone's glyph, the emoji's name and tone, and the keys
// that fit (the six tones first when no bubble floats).
fn tone_row(
    e: &Emoji,
    index: usize,
    inner: usize,
    popped: bool,
    p: &mut Pen,
    hit: &mut impl FnMut(Rect, Hit),
) {
    let choices = choices(e);
    let glyph = format!(" {} ", choices.get(index).copied().unwrap_or(""));
    let slots = if popped { 0 } else { choices.len() * CELL };
    if !popped {
        for (n, choice) in choices.iter().enumerate() {
            hit(
                p.emoji(choice, if n == index { LIT } else { BEZEL }, n == index),
                Hit::Tone(n as u8),
            );
        }
    }
    let text = clip(
        &format!("{} · {} skin tone", e.name, TONE_NAMES[index]),
        inner.saturating_sub(slots + width(&glyph)),
    );
    p.cell(
        inner.saturating_sub(slots),
        BEZEL,
        &[(&glyph, WHITE, false), (&text, WHITE, true)],
    );
}
