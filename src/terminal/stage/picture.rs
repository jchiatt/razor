//! A room scene becomes layered pixels; only light and moving sky remain arithmetic.
use super::super::motion::ease_out;
use super::art::{self, Pose, Rgb, Sprite, mix, rgb};

pub(super) const BENCH_Y: f64 = 18.0;
pub(super) const SEAT_Y: f64 = 8.0;
/// States control the name tag, never a creature's identity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ActorState {
    Working,
    #[default]
    Idle,
    Waiting,
    Ended,
}
/// The window's skies: the hour's, or a test run's rain and rainbow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Sky {
    #[default]
    Day,
    Rain,
    Rainbow,
    Night,
    Dusk,
    Dawn,
}
/// One crew member, placed by its centre and portrait top in room units.
#[derive(Clone, Debug, Default)]
pub(super) struct Actor {
    pub(super) who: usize,
    pub(super) name: String,
    pub(super) state: ActorState,
    pub(super) x: f64,
    pub(super) y: Option<f64>,
    pub(super) pose: Pose,
    pub(super) flip: bool,
    pub(super) is_self: bool,
    pub(super) hidden: bool,
    pub(super) asleep: bool,
    pub(super) arm_up: bool,
    pub(super) carry: Option<Placed>,
    pub(super) dim: f64,
    pub(super) behind: bool,
    pub(super) tag_at: Option<f64>,
    pub(super) wear: Option<Placed>,
    pub(super) accent: usize,
}
/// A named exported prop, centred horizontally and placed by its foot.
#[derive(Clone, Debug, Default)]
pub(super) struct Placed {
    pub(super) name: &'static str,
    pub(super) frame: usize,
    pub(super) x: f64,
    pub(super) y: Option<f64>,
}
/// A bounded radial firework; pane coordinates are local to the window.
#[derive(Clone, Debug, Default)]
pub(super) struct Burst {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) r: f64,
    pub(super) age: f64,
    pub(super) pane: bool,
}
/// The director's complete room; its caller supplies the width and lamp position. No Post,
/// quote-board or tooling surfaces are retained.
#[derive(Clone, Debug, Default)]
pub(super) struct Scene {
    pub(super) actors: Vec<Actor>,
    pub(super) lamp: f64,
    pub(super) swing: f64,
    pub(super) bench: Vec<Placed>,
    pub(super) air: Vec<Placed>,
    pub(super) sky: Sky,
    pub(super) tick: i64,
    pub(super) bow: f64,
    pub(super) dark: f64,
    pub(super) bright: f64,
    pub(super) w: usize,
    pub(super) bursts: Vec<Burst>,
    pub(super) pane: Vec<Placed>,
    pub(super) note: Option<String>,
    pub(super) note_fg: Option<Rgb>,
    pub(super) z: Option<f64>,
    /// The project's merges, how far the newest frame has dropped into place (0 to 1), and the
    /// plant's stage (0, an empty pot, to 5, flowering); no plant while the project is unknown.
    pub(super) frames: u64,
    pub(super) hang: f64,
    pub(super) plant: Option<usize>,
}

/// Paints catalogue pixels over the painted furniture, preserving half-unit movement in the
/// octant room.
pub(super) fn paint(sc: &Scene, octant: bool) -> Sprite {
    let k = if octant { 3 } else { 1 };
    let mut c = room(sc.w as i64, k as i64, &window(sc, k, octant));
    memory(&mut c, sc, k as i64);
    light_ground(&mut c, sc, k, true);
    for behind in [true, false] {
        for a in sc.actors.iter().filter(|a| !a.hidden && a.behind == behind) {
            actor(&mut c, sc, a, octant);
        }
    }
    // The bench's lit edge and top, two units deep: the front with the names is cells.
    let mut bench = Sprite::new(c.w, c.h);
    let (top, wide) = (BENCH_Y as i64 * k as i64, c.w as i64);
    rect(&mut bench, 0, top, wide, k as i64, rgb(0x3a434b));
    rect(&mut bench, 0, top + k as i64, wide, k as i64, rgb(0x2b3238));
    light_ground(&mut bench, sc, k, false);
    c.blit(&bench, 0, 0, false);
    for a in sc.actors.iter().filter(|a| !a.hidden && a.asleep) {
        let mask = art::actor(a.who, a.accent, &format!("sleep/{}", art::density(octant)));
        let mask = light_sprite(mask.clone(), light_on(a.x, SEAT_Y, sc.lamp, sc.bright), sc);
        c.blit(&mask, rd((a.x - 5.0) * k as f64), 0, false);
    }
    for p in &sc.bench {
        place(&mut c, sc, p, k, octant, true);
    }
    lamp(&mut c, sc, k as i64);
    for b in sc.bursts.iter().filter(|b| !b.pane) {
        burst(&mut c, k, b, 0.0, 0.0, None);
    }
    for p in &sc.air {
        place(&mut c, sc, p, k, octant, false);
    }
    c
}
const FRAME: Rgb = rgb(0x262d33);
const FRAME_HI: Rgb = rgb(0x343c43);
const FRAME_LO: Rgb = rgb(0x1b2126);
const RAIL: Rgb = rgb(0x171e24);
const RAIL_HI: Rgb = rgb(0x232b32);
const SEAM: Rgb = rgb(0x0a0f13);
fn rect(c: &mut Sprite, x: i64, y: i64, w: i64, h: i64, col: Rgb) {
    for j in y..y + h {
        for i in x..x + w {
            c.set(i, j, Some(col));
        }
    }
}
/// The empty room, `w` units wide at `k` pixels a unit; `sky` shows through its window.
fn room(w: i64, k: i64, sky: &Sprite) -> Sprite {
    let mut c = Sprite::new((w * k) as usize, (20 * k) as usize);
    let unit = |c: &mut Sprite, x: i64, y: i64, cw: i64, ch: i64, col| {
        rect(c, x * k, y * k, cw * k, ch * k, col);
    };
    unit(&mut c, 0, 0, w, 20, rgb(0x0d1318));
    unit(&mut c, 0, 12, w, 6, rgb(0x0f151a));
    if k > 1 {
        for x in (4..w * k).step_by(8) {
            rect(&mut c, x, 1, 1, 12 * k - 1, SEAM);
        }
        for x in (2..w * k - 10).step_by(22) {
            rect(&mut c, x, 12 * k + 3, 20, 1, SEAM);
            rect(&mut c, x, 12 * k + 4, 1, 10, SEAM);
            rect(&mut c, x + 1, 12 * k + 14, 19, 1, rgb(0x121a20));
            rect(&mut c, x + 20, 12 * k + 4, 1, 11, rgb(0x121a20));
        }
    }
    unit(&mut c, 0, 12, w, 1, RAIL);
    unit(&mut c, 0, 0, w, 1, RAIL);
    unit(&mut c, 1, 1, 10, 4, FRAME);
    for x in [2, 5, 8] {
        unit(&mut c, x, 2, 2, 2, rgb(0x06090c));
    }
    if k == 1 {
        c.set(2, 3, Some(rgb(0x7d858a)));
        c.set(3, 3, Some(rgb(0x5d656a)));
        c.set(9, 2, Some(rgb(0x8a7a62)));
        c.set(9, 3, Some(rgb(0x6a5a44)));
    } else {
        rect(&mut c, 0, 12 * k, w * k, 1, RAIL_HI);
        rect(&mut c, 0, 1, w * k, 1, RAIL_HI);
        for x in (6..w * k).step_by(30) {
            rect(&mut c, x, 0, 2, 3, FRAME);
        }
        rect(&mut c, k, k, 10 * k, 1, FRAME_HI);
        rect(&mut c, k, 5 * k - 1, 10 * k, 1, FRAME_LO);
        for x in [2, 5, 8] {
            rect(&mut c, x * k, 2 * k, 2 * k, 1, rgb(0x030507));
        }
        // A letter with a red seal leaning in the first cubby, a rolled note in the third.
        rect(&mut c, 2 * k + 1, 3 * k - 1, 4, 4, rgb(0x8a9298));
        rect(&mut c, 2 * k + 1, 3 * k - 1, 4, 1, rgb(0xa7aeb3));
        c.set(2 * k + 3, 3 * k + 1, Some(rgb(0xb0453a)));
        rect(&mut c, 8 * k + 2, 2 * k + 2, 2, 4, rgb(0x9a8a6e));
        rect(&mut c, 8 * k + 2, 2 * k + 2, 1, 4, rgb(0xb8a684));
        rect(&mut c, 8 * k + 4, 2 * k + 3, 1, 3, rgb(0x6a5a44));
    }
    let x = w - 12;
    unit(&mut c, x, 1, 11, 6, FRAME);
    c.blit(sky, (x + 1) * k, 2 * k, false);
    unit(&mut c, x + 5, 2, 1, 4, FRAME);
    if k > 1 {
        rect(&mut c, x * k, k, 11 * k, 1, FRAME_HI);
        rect(&mut c, (x + 5) * k + 1, 2 * k, 1, 4 * k, FRAME_HI);
        rect(&mut c, (x + 1) * k, 4 * k, 9 * k, 1, FRAME);
    }
    unit(&mut c, x - 1, 7, 13, 1, FRAME_HI);
    if k > 1 {
        rect(&mut c, (x - 1) * k, 8 * k - 1, 13 * k, 1, FRAME_LO);
    }
    c
}
// The newest merges framed in up to two rows hung from the rail between the pigeonholes and the
// window, each a wood border round two bands from a rotation of eight, and the sill's plant.
fn memory(c: &mut Sprite, sc: &Scene, k: i64) {
    const BANDS: [u32; 8] = [
        0xc8553d, 0x2b90a1, 0xe0b04a, 0x5a9a4a, 0xd6a0ff, 0x4a6fa5, 0xe07ab0, 0x8a9298,
    ];
    let space = sc.w as i64 - 26;
    let per = ((space + 1) / 4).clamp(0, 4);
    let n = sc.frames.min(2 * per as u64) as i64;
    for i in 0..n {
        let number = (sc.frames as i64 - n + i) as usize;
        let drop = if i + 1 == n { 1. - sc.hang } else { 0. };
        let x = (13 + (space + 1 - 4 * per) / 2 + 4 * (i % per)) * k;
        let y = (1 + 3 * (i / per)) * k - (drop * 2. * k as f64).round() as i64;
        let b = i64::from(k > 1);
        rect(c, x, y, 3 * k, 2 * k, rgb(0x6e5436));
        let low = rgb(BANDS[(number * 5 + 3) % 8]);
        rect(c, x + 1, y + b, 3 * k - 2, k - b, rgb(BANDS[number % 8]));
        rect(c, x + 1, y + k, 3 * k - 2, k - b, low);
    }
    let Some(stage) = sc.plant else {
        return;
    };
    let unit = |c: &mut Sprite, x: i64, y: i64, w: i64, col| rect(c, x * k, y * k, w * k, k, col);
    let x = sc.w as i64 - 11;
    unit(c, x - 1, 6, 3, rgb(0x8a4b32));
    // A stem a unit a stage, from the second its leaves alternating sides.
    for t in 1..=stage.min(4) as i64 {
        unit(c, x, 6 - t, 1, rgb(0x2f6b33));
        if t > 1 {
            unit(c, x + 1 - 2 * (t % 2), 6 - t, 1, rgb(0x4f8f43));
        }
    }
    if stage == 5 {
        unit(c, x, 1, 1, rgb(0xe07ab0));
    }
}

/// The lamp scaled whole from one pixel a unit, its shade swung by at most eight units.
fn lamp(c: &mut Sprite, sc: &Scene, k: i64) {
    let x = rd(sc.lamp);
    let s = x + (rd(sc.lamp + sc.swing) - x).clamp(-8, 8);
    let hot = sc.dark > 0.5 || sc.bright > 0.4;
    let unit = |c: &mut Sprite, x: i64, y: i64, w: i64, col| rect(c, x * k, y * k, w * k, k, col);
    unit(c, x - 1, 1, 3, FRAME);
    unit(c, rd((x + s) as f64 / 2.0), 2, 1, rgb(0x232a30));
    unit(c, s - 1, 3, 3, rgb(0x4c3c2a));
    unit(c, s - 3, 4, 7, rgb(0x6e5436));
    unit(c, s - 1, 5, 3, rgb(if hot { 0xfff0d8 } else { 0xffc58a }));
}
fn rd(x: f64) -> i64 {
    (x + 0.5).floor() as i64
}
/// The window's sky with its fireworks and passing props, nine units by four.
fn window(sc: &Scene, k: usize, octant: bool) -> Sprite {
    let (name, frame) = match sc.sky {
        Sky::Day => ("day", 0),
        Sky::Night => ("night", sc.tick.clamp(0, 5) as usize),
        Sky::Rain => ("rain", sc.tick.clamp(0, 24) as usize),
        Sky::Rainbow => (
            "rainbow",
            (0..9 * k)
                .filter(|x| (*x as f64 + 0.5) / (k as f64) < sc.bow * 9.0)
                .count(),
        ),
        Sky::Dusk => ("dusk", 0),
        Sky::Dawn => ("dawn", 0),
    };
    let picture = format!("{name}/{frame}/{}", art::density(octant));
    let mut sky = art::sprite("sky", &picture).clone();
    for b in sc.bursts.iter().filter(|b| b.pane) {
        burst(
            &mut sky,
            k,
            b,
            sc.w.saturating_sub(11) as f64,
            2.0,
            Some((9.0, 4.0)),
        );
    }
    for p in &sc.pane {
        let s = art::paint_prop(p.name, p.frame, octant);
        let x = rd(p.x * k as f64 + if octant { 1.0 } else { 0.5 } - s.w as f64 / 2.0);
        let y = rd(p.y.unwrap_or(4.0) * k as f64 - s.h as f64);
        sky.blit(s, x, y, false);
    }
    sky
}
fn actor(c: &mut Sprite, sc: &Scene, a: &Actor, octant: bool) {
    let y = a.y.unwrap_or(SEAT_Y + if a.asleep { 3.0 } else { 0.0 });
    let pose = if a.asleep { Pose::Sleep } else { a.pose };
    let picture = format!("{}/{}", pose as usize, art::density(octant));
    let mut body = art::actor(a.who, a.accent, &picture).clone();
    if a.dim > 0.0 {
        for p in body.px.iter_mut().flatten() {
            *p = mix(*p, [13, 19, 24], a.dim);
        }
    }
    let lit = light_on(a.x, a.y.unwrap_or(SEAT_Y), sc.lamp, sc.bright);
    let body = light_sprite(body, lit, sc);
    let (x0, y0) = (rd(a.x * 2.0) - 10, rd(y * 2.0));
    if let Some(p) = &a.wear {
        attachment(c, sc, a, p, y, octant, true);
    }
    if octant {
        blit_subpixels(c, &body, x0, y0, a.flip);
    } else {
        c.blit(&body, rd(a.x - 5.0), rd(y), a.flip);
    }
    if a.arm_up {
        let mask = art::actor(a.who, a.accent, &format!("arm/{}", art::density(octant)));
        let mask = light_sprite(mask.clone(), lit, sc);
        if octant {
            blit_subpixels(c, &mask, x0, y0, false);
        } else {
            c.blit(&mask, rd(a.x - 5.0), rd(y), false);
        }
    }
    if let Some(p) = &a.carry {
        attachment(c, sc, a, p, y, octant, false);
    }
}
fn attachment(c: &mut Sprite, sc: &Scene, a: &Actor, p: &Placed, y: f64, octant: bool, worn: bool) {
    let (sx, sy) = (rd(a.x * 2.0), rd(y * 2.0));
    let k = if octant { 3.0 } else { 1.0 };
    let left = if octant {
        sx.div_euclid(2) as f64 - 5.0
    } else {
        a.x - 5.0
    };
    let top = if octant { sy.div_euclid(2) as f64 } else { y };
    let s = art::paint_prop(p.name, p.frame, octant);
    let mut layer = Sprite::new(c.w, c.h);
    if worn {
        layer.blit(
            s,
            rd(left * k + if octant { 27.0 } else { 10.0 } - s.w as f64),
            rd((top - 3.0) * k),
            false,
        );
    } else {
        let hx = left * k
            + if a.flip {
                if octant { -2.0 } else { -1.0 }
            } else if octant {
                31.0
            } else {
                11.0
            };
        let x = if a.flip {
            hx - s.w as f64 + if octant { 2.0 } else { 1.0 }
        } else {
            hx - if octant { 1.0 } else { 0.0 }
        };
        if octant {
            layer.blit(s, rd(x), rd(top * k + 29.0 - s.h as f64), false);
        }
        let hand = art::actor(a.who, a.accent, &format!("hand/{}", art::density(octant)));
        layer.blit(
            hand,
            rd(hx - if octant { 1.0 } else { 0.0 }),
            rd(top * k + if octant { 24.0 } else { 8.0 }),
            false,
        );
        // At cell density the prop covers the holding pixel; painted paws cover the grip.
        if !octant {
            layer.blit(s, rd(x), rd(top * k + 10.0 - s.h as f64), false);
        }
    }
    let layer = light_sprite(
        layer,
        light_on(a.x, a.y.unwrap_or(SEAT_Y), sc.lamp, sc.bright),
        sc,
    );
    if octant {
        blit_subpixels(c, &layer, sx.rem_euclid(2), sy.rem_euclid(2), false);
    } else {
        c.blit(&layer, 0, 0, false);
    }
}
fn place(c: &mut Sprite, sc: &Scene, p: &Placed, k: usize, octant: bool, lit: bool) {
    let s = art::prop(p.name, p.frame, octant);
    let foot = p.y.unwrap_or(BENCH_Y);
    let s = if lit {
        light_sprite(s.clone(), light_on(p.x, foot - 6.0, sc.lamp, sc.bright), sc)
    } else {
        s.clone()
    };
    let x = p.x * k as f64 + if octant { 1.0 } else { 0.5 } - s.w as f64 / 2.0;
    let y = foot * k as f64 + if octant { 3.0 } else { 1.0 } - s.h as f64;
    if octant {
        blit_subpixels(c, &s, rd(x / 1.5), rd(y / 1.5), false);
    } else {
        c.blit(&s, rd(x), rd(y), false);
    }
}
/// The original lamp's cone, including its faint halo above the shade.
pub(super) fn pool(u: f64, v: f64, lx: f64, reach: f64) -> f64 {
    let dx = u - lx;
    let dy = v - 5.0;
    let d = art::js_hypot(&[dx / 1.2, dy]);
    let radial = (1.0 - d / reach).max(0.0).powi(2);
    if dy <= 0.0 {
        return radial * 0.25;
    }
    // Beyond its reach the cone adds nothing; this skips most of the room's arctangents.
    if radial == 0.0 {
        return 0.0;
    }
    let angle = dx.abs().atan2(dy);
    let cone = if angle < 0.62 {
        1.0
    } else {
        (1.0 - (angle - 0.62) / 0.5).max(0.0)
    };
    radial * (0.3 + 0.7 * cone)
}
/// A face receives a single light level, never a half-lit expression.
pub(super) fn light_on(x: f64, y: f64, lx: f64, boost: f64) -> f64 {
    let d = art::js_hypot(&[(x - lx) / 1.2, y + 4.0 - 5.0]);
    ((1.0 - d / (12.0 + boost * 14.0)).max(0.0).powf(1.5) + boost * 0.25).min(1.0)
}
fn light_ground(c: &mut Sprite, sc: &Scene, k: usize, skip_sky: bool) {
    let reach = 15.0 - sc.dark * 2.0 + sc.bright * 9.0;
    let strength = 0.5 + sc.dark * 0.2 + sc.bright * 0.2;
    let glow = mix([122, 90, 44], [148, 105, 44], sc.dark);
    let sky = sc.w.saturating_sub(11) * k..sc.w.saturating_sub(2) * k;
    for (y, row) in c.px.chunks_mut(c.w.max(1)).enumerate() {
        let v = (y as f64 + 0.5) / k as f64;
        let window = skip_sky && (2 * k..6 * k).contains(&y);
        for (x, pixel) in row.iter_mut().enumerate() {
            let Some(p) = *pixel else {
                continue;
            };
            if window && sky.contains(&x) {
                continue;
            }
            let t = pool((x as f64 + 0.5) / k as f64, v, sc.lamp, reach);
            let base = if sc.dark > 0.0 {
                mix(p, [5, 8, 11], sc.dark * 0.62 * (1.0 - t))
            } else {
                p
            };
            *pixel = Some(mix(base, glow, (t * strength).min(1.0)));
        }
    }
}
fn light_sprite(mut s: Sprite, t: f64, sc: &Scene) -> Sprite {
    let dim_to = mix([13, 19, 24], [5, 8, 11], sc.dark);
    let dim = (0.3 + sc.dark * 0.38) * (1.0 - t);
    let warm = (0.12 + sc.dark * 0.04 + sc.bright * 0.1) * t;
    for p in s.px.iter_mut().flatten() {
        *p = mix(mix(*p, dim_to, dim), [255, 217, 168], warm);
    }
    s
}

fn blit_subpixels(c: &mut Sprite, s: &Sprite, u0: i64, v0: i64, flip: bool) {
    let (w, h) = ((s.w * 2 / 3) as i64, (s.h * 2 / 3) as i64);
    // ceil(3*x/2), also for negative origins: each subpixel paints one or two pixels.
    let edge = |x: i64| (x * 3 + 1).div_euclid(2);
    for v in 0..h {
        for u in 0..w {
            let from = if flip { w - 1 - u } else { u };
            if let Some(p) = s.get((from * 3 + 1) / 2, (v * 3 + 1) / 2) {
                rect(
                    c,
                    edge(u0 + u),
                    edge(v0 + v),
                    edge(u0 + u + 1) - edge(u0 + u),
                    edge(v0 + v + 1) - edge(v0 + v),
                    p,
                );
            }
        }
    }
}

fn floor(x: f64) -> i64 {
    x.floor() as i64
}
fn burst(c: &mut Sprite, k: usize, b: &Burst, ox: f64, oy: f64, clip: Option<(f64, f64)>) {
    let kf = k as f64;
    let set = |c: &mut Sprite, u: f64, v: f64, col: Rgb| {
        if let Some((cw, ch)) = clip
            && (u < 0.0 || v < 0.0 || u >= cw || v >= ch)
        {
            return;
        }
        c.set(
            floor((ox + u) * kf) - floor(ox * kf),
            floor((oy + v) * kf) - floor(oy * kf),
            Some(col),
        );
    };
    let ember = [10, 15, 19];
    let cols = [[255, 61, 50], [238, 242, 244], [60, 92, 240]];
    let first = cols[0];
    if b.age < 0.25 {
        let t = b.age / 0.25;
        let v = b.y + b.r * 1.8 * (1.0 - ease_out(t));
        set(c, b.x, v, mix(first, [255, 255, 255], 0.5));
        set(c, b.x, v + 1.0 / kf, mix(first, ember, 0.4));
        if k > 1 {
            set(c, b.x, v + 2.0 / kf, mix(first, ember, 0.7));
        }
        return;
    }
    let e = (b.age - 0.25) / 0.75;
    let rad = b.r * ease_out(1.0_f64.min(e * 1.5));
    let drop = e * e * b.r * 0.45;
    let n: i64 = if k == 1 { 8 } else { 14 };
    for i in 0..n {
        // late in its life an ember goes out early, every other one first
        if e > 0.75 && i % 2 == 1 {
            continue;
        }
        let a = (i as f64 / n as f64) * std::f64::consts::PI * 2.0 + 0.3;
        let base = cols[i as usize % cols.len()];
        let col = mix(base, ember, e.powf(1.6) * 0.8);
        let dx = a.cos();
        let dy = a.sin() * 0.85;
        set(c, b.x + dx * rad, b.y + dy * rad + drop, col);
        // in half blocks, a second, inner ring while it is young, so eight embers read as one burst
        if k == 1 && e < 0.55 {
            set(c, b.x + dx * rad * 0.5, b.y + dy * rad * 0.5 + drop, col);
        }
        if k > 1 {
            for j in 1..=2_i64 {
                let fj = j as f64;
                let tr = rad - (fj * 0.55) / (1.0 + e);
                if tr > 0.2 {
                    set(
                        c,
                        b.x + dx * tr,
                        b.y + dy * tr + drop * 0.8,
                        mix(col, ember, 0.3 * fj),
                    );
                }
            }
        }
    }
    if e < 0.18 {
        set(c, b.x, b.y, [255, 244, 220]);
        if k > 1 {
            for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                set(c, b.x + dx / kf, b.y + dy / kf, first);
            }
        }
    }
}
