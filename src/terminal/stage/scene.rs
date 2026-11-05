//! A room is a pure function of record times, status observations and the local clock.
use super::super::App;
use super::{
    Stage,
    art::{self, Pose},
    observations::Prop,
    picture::{Actor, ActorState, Placed, Scene, Sky},
};
use crate::protocol::{AgentView, SessionId, State};

/// Returns twelve room rows and the earliest instant their cells can change.
pub(super) fn draw(
    app: &App,
    crew: &[SessionId],
    width: usize,
    now: u64,
    motion: bool,
    octant: bool,
) -> (super::Grid, u64) {
    let mut scene = Scene {
        w: width,
        lamp: round(width as f64 / 2.0),
        ..Scene::default()
    };
    let mut wake = Wake {
        now,
        motion: motion && !app.stage.resting,
        at: hour(app, now),
    };
    cast(app, crew, &mut scene);
    lamp(&mut scene, &app.stage, crew, &mut wake);
    running(&mut scene, &app.stage, &mut wake);
    sky(&mut scene, &app.stage, app, &mut wake);
    moments(&mut scene, &app.stage, crew, &mut wake);
    super::eggs::draw(&mut scene, &app.stage, app, &mut wake);
    // Razor's hello: the crew raise their arms twice in its first second.
    let opened = app.presence.opened;
    let age = wake.age(opened, 750);
    for a in &mut scene.actors {
        a.arm_up |= age < 750 && age / 250 != 1;
    }
    wake.animate(opened, opened + 750, 250);
    memory(&mut scene, &app.stage, &mut wake);
    rest(&mut scene, app, crew, &mut wake);
    oil(&mut scene, app, &mut wake);
    (super::cells::draw(&scene, octant), wake.at)
}

// A frame on the wall for each merge, the newest dropping into place as it hangs, and the sill's
// plant: an empty pot, a sprout from 1 commit, a leaf from 10, two from 50, three (grown) from
// 200, flowering from 1,000.
fn memory(scene: &mut Scene, stage: &Stage, w: &mut Wake) {
    let Some((commits, merges)) = stage.project else {
        return;
    };
    let plant = [1, 10, 50, 200, 1000].partition_point(|n| *n <= commits);
    (scene.frames, scene.plant) = (merges, Some(plant));
    scene.hang = super::super::motion::ease_out(w.age(stage.hung, 600) as f64 / 600.);
    w.animate(stage.hung, stage.hung + 600, 125);
}

// Past 85% of the focused context the lamp's light dips by a tenth for 120 ms once in each five
// seconds, at an instant seeded by the session; a rebuild refills it, blooming four seconds on.
fn oil(scene: &mut Scene, app: &App, w: &mut Wake) {
    if !w.motion {
        return;
    }
    let bloom = app.stage.records.rebuild.filter(|at| *at > 0);
    if let Some(at) = bloom.map(|at| at + 4000) {
        let age = w.now.saturating_sub(at) as f64;
        if w.now < at {
            w.at(at);
        } else {
            scene.bright += 0.5 * (age / 200.).min((800. - age) / 600.).max(0.);
            w.animate(at, at + 800, 125);
        }
    }
    let Some(id) = app.focus.as_ref().filter(|id| {
        let row = app.agents.get(*id);
        row.and_then(super::super::gauge::share)
            .is_some_and(|s| s > 85.)
    }) else {
        return;
    };
    let seed = super::emotes::seed(id.as_str());
    let dip = |slot: u64| slot * 5000 + 1500 + super::emotes::random(seed ^ slot) % 2000;
    let at = dip(w.now / 5000);
    if (at..at + 120).contains(&w.now) {
        scene.bright -= 0.17;
    }
    for t in [at, at + 120, dip(w.now / 5000 + 1)] {
        w.at(t);
    }
}

/// When the room rests: ten minutes after the newer of the last key and the crew's last
/// activity; `None` while anyone in the crew works.
pub(super) fn rests(app: &App, crew: &[SessionId]) -> Option<u64> {
    let mut last = app.stage.last_key;
    for seen in crew.iter().filter_map(|id| app.stage.seen.get(id)) {
        if seen.state == State::Working {
            return None;
        }
        last = last.max(seen.activity);
    }
    Some(last.saturating_add(600_000))
}

// Resting, the lamp is at half and the crew asleep and still (the room's own motion stops with
// it). The first draw awake brings the lamp up over 300 ms and the crew blinks once.
fn rest(scene: &mut Scene, app: &App, crew: &[SessionId], w: &mut Wake) {
    if app.stage.resting {
        scene.bright = -0.5;
        scene.actors.iter_mut().for_each(|a| a.asleep = true);
        return;
    }
    if let Some(at) = rests(app, crew) {
        w.at(at);
    }
    let age = w.age(app.stage.woke, 375);
    if age < 375 {
        scene.bright = -0.5 * (1. - age as f64 / 300.).max(0.);
        scene.actors.iter_mut().for_each(|a| a.pose = Pose::Blink);
        w.animate(app.stage.woke, app.stage.woke + 375, 125);
    }
}

fn cast(app: &App, crew: &[SessionId], scene: &mut Scene) {
    let width = scene.w;
    let root = app.root.as_ref().or(app.focus.as_ref());
    let root = root.map_or("self", |id| {
        app.agents
            .get(id)
            .and_then(|r| r.resident.as_ref())
            .unwrap_or(id)
            .as_str()
    });
    if crew.is_empty() {
        scene.actors.push(Actor {
            who: art::member("self"),
            name: "me".into(),
            x: scene.lamp,
            is_self: true,
            ..Actor::default()
        });
    }
    let seats = crew.len().min(4).min(width.saturating_sub(1) / 9);
    let gap = 14.min(width.saturating_sub(1) / seats.max(2));
    let left = round((width - gap * seats.saturating_sub(1)) as f64 / 2.0);
    for (i, id) in crew.iter().take(seats).enumerate() {
        let row = app.agents.get(id);
        let state = state(row, i == 0);
        let x = left + (i * gap) as f64;
        let fill =
            row.and_then(|r| Some(r.context? as f64 / r.window.filter(|n| *n > 0)? as f64 * 100.));
        let accent = if i == 0 {
            0
        } else {
            (art::hash(id.as_str()) as usize % 11) + 1
        };
        scene.actors.push(Actor {
            who: art::member(root),
            name: name(row, i == 0),
            state,
            x,
            is_self: i == 0,
            pose: if fill.is_some_and(|f| f > 80.) {
                Pose::Messy
            } else {
                Pose::Idle
            },
            dim: if state == ActorState::Ended { 0.5 } else { 0. },
            accent,
            ..Actor::default()
        });
        if let Some(fill) = fill {
            let mug = if fill < 34. {
                "mug_full"
            } else if fill < 67. {
                "mug_half"
            } else {
                "mug_empty"
            };
            scene.bench.push(placed(mug, 0, x + 3., None));
        }
    }
    if crew.len() > seats {
        scene.note = Some(format!("+{}", crew.len() - seats));
    }
}

fn name(row: Option<&AgentView>, own: bool) -> String {
    if own {
        return "me".into();
    }
    row.map_or_else(|| "child".into(), |r| super::super::text::clip(&r.name, 32))
}
fn state(row: Option<&AgentView>, own: bool) -> ActorState {
    match row {
        Some(r) if !own && r.report.is_some() => ActorState::Ended,
        Some(r) if !r.asks.is_empty() || r.state == State::Stuck => ActorState::Waiting,
        Some(r) if r.state == State::Working => ActorState::Working,
        _ => ActorState::Idle,
    }
}
pub(super) fn placed(name: &'static str, frame: usize, x: f64, y: Option<f64>) -> Placed {
    Placed { name, frame, x, y }
}
fn round(n: f64) -> f64 {
    (n + 0.5).floor()
}
fn blend(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t.clamp(0., 1.)
}
fn ease(t: f64) -> f64 {
    let t = t.clamp(0., 1.);
    if t < 0.5 {
        2. * t * t
    } else {
        1. - (-2. * t + 2.).powi(2) / 2.
    }
}

pub(super) struct Wake {
    pub(super) now: u64,
    pub(super) motion: bool,
    pub(super) at: u64,
}
impl Wake {
    pub(super) fn at(&mut self, at: u64) {
        if at > self.now {
            self.at = self.at.min(at);
        }
    }
    pub(super) fn animate(&mut self, at: u64, until: u64, step: u64) {
        if self.motion && self.now < until {
            self.at((self.now + step - self.now.saturating_sub(at) % step).min(until));
        }
    }
    pub(super) fn age(&self, at: u64, duration: u64) -> u64 {
        if self.motion {
            self.now.saturating_sub(at)
        } else {
            duration
        }
    }
}

pub(super) fn local(app: &App, now: u64) -> Option<jiff::Zoned> {
    crate::terminal::text::zoned(now, app.zone.as_ref())
}
pub(super) fn hour(app: &App, now: u64) -> u64 {
    local(app, now)
        .and_then(|z| z.checked_add(jiff::Span::new().hours(1)).ok())
        .and_then(|z| at_hour(&z, z.hour()))
        .filter(|at| *at > now)
        .unwrap_or(now.saturating_add(3_600_000))
}
/// `z`'s day at `hour` o'clock sharp, in wall milliseconds.
pub(super) fn at_hour(z: &jiff::Zoned, hour: i8) -> Option<u64> {
    let z = z
        .with()
        .hour(hour)
        .minute(0)
        .second(0)
        .subsec_nanosecond(0)
        .build()
        .ok()?;
    u64::try_from(z.timestamp().as_millisecond()).ok()
}

fn lamp(scene: &mut Scene, stage: &Stage, crew: &[SessionId], w: &mut Wake) {
    let home = scene.lamp;
    let seat = |id: &SessionId| {
        let i = crew.iter().position(|c| c == id)?;
        scene.actors.get(i).map(|a| a.x)
    };
    // An act for a member without a seat moves nothing. Each move starts where the one before
    // had the lamp when it came; the oldest kept starts from home.
    let mut acts = stage
        .acts
        .iter()
        .filter_map(|(id, at)| Some((seat(id)?, *at)));
    let Some(mut act) = acts.next().map(|(target, at)| (home, target, at)) else {
        return;
    };
    let duration =
        |(from, target, _): (f64, f64, u64)| round((target - from).abs() / 2.2).max(4.) * 125.;
    for (target, at) in acts {
        let age = at.saturating_sub(act.2);
        let from = if age >= 120_000 {
            blend(act.1, home, ease((age - 120_000) as f64 / 1750.))
        } else {
            blend(act.0, act.1, ease(age as f64 / duration(act)))
        };
        act = (from, target, at);
    }
    let (from, target, at) = act;
    let d = duration(act);
    let age = w.age(at, d as u64 + 250) as f64;
    scene.lamp = blend(from, target, ease(age / d));
    let dir = if target == from {
        0.0
    } else {
        (target - from).signum()
    };
    scene.swing = if !w.motion || age >= d + 250. {
        0.
    } else if age < d {
        -dir * (std::f64::consts::PI * (age / d).clamp(0., 1.)).sin() * 0.9
    } else if age < d + 125. {
        dir * 0.7
    } else {
        -dir * 0.3
    };
    w.animate(at, at + d as u64 + 250, 125);
    let home_at = at + 120_000;
    if w.now >= home_at {
        scene.lamp = blend(target, home, ease(w.age(home_at, 1750) as f64 / 1750.));
        w.animate(home_at, home_at + 1750, 125);
    } else {
        w.at(home_at);
    }
}

fn running(scene: &mut Scene, stage: &Stage, w: &mut Wake) {
    let Some(actor) = scene.actors.first() else {
        return;
    };
    let x = actor.x;
    let n = stage.records.calls.len();
    for (i, call) in stage.records.calls.iter().take(n).enumerate() {
        let k = if w.motion {
            w.now.saturating_sub(call.at) / 500
        } else {
            0
        };
        let (name, frame) = match call.prop {
            Prop::Tests => ("beaker", k as usize % 4),
            Prop::Edit | Prop::Shell => ("wrench", k as usize % 2),
            Prop::Read => ("magnifier", 0),
            Prop::Write => ("quill", 0),
        };
        let x = x - 2. + (i as f64 - (n as f64 - 1.) / 2.) * 2.;
        if matches!(call.prop, Prop::Write) {
            scene.bench.push(placed("pad", 0, x, None));
        }
        let nudge = if matches!(call.prop, Prop::Read | Prop::Write) {
            (k % 2) as f64
        } else {
            0.
        };
        scene.bench.push(placed(name, frame, x + nudge, None));
        w.animate(call.at, u64::MAX, 500);
    }
}

fn sky(scene: &mut Scene, stage: &Stage, app: &App, w: &mut Wake) {
    let local = local(app, w.now);
    let hour = local.as_ref().map_or(12, |z| z.hour());
    if hour < 6 {
        scene.sky = Sky::Night;
        let age = local.map_or(750, |z| {
            z.hour() as u64 * 3_600_000
                + z.minute() as u64 * 60_000
                + z.second() as u64 * 1000
                + w.now % 1000
        });
        scene.dark = if w.motion {
            (age / 125) as f64 / 6.
        } else {
            1.
        }
        .min(1.);
        w.animate(
            w.now.saturating_sub(age),
            w.now.saturating_sub(age) + 750,
            125,
        );
        return;
    }
    // The window keeps the day: dawn to 8, day to 18, dusk to 20, then night with the room lit.
    scene.sky = match hour {
        6..8 => Sky::Dawn,
        18..20 => Sky::Dusk,
        20.. => Sky::Night,
        _ => Sky::Day,
    };
    let Some(&(pass, at)) = stage.records.tests.back() else {
        return;
    };
    let age = w.now.saturating_sub(at);
    let was_rain = stage
        .records
        .tests
        .iter()
        .rev()
        .nth(1)
        .is_some_and(|(pass, _)| !pass);
    scene.sky = if !pass {
        Sky::Rain
    } else if was_rain && w.motion && age < 3000 {
        Sky::Rainbow
    } else {
        scene.sky
    };
    scene.tick = if w.motion {
        age.min(3000) as i64 / 125
    } else {
        0
    };
    scene.bow = (age as f64 / 1000.).min(1.);
    if !pass || was_rain {
        w.animate(at, at + 3000, 125);
    }
}

fn moments(scene: &mut Scene, stage: &Stage, crew: &[SessionId], w: &mut Wake) {
    let parent = scene.actors.first().map_or(scene.lamp, |a| a.x);
    let follows = |at| stage.acts.back().is_some_and(|(_, last)| at >= *last);
    for (i, id) in crew.iter().take(scene.actors.len()).enumerate() {
        let Some(seen) = stage.seen.get(id) else {
            continue;
        };
        if i > 0
            && let Some(at) = seen.joined.filter(|at| w.now < at + 7000)
        {
            hatch(scene, i, parent, at, follows(at), w);
        }
        if i > 0
            && let Some(at) = seen.ended.filter(|at| w.now < at + 6000)
        {
            report(scene, i, parent, at, follows(at), w);
        }
    }
    if let Some(at) = stage
        .records
        .rebuild
        .filter(|at| *at > 0 && w.now < at + 8000)
    {
        handover(scene, at, w);
    }
}

fn hatch(s: &mut Scene, i: usize, parent: f64, at: u64, follows: bool, w: &mut Wake) {
    let age = w.age(at, 4000);
    let k = age / 125;
    let seat = s.actors[i].x;
    let x = if age < 1250 {
        blend(parent + 3., seat, age as f64 / 1250.)
    } else {
        seat + (k as f64 * std::f64::consts::PI / 2.).sin() * 0.34
    };
    if age < 2750 {
        s.actors[i].hidden = true;
        s.bench.push(placed(
            if age < 2250 { "egg" } else { "egg_crack" },
            0,
            x,
            None,
        ));
    } else {
        s.actors[i].y = Some(round(blend(18., 8., (age as f64 - 2750.) / 750.)));
        s.bench.push(placed("shell_cup", 0, seat - 2., None));
        if age < 3250 {
            s.air.push(placed(
                "shell_top",
                0,
                seat + 1. + (k % 4) as f64,
                Some(14. - (k % 4) as f64 * 2.),
            ));
        }
    }
    if follows {
        s.lamp = blend(parent, seat, ease(age as f64 / 1250.));
    }
    w.animate(at, at + 4000, 125);
    w.at(at + 7000);
}

fn report(s: &mut Scene, i: usize, parent: f64, at: u64, follows: bool, w: &mut Wake) {
    let age = w.age(at, 3000);
    let seat = s.actors[i].x;
    let to = parent + if seat >= parent { 6. } else { -6. };
    if age < 2500 {
        let a = &mut s.actors[i];
        a.x = if age < 1000 {
            blend(seat, to, age as f64 / 1000.)
        } else {
            blend(to, seat, (age as f64 - 1750.) / 750.)
        };
        a.y = Some(5. + (age / 125 % 2) as f64);
        a.behind = true;
        a.tag_at = Some(seat);
        a.flip = seat > parent;
        if age < 1000 {
            a.carry = Some(placed("scroll", 0, 0., None));
        }
    }
    if age >= 1000 {
        s.bench.push(placed("scroll_flat", 0, parent + 1., None));
    }
    if follows {
        s.lamp = if age < 1000 { s.actors[i].x } else { parent };
    }
    w.animate(at, at + 3000, 125);
    w.at(at + 6000);
}

fn handover(s: &mut Scene, at: u64, w: &mut Wake) {
    let Some(a) = s.actors.first_mut() else {
        return;
    };
    let age = w.age(at, 5000);
    if age < 1500 {
        a.pose = Pose::MessyYawn;
    } else if age < 3500 {
        a.asleep = true;
        a.pose = Pose::Sleep;
        a.dim = super::super::motion::progress(age, 2500, 1000);
        s.z = Some(a.x + 4.0);
    } else if age < 4000 {
        a.hidden = true;
    } else if age < 4500 {
        a.y = Some(8. + round((4500 - age) as f64 / 125.));
        a.pose = Pose::Blink;
    } else {
        a.pose = Pose::Idle;
        a.state = ActorState::Working;
    }
    let seat = a.x;
    s.bench.retain(|p| p.x != seat + 3.);
    s.bench.push(placed(
        if age < 4500 { "mug_empty" } else { "mug_full" },
        0,
        seat + 3.,
        None,
    ));
    w.animate(at, at + 5000, 125);
    w.at(at + 8000);
}
