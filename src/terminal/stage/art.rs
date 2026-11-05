//! Each picture is stored once, as palette indexes; each colouring of
//! it (a crew member's accent, an avatar's seed) is one named palette of its family. A picture is
//! decoded once per colouring, on first use; the furniture is painted in `picture.rs`.
use std::{
    collections::BTreeMap,
    sync::{LazyLock, OnceLock},
};

/// A true-colour pixel.
pub(super) type Rgb = [u8; 3];
/// A bounded image, row-major, with transparent pixels.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Sprite {
    pub(super) w: usize,
    pub(super) h: usize,
    pub(super) px: Vec<Option<Rgb>>,
}
impl Sprite {
    pub(super) fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            px: vec![None; w * h],
        }
    }
    pub(super) fn get(&self, x: i64, y: i64) -> Option<Rgb> {
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
            return None;
        }
        self.px[y as usize * self.w + x as usize]
    }
    pub(super) fn set(&mut self, x: i64, y: i64, p: Option<Rgb>) {
        if x >= 0 && y >= 0 && x < self.w as i64 && y < self.h as i64 {
            self.px[y as usize * self.w + x as usize] = p;
        }
    }
    pub(super) fn blit(&mut self, s: &Self, x: i64, y: i64, flip: bool) {
        for j in 0..s.h {
            for i in 0..s.w {
                let u = if flip { s.w - 1 - i } else { i };
                if let Some(p) = s.get(u as i64, j as i64) {
                    self.set(x + i as i64, y + j as i64, Some(p));
                }
            }
        }
    }
}
/// Portrait poses keep the catalogue indexes; the yawn is never played.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Pose {
    #[default]
    Idle,
    Blink,
    Talk,
    Sleep,
    Messy = 5,
    MessyYawn,
}
/// Stable FNV-1a selection.
pub(super) fn hash(seed: &str) -> u32 {
    seed.bytes()
        .fold(2166136261, |h, c| (h ^ u32::from(c)).wrapping_mul(16777619))
}
/// A root's native id always chooses the same family as its server's voice selection.
pub(super) fn member(root: &str) -> usize {
    crate::protocol::family(root.split('@').next().unwrap_or(root))
}
/// Linear RGB mixing with JavaScript's half-up rounding: the cast floors and saturates, as
/// `floor().clamp(0, 255)` would, without a call to `floor` on every channel.
pub(super) fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    std::array::from_fn(|i| {
        let x = f64::from(a[i]);
        (x + (f64::from(b[i]) - x) * t + 0.5) as u8
    })
}
static EMPTY: Sprite = Sprite {
    w: 0,
    h: 0,
    px: Vec::new(),
};
/// `picture` in `colouring`: `actor/heron/3`, `avatar/17`, `sky`, `prop/beaker`. An unknown pair
/// is an empty sprite, so a missing picture never breaks the draw.
pub(super) fn sprite(colouring: &str, picture: &str) -> &'static Sprite {
    let Some(&(family, index)) = ART.colourings.get(colouring) else {
        return &EMPTY;
    };
    let family = &ART.families[family];
    family.pictures.get(picture).map_or(&EMPTY, |p| {
        p.drawn[index].get_or_init(|| {
            let hex = |c: &str| u32::from_str_radix(c, 16).ok().map(rgb);
            decode(p, family.palettes[index].split(' ').map(hex))
        })
    })
}
/// `picture` of a colouring whose palette names colour slots, each coloured by `ink` and
/// decoded on every call, since each strip member's colours are its own (`avatar.rs`).
pub(super) fn inked(colouring: &str, picture: &str, ink: impl Fn(char) -> Option<Rgb>) -> Sprite {
    let Some(&(family, index)) = ART.colourings.get(colouring) else {
        return Sprite::default();
    };
    let family = &ART.families[family];
    let slots = family.palettes[index].chars().step_by(2).map(ink);
    family
        .pictures
        .get(picture)
        .map_or_else(Sprite::default, |p| decode(p, slots))
}
/// `0xrrggbb` as a pixel.
pub(super) const fn rgb(n: u32) -> Rgb {
    [(n >> 16) as u8, (n >> 8) as u8, n as u8]
}
/// A crew member's `picture` (`5/oct`, `arm/half`, `sleep/oct`, `hand/oct`) in its accent, 0 to 11.
pub(super) fn actor(who: usize, accent: usize, picture: &str) -> &'static Sprite {
    let names = &*crate::protocol::FAMILIES;
    let name = names[who % names.len()];
    sprite(&format!("actor/{name}/{accent}"), picture)
}
/// A prop's frame, bounded by its caller to the frames exported.
pub(super) fn prop(name: &str, frame: usize, octant: bool) -> &'static Sprite {
    let form = density(octant);
    sprite(&format!("prop/{name}"), &format!("{frame}/{form}"))
}
/// A prop's painted pixels, carried by a moving actor or the window before sampling.
pub(super) fn paint_prop(name: &str, frame: usize, octant: bool) -> &'static Sprite {
    let form = if octant { "paint" } else { "half" };
    sprite(&format!("prop/{name}"), &format!("{frame}/{form}"))
}
/// Density tags shared by the renderer and the catalogue.
pub(super) fn density(octant: bool) -> &'static str {
    if octant { "oct" } else { "half" }
}

// The embedded catalogue bounds every family and picture; nothing is admitted at runtime.
static ART: LazyLock<Art> = LazyLock::new(load);
struct Art {
    /// Each colouring's family, and its place among that family's palettes.
    colourings: BTreeMap<&'static str, (usize, usize)>,
    families: Vec<Family>,
}
#[derive(Default)]
struct Family {
    palettes: Vec<&'static str>,
    pictures: BTreeMap<&'static str, Picture>,
}
struct Picture {
    w: usize,
    rows: Vec<&'static str>,
    /// One decoded sprite per colouring of the family.
    drawn: Box<[OnceLock<Sprite>]>,
}

/// Consecutive `%colouring` lines open a family; the `@picture width height` blocks after them
/// belong to it.
fn load() -> Art {
    let mut art = Art {
        colourings: BTreeMap::new(),
        families: Vec::new(),
    };
    let mut lines = include_str!("../data/stage.txt").lines();
    let mut open = false;
    while let Some(line) = lines.next() {
        if let Some(palette) = line.strip_prefix('%') {
            if !open {
                art.families.push(Family::default());
                open = true;
            }
            let (name, colours) = palette.split_once(' ').unwrap_or((palette, ""));
            let index = art.families.len() - 1;
            let family = &mut art.families[index];
            art.colourings.insert(name, (index, family.palettes.len()));
            family.palettes.push(colours);
        } else if let Some(header) = line.strip_prefix('@') {
            open = false;
            let mut parts = header.split(' ');
            let (Some(name), Some(w), Some(h)) = (
                parts.next(),
                parts.next().and_then(|n| n.parse().ok()),
                parts.next().and_then(|n| n.parse().ok()),
            ) else {
                continue;
            };
            let rows = lines.by_ref().take(h).collect();
            if let Some(family) = art.families.last_mut() {
                let drawn = family.palettes.iter().map(|_| OnceLock::new()).collect();
                family.pictures.insert(name, Picture { w, rows, drawn });
            }
        }
    }
    art
}

fn decode(p: &Picture, palette: impl Iterator<Item = Option<Rgb>>) -> Sprite {
    let colours: Vec<Option<Rgb>> = palette.collect();
    let mut s = Sprite::new(p.w, p.rows.len());
    for (y, row) in p.rows.iter().enumerate() {
        for (x, b) in row.bytes().take(p.w).enumerate() {
            // Indexes run from `!`, skipping `.`, which is transparent like a short row.
            let i = b.wrapping_sub(b'!').wrapping_sub(u8::from(b > b'.'));
            if b != b'.' {
                s.px[y * p.w + x] = colours.get(usize::from(i)).copied().flatten();
            }
        }
    }
    s
}

/// Scaled, compensated hypotenuse matching the original V8 arithmetic.
pub(super) fn js_hypot(v: &[f64]) -> f64 {
    let max = v.iter().map(|a| a.abs()).fold(0.0, f64::max);
    if max == 0.0 {
        return 0.0;
    }
    let mut sum = 0.0_f64;
    let mut compensation = 0.0_f64;
    for a in v {
        let q = a.abs() / max;
        let summand = q * q - compensation;
        let preliminary = sum + summand;
        compensation = (preliminary - sum) - summand;
        sum = preliminary;
    }
    sum.sqrt() * max
}
