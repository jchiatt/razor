//! The original two-colour least-squares octant reducer and half-block fallback.
use super::{
    art::{self, Rgb, Sprite, mix},
    picture::{self, ActorState, Scene},
};
use ratatui::style::Color;
use std::sync::OnceLock;
use unicode_width::UnicodeWidthChar;
type Px = Option<Rgb>;
#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    fg: Rgb,
    bg: Rgb,
}
const OLDER: [(usize, u32); 26] = [
    (0, 0x20),
    (1, 0x1cea8),
    (2, 0x1ceab),
    (3, 0x1fb82),
    (5, 0x2598),
    (10, 0x259d),
    (15, 0x2580),
    (20, 0x1fbe6),
    (40, 0x1fbe7),
    (63, 0x1fb85),
    (64, 0x1cea3),
    (80, 0x2596),
    (85, 0x258c),
    (90, 0x259e),
    (95, 0x259b),
    (128, 0x1cea0),
    (160, 0x2597),
    (165, 0x259a),
    (170, 0x2590),
    (175, 0x259c),
    (192, 0x2582),
    (240, 0x2584),
    (245, 0x2599),
    (250, 0x259f),
    (252, 0x2586),
    (255, 0x2588),
];

pub(super) fn octant_table() -> &'static [char; 256] {
    static TABLE: OnceLock<[char; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut out = [' '; 256];
        let mut next = 0x1cd00_u32;
        for (m, slot) in out.iter_mut().enumerate() {
            let older = OLDER.iter().find(|(k, _)| *k == m).map(|(_, c)| *c);
            *slot = char::from_u32(older.unwrap_or(next)).unwrap_or(' ');
            if older.is_none() {
                next += 1;
            }
        }
        out
    })
}

pub(super) fn octant(mask: usize) -> char {
    octant_table().get(mask).copied().unwrap_or(' ')
}

// At most eight colours enter one octant. Keeping their first-seen order preserves tie breaks.
fn reduce(rgb: &[Px], indexes: [usize; 8], w: usize, h: usize, ground: Rgb) -> (usize, Rgb, Rgb) {
    let pixels = indexes.map(|i| rgb[i]);
    let mut colours = Vec::with_capacity(8);
    let mut open = false;
    for p in pixels {
        if let Some(p) = p {
            if !colours.contains(&p) {
                colours.push(p);
            }
        } else {
            open = true;
        }
    }
    let n = colours.len();
    if n == 0 {
        return (0, ground, ground);
    }
    if n == 1 {
        let fg = colours[0];
        let mask = if open {
            pixels
                .iter()
                .enumerate()
                .fold(0_usize, |m, (i, p)| m | (usize::from(p.is_some()) << i))
        } else {
            0
        };
        return (mask, fg, if open { ground } else { fg });
    }
    let weights: [f64; 8] = std::array::from_fn(|b| {
        if n == 2 && !open {
            1.0
        } else {
            1.0 + 8.0 * salience(rgb, w, h, indexes[b])
        }
    });
    let (mut best, mut mask, mut fg, mut bg) = (f64::INFINITY, 0_usize, ground, ground);
    let distance = |a: Rgb, b: Rgb| -> f64 {
        a.into_iter()
            .zip(b)
            .map(|(a, b)| (f64::from(a) - f64::from(b)).powi(2))
            .sum()
    };
    let mut consider = |f, b| {
        let (mut err, mut m) = (0.0, 0);
        for (i, p) in pixels.iter().enumerate() {
            if err >= best {
                break;
            }
            if let Some(p) = p {
                let (ef, eb) = (distance(*p, f), distance(*p, b));
                if ef < eb {
                    m |= 1 << i;
                    err += weights[i] * ef;
                } else {
                    err += weights[i] * eb;
                }
            }
        }
        if err < best {
            (best, mask, fg, bg) = (err, m, f, b);
        }
    };
    for (i, &a) in colours[..n].iter().enumerate() {
        if open {
            consider(a, ground);
        } else {
            for &b in &colours[i + 1..n] {
                consider(b, a);
            }
        }
    }
    if !open && mask.count_ones() > 4 {
        mask ^= 255;
        std::mem::swap(&mut fg, &mut bg);
    }
    (mask, if mask == 0 { bg } else { fg }, bg)
}
fn salience(rgb: &[Px], w: usize, h: usize, i: usize) -> f64 {
    let Some(p) = rgb[i] else {
        return 0.0;
    };
    let (x, y) = (i % w, i / w);
    let (mut sum, mut n) = ([0.0; 3], 0.0);
    for v in y.saturating_sub(1)..=(y + 1).min(h - 1) {
        for u in x.saturating_sub(1)..=(x + 1).min(w - 1) {
            if (u, v) != (x, y)
                && let Some(q) = rgb[v * w + u]
            {
                for c in 0..3 {
                    sum[c] += f64::from(q[c]);
                }
                n += 1.0;
            }
        }
    }
    if n == 0.0 {
        return 0.0;
    }
    let delta: [f64; 3] = std::array::from_fn(|c| f64::from(p[c]) - sum[c] / n);
    (art::js_hypot(&delta) / 160.0).min(1.0)
}

/// The room's rule, ten art rows, and lit name row, ready for the pane's foot.
pub(super) fn draw(sc: &Scene, octant: bool) -> super::Grid {
    let w = sc.w;
    let image = picture::paint(sc, octant);
    let ground = Cell {
        ch: ' ',
        fg: [223, 230, 234],
        bg: [5, 8, 11],
    };
    let mut grid = vec![vec![ground; w]; 12];
    for cell in &mut grid[0] {
        cell.ch = '─';
        cell.fg = [67, 77, 86];
    }
    if let Some(note) = &sc.note {
        let note: String = note
            .chars()
            .filter(|c| c.width() == Some(1))
            .take(w - 2)
            .collect();
        let x = w - note.chars().count();
        put(
            &mut grid[0],
            x.saturating_sub(1),
            &format!(" {note}"),
            sc.note_fg.unwrap_or([125, 137, 147]),
        );
    }
    if octant {
        encode_octant(&image, &mut grid[1..11]);
    } else {
        encode_half(&image, &mut grid[1..11]);
    }
    if let Some(x) = sc.z.filter(|x| *x >= 0.0 && x.fract() == 0.0) {
        put(&mut grid[3], x as usize, "z", [160, 168, 181]);
    }
    names(sc, &mut grid[11]);
    let rgb = |c: Rgb| Color::Rgb(c[0], c[1], c[2]);
    let cell = |c: Cell| (c.ch, rgb(c.fg), rgb(c.bg));
    grid.into_iter()
        .map(|row| row.into_iter().map(cell).collect())
        .collect()
}
fn put(row: &mut [Cell], x: usize, text: &str, fg: Rgb) {
    for (i, ch) in text.chars().filter(|c| c.width() == Some(1)).enumerate() {
        if let Some(c) = row.get_mut(x + i) {
            c.ch = ch;
            c.fg = fg;
        }
    }
}
fn names(sc: &Scene, row: &mut [Cell]) {
    for (x, c) in row.iter_mut().enumerate() {
        let t = picture::pool(
            x as f64 + 0.5,
            20.5,
            sc.lamp,
            15.0 - sc.dark * 2.0 + sc.bright * 9.0,
        );
        c.bg = mix(
            mix([25, 30, 35], [5, 8, 11], sc.dark * 0.5 * (1.0 - t)),
            [122, 90, 44],
            (t * (0.75 + sc.dark * 0.2)).min(0.6),
        );
    }
    let mut homes: Vec<f64> = sc
        .actors
        .iter()
        .filter(|a| !a.hidden)
        .map(|a| a.tag_at.unwrap_or(a.x))
        .collect();
    homes.sort_by(f64::total_cmp);
    let gap = homes.windows(2).map(|p| p[1] - p[0]).fold(10.0, f64::min);
    let width = (gap - 1.0).clamp(6.0, 9.0) as usize;
    for a in sc.actors.iter().filter(|a| !a.hidden) {
        let mut name: String = a.name.chars().filter(|c| c.width() == Some(1)).collect();
        if name.chars().count() > width - 2 {
            name = format!("{}…", name.chars().take(width - 3).collect::<String>());
        }
        let x = a.tag_at.unwrap_or(a.x) - ((2 + name.chars().count()) as f64 / 2.0).floor();
        if x < 0.0 || x.fract() != 0.0 {
            continue;
        }
        let (mark, fg) = match a.state {
            ActorState::Working => ("●", [62, 229, 109]),
            ActorState::Idle => ("○", [125, 137, 147]),
            ActorState::Waiting => ("◐", [255, 179, 25]),
            ActorState::Ended => ("×", [67, 77, 86]),
        };
        put(row, x as usize, mark, fg);
        let lit = picture::light_on(a.x, 8.0, sc.lamp, 0.0) > 0.6;
        let fg = if a.state == ActorState::Ended {
            [125, 137, 147]
        } else if lit || a.is_self {
            [238, 242, 244]
        } else {
            [223, 230, 234]
        };
        put(row, x as usize + 2, &name, fg);
    }
}
fn encode_half(s: &Sprite, grid: &mut [Vec<Cell>]) {
    for (y, row) in grid.iter_mut().enumerate() {
        for (x, c) in row.iter_mut().enumerate() {
            (c.ch, c.fg, c.bg) = match (
                s.get(x as i64, y as i64 * 2),
                s.get(x as i64, y as i64 * 2 + 1),
            ) {
                (Some(t), Some(b)) if t == b => (' ', t, t),
                (Some(t), Some(b)) => ('▀', t, b),
                (Some(t), None) => ('▀', t, c.bg),
                (None, Some(b)) => ('▄', b, c.bg),
                _ => continue,
            };
        }
    }
}
fn encode_octant(s: &Sprite, grid: &mut [Vec<Cell>]) {
    let (w, h) = (s.w.div_ceil(3) * 2, s.h.div_ceil(6) * 4);
    let pixels: Vec<_> = (0..w * h)
        .map(|i| {
            s.get(
                (i % w * 3).div_ceil(2) as i64,
                (i / w * 3).div_ceil(2) as i64,
            )
        })
        .collect();
    for (y, row) in grid.iter_mut().enumerate() {
        for (x, target) in row.iter_mut().enumerate() {
            let indexes: [usize; 8] = std::array::from_fn(|b| (y * 4 + b / 2) * w + x * 2 + b % 2);
            if indexes.iter().all(|&i| pixels[i].is_none()) {
                continue;
            }
            let (mask, fg, bg) = reduce(&pixels, indexes, w, h, target.bg);
            *target = Cell {
                ch: octant(mask),
                fg: if mask == 0 { bg } else { fg },
                bg,
            };
        }
    }
}
