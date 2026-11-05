//! Small moments derived from the last tests, thanks and the local clock; none runs a timer.
use super::super::{App, text};
use super::{
    Stage,
    art::Pose,
    picture::{Burst, Scene, Sky},
    scene::{Wake, at_hour, local, placed},
};

pub(super) fn draw(s: &mut Scene, stage: &Stage, app: &App, w: &mut Wake) {
    thanks(s, stage, w);
    duck(s, stage, w);
    owl(s, stage, app, w);
    july(s, stage, app, w);
}

fn thanks(s: &mut Scene, stage: &Stage, w: &mut Wake) {
    let Some((words, at)) = stage
        .records
        .thanks
        .as_ref()
        .filter(|(_, at)| w.now < at + 3000)
    else {
        return;
    };
    let age = w.now.saturating_sub(*at);
    s.note_fg = Some([63, 217, 242]);
    s.note = Some(text::clip(
        &words.replace(['\n', '\r', '\t'], " "),
        s.w.saturating_sub(3),
    ));
    for a in &mut s.actors {
        a.pose = if !w.motion || (125..875).contains(&age) {
            Pose::Sleep
        } else if age < 1125 {
            Pose::Blink
        } else {
            Pose::Talk
        };
        a.y = Some(if a.pose == Pose::Sleep {
            10.
        } else if age < 1125 {
            9.
        } else {
            8.
        });
    }
    s.bright = if !w.motion {
        1.
    } else if age < 250 {
        age as f64 / 250.
    } else if age < 875 {
        1.
    } else {
        (1. - (age - 875) as f64 / 500.).clamp(0., 1.)
    };
    w.animate(*at, *at + 1375, 125);
    w.at(*at + 3000);
}

fn duck(s: &mut Scene, stage: &Stage, w: &mut Wake) {
    let Some((at, last, leave)) = stage.records.duck else {
        return;
    };
    if w.now >= last + 7_200_000 {
        return;
    }
    let Some(a) = s.actors.first() else {
        return;
    };
    let home = a.x - 3.;
    let age = w.age(
        leave.unwrap_or(at),
        if leave.is_some() { 2000 } else { 625 },
    );
    if leave.is_some() && age >= 2000 {
        return;
    }
    let (mut x, mut y) = (home, 18.);
    if leave.is_some() {
        if age < 500 {
            y -= (std::f64::consts::PI * age as f64 / 500.).sin() * 3.;
            s.air.push(placed("squeak", 0, home - 1., Some(12.)));
        } else {
            let k = (age - 500) as f64 / 1500.;
            x += (-5. - home) * k;
            y -= (2. * std::f64::consts::PI * k).sin().abs() * 2.5;
        }
        w.animate(leave.unwrap_or(at), leave.unwrap_or(at) + 2000, 125);
    } else {
        y += [-9., -5., -1., 0., -1.]
            .get((age / 125) as usize)
            .copied()
            .unwrap_or(0.);
        w.animate(at, at + 625, 125);
        w.at(last + 7_200_000);
        let note = s.note.take().map_or(String::new(), |n| format!(" · {n}"));
        s.note = Some(format!("tests failed 3×{note}"));
        s.note_fg = Some([255, 179, 25]);
    }
    s.bench.push(placed("duck", 0, x, Some(y)));
}

fn owl(s: &mut Scene, stage: &Stage, app: &App, w: &mut Wake) {
    let Some(z) = local(app, w.now) else {
        return;
    };
    let present = (2..6).contains(&z.hour());
    let boundary = if z.hour() >= 6 { 6 } else { 2 };
    let at = at_hour(&z, boundary).unwrap_or(w.now);
    let arrival = stage.clock.is_some_and(|first| first < at);
    let age = w.now.saturating_sub(at);
    if arrival && w.motion && w.now >= at && age < 625 {
        let t = age as f64 / 625.;
        let x = if present {
            10.5 - t * 3.7
        } else {
            6.8 + t * 4.
        };
        let y = if present { 1.2 + t * 2.8 } else { 4. - t * 3.4 };
        s.pane
            .push(placed("owl_flying", (age / 125 % 2) as usize, x, Some(y)));
        w.animate(at, at + 625, 125);
    } else if present {
        let seed = app
            .root
            .as_ref()
            .map_or(0, |id| super::art::hash(id.as_str()));
        let period = 21_125 + ((seed ^ 0x9e37_79b9).wrapping_mul(0x85eb_ca6b) % 18_876) as u64;
        let start = stage
            .clock
            .unwrap_or(w.now)
            .max(at + u64::from(arrival) * 625);
        let age = w.now.saturating_sub(start) % period;
        let frame = if !w.motion || age < period - 1125 {
            0
        } else if age < period - 875 || age >= period - 250 {
            1
        } else {
            2
        };
        s.pane.push(placed("owl", frame, 6.8, None));
        if w.motion {
            let end = w.now - age + period;
            w.at(if frame == 0 {
                end - 1125
            } else if age < period - 875 {
                end - 875
            } else if age < period - 250 {
                end - 250
            } else {
                end
            });
        }
    }
}

fn july(s: &mut Scene, stage: &Stage, app: &App, w: &mut Wake) {
    let Some(local) = local(app, w.now) else {
        return;
    };
    if local.month() != 7 || local.day() != 4 {
        return;
    }
    if local.hour() >= 18 {
        s.sky = Sky::Dusk;
        s.dark = 0.3;
    }
    let firing = stage.records.firework.filter(|at| w.now < at + 3500);
    let frame = if w.motion && firing.is_some() {
        (w.now / 250 % 3) as usize
    } else {
        0
    };
    for a in &mut s.actors {
        a.wear = Some(placed("flag", frame, 0.0, None));
    }
    let Some(at) = firing else {
        return;
    };
    for a in &mut s.actors {
        a.pose = Pose::Talk;
    }
    let tick = w.age(at, 1400) as f64 / 125.;
    for (x, y, r, start, len, pane) in [
        (2.6, 3., 1.8, 0., 10., true),
        (6., 2.5, 2., 3., 10., true),
        (9., 3.8, 1.7, 7., 10., true),
        (s.w as f64 / 2. - 5., 3.5, 4., 10., 13., false),
        (s.w as f64 / 2. + 5., 3.5, 4., 12., 13., false),
    ] {
        let age = if w.motion { (tick - start) / len } else { 0.4 };
        if !(0.0..1.0).contains(&age) || (!w.motion && pane && x != 2.6) {
            continue;
        }
        s.bursts.push(Burst { x, y, r, age, pane });
    }
    w.animate(at, at + 3500, 125);
}
