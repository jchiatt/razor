//! A member's id picks its character, variant, accent and colour slots. Each variant's shape
//! uses slot letters where the id chooses its colour (`avatar/<character>/<variant>`).
use super::art::{self, Sprite};

/// Character order, variant counts and hand slots determine each id's avatar.
const CHARACTERS: [(&str, u32, char); 7] = [
    ("bot", 3, '2'),
    ("critter", 3, 'A'),
    ("shroom", 3, '2'),
    ("kid", 3, 'P'),
    ("owl", 2, 'A'),
    ("ghost", 2, 'A'),
    ("crab", 2, 'K'),
];
const PALETTE: [u32; 16] = [
    0x0b0e14, 0x1f2d52, 0x6e7a88, 0xd8dfe5, 0x3c5cf0, 0x3fd9f2, 0x3ee56d, 0x1d7a45, 0xf2d85a,
    0xff8a2a, 0xff3d32, 0xff5bdc, 0x8f5cf0, 0xff9ec8, 0xe8b089, 0x7c4a2d,
];

/// `body/<pose>` or `head/<pose>` for `id`; empty for a missing pose.
pub(super) fn sprite(id: &str, picture: &str) -> Sprite {
    // Mulberry32, seeded with the id's FNV-1a hash.
    let mut state = art::hash(id);
    let mut next = || {
        state = state.wrapping_add(0x6d2b_79f5);
        let mut t = state;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        t ^ (t >> 14)
    };
    let (name, variants, hand) = CHARACTERS[next() as usize % CHARACTERS.len()];
    let variant = next() % variants;
    // Accents are never pale, or tan for the mushroom.
    let accents: Vec<u8> = [2, 4, 6, 7, 9, 11, 12, 13, 14, 15]
        .into_iter()
        .filter(|a| name != "shroom" || *a != 14)
        .collect();
    let a = accents[next() as usize % accents.len()];
    let mut slots = slots(name, a, [next(), next(), next()]);
    let hand = slots.iter().find(|(s, _)| *s == hand).map_or(2, |s| s.1);
    // The gestures paint palms in the hand's shade.
    slots.push(('Q', shade(hand)));
    art::inked(&format!("avatar/{name}/{variant}"), picture, |c| {
        let digit = if c.is_ascii_uppercase() {
            slots.iter().find(|(s, _)| *s == c)?.1
        } else {
            c.to_digit(16)? as u8
        };
        PALETTE.get(usize::from(digit)).map(|n| art::rgb(*n))
    })
}

/// Colours for the character's slots, at accent `a` with picks `p`.
fn slots(name: &str, a: u8, p: [u32; 3]) -> Vec<(char, u8)> {
    let pick = |xs: &[u8], n: usize| xs[p[n] as usize % xs.len()];
    let light = match a {
        4 => 12,
        7 => 6,
        9 | 15 => 14,
        11 | 12 => 13,
        _ => 3,
    };
    let mut slots = vec![('A', a), ('S', shade(a)), ('H', light)];
    match name {
        "bot" => slots.extend([
            ('K', if a == 2 { 1 } else { 2 }),
            ('E', pick(&[6, 9, 13, 11, 6], 0)),
            ('T', pick(&[9, 11, 6, 13], 1)),
        ]),
        "critter" => slots.extend([
            ('C', if a == 13 { 11 } else { 13 }),
            ('K', if a == 14 { 3 } else { pick(&[3, 14], 0) }),
        ]),
        "kid" => {
            let skin = match a {
                14 => 15,
                15 => 14,
                _ => pick(&[14, 15], 0),
            };
            let hairs: &[u8] = if skin == 15 {
                &[2, 12, 9, 11]
            } else {
                &[15, 9, 12, 2, 11]
            };
            let hair = pick(hairs, 1);
            // Hair is never the shirt's accent.
            let hair = match (hair == a, skin) {
                (false, _) => hair,
                (true, 15) => 2,
                (true, _) => 15,
            };
            let shirt = if a == 4 { 1 } else { pick(&[1, 4, 2], 2) };
            slots.extend([('P', skin), ('R', hair), ('K', shirt)]);
        }
        "owl" => slots.extend([
            ('K', if a == 14 { 3 } else { pick(&[14, 3], 0) }),
            ('L', if a == 14 { 2 } else { 15 }),
            ('B', if a == 9 { 15 } else { 9 }),
        ]),
        "ghost" | "crab" => {
            // The ghost's hat and the crab's claws are never the accent.
            let pool = if name == "ghost" {
                [12, 9, 15, 11]
            } else {
                [9, 13, 14, 3]
            };
            let pool: Vec<u8> = pool.into_iter().filter(|c| *c != a).collect();
            slots.push(('K', pick(&pool, 0)));
        }
        _ => {}
    }
    slots
}

fn shade(a: u8) -> u8 {
    match a {
        3 => 2,
        6 => 7,
        9 | 14 => 15,
        11 => 12,
        13 => 11,
        _ => 1,
    }
}
