//! The top of the agents list: one line for the servers and one for the residents, each folded
//! to what needs the person (a silent server, a usage window from 70%, a resident asking or
//! failed) and opening into its table.
use super::{App, agents, cards::tone, status::Row, text::*, view::Action};
use crate::protocol::{ResidentDay, RunState, Server, SessionId, State, views::SectionKind};
use ratatui::text::{Line, Span};
use std::collections::{BTreeMap, BTreeSet};

/// Both lines, each followed by its table while open, then a blank row.
pub(super) fn rows(app: &App, width: usize, now: u64) -> Vec<Row> {
    let mut out = servers(app, width, now);
    let residents = residents(app, width, now);
    // The residents end in the rule each group ends in; without them, a blank row.
    let end = "─".repeat(width * usize::from(!residents.is_empty()));
    out.extend(residents);
    out.push(Row(Line::from(span(end, GRAY_DIM)), None, Vec::new()));
    out
}

fn servers(app: &App, width: usize, now: u64) -> Vec<Row> {
    let usage = usage(app, now);
    let silent: Vec<_> = (app.servers.iter())
        .filter_map(|s| s.silent.map(|at| (s, age(at, now))))
        .map(|(s, age)| format!("{}·{} {age}m", s.machine, realm(s)))
        .collect();
    let state = if silent.is_empty() {
        span(format!("all {} answer", app.servers.len()), GRAY)
    } else {
        app.next.every(now, 60_000);
        Span::styled(format!("⊘ {}", silent.join(", ")), bold(AMBER, GLASS))
    };
    let mut parts = vec![vec![state]];
    if let Some(((_, name), p)) = usage.iter().max_by(|a, b| a.1.total_cmp(b.1)) {
        parts.push(vec![
            span(format!("{name} "), tone(p / 100., 0.9)),
            reading(Some(*p)),
        ]);
    }
    let mut out = vec![folded(app, "servers", parts, width)];
    if !app.list.toggled.contains("servers") {
        return out;
    }
    // Right edges of the load, memory, memory used, 5-hour and 7-day columns.
    let wide = width >= 60;
    let (load, mem) = if wide { (31, 24) } else { (18, 12) };
    let [load, mem, used, hour, week] = [load, mem, 17, 7, 1].map(|c| width.saturating_sub(c));
    let head = if wide { "machine / server" } else { "machine" };
    let heads = [1, load, mem, hour, week]
        .into_iter()
        .zip([head, "load", "mem", "5h", "7d"]);
    out.push(placed(heads.map(|(at, s)| (at, span(s, GRAY_DIM)))));
    let machines: BTreeSet<_> = app.servers.iter().map(|s| s.machine.as_str()).collect();
    for machine in machines {
        let mine = app.servers.iter().filter(|s| s.machine == machine);
        let mut cells = vec![(1, Span::styled(machine.to_owned(), bold(WHITE, GLASS)))];
        if let Some(l) = mine.clone().find_map(|s| s.load) {
            let pct = 100. - l.free as f64 * 100. / l.total.max(1) as f64;
            cells.push((load, span(format!("{:.2}", l.cpu), DATA)));
            cells.push((mem, reading(Some(pct))));
            let gib = format!("{}/{}G", (l.total - l.free) >> 30, l.total >> 30);
            cells.extend(wide.then(|| (used, span(gib, GRAY))));
        }
        out.push(placed(cells));
        for s in mine {
            let window = |name| reading(usage.get(&(s.name.as_str(), name)).copied());
            let name = span(format!("  {}", realm(s)), GRAY);
            out.push(match s.silent {
                Some(at) => {
                    let words = format!(" ⊘ {}  no answer {}m", realm(s), age(at, now));
                    Row(Line::from(span(words, AMBER)), None, Vec::new())
                }
                None => placed([(0, name), (hour, window("5-hour")), (week, window("7-day"))]),
            });
        }
    }
    out
}

/// Each server's highest reading of each usage window among its agents, by server name, leaving
/// out a reading past its reset; the list is drawn again when the soonest kept one resets.
fn usage(app: &App, now: u64) -> BTreeMap<(&str, &str), f64> {
    let mut top = BTreeMap::new();
    for a in app.agents.values() {
        let server = a.session.as_str().split_once('@').map_or("", |s| s.1);
        for w in &a.usage {
            match w.resets_at {
                Some(at) if at <= now => continue,
                Some(at) => app.next.at(at),
                None => {}
            }
            if let Some(u) = w.utilization {
                let at = top.entry((server, w.name.as_str())).or_insert(u);
                *at = at.max(u);
            }
        }
    }
    top
}

/// `work` from the label `box · work`: the server's own part, else its whole label or name.
fn realm(s: &Server) -> &str {
    let label = s.label.rsplit(" · ").next().unwrap_or_default();
    if label.is_empty() { &s.name } else { label }
}

fn residents(app: &App, width: usize, now: u64) -> Vec<Row> {
    let all: Vec<_> = app
        .servers
        .iter()
        .flat_map(|s| s.residents.iter().map(|r| (r.session.on(&s.name), r)))
        .collect();
    if all.is_empty() {
        return Vec::new();
    }
    let action = Action::Section("residents".into());
    let open = app.list.toggled.contains("residents");
    let room = width.saturating_sub(if width >= 46 {
        agents::GUTTER as usize + 2
    } else {
        0
    });
    let mut body = Vec::new();
    for (id, r) in all.iter().filter(|_| open) {
        // Each resident's block after the first stands a blank row below the one before.
        if !body.is_empty() {
            body.push(Row(Line::default(), None, Vec::new()));
        }
        body.extend(resident(app, id, r, room, now));
    }
    let next = all
        .iter()
        .filter_map(|(_, r)| r.upcoming.first())
        .min_by_key(|(_, at)| at);
    let mark = agents::mark(app, &action, !open);
    let name = format!("{mark} Residents");
    let state = format!("{} resident{}", all.len(), plural(all.len()));
    let next = next.map(|(_, at)| format!("next {}", hhmm(app, *at)));
    let mut labels = vec![Line::from(span(name.clone(), WHITE))];
    // Folded, the section is one line: its name and, beside it, its count and next wake.
    if !open {
        let held = [Some(state), next].into_iter().flatten();
        let held = Line::from(span(held.collect::<Vec<_>>().join(" · "), GRAY));
        body = vec![Row(held, Some(action.clone()), Vec::new())];
    } else {
        labels.push(Line::from(span(format!("  {state}"), GRAY)));
        labels.extend(next.map(|n| Line::from(span(format!("  {n}"), GRAY))));
    }
    if width >= 46 {
        agents::gutter(body, labels, action)
    } else {
        // Narrow, the heading is its own line and holds the count; folded, nothing follows it.
        if !open {
            body.clear();
        }
        let name = Line::from(span(format!("{name} · {}", all.len()), WHITE));
        body.insert(0, Row(name, Some(action), Vec::new()));
        body
    }
}

// One resident's line (asking, its last run failed, or working or idle with its next wake) and,
// unless its block is shut, the new-thread line and its threads.
fn resident(app: &App, id: &SessionId, r: &ResidentDay, room: usize, now: u64) -> Vec<Row> {
    let mut body = Vec::new();
    let agent = app.agents.get(id);
    let (mark, words, ink) = if agent.is_some_and(|a| !a.asks.is_empty()) {
        ("?", "asking".into(), CYAN)
    } else if let Some((wake, _, RunState::Failed)) = r.runs.first() {
        ("!", format!("{wake} failed"), RED)
    } else {
        let active = agent.is_some_and(|a| a.state == State::Working);
        let next = r.upcoming.first().map_or(String::new(), |(wake, at)| {
            format!("{wake} {}", hhmm(app, *at))
        });
        let (mark, ink) = if active {
            ("◐", GREEN)
        } else {
            ("○", GRAY)
        };
        (mark, next, ink)
    };
    let space = room.saturating_sub(super::text::width(&words) + 5);
    let name = clip(r.session.as_str(), space.max(1));
    let gap = " ".repeat(space.saturating_sub(super::text::width(&name)) + 1);
    let selected = app.list.selected == Some(Action::Agent(id.clone()));
    // The resident's block folds as a parent's children do, by its home session's id.
    let (fold, kids) = (id.to_string(), app.list.layout().into_iter());
    let shut = app.list.toggled.contains(&fold);
    let kids: Vec<_> = (kids.flat_map(|l| &l.sections))
        .filter(|s| s.kind == SectionKind::Residents)
        .flat_map(|s| &s.sessions)
        .skip_while(|r| &r.session != id)
        .skip(1)
        .take_while(|r| r.depth > 0)
        .collect();
    let arrow = ["▾", "▸"][usize::from(shut)];
    let arrow = Span::styled(arrow, bold([GRAY, CYAN][usize::from(selected)], GLASS));
    let mut line = Line::from(vec![
        arrow,
        span(" ", GRAY),
        span(mark, ink),
        span(" ", GRAY),
        Span::styled(name, bold(WHITE, GLASS)),
        span(format!("{gap}{words}"), GRAY),
    ]);
    let hides = kids.iter().any(|r| app.focus.as_ref() == Some(&r.session));
    agents::addressed(&mut line, app.focus.as_ref() == Some(id) || shut && hides);
    let action = Some(Action::Agent(id.clone()));
    body.push(Row(line, action, vec![(0, Action::Section(fold))]));
    if shut {
        return body;
    }
    let new = Action::Thread(id.clone());
    let mark = [" ", "❯"][usize::from(app.list.selected.as_ref() == Some(&new))];
    let line = Line::from(span(format!("{mark} + New thread"), CYAN));
    body.push(Row(line, Some(new), Vec::new()));
    body.extend(agents::section_rows(app, kids, room as u16, now));
    body
}

/// `14:00` in the terminal's zone; empty without one.
fn hhmm(app: &App, at: u64) -> String {
    zoned(at, app.zone.as_ref()).map_or_else(String::new, |t| t.strftime("%H:%M").to_string())
}

/// `▸ word · part · part`, marked as the list's headings are; without `word` when that would not
/// fit `width`.
fn folded(app: &App, word: &str, parts: Vec<Vec<Span<'static>>>, width: usize) -> Row {
    let action = Action::Section(word.into());
    let mark = match app.list.open && app.list.selected.as_ref() == Some(&action) {
        true => "❯",
        false => ["▾", "▸"][usize::from(!app.list.toggled.contains(word))],
    };
    let mut body: Vec<_> = parts.join(&span(" · ", GRAY_DIM));
    let used: usize = body.iter().map(|s| super::text::width(&s.content)).sum();
    if 2 + word.len() + 3 + used <= width {
        body.splice(0..0, [span(word.to_owned(), GRAY), span(" · ", GRAY_DIM)]);
    }
    body.insert(0, span(format!("{mark} "), GRAY));
    Row(Line::from(body), Some(action), Vec::new())
}

/// Cells left to right, each ending at its column where it fits, and at least one space after
/// the one before it (the first, after the edge).
fn placed(cells: impl IntoIterator<Item = (usize, Span<'static>)>) -> Row {
    let mut x = 0;
    let mut out = Vec::new();
    for (at, s) in cells {
        let n = super::text::width(&s.content);
        let start = at.saturating_sub(n).max(x + 1);
        out.push(span(" ".repeat(start - x), GRAY));
        x = start + n;
        out.push(s);
    }
    Row(Line::from(out), None, Vec::new())
}

/// A percentage, amber from 70 and red from 90; `–` when unmeasured.
fn reading(p: Option<f64>) -> Span<'static> {
    p.map_or(span("–", GRAY_DIM), |p| {
        span(format!("{p:.0}%"), tone(p / 100., 0.9))
    })
}

/// Whole minutes since `at`.
fn age(at: u64, now: u64) -> u64 {
    now.saturating_sub(at) / 60_000
}
