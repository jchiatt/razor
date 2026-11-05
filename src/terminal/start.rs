//! Where a new agent starts: every server's past folders and repositories, one row per
//! repository with the servers that have it. Choosing one aims the new-session composer at that
//! folder on that server; its model chip, `/model` and first message then work as at a launch.
use super::{App, Composer, fail, picker, text::*, view::Action, view::Hit};
use crate::{protocol::SessionId, terminal::bindings};
use crossterm::event::{KeyCode as K, KeyEvent, KeyModifiers as Mods};
use ratatui::{buffer::Buffer, layout::Rect};
use reqwest::Method;
use serde::Deserialize;
use std::{cmp::Reverse, collections::BTreeMap, path::PathBuf};

/// A folder on a server; `at` is when a session last started there, none for a found checkout.
#[derive(Deserialize)]
struct Place {
    path: PathBuf,
    repo: Option<String>,
    at: Option<u64>,
}

#[derive(Default)]
pub struct Start {
    // Each reachable server by enrollment name (empty for this one): its label, and its places
    // once it answers.
    servers: BTreeMap<String, (String, Option<Vec<Place>>)>,
    filter: String,
    // The chosen project and server by name, so a late answer that moves the rows keeps them.
    chosen: Option<(String, String)>,
}

/// A project and its places, one a server.
type Project<'a> = (String, Vec<(&'a str, &'a Place)>);

impl Start {
    /// One row per repository, or folder name outside git, matching the filter: recently used
    /// first, then found ones; each with its newest folder on each server, newest first.
    fn projects(&self) -> Vec<Project<'_>> {
        let mut by = BTreeMap::<String, BTreeMap<&str, &Place>>::new();
        for (server, (_, places)) in &self.servers {
            for p in places.iter().flatten() {
                let folder = p.path.file_name().map(|n| n.to_string_lossy().into_owned());
                let name = p.repo.clone().or(folder).unwrap_or_default();
                let newest = by.entry(name).or_default().entry(server).or_insert(p);
                *newest = if p.at > newest.at { p } else { newest };
            }
        }
        let filter = self.filter.to_lowercase();
        let mut all: Vec<_> = (by.into_iter())
            .filter(|(name, _)| name.to_lowercase().contains(&filter))
            .map(|(name, places)| {
                let mut places: Vec<_> = places.into_iter().collect();
                places.sort_by_key(|(_, p)| Reverse(p.at));
                (name, places)
            })
            .collect();
        all.sort_by_key(|(_, places)| Reverse(places[0].1.at));
        all
    }

    /// The chosen project's row and server's place: the first and its newest until chosen.
    fn at(&self, projects: &[Project]) -> (usize, usize) {
        let Some((name, server)) = &self.chosen else {
            return (0, 0);
        };
        let row = projects.iter().position(|p| &p.0 == name).unwrap_or(0);
        let places = projects.get(row).map_or(&[][..], |p| &p.1[..]);
        (row, places.iter().position(|p| p.0 == server).unwrap_or(0))
    }

    fn select(&mut self, row: usize, place: usize) {
        self.chosen = self
            .projects()
            .get(row)
            .and_then(|p| Some((p.0.clone(), p.1.get(place)?.0.into())));
    }

    /// The chosen server and folder.
    fn target(&self) -> Option<(String, PathBuf)> {
        let projects = self.projects();
        let (row, place) = self.at(&projects);
        let (server, p) = projects.get(row)?.1.get(place)?;
        Some((server.to_string(), p.path.clone()))
    }
}

/// Opens the chooser beside the agents list and asks every reachable server for its projects;
/// each answer lands on its own, so a slow server holds up none of the others.
pub fn open(app: &mut App) {
    if !app.list.open {
        super::agents::toggle(app);
    }
    let mut start = Start::default();
    for s in app.servers.iter().filter(|s| s.silent.is_none()) {
        start
            .servers
            .insert(s.name.clone(), (s.label.clone(), None));
    }
    for name in start.servers.keys() {
        let (path, name) = (format!("v1/repos?server={name}"), name.clone());
        app.spawn(|api| async move {
            let invalid = "A server's projects are invalid; reconnect to a matching server.";
            let read = api.json(Method::GET, &path, None).await;
            let places = read
                .and_then(|v| serde_json::from_value(v["repos"].clone()).map_err(fail(invalid)));
            move |app: &mut App| {
                if let Err(e) = &places {
                    app.say(&e.to_string());
                }
                let Some(start) = &mut app.start else { return };
                if start.chosen.is_none() {
                    start.select(0, 0);
                }
                if let Some(s) = start.servers.get_mut(&name) {
                    s.1 = Some(places.unwrap_or_default());
                }
            }
        });
    }
    (app.start, app.list.keys) = (Some(start), false);
}

/// Takes every key while open: typing filters, Up and Down choose the project, Left and Right
/// its server, Enter aims the composer there and Escape closes.
pub fn key(app: &mut App, k: KeyEvent) {
    let Some(start) = &mut app.start else { return };
    let projects = start.projects();
    let ((row, place), rows) = (start.at(&projects), projects.len().saturating_sub(1));
    let places = projects.get(row).map_or(0, |p| p.1.len().saturating_sub(1));
    match k.code {
        K::Esc => app.start = None,
        K::Enter => choose(app, None),
        K::Up => start.select(row.saturating_sub(1), 0),
        K::Down => start.select((row + 1).min(rows), 0),
        K::Left => start.select(row, place.saturating_sub(1)),
        K::Right => start.select(row, (place + 1).min(places)),
        K::Backspace | K::Char(_) if !k.modifiers.intersects(Mods::CONTROL | Mods::ALT) => {
            match k.code {
                K::Char(c) => start.filter.push(c),
                _ => drop(start.filter.pop()),
            }
            start.chosen = None;
        }
        _ => {}
    }
}

/// Aims the new-session composer at the chosen folder on the chosen server, or at the clicked
/// one as it was drawn: the next first message starts it there.
pub fn choose(app: &mut App, clicked: Option<(String, PathBuf)>) {
    let chosen = app
        .start
        .as_ref()
        .and_then(|s| clicked.or_else(|| s.target()));
    if let Some((server, cwd)) = chosen {
        aim(app, server, cwd, None);
    }
}

/// Aims the new-session composer at `cwd`, or at a fresh thread of `resident` (its native name),
/// on `server`: the next first message starts it there.
pub fn aim(app: &mut App, server: String, cwd: PathBuf, resident: Option<SessionId>) {
    if app.sending.is_some() || super::audio::unsent(app) {
        return app.say("A note or input is pending; wait before starting a new agent.");
    }
    app.save();
    super::recap::leave(app);
    for id in [app.root.take(), app.focus.clone()].into_iter().flatten() {
        app.streams.remove(&id).iter().for_each(|task| task.abort());
    }
    app.back = app.focus.take().or(app.back.take());
    let selected = resident
        .as_ref()
        .map_or(Action::New, |r| Action::Thread(r.on(&server)));
    (app.server, app.cwd, app.resident) = (server, cwd, resident);
    (app.start, app.initial, app.request) = (None, None, None);
    (app.composer, app.list.selected) = (Composer::default(), Some(selected));
    picker::request(app, true);
}

/// Draws the chooser over the foot of `area`, as the model picker draws, and records its clicks.
pub fn draw(app: &App, b: &mut Buffer, area: Rect, hits: &mut Vec<Hit>) {
    let Some(start) = &app.start else { return };
    let projects = start.projects();
    let rows = (area.height.saturating_sub(2) as usize).min(projects.len() + 2) as u16;
    let r = Rect::new(area.x, area.bottom().saturating_sub(rows), area.width, rows);
    let (x, w) = (r.x + 1, r.width.saturating_sub(2));
    fill(b, r, style(DATA, RECESS));
    let waiting = start.servers.values().any(|s| s.1.is_none());
    let note = ["", "asking the servers… · "][usize::from(waiting)];
    let keys = bindings::text(
        app,
        &format!(
            "new agent · {note}type to filter · <start.up>/<start.down> project · <start.left>/<start.right> server · <start.accept> · <start.cancel>"
        ),
    );
    put(b, x, r.y, &keys, w, style(GRAY, RECESS));
    let filter = format!("❯ {}", start.filter);
    put(b, x, r.y + 1, &filter, w, style(WHITE, RECESS));
    let (row, place) = start.at(&projects);
    let first = row.saturating_sub((rows as usize).saturating_sub(3));
    let shown = projects.iter().enumerate().skip(first);
    for ((i, (name, places)), y) in shown.zip(r.y + 2..r.bottom()) {
        let mine = i == row;
        let used = style([GRAY, WHITE][usize::from(places[0].1.at.is_some())], RECESS);
        let name = format!("{} {:<24}", [" ", "›"][usize::from(mine)], clip(name, 24));
        let ink = if mine { bold(CYAN, RECESS) } else { used };
        let mut end = put(b, x, y, &name, w, ink);
        let aim = |(server, p): &(&str, &Place)| Action::Choose(server.to_string(), p.path.clone());
        hits.push((Rect::new(r.x, y, r.width, 1), aim(&places[0])));
        for (at, (server, _)) in places.iter().enumerate() {
            let on = mine && at == place;
            let label = format!(" {} ", start.servers.get(*server).map_or("", |s| &s.0));
            let chip = [style(GRAY, RECESS), bold(GLASS, CYAN)][usize::from(on)];
            let next = put(b, end + 1, y, &label, (x + w).saturating_sub(end + 1), chip);
            let r = Rect::new(end + 1, y, next - end - 1, 1);
            hits.push((r, aim(&places[at])));
            end = next;
        }
    }
}
