//! The sessions renderer shared by the status pane and the agents view: each section's heading
//! in the gutter with its counts, and its rows as cards under their folds, from the server's
//! layout with local folding only.
use super::super::cards::{self, Card, Size};
use super::super::text::{self, *};
use super::super::{App, status, view::Action};
use crate::protocol::AgentView;
use crate::protocol::views::{Colour, Section, SectionKind, ViewRow};
use ratatui::style::Color;
use ratatui::text::{Line, Span};

fn colour(c: Option<Colour>) -> Color {
    match c {
        Some(Colour::Slate) => Color::Rgb(130, 143, 156),
        Some(Colour::Steel) => Color::Rgb(113, 148, 161),
        Some(Colour::Indigo) => Color::Rgb(132, 135, 179),
        Some(Colour::Plum) => Color::Rgb(163, 132, 166),
        Some(Colour::Rose) => Color::Rgb(180, 133, 143),
        Some(Colour::Clay) => Color::Rgb(181, 143, 117),
        Some(Colour::Sand) => Color::Rgb(176, 164, 120),
        Some(Colour::Moss) => Color::Rgb(136, 160, 124),
        None => GRAY,
    }
}
/// The single sessions renderer: server headings and Card rows, with local folding only.
pub fn rows(app: &App, width: u16, now: u64) -> Vec<status::Row> {
    let mut out = super::super::servers::rows(app, width as usize, now);
    if app.list.open {
        let mark = ["+", "❯"][usize::from(app.list.selected == Some(Action::New))];
        let label = app.servers.iter().find(|s| s.name == app.server);
        let label = label.map_or("", |s| s.label.as_str());
        let aimed = app.cwd.file_name().filter(|_| app.focus.is_none());
        let aimed = aimed.map(|f| format!(" in {} on {label}", f.to_string_lossy()));
        let new = format!("{mark} New agent{}", aimed.unwrap_or_default());
        let words = Line::from(span(new, CYAN));
        out.insert(0, status::Row(words, Some(Action::New), Vec::new()));
    }
    let Some(layout) = app.list.layout() else {
        let text = app.list.status.as_ref().and_then(|s| s.error.as_deref());
        let text = clip(text.unwrap_or("Views are loading…"), width as usize);
        out.push(status::Row(Line::from(span(text, GRAY)), None, Vec::new()));
        return out;
    };
    let sections = &layout.sections;
    let mut start = 0;
    while start < sections.len() {
        let root = &sections[start];
        if root.kind == SectionKind::Residents {
            start += 1;
            continue;
        }
        let group = subtree(&sections[start..]);
        let end = start + group.len();
        let wide = width >= 46; // The list frame consumes four of its 50 columns.
        let room = width.saturating_sub(if wide { GUTTER + 2 } else { 0 });
        let joined = group.get(1).map(|s| format!("{} › {}", root.name, s.name));
        let joined = joined.filter(|s| {
            wide && group.len() == 2
                && root.sessions.is_empty()
                && text::width(s) <= GUTTER as usize - 2
        });
        let (agents, helpers) = totals(group);
        let mut body = section_body(app, group, room, now, wide, joined.is_some());
        if wide {
            let action = Action::Section(root.id.clone());
            let folded = (root.kind == SectionKind::Earlier) != app.list.toggled.contains(&root.id);
            let mark = mark(app, &action, folded);
            let name = joined.as_deref().unwrap_or(&root.name);
            let mut labels = heading(mark, name, colour(root.colour));
            // Folded, the group is one line: its name and, beside it, what it holds.
            if folded {
                labels.truncate(1);
                let held = Line::from(span(counts(agents, helpers), GRAY));
                body = vec![status::Row(held, Some(action.clone()), Vec::new())];
            } else {
                labels.extend(
                    counts(agents, helpers)
                        .split(" · ")
                        .map(|s| Line::from(span(format!("  {s}"), GRAY))),
                );
            }
            body = gutter(body, labels, action);
            if folded && contains(app, group) {
                body.iter_mut().for_each(|r| addressed(&mut r.0, true));
            }
        }
        out.extend(body);
        out.push(status::Row(
            Line::from(span("─".repeat(width as usize), GRAY_DIM)),
            None,
            Vec::new(),
        ));
        start = end;
    }
    out
}
fn section_body(
    app: &App,
    group: &[Section],
    room: u16,
    now: u64,
    wide: bool,
    joined: bool,
) -> Vec<status::Row> {
    let mut body = Vec::new();
    let mut hidden = None::<u16>;
    for (i, section) in group.iter().enumerate() {
        if hidden.is_some_and(|depth| section.depth > depth) {
            continue;
        }
        let folded =
            (section.kind == SectionKind::Earlier) != app.list.toggled.contains(&section.id);
        hidden = folded.then_some(section.depth);
        let descendants = subtree(&group[i..]);
        let (n, h) = totals(descendants);
        let action = Action::Section(section.id.clone());
        if (!wide || i > 0) && !(i == 1 && joined) {
            let mark = mark(app, &action, folded);
            let label = format!(
                "{}{mark} {} · {}",
                "  ".repeat(section.depth as usize),
                section.name,
                counts(n, h)
            );
            let mut line = Line::from(span(clip(&label, room as usize), colour(section.colour)));
            addressed(&mut line, folded && contains(app, descendants));
            body.push(status::Row(line, Some(action), Vec::new()));
        }
        if !folded {
            body.extend(section_rows(app, section.sessions.iter(), room, now));
        }
    }
    body
}
fn totals(sections: &[Section]) -> (usize, usize) {
    let rows = || sections.iter().flat_map(|s| &s.sessions);
    let agents = rows().filter(|r| r.depth == 0).count();
    (agents, rows().count() - agents)
}
fn subtree(sections: &[Section]) -> &[Section] {
    let end = 1 + sections[1..]
        .iter()
        .take_while(|s| s.depth > sections[0].depth)
        .count();
    &sections[..end]
}
pub fn mark(app: &App, action: &Action, folded: bool) -> &'static str {
    match (app.list.selected.as_ref() == Some(action), folded) {
        (true, _) => "❯",
        (false, true) => "▸",
        (false, false) => "▾",
    }
}
fn counts(agents: usize, helpers: usize) -> String {
    [(agents, "agent"), (helpers, "helper")]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, word)| format!("{n} {word}{}", plural(n)))
        .collect::<Vec<_>>()
        .join(" · ")
}
fn contains(app: &App, sections: &[Section]) -> bool {
    sections
        .iter()
        .flat_map(|s| &s.sessions)
        .any(|r| app.focus.as_ref() == Some(&r.session))
}
/// A heading's name in the gutter after its fold mark: two lines at most, the second indented
/// under the first's words and cut with `…`.
fn heading(mark: &str, name: &str, ink: Color) -> Vec<Line<'static>> {
    let (mut words, mut at) = ([String::new(), String::new()], 0);
    for word in name.split_whitespace() {
        if at == 0
            && !words[0].is_empty()
            && width(&words[0]) + 1 + width(word) > GUTTER as usize - 2
        {
            at = 1;
        }
        let line = &mut words[at];
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    let [first, rest] = words;
    let mut lines = vec![Line::from(span(
        clip(&format!("{mark} {first}"), GUTTER as usize),
        ink,
    ))];
    if !rest.is_empty() {
        lines.push(Line::from(span(
            clip(&format!("  {rest}"), GUTTER as usize),
            ink,
        )));
    }
    lines
}
/// The gutter's columns, before its rule and a space.
pub const GUTTER: u16 = 13;
/// The common left column for projects and residents; row actions keep their own targets.
pub fn gutter(
    mut rows: Vec<status::Row>,
    labels: Vec<Line<'static>>,
    action: Action,
) -> Vec<status::Row> {
    while rows.len() < labels.len() {
        rows.push(status::Row(Line::default(), None, Vec::new()));
    }
    for (i, row) in rows.iter_mut().enumerate() {
        let mut lead = clip_spans(
            labels.get(i).cloned().unwrap_or_default().spans,
            GUTTER as usize,
        );
        let used: usize = lead.iter().map(|s| width(&s.content)).sum();
        lead.push(span(
            " ".repeat((GUTTER as usize).saturating_sub(used)),
            GRAY,
        ));
        lead.push(span("│ ", GRAY_DIM));
        row.0.spans.splice(0..0, lead);
        for (x, _) in &mut row.2 {
            *x += GUTTER + 2;
        }
        if i == 0 {
            row.2.push((0, action.clone()));
        }
    }
    rows
}
// Tints the row when it is, or hides, the session the composer addresses; the list draws the
// tint to its edges.
pub fn addressed(line: &mut Line, here: bool) {
    let tint = |style: &mut ratatui::style::Style| style.bg = Some(YOU_BG);
    if here {
        tint(&mut line.style);
        line.spans.iter_mut().for_each(|s| tint(&mut s.style));
    }
}
// The local fold of `a`'s folded finished siblings, closed until opened.
fn finished(a: &AgentView) -> String {
    format!("finished/{}", a.parent.as_ref().map_or("", |p| p.as_str()))
}
// A row under a heading as the list shows it: the fold its ▾ or ▸ toggles (a parent's children,
// or the folded finished children a line stands for) and whether it is shut; with a count, that
// line instead of a card; and whether the row is, or hides, the session the composer addresses.
type Shown<'a> = (&'a ViewRow, Option<(String, bool)>, Option<usize>, bool);
fn shown<'a>(app: &'a App, rows: impl Iterator<Item = &'a ViewRow>) -> Vec<Shown<'a>> {
    let rows: Vec<_> = rows
        .filter(|r| app.agents.contains_key(&r.session))
        .collect();
    // Shut below this depth, and at it for folded rows while their finished line is shut.
    let (mut out, mut hidden) = (Vec::<Shown>::new(), None::<(u16, bool)>);
    for (i, r) in rows.iter().copied().enumerate() {
        let here = app.focus.as_ref() == Some(&r.session);
        let shut = |(depth, folded)| r.depth > depth || folded && r.depth == depth && r.folded;
        if hidden.is_some_and(shut) {
            out.last_mut().into_iter().for_each(|hider| hider.3 |= here);
            continue;
        }
        // A parent's folded finished children follow the others; the first heads their line.
        let previous = rows[..i].iter().rev().find(|p| p.depth <= r.depth);
        if r.folded && previous.is_none_or(|p| p.depth < r.depth || !p.folded) {
            let id = finished(&app.agents[&r.session]);
            let siblings = rows[i..].iter().take_while(|n| n.depth >= r.depth);
            let count = siblings.filter(|n| n.depth == r.depth).count();
            let shut = !app.list.toggled.contains(&id);
            out.push((r, Some((id, shut)), Some(count), here && shut));
            if shut {
                hidden = Some((r.depth, true));
                continue;
            }
        }
        let parent = rows.get(i + 1).is_some_and(|next| next.depth > r.depth);
        let id = r.session.to_string();
        let fold = parent.then(|| (id.clone(), app.list.toggled.contains(&id)));
        hidden = fold
            .as_ref()
            .and_then(|(_, shut)| shut.then_some((r.depth, false)));
        out.push((r, fold, None, here));
    }
    out
}
pub fn section_rows<'a>(
    app: &'a App,
    rows: impl IntoIterator<Item = &'a ViewRow>,
    width: u16,
    now: u64,
) -> Vec<status::Row> {
    let shown = shown(app, rows.into_iter());
    let cards: Vec<_> = (shown.iter().filter(|(.., count, _)| count.is_none()))
        .map(|(r, ..)| {
            let mut card = Card::of(app, &app.agents[&r.session], now);
            // A root and a resident's thread say how long ago they last did anything.
            if r.depth == 0 || card.agent.resident.is_some() {
                card.clock = timer(now.saturating_sub(card.agent.last_activity));
            }
            if r.depth == 0 {
                card.location = status::location(app, &card.agent.session, width as usize / 2);
            }
            card
        })
        .collect();
    let all: Vec<_> = cards.iter().collect();
    let detail = cards::detail(&all, width as usize, Size::Compact);
    let (mut out, mut cards) = (Vec::new(), cards.into_iter());
    for (row, fold, count, here) in &shown {
        let room = (width as usize).saturating_sub(1);
        let mark = fold
            .as_ref()
            .map_or(" ", |(_, shut)| ["▾", "▸"][usize::from(*shut)]);
        let toggle = fold.as_ref().map(|(id, _)| Action::Section(id.clone()));
        if let Some(n) = count {
            let selected = app.list.open && app.list.selected == toggle;
            let words = format!("{} {mark} {n} finished", if selected { "❯" } else { " " });
            let mut line = Line::from(span(clip(&words, room + 1), GRAY));
            addressed(&mut line, *here);
            out.push(status::Row(line, toggle, Vec::new()));
            continue;
        }
        let Some(card) = cards.next() else { break };
        let action = Action::Agent(card.agent.session.clone());
        let selected = app.list.open && app.list.selected.as_ref() == Some(&action);
        let size = [Size::Compact, Size::Mini][usize::from(row.depth > 0)];
        let branch = match row.depth {
            0 => String::new(),
            depth => format!("{}└ ", "│ ".repeat((depth - 1).min(8) as usize)),
        };
        // The selection shows in the fold's column, so the card's own mark column is the space
        // between it and the state.
        let columns = room.saturating_sub(text::width(&branch));
        let mut lines = card.rows(size, columns, detail, (false, false));
        if row.depth > 0 {
            for line in &mut lines {
                line.spans
                    .insert(3.min(line.spans.len()), span(branch.clone(), GRAY_DIM));
            }
        }
        for (i, mut line) in lines.into_iter().enumerate() {
            // Selected, a parent's fold arrow turns cyan and keeps its shape; another row shows ❯.
            let (mark, ink) = match i {
                0 if selected && fold.is_some() => (mark, CYAN),
                0 if selected => ("❯", CYAN),
                0 => (mark, GRAY),
                _ => (" ", GRAY),
            };
            line.spans.insert(0, Span::styled(mark, bold(ink, GLASS)));
            addressed(&mut line, *here);
            let folds = Vec::from_iter(toggle.clone().filter(|_| i == 0).map(|a| (0, a)));
            out.push(status::Row(line, Some(action.clone()), folds));
        }
    }
    out
}
