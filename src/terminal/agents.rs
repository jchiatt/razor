//! The agents view: the list beside the cockpit, its keys and its commands; `sections` renders
//! the server-resolved sessions it shares with the status pane.
mod groups;
mod sections;
use super::view::{self, Action, Hit, Question};
use super::{App, keys, status, text::*};
use crate::protocol::views::{Layout, SectionKind, ViewsStatus};
use crate::protocol::{SessionId, now};
use crossterm::event::{KeyCode as K, KeyEvent};
pub(super) use groups::edit;
use ratatui::{buffer::Buffer, layout::Rect};
pub(super) use sections::{GUTTER, addressed, gutter, mark, rows, section_rows};
use std::collections::BTreeSet;
/// Disposable selection, folding and layouts; layouts are bounded to 8 MiB by the server.
#[derive(Default)]
pub(super) struct List {
    pub open: bool,
    pub keys: bool,
    pub area: Option<Rect>,
    pub editor: bool,
    changed: u64,
    pub selected: Option<Action>,
    pub view: Option<String>,
    status: Option<ViewsStatus>,
    pub scroll: usize,
    reveal: bool,
    /// Headings and children folded from their default (open), and Earlier, finished children's
    /// lines and the servers and residents lines opened (closed); held while their layout holds
    /// them, the servers and residents lines always.
    pub toggled: BTreeSet<String>,
    pub rows: Vec<Action>,
    /// The list's columns as the person last dragged or keyed them, kept in the home's
    /// `list-width`; none, a third of the screen. Set while the edge is held.
    pub width: Option<u16>,
    pub dragging: bool,
    pub kept: std::path::PathBuf,
}
impl List {
    pub fn received(&mut self, mut status: ViewsStatus) {
        if status.error.is_some() {
            self.status = Some(status);
            return;
        }
        if let Some(old) = &mut self.status {
            old.layouts.retain(|l| {
                status.names.contains(&l.name)
                    && !status.layouts.iter().any(|new| new.name == l.name)
            });
            status.layouts.append(&mut old.layouts);
        }
        if (self.view.as_ref()).is_none_or(|name| !status.names.contains(name)) {
            self.view = status.names.first().cloned();
        }
        let sections = || status.layouts.iter().flat_map(|l| &l.sections);
        let rows = || sections().flat_map(|s| &s.sessions);
        self.toggled.retain(|id| {
            let session = id.strip_prefix("finished/").unwrap_or(id);
            let header = ["servers", "residents"].contains(&id.as_str());
            header
                || sections().any(|s| &s.id == id)
                || rows().any(|r| r.session.as_str() == session)
        });
        self.status = Some(status);
    }
    pub(super) fn layout(&self) -> Option<&Layout> {
        let layouts = &self.status.as_ref()?.layouts;
        layouts.iter().find(|l| Some(&l.name) == self.view.as_ref())
    }
}
/// Opens the list with the addressed session highlighted, or closes it to full screen.
pub(super) fn toggle(app: &mut App) {
    if app.list.open && (app.sending.is_some() || super::audio::unsent(app)) {
        return app.say("A note or input is pending; wait before entering a session.");
    }
    app.list.open = !app.list.open;
    app.list.changed = now();
    app.list.reveal = true;
    app.list.keys = app.list.open;
    app.list.selected = app.focus.clone().map(Action::Agent);
    if !app.list.open {
        full(app);
    }
}
fn full(app: &mut App) {
    if app.sending.is_some() || super::audio::unsent(app) {
        return;
    }
    if let Some(id) = app.focus.clone().filter(|id| app.root.as_ref() != Some(id)) {
        let old = app.root.replace(id.clone());
        if let Some(task) = old.and_then(|old| app.streams.remove(&old)) {
            task.abort();
        }
        app.open(id);
    }
}
/// Clicks share the renderer's actions; a list click addresses the right-hand composer.
pub(super) fn press(app: &mut App, action: Action) {
    match action {
        Action::Section(id) => {
            if !app.list.toggled.remove(&id) {
                app.list.toggled.insert(id);
            }
        }
        Action::New => super::start::open(app),
        Action::Thread(home) => thread(app, &home),
        Action::Agent(id) => {
            app.list.selected = Some(Action::Agent(id.clone()));
            keys::focus(app, Some(id));
            app.list.keys = false;
        }
        _ => {}
    }
}
/// List movement moves only the highlight; opening is always an explicit act.
pub(super) fn key(app: &mut App, k: KeyEvent) {
    if k.code == K::F(2) {
        return toggle(app);
    }
    let action = app.list.selected.clone();
    match k.code {
        K::Esc => app.ask_open = None,
        K::Up | K::Down | K::Char('j' | 'k') => {
            let list = &mut app.list;
            let at = list.rows.iter().position(|a| Some(a) == action.as_ref());
            let down = matches!(k.code, K::Down | K::Char('j'));
            let at = at
                .unwrap_or(0)
                .saturating_add_signed(if down { 1 } else { -1 });
            list.selected = list
                .rows
                .get(at.min(list.rows.len().saturating_sub(1)))
                .cloned();
            list.reveal = true;
        }
        K::Left | K::Char('h') => {
            app.list.selected = action.as_ref().and_then(|a| parent(app, a)).or(action)
        }
        K::Right | K::Char('l') => {
            if let Some(ref action) = action {
                let id = match action {
                    Action::Section(id) => Some(id.clone()),
                    Action::Agent(id) => Some(id.to_string()),
                    _ => None,
                };
                if let Some(id) = id {
                    if matches!(id.as_str(), "residents" | "servers" | "earlier") {
                        app.list.toggled.insert(id);
                    } else {
                        app.list.toggled.remove(&id);
                    }
                }
                app.list.selected = into(app, action).or(Some(action.clone()));
            }
        }
        K::Char('<' | '>') => {
            let now = app.list.area.map_or(32, |a| a.width);
            let step = if k.code == K::Char('>') { 4 } else { -4 };
            resize(app, now.saturating_add_signed(step), true);
        }
        K::Char('n') => match aimed(app) {
            Some(home) => thread(app, &home),
            None => app.say(NOT_RESIDENT),
        },
        K::Char(' ') => match action {
            Some(Action::Agent(id)) => press(app, Action::Section(id.to_string())),
            Some(a) => press(app, a),
            None => {}
        },
        K::Enter => match action {
            Some(Action::Agent(id)) => {
                press(app, Action::Agent(id.clone()));
                if app.focus.as_ref() == Some(&id) {
                    toggle(app);
                }
            }
            Some(a) => press(app, a),
            None => {}
        },
        _ => {}
    }
}
/// Takes `columns` as the list's width from here on, at least 32; `keep` saves it for the next
/// start, once a drag lets go.
pub(super) fn resize(app: &mut App, columns: u16, keep: bool) {
    let columns = columns.max(32);
    app.list.width = Some(columns);
    app.list.dragging = !keep;
    if keep {
        let _ = std::fs::write(&app.list.kept, columns.to_string());
    }
}
fn parent(app: &App, action: &Action) -> Option<Action> {
    let sections = &app.list.layout()?.sections;
    let at = match action {
        Action::Agent(id) => {
            let Some((at, _)) = sections
                .iter()
                .enumerate()
                .filter(|(_, s)| s.sessions.iter().any(|r| &r.session == id))
                .max_by_key(|(_, s)| (s.kind != SectionKind::NeedsYou, s.depth))
            else {
                return app.agents.get(id)?.parent.clone().map(Action::Agent);
            };
            let rows = &sections[at].sessions;
            let row = rows.iter().position(|r| &r.session == id)?;
            if let Some(parent) = rows[..row].iter().rev().find(|r| r.depth < rows[row].depth) {
                return Some(Action::Agent(parent.session.clone()));
            }
            return sections[..=at]
                .iter()
                .rev()
                .find(|s| app.list.rows.contains(&Action::Section(s.id.clone())))
                .map(|s| Action::Section(s.id.clone()));
        }
        Action::Section(id) => sections.iter().position(|s| &s.id == id)?,
        Action::Thread(home) => return Some(Action::Agent(home.clone())),
        _ => return None,
    };
    sections[..at]
        .iter()
        .rev()
        .find(|s| s.depth < sections[at].depth)
        .map(|s| Action::Section(s.id.clone()))
}
fn into(app: &App, action: &Action) -> Option<Action> {
    let sections = &app.list.layout()?.sections;
    match action {
        Action::Section(id) => {
            let at = sections
                .iter()
                .position(|s| &s.id == id && s.kind != SectionKind::Residents)?;
            if let Some(child) = sections
                .get(at + 1)
                .filter(|s| s.depth > sections[at].depth)
            {
                let action = Action::Section(child.id.clone());
                if app.list.rows.contains(&action) || !sections[at].sessions.is_empty() {
                    return Some(action);
                }
                return child
                    .sessions
                    .first()
                    .map(|r| Action::Agent(r.session.clone()))
                    .or(Some(action));
            }
            sections[at]
                .sessions
                .first()
                .map(|r| Action::Agent(r.session.clone()))
        }
        Action::Agent(id) => sections.iter().find_map(|s| {
            let at = s.sessions.iter().position(|r| &r.session == id)?;
            s.sessions
                .get(at + 1)
                .filter(|r| r.depth > s.sessions[at].depth)
                .map(|r| Action::Agent(r.session.clone()))
        }),
        _ => None,
    }
}
/// One prefix action, already resolved by the configurable key table.
pub(super) fn prefixed(app: &mut App, k: KeyEvent) {
    if matches!(k.code, K::Char('n' | 'p')) {
        let here = app.focus.clone().map(|id| root(app, id));
        let project = |id: &SessionId| app.agents.get(id).map(|a| &a.project);
        let mine = here.as_ref().and_then(project);
        // The project's roots in list order, whatever heading holds them, less NEEDS YOU, which
        // repeats rows listed below it, and Earlier, whose closed roots n/p never reopens.
        let roots: Vec<_> = (app.list.layout().into_iter())
            .flat_map(|l| &l.sections)
            .filter(|s| !matches!(s.kind, SectionKind::NeedsYou | SectionKind::Earlier))
            .flat_map(|s| &s.sessions)
            .filter(|r| r.depth == 0 && mine.is_some() && project(&r.session) == mine)
            .map(|r| r.session.clone())
            .collect();
        let at = roots.iter().position(|id| Some(id) == here.as_ref());
        if let Some(at) =
            at.and_then(|i| i.checked_add_signed(if k.code == K::Char('n') { 1 } else { -1 }))
            && let Some(id) = roots.get(at).cloned()
        {
            if !app.list.open {
                toggle(app);
            }
            press(app, Action::Agent(id));
        }
    } else if matches!(k.code, K::Char('h' | 'j' | 'k' | 'l')) {
        if !app.list.open {
            toggle(app);
        }
        app.list.keys = true;
        key(app, k);
    }
}
const NOT_RESIDENT: &str =
    "Select a resident or one of its threads, or name one: /thread <resident>.";
/// The resident home sessions on every server, as the list names them.
fn homes(app: &App) -> impl Iterator<Item = SessionId> {
    (app.servers.iter()).flat_map(|s| s.residents.iter().map(|r| r.session.on(&s.name)))
}
/// The resident whose thread `app` would start: that of the selected row while the list has the
/// keys, else of the addressed session; a thread's helper counts for its thread's resident.
fn aimed(app: &App) -> Option<SessionId> {
    let id = match app.list.selected.as_ref().filter(|_| app.list.keys) {
        Some(Action::Agent(id) | Action::Thread(id)) => id,
        _ => app.focus.as_ref()?,
    };
    let top = root(app, id.clone());
    let server = top.address().map_or("", |(_, server)| server);
    let home =
        (app.agents.get(&top)?.resident.as_ref()).map_or_else(|| top.clone(), |r| r.on(server));
    homes(app).find(|h| *h == home)
}
/// Addresses the most recent top-level agent that is neither a resident's home nor its thread,
/// the list keeping the keys: `razor --agents` with nothing else to open (Trey, 2026-11-03).
pub(super) fn recent(app: &mut App) {
    let id = (app.agents.values())
        .filter(|a| a.parent.is_none() && a.resident.is_none())
        .filter(|a| !homes(app).any(|h| h == a.session))
        .max_by_key(|a| (a.last_activity, &a.session))
        .map(|a| a.session.clone());
    if let Some(id) = id {
        press(app, Action::Agent(id));
        app.list.keys = true;
    }
}
/// Aims the composer at a fresh thread with the resident whose home session is `home`.
fn thread(app: &mut App, home: &SessionId) {
    app.list.keys = false;
    let (name, server) = home.address().unwrap_or((home.clone(), ""));
    super::start::aim(app, server.into(), Default::default(), Some(name));
}
fn root(app: &App, mut id: SessionId) -> SessionId {
    for _ in 0..app.agents.len() {
        let Some(Action::Agent(parent)) = parent(app, &Action::Agent(id.clone())) else {
            break;
        };
        id = parent;
    }
    id
}
/// The agents list's own commands, `/group`, `/agents`, `/new`, `/thread`, `/view` and `/views`;
/// false for another.
pub(super) fn command(app: &mut App, word: &str, args: &str) -> bool {
    match word {
        "/group" => groups::file(app, args),
        "/agents" => toggle(app),
        "/new" => super::start::open(app),
        "/thread" => {
            let named = homes(app).find(|h| {
                h.as_str() == args || h.address().is_some_and(|(n, _)| n.as_str() == args)
            });
            match named.or_else(|| aimed(app).filter(|_| args.is_empty())) {
                Some(home) => thread(app, &home),
                None => app.say(NOT_RESIDENT),
            }
        }
        "/view"
            if (app.list.status.as_ref()).is_some_and(|s| s.names.iter().any(|n| n == args)) =>
        {
            app.list.view = Some(args.into())
        }
        "/view" => app.say("That view is unknown; use a name from /views."),
        "/views" => app.list.editor = true,
        _ => return false,
    }
    true
}
/// Slides the shared list beside the cockpit; narrow screens give the whole width to its owner.
pub(super) fn frame(
    app: &mut App,
    b: &mut Buffer,
    area: Rect,
    question: Question,
    now: u64,
    hits: &mut Vec<Hit>,
) -> Option<(u16, u16)> {
    let elapsed = super::motion::played(super::motion::on(app), now, app.list.changed, 250);
    let share = super::motion::ease_out(elapsed);
    let share = if app.list.open { share } else { 1. - share };
    if elapsed < 1. {
        app.next.every(now, 33);
    }
    let wide = area.width >= 80;
    let width = if wide {
        let chosen = app.list.width.unwrap_or((area.width / 3).clamp(32, 64));
        (chosen.clamp(32, area.width.saturating_sub(40).max(32)) as f64 * share).round() as u16
    } else {
        area.width * u16::from(app.list.open && app.list.keys)
    };
    let list = Rect { width, ..area };
    app.list.area = (width > 0).then_some(list);
    let right = Rect {
        x: area.x + width,
        width: area.width - width,
        ..area
    };
    // Beside the list the pane is closed until asked for.
    let pane = app.pane_open.replace(app.pane_open == Some(true));
    let cursor = (right.width > 0).then(|| view::normal(app, b, right, question, now, hits));
    app.pane_open = pane;
    // Narrower than its edges and a column each side, the sliding list is blank.
    if width > 5 && area.height > 2 {
        draw(app, b, list, wide, now, hits);
    }
    cursor.flatten().filter(|_| !app.list.keys)
}

// The list in its frame, its drag edge when wide, and its rows from the scroll, which reveals a
// moved selection; the rows' actions are the arrows' stops, and the bottom edge names the
// addressed row while it is scrolled away.
fn draw(app: &mut App, b: &mut Buffer, list: Rect, wide: bool, now: u64, hits: &mut Vec<Hit>) {
    let title = format!("AGENTS · {}", app.list.view.as_deref().unwrap_or("…"));
    let edge = if app.list.dragging { WHITE } else { CYAN };
    let inner = super::panel::frame_colour(b, list, &title, edge);
    if wide {
        let column = Rect::new(
            list.right() - 1,
            list.y + 1,
            1,
            list.height.saturating_sub(2),
        );
        hits.push((column, Action::Edge));
    }
    let rows = rows(app, inner.width, now);
    // A parent's own fold is no stop of its own: Space on its row folds it.
    let own = |r: &status::Row, a: &Action| matches!((&r.1, a), (Some(Action::Agent(id)), Action::Section(s)) if s == id.as_str());
    let (mut seen, actions) = (
        BTreeSet::new(),
        rows.iter()
            .flat_map(|r| (r.2.iter().map(|(_, a)| a).filter(|a| !own(r, a))).chain(r.1.iter())),
    );
    // An agent listed twice, a shortcut and its row, is one stop for the arrows.
    let unique = actions.filter(|a| !matches!(a, Action::Agent(id) if !seen.insert(id)));
    app.list.rows = unique.cloned().collect();
    // A folded heading's mark and its line are one stop.
    app.list.rows.dedup();
    let selected = rows
        .iter()
        .position(|status::Row(_, a, _)| a.is_some() && a == &app.list.selected);
    let height = inner.height as usize;
    if std::mem::take(&mut app.list.reveal)
        && let Some(at) = selected
    {
        app.list.scroll = app.list.scroll.min(at).max(at.saturating_sub(height - 1));
    }
    let scroll = app.list.scroll.min(rows.len().saturating_sub(height));
    // The bottom edge names the addressed row while it is scrolled away.
    let here = rows.iter().position(|row| row.0.style.bg == Some(YOU_BG));
    let name = app.focus.as_ref().and_then(|id| app.agents.get(id));
    let name = name.map_or("", |a| a.name.as_str());
    let (words, ink) = match here {
        Some(at) if at < scroll => (format!(" ▲ talking to {name} "), bold(CYAN, GLASS)),
        Some(at) if at >= scroll + height => (format!(" ▼ talking to {name} "), bold(CYAN, GLASS)),
        _ => (String::new(), style(GRAY, GLASS)),
    };
    put(b, inner.x, inner.bottom(), &words, inner.width, ink);
    app.list.scroll = scroll;
    for (y, status::Row(words, action, fold)) in
        (inner.y..inner.bottom()).zip(rows.into_iter().skip(scroll))
    {
        let r = Rect::new(inner.x, y, inner.width, 1);
        if words.style.bg == Some(YOU_BG) {
            fill(
                b,
                Rect::new(list.x + 1, y, list.width - 2, 1),
                style(WHITE, YOU_BG),
            );
            put(b, list.x + 1, y, "▌", 1, style(CYAN, YOU_BG));
        }
        line(b, r, y, &words, style(WHITE, GLASS));
        let fold = fold
            .into_iter()
            .map(|(x, a)| (Rect::new(r.x + x, y, 2, 1), a));
        hits.extend(action.map(|a| (r, a)).into_iter().chain(fold));
    }
}
