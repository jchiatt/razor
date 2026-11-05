//! The status pane's room, or its short-pane strip, with bounded observations and one picture.
mod art;
mod avatar;
mod cells;
mod eggs;
mod emotes;
mod observations;
mod picture;
mod scene;
mod strip;

use super::App;
use crate::protocol::{AgentView, Entry, SessionId, State};
use ratatui::{buffer::Buffer, layout::Rect, style::Color};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
pub(super) struct Seen {
    pub(super) state: State,
    pub(super) activity: u64,
    pub(super) working: Option<u64>,
    pub(super) joined: Option<u64>,
    pub(super) ended: Option<u64>,
    finished: bool,
}

/// The observations of at most 64 members and calls, the last three test outcomes, and one
/// stage-sized picture. The first drawn crew is a baseline: attaching plays no moments.
#[derive(Default)]
pub(super) struct Stage {
    seen: BTreeMap<SessionId, Seen>,
    focus: Option<SessionId>,
    ready: bool,
    clock: Option<u64>,
    last_key: u64,
    // Whether the room rested at the last draw, and when the last rest ended.
    resting: bool,
    woke: u64,
    // The root's project (commits, merges), when a frame last hung, and whether a read is wanted
    // (after a focused turn's end, where settles happen) or under way.
    project: Option<(u64, u64)>,
    hung: u64,
    want: bool,
    reading: bool,
    cleared: Option<u64>,
    records: observations::Records,
    cache: Option<Cache>,
    revision: u64,
    // The last eight acts: each move starts where the one before had the lamp.
    acts: std::collections::VecDeque<(SessionId, u64)>,
}

/// Rows of cells, each a character in its colours.
type Grid = Vec<Vec<(char, Color, Color)>>;

struct Cache {
    key: (u16, u16, u64, bool, bool),
    until: u64,
    rows: Grid,
}

impl Stage {
    /// Observes a status row without replaying any of the first snapshot's arrivals.
    pub(super) fn agent(&mut self, row: &AgentView, now: u64) {
        if self.focus.is_some()
            && self.focus.as_ref() != Some(&row.session)
            && row.parent != self.focus
        {
            return;
        }
        // A placeholder may have drawn before the first status snapshot arrived.
        if self.seen.is_empty() {
            self.ready = false;
        }
        let crew = self.focus.as_ref() == Some(&row.session) || row.parent == self.focus;
        if self.ready
            && crew
            && self
                .acts
                .back()
                .is_none_or(|(_, at)| row.last_activity > *at)
        {
            if self.acts.len() == 8 {
                self.acts.pop_front();
            }
            self.acts
                .push_back((row.session.clone(), row.last_activity));
        }
        let ended = row.report.is_some();
        let old = self.seen.get(&row.session);
        let seen = Seen {
            state: row.state,
            activity: row.last_activity,
            working: if row.state != State::Working {
                None
            } else {
                old.and_then(|s| s.working).or(Some(now))
            },
            joined: old
                .and_then(|s| s.joined)
                .or_else(|| (self.ready && old.is_none()).then_some(now)),
            ended: old.and_then(|s| s.ended).or_else(|| {
                (ended && self.ready && old.is_none_or(|s| !s.finished)).then_some(now)
            }),
            finished: ended,
        };
        if self.seen.len() == 64
            && old.is_none()
            && let Some(id) = self
                .seen
                .iter()
                .min_by_key(|(_, s)| s.activity)
                .map(|(id, _)| id.clone())
        {
            self.seen.remove(&id);
        }
        self.seen.insert(row.session.clone(), seen);
        self.revision = self.revision.wrapping_add(1);
    }

    /// Keeps focused record observations separate when the person attaches another session.
    pub(super) fn select(&mut self, focus: Option<&SessionId>) {
        if self.focus.as_ref() == focus {
            return;
        }
        self.focus = focus.cloned();
        self.want = true;
        self.records = observations::Records::default();
        self.acts.clear();
        self.cache = None;
        self.ready = false;
        self.clock = None;
        for seen in self.seen.values_mut() {
            seen.joined = None;
            seen.ended = None;
        }
    }

    /// Observes the focused record once, retaining no text except a bounded thanks.
    pub(super) fn entry(&mut self, entry: &Entry, now: u64) {
        self.records.entry(entry, now);
        self.want |= matches!(entry.body, crate::protocol::Body::TurnEnd { .. });
        if !self.ready {
            self.records.rebuild = None;
            self.records.thanks = None;
        }
        self.revision = self.revision.wrapping_add(1);
    }

    /// Takes the project's counts: a new merge hangs a frame, except on the first answer, a
    /// baseline; an unanswered read keeps the last counts.
    pub(super) fn counted(&mut self, counts: Option<(u64, u64)>, now: u64) {
        if let (Some((_, old)), Some((_, merges))) = (self.project, counts) {
            self.hung = if merges > old { now } else { self.hung };
        }
        (self.project, self.reading) = (counts.or(self.project), false);
        self.revision = self.revision.wrapping_add(1);
    }

    /// Observes a key of this client: the first after a pause stops the strip's idle antics, and
    /// `cleared` (a draft of 40 characters or more emptied without a send) is a cue.
    pub(super) fn key(&mut self, now: u64, cleared: bool) {
        if cleared {
            self.cleared = Some(now);
        }
        if cleared || now.saturating_sub(self.last_key) >= 1500 {
            self.revision = self.revision.wrapping_add(1);
        }
        self.last_key = now;
    }
}

/// Draws the room when it fits, else the strip, keeping twelve rows for the status list.
/// Returns the occupied foot rows and asks for a wake only when its visible cells can change.
pub(super) fn draw(app: &mut App, b: &mut Buffer, pane: Rect, now: u64) -> u16 {
    let width = pane.width.saturating_sub(3);
    // The tab and the blank row above the stage do not belong to the status list.
    let available = pane.height.saturating_sub(2);
    app.stage.select(app.focus.as_ref());
    if app.stage.last_key == 0 {
        app.stage.last_key = now;
    }
    let crew = crew(app);
    if !app.stage.ready {
        app.stage.seen.retain(|id, _| crew.contains(id));
        for row in crew.iter().filter_map(|id| app.agents.get(id)) {
            app.stage.agent(row, now);
        }
    }
    let room = width >= 30 && available >= 24;
    let bodies = !room
        && crew.len() <= 2
        && available >= 30
        && usize::from(width) >= crew.len() * 16 + crew.len().saturating_sub(1) * 2;
    let rows = if room || bodies { 12 } else { 8 };
    let off = app.ui.stage == crate::protocol::Stage::Off;
    if off || width < 8 || available < rows + 12 {
        return 0;
    }
    let motion = super::motion::on(app);
    let resting = scene::rests(app, &crew).is_some_and(|at| now >= at);
    if app.stage.resting && !resting {
        app.stage.woke = now;
    }
    app.stage.resting = resting;
    let octant = app.octant();
    let key = (width, rows, app.stage.revision, motion, octant);
    let cached = app
        .stage
        .cache
        .as_ref()
        .is_some_and(|c| c.key == key && now < c.until);
    if !cached {
        let (lines, until) = if room {
            scene::draw(app, &crew, width as usize, now, motion, octant)
        } else {
            strip::draw(app, &crew, width as usize, bodies, now, motion)
        };
        app.stage.cache = Some(Cache {
            key,
            until,
            rows: lines,
        });
    }
    let foot = Rect::new(pane.x + 2, pane.bottom() - rows, width, rows);
    if let Some(cache) = &app.stage.cache {
        for (y, row) in (foot.y..foot.bottom()).zip(&cache.rows) {
            for (x, (ch, fg, bg)) in (foot.x..foot.right()).zip(row) {
                if let Some(cell) = b.cell_mut((x, y)) {
                    cell.set_char(*ch).set_fg(*fg).set_bg(*bg);
                }
            }
        }
        app.next.at(cache.until);
    }
    app.stage.ready = true;
    app.stage.clock.get_or_insert(now);
    rows
}

/// Reads the root's counts and the focused folder's branch, independent of a visible stage.
/// Focus changes and focused turn ends ask again; at most one request is in flight.
pub(super) fn remember(app: &mut App) {
    app.stage.select(app.focus.as_ref());
    if !app.stage.want || app.stage.reading || app.pending >= 8 {
        return;
    }
    let Some((root, focus)) = app.root.clone().zip(app.focus.clone()) else {
        return;
    };
    app.stage.reading = app.spawn(|api| async move {
        let path = format!("v1/sessions/{root}/project");
        let read = api
            .json(reqwest::Method::GET, &path, None)
            .await
            .unwrap_or_default();
        let counts = read["commits"].as_u64().zip(read["merges"].as_u64());
        let focused = if root == focus {
            read
        } else {
            let path = format!("v1/sessions/{focus}/project");
            api.json(reqwest::Method::GET, &path, None)
                .await
                .unwrap_or_default()
        };
        let branch = focused["branch"].as_str().unwrap_or_default().to_owned();
        let changed = focused["changed"].as_u64().unwrap_or_default();
        move |app: &mut App| {
            if app.focus.as_ref() == Some(&focus) {
                // The branch lights the place; the changed count never does (Trey, 2026-10-15).
                let old = app.folder.as_ref().map(|(id, branch, _)| (id, branch));
                if old.is_some_and(|old| old != (&focus, &branch)) {
                    app.changed[0] = Some(super::now());
                }
                app.folder = Some((focus, branch, changed));
            }
            app.stage.counted(counts, super::now());
        }
    });
    app.stage.want = !app.stage.reading;
}

fn crew(app: &App) -> Vec<SessionId> {
    let mut children: Vec<_> = app
        .agents
        .values()
        .filter(|a| app.focus.is_some() && a.parent == app.focus)
        .collect();
    children.sort_by_key(|a| (a.launched, a.session.clone()));
    app.focus
        .iter()
        .chain(children.iter().map(|a| &a.session))
        .take(64)
        .cloned()
        .collect()
}
