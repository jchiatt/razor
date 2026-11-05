//! Strip emotes are poses at an age, never queued runs or a scheduler.

// Pose, duration, movement, particle.
type Step = (&'static str, u64, u8, &'static str);

// Motion: 0 still, 1 hop, 2 shake, 3 flip, 4 arcade flicker.
fn steps(name: &str) -> &'static [Step] {
    match name {
        "wave" => &[
            ("wave", 250, 0, ""),
            ("idle", 250, 0, ""),
            ("wave", 250, 0, ""),
            ("idle", 250, 0, ""),
        ],
        "hop" => &[("idle", 500, 1, "")],
        "shake" => &[("idle", 500, 2, "")],
        "blink" => &[("idle", 500, 4, "")],
        "celebrate" => &[("celebrate", 750, 1, "spark"), ("idle", 250, 0, "")],
        "think" => &[("think", 1000, 0, "question")],
        "sleep" => &[("sleep", 1500, 0, "zzz")],
        "heart" => &[("idle", 1000, 0, "heart")],
        "spark" => &[("celebrate", 750, 0, "spark")],
        "zzz" => &[("sleep", 1000, 0, "zzz")],
        "question" => &[("think", 750, 0, "question")],
        "exclaim" => &[("idle", 500, 1, "exclaim")],
        "dance" => &[
            ("dance-left", 250, 0, ""),
            ("dance-right", 250, 1, ""),
            ("flex", 250, 1, ""),
            ("dance-left", 250, 0, ""),
            ("dance-right", 250, 0, ""),
        ],
        "bonk" => &[("bonk-up", 250, 0, ""), ("bonk-down", 375, 0, "spark")],
        "nyah" => &[("nyah", 250, 2, ""), ("nyah", 500, 0, "")],
        "shades" => &[("idle", 125, 0, ""), ("shades", 750, 0, "")],
        "sneak" => &[
            ("sneak", 250, 0, ""),
            ("sneak", 250, 3, ""),
            ("sneak", 250, 0, ""),
        ],
        "facepalm" => &[("idle", 125, 0, ""), ("facepalm", 875, 0, "")],
        "flex" => &[("idle", 125, 0, ""), ("flex", 750, 0, "spark")],
        _ => &[],
    }
}

/// A pose and pixel offsets resolved from the same step as its next change.
#[derive(Clone, Copy)]
pub(super) struct Frame {
    pub(super) pose: &'static str,
    pub(super) dx: i32,
    pub(super) dy: i32,
    pub(super) flip: bool,
    pub(super) visible: bool,
    pub(super) particle: &'static str,
    pub(super) rise: i32,
    pub(super) next: u64,
}

/// Resolves an emote at `age` ms; antics use speed 2, natural finish emotes speed 1.
/// The last step is still with motion off, for the three-second hold.
pub(super) fn frame(name: &str, age: u64, speed: u64, motion: bool) -> Option<Frame> {
    let steps = steps(name);
    if !motion && age >= 3000 {
        return None;
    }
    let mut end = 0;
    let &(pose, ms, movement, particle) = if motion {
        steps.iter().find(|(_, ms, _, _)| {
            end += ms * speed;
            age < end
        })?
    } else {
        steps.last()?
    };
    let start = if motion { end - ms * speed } else { 0 };
    let tick = 125 * speed;
    let k = if motion { (age - start) / tick } else { 0 };
    let even = k.is_multiple_of(2);
    let moving = if motion { movement } else { 0 };
    let next = if !motion {
        3000
    } else if moving == 1 || moving == 2 || moving == 4 {
        end.min(start + (k + 1) * tick)
    } else if !particle.is_empty() {
        end.min(start + (k / 2 + 1) * tick * 2)
    } else {
        end
    };
    Some(Frame {
        pose,
        dx: if moving == 2 {
            if even { 1 } else { -1 }
        } else {
            0
        },
        dy: if moving == 1 && even { -2 } else { 0 },
        flip: moving == 3,
        visible: moving != 4 || even,
        particle,
        rise: (k / 2) as i32 * 2,
        next,
    })
}

/// Particle pixels; their bottom-left starts four pixels before the sprite's edge.
pub(super) fn particle(name: &str) -> &'static [&'static str] {
    match name {
        "heart" => &["dd.dd", "ddddd", "ddddd", ".ddd.", "..d.."],
        "spark" => &["..8..", "..8..", "88.88", "..8..", "..8.."],
        "zzz" => &["33333", "...3.", "..3..", ".3...", "33333"],
        "question" => &[".33.", "3..3", "..3.", "....", "..3."],
        "exclaim" => &["99", "99", "99", "..", "99"],
        _ => &[],
    }
}

/// A stable seed: no platform hash state, random source or growing history.
pub(super) fn seed(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

pub(super) fn random(seed: u64) -> u64 {
    let mut n = seed.wrapping_add(0x9e3779b97f4a7c15);
    n = (n ^ (n >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    n = (n ^ (n >> 27)).wrapping_mul(0x94d049bb133111eb);
    n ^ (n >> 31)
}
