//! The short-pane fallback: faithful avatar pixels under a rule, with pure seeded antics.
use super::super::{App, text::*};
use super::{
    Grid, Seen,
    art::Sprite,
    avatar,
    emotes::{self, Frame},
};
use crate::protocol::{AgentView, SessionId, State};
use ratatui::style::Color;

#[derive(Clone, Copy)]
struct Run {
    member: usize,
    name: &'static str,
    at: u64,
    speed: u64,
    target: Option<usize>,
    hold: u64,
}

impl Run {
    fn new(member: usize, name: &'static str, at: u64, speed: u64, hold: u64) -> Self {
        Self {
            member,
            name,
            at,
            speed,
            target: None,
            hold,
        }
    }
}

/// Draws eight rows of heads or twelve of bodies; `until` is the earliest visible change.
/// The caller owns the fit rule and cache. `u64::MAX` means no frame is requested.
pub(super) fn draw(
    app: &App,
    crew: &[SessionId],
    width: usize,
    bodies: bool,
    now: u64,
    motion: bool,
) -> (Grid, u64) {
    let size = if bodies { 16 } else { 8 };
    let rows = size / 2 + 4;
    let width = width.min(u16::MAX as usize);
    let shown = crew.len().min(64).min((width + 2) / (size + 2));
    let mut grid = vec![vec![(' ', DATA, GLASS); width]; rows];
    grid[0].fill(('─', GRAY_DIM, GLASS));
    if crew.len() > shown {
        let label = format!(" +{} ", crew.len() - shown);
        words(
            &mut grid,
            width.saturating_sub(label.len()),
            0,
            &label,
            GRAY,
        );
    }
    if shown == 0 {
        return (grid, u64::MAX);
    }
    let gap = if shown > 1 {
        ((width - shown * size) / (shown - 1)).clamp(2, 3)
    } else {
        0
    };
    let left = (width - shown * size - (shown - 1) * gap) / 2;
    let mut until = u64::MAX;
    let cue = reaction(app, &crew[..shown], now);
    let last_key = app.stage.last_key;
    let antic = motion
        .then(|| idle(app, crew, shown, last_key, now, &mut until))
        .flatten();
    if motion
        && let Some(run) = antic
        && run.target.is_some()
        && now < run.at + 250
    {
        until = until.min(run.at + 250);
    }
    for (i, id) in crew.iter().take(shown).enumerate() {
        let Some(row) = app.agents.get(id) else {
            continue;
        };
        let x = left + i * (size + gap);
        let seen = app.stage.seen.get(id);
        let run = cue
            .filter(|r| r.member == i)
            .or_else(|| antic.filter(|r| r.member == i));
        let run = run.or_else(|| antic.and_then(|r| partner(r, i, now)));
        let frame = sample(run, id, last_key, now, motion, &mut until);
        let mut canvas = Sprite::new(size, (rows - 1) * 2);
        let px = avatar(id, bodies, frame.pose);
        if frame.visible {
            canvas.blit(
                &px,
                i64::from(frame.dx),
                6 + i64::from(frame.dy),
                frame.flip,
            );
        }
        particles(&mut canvas, &frame, size);
        raster(&mut grid, &canvas, x);
        name(&mut grid, (x, rows - 1), size, row, seen, i == 0, bodies);
    }
    (grid, if motion { until } else { u64::MAX })
}

fn sample(
    run: Option<Run>,
    id: &SessionId,
    last_key: u64,
    now: u64,
    motion: bool,
    until: &mut u64,
) -> Frame {
    if let Some(r) = run {
        let age = now.saturating_sub(r.at);
        if age < r.hold
            && let Some(f) = emotes::frame(r.name, age, r.speed, motion)
        {
            if motion {
                *until = (*until).min(r.at.saturating_add(f.next.min(r.hold)));
            }
            return f;
        }
    }
    if motion && blink(id, last_key, now, until) {
        rest("blink")
    } else {
        rest("idle")
    }
}

fn rest(pose: &'static str) -> Frame {
    Frame {
        pose,
        dx: 0,
        dy: 0,
        flip: false,
        visible: true,
        particle: "",
        rise: 0,
        next: u64::MAX,
    }
}

fn avatar(id: &SessionId, body: bool, pose: &str) -> Sprite {
    let size = if body { "body" } else { "head" };
    let frame = avatar::sprite(id.as_str(), &format!("{size}/{pose}"));
    if frame.w == 0 {
        avatar::sprite(id.as_str(), &format!("{size}/idle"))
    } else {
        frame
    }
}

fn reaction(app: &App, crew: &[SessionId], now: u64) -> Option<Run> {
    let mut selected = None;
    let mut consider = |member, name, at: Option<u64>, speed| {
        if let Some(at) = at.filter(|at| *at <= now && now - *at < 3000)
            && selected.is_none_or(|r: Run| at >= r.at)
        {
            selected = Some(Run::new(member, name, at, speed, 3000));
        }
    };
    let records = &app.stage.records;
    consider(0, "hop", Some(app.presence.opened), 2);
    consider(0, "facepalm", records.failed, 2);
    if let Some((passed, at)) = records.tests.back() {
        consider(0, if *passed { "dance" } else { "shake" }, Some(*at), 2);
    }
    consider(0, "sleep", records.rebuild, 2);
    if let Some(at) = records.input
        && emotes::random(at) % 10 < 3
    {
        consider(
            0,
            if emotes::random(at + 1).is_multiple_of(2) {
                "wave"
            } else {
                "exclaim"
            },
            Some(at),
            2,
        );
    }
    if let Some(at) = app.stage.cleared
        && emotes::random(at) % 10 < 4
    {
        consider(0, "nyah", Some(at), 2);
    }
    for (i, id) in crew.iter().take(64).enumerate() {
        let Some(seen) = app.stage.seen.get(id) else {
            continue;
        };
        if i > 0 {
            consider(i, "hop", seen.joined, 2);
            consider(i, "celebrate", seen.ended, 1);
        }
        if seen.state == State::NeedsYou
            && app.agents.get(id).is_some_and(|r| !r.asks.is_empty())
            && app.stage.clock.is_some_and(|at| seen.activity >= at)
        {
            consider(i, "question", Some(seen.activity), 2);
        }
    }
    selected
}

fn idle(
    app: &App,
    crew: &[SessionId],
    shown: usize,
    last_key: u64,
    now: u64,
    until: &mut u64,
) -> Option<Run> {
    if now.saturating_sub(last_key) < 1500 {
        *until = (*until).min(last_key.saturating_add(1500));
        return None;
    }
    let sleepy = now.saturating_sub(last_key) >= 600_000;
    let period = if sleepy { 60_000 } else { 10_000 };
    let spread = if sleepy { 15_001 } else { 4001 };
    let base = emotes::seed(crew.first()?.as_str());
    let epoch = app.stage.clock.unwrap_or(now);
    let slot = now.saturating_sub(epoch) / period;
    let available: Vec<_> = (0..shown)
        .filter(|i| {
            crew.get(*i)
                .and_then(|id| app.agents.get(id))
                .is_some_and(|r| r.report.is_none())
        })
        .collect();
    if available.is_empty() {
        return None;
    }
    let mut selected = None;
    // Three bounded slots cover jitter on either side of now, without a retained scheduler.
    for slot in slot.saturating_sub(1)..=slot.saturating_add(1) {
        let hash = emotes::random(base.wrapping_add(slot));
        let at = epoch
            .saturating_add(slot.saturating_mul(period))
            .saturating_add(period - spread / 2 + hash % spread);
        if at < last_key.saturating_add(1500) {
            continue;
        }
        if at > now {
            *until = (*until).min(at);
            continue;
        }
        let member = available[slot as usize % available.len()];
        let name = idle_name(slot / available.len() as u64, hash, sleepy);
        let mut run = Run::new(member, name, at, 2, 600 + emotes::random(hash) % 3401);
        if !sleepy && shown > 1 && (slot / available.len() as u64).is_multiple_of(2) && hash % 5 < 3
        {
            run.name = ["bonk", "wave", "dance"][(hash / 10 % 3) as usize];
            run.target = target(app, crew, &available, member, hash, run.name);
        }
        if now - at < run.hold && selected.is_none_or(|r: Run| r.at < at) {
            selected = Some(run);
        }
    }
    troll(app, crew, last_key, now, until).or(selected)
}

fn idle_name(turn: u64, hash: u64, sleepy: bool) -> &'static str {
    if sleepy {
        return if turn.is_multiple_of(2) {
            "sleep"
        } else {
            "zzz"
        };
    }
    // Disjoint alternating pools prevent a member repeating its previous solo antic.
    let pool: &[&str] = if turn.is_multiple_of(2) {
        &["blink", "blink", "blink", "hop", "flex", "spark"]
    } else {
        &["think", "think", "think", "shades", "sneak", "heart"]
    };
    pool[(hash % pool.len() as u64) as usize]
}

fn target(
    app: &App,
    crew: &[SessionId],
    available: &[usize],
    member: usize,
    hash: u64,
    name: &str,
) -> Option<usize> {
    let others: Vec<_> = available.iter().copied().filter(|i| *i != member).collect();
    let idle: Vec<_> = others
        .iter()
        .copied()
        .filter(|i| {
            crew.get(*i)
                .and_then(|id| app.agents.get(id))
                .is_some_and(|a| a.state != State::Working && a.asks.is_empty())
        })
        .collect();
    let candidates = if name == "bonk" && !idle.is_empty() && !hash.is_multiple_of(4) {
        &idle
    } else {
        &others
    };
    if candidates.is_empty() {
        None
    } else {
        candidates
            .get((hash as usize / 4) % candidates.len())
            .copied()
    }
}

fn partner(run: Run, member: usize, now: u64) -> Option<Run> {
    if run.target != Some(member) {
        return None;
    }
    let at = run.at.saturating_add(250);
    if now < at {
        return None;
    }
    Some(Run {
        member,
        at,
        name: if run.name == "dance" {
            "dance"
        } else if emotes::random(at).is_multiple_of(2) {
            "blink"
        } else {
            "hop"
        },
        target: None,
        ..run
    })
}

fn troll(app: &App, crew: &[SessionId], last_key: u64, now: u64, until: &mut u64) -> Option<Run> {
    let row = app.agents.get(crew.first()?)?;
    let working = app.stage.seen.get(crew.first()?).and_then(|s| s.working);
    let high = row
        .context
        .zip(row.window)
        .is_some_and(|(used, total)| total > 0 && used as f64 / total as f64 > 0.85);
    let mut selected = None;
    let mut consider = |start: u64, name, repeat| {
        let at = if repeat && now >= start {
            start + (now - start) / 600_000 * 600_000
        } else {
            start
        };
        if at > now {
            *until = (*until).min(at);
        } else if now - at < 3000 {
            selected = Some(Run::new(0, name, at, 2, 3000));
        } else if repeat {
            *until = (*until).min(at.saturating_add(600_000));
        }
    };
    if high {
        consider(now / 600_000 * 600_000, "sleep", true);
    }
    if let Some(at) = working {
        consider(at.saturating_add(300_000), "question", true);
    }
    if app.composer.text.chars().take(401).count() > 400 {
        consider(last_key.saturating_add(20_000), "question", false);
    }
    selected
}

fn blink(id: &SessionId, last_key: u64, now: u64, until: &mut u64) -> bool {
    if now.saturating_sub(last_key) >= 120_000 {
        return false;
    }
    let slot = now / 6500;
    let seed = emotes::seed(id.as_str());
    let mut visible = false;
    for slot in slot.saturating_sub(1)..=slot + 1 {
        let at = slot * 6500 + 2000 + emotes::random(seed.wrapping_add(slot)) % 2501;
        if at > now {
            *until = (*until).min(at);
        } else if now - at < 160 {
            visible = true;
            *until = (*until).min(at + 160);
        }
    }
    visible
}

fn particles(canvas: &mut Sprite, frame: &Frame, size: usize) {
    let rows = emotes::particle(frame.particle);
    let top = 6 - 1 - frame.rise - rows.len() as i32 + 1;
    for (y, row) in rows.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            let rgb = match ch {
                'd' => Some([255, 158, 200]),
                '8' => Some([242, 216, 90]),
                '3' => Some([216, 223, 229]),
                '9' => Some([255, 138, 42]),
                _ => None,
            };
            if rgb.is_some() {
                canvas.set((size - 4 + x) as i64, i64::from(top) + y as i64, rgb);
            }
        }
    }
}

fn raster(grid: &mut Grid, canvas: &Sprite, x: usize) {
    for y in 0..canvas.h / 2 {
        for u in 0..canvas.w {
            let top = canvas.get(u as i64, (y * 2) as i64);
            let bottom = canvas.get(u as i64, (y * 2 + 1) as i64);
            let rgb = |[r, g, b]: [u8; 3]| Color::Rgb(r, g, b);
            let cell = match (top, bottom) {
                (Some(t), b) => Some(('▀', rgb(t), b.map_or(GLASS, rgb))),
                (None, Some(b)) => Some(('▄', rgb(b), GLASS)),
                _ => None,
            };
            if let Some(cell) = cell
                && let Some(target) = grid.get_mut(y).and_then(|r| r.get_mut(x + u))
            {
                *target = cell;
            }
        }
    }
}

fn name(
    grid: &mut Grid,
    (x, y): (usize, usize),
    room: usize,
    row: &AgentView,
    seen: Option<&Seen>,
    focus: bool,
    bodies: bool,
) {
    let (mark, colour, word) = if row.report.is_some() || seen.is_some_and(|s| s.ended.is_some()) {
        ('×', GRAY_DIM, "ended")
    } else {
        match row.state {
            State::Working => ('●', GREEN, "working"),
            State::Stuck => ('◐', AMBER, "waiting"),
            State::NeedsYou if !row.asks.is_empty() => ('◐', AMBER, "waiting"),
            State::NeedsYou | State::Done => ('○', GRAY, "idle"),
        }
    };
    words(grid, x, y, &mark.to_string(), colour);
    let clean: String = row
        .name
        .chars()
        .filter(|c| !c.is_control() && !format_char(*c) && width(&c.to_string()) == 1)
        .take(256)
        .collect();
    let clean = if clean.trim().is_empty() {
        if focus { "me" } else { row.session.as_str() }
    } else {
        clean.trim()
    };
    let clipped = clip(clean, room.saturating_sub(2));
    words(grid, x + 2, y, &clipped, if focus { WHITE } else { DATA });
    let used = 2 + width(&clipped);
    if bodies && used + 1 + word.len() <= room {
        words(grid, x + used + 1, y, word, GRAY);
    }
}

fn format_char(c: char) -> bool {
    matches!(c as u32,
        0x00AD | 0x0600..=0x0605 | 0x061C | 0x06DD | 0x070F | 0x0890..=0x0891 | 0x08E2 | 0x180E
        | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF
        | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F | 0x1BCA0..=0x1BCA3
        | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}

fn words(grid: &mut Grid, x: usize, y: usize, text: &str, fg: Color) {
    let Some(row) = grid.get_mut(y) else {
        return;
    };
    for (i, ch) in text.chars().enumerate() {
        if let Some(cell) = row.get_mut(x + i) {
            *cell = (ch, fg, GLASS);
        }
    }
}
