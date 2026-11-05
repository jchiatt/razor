//! The completion list in the band: this terminal's commands and the addressed session's skills
//! for the `/word` being typed, ranked, with Tab, Space, Enter, the arrows and Esc.
use super::super::{
    App,
    cards::Card,
    text::*,
    view::{Action, Hit, PHONE},
};
use crate::protocol::{Body, Skill, now};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Color,
    text::{Line, Span},
};
use reqwest::Method;

/// Each command: its name, what it is for, and its arguments when it takes any.
const COMMANDS: [(&str, &str, Option<&str>); 35] = [
    ("agents", "all agents beside a session", None),
    (
        "name",
        "rename the addressed session now",
        Some("<new name>"),
    ),
    ("new", "start a new agent on any server", None),
    (
        "thread",
        "start a fresh thread with a resident",
        Some("[resident]"),
    ),
    (
        "keys",
        "list every keyboard action and its effective keys",
        None,
    ),
    ("view", "switch the saved view", Some("<name>")),
    ("views", "edit saved views in $EDITOR", None),
    ("record", "record a voice note", None),
    ("listen", "read the newest completed reply aloud", None),
    ("help", "the keys and commands", None),
    ("continue", "continue interrupted sessions", Some("all")),
    (
        "skill",
        "hand the agent one of its skills",
        Some("<name> [words]"),
    ),
    ("detach", "leave; the session keeps running", None),
    (
        "model",
        "pick the model and its effort",
        Some("[name [effort]]"),
    ),
    (
        "effort",
        "pick the effort level",
        Some("[low|medium|high|xhigh|max]"),
    ),
    ("fast", "pick the speed", Some("[on|off|fast|ultrafast]")),
    ("ultrafast", "use Ultrafast for the next input", None),
    ("status", "the session's status card", None),
    ("tasks", "this project's open tasks", None),
    ("resume", "the sessions list, to open another", None),
    ("pin", "pin the newest reply, or unpin it", None),
    ("unattach", "address the parent session again", None),
    (
        "handover",
        "write the continuation note, then rebuild the context",
        None,
    ),
    (
        "attach",
        "open an agent's split by its name or session id",
        Some("<name>"),
    ),
    (
        "to",
        "address an agent by its name or session id",
        Some("<name>"),
    ),
    ("emoji", "the emoji picker", None),
    (
        "gauge",
        "fold or open the context gauge",
        Some("[fold|open]"),
    ),
    (
        "startup",
        "what this conversation was sent at its opening",
        None,
    ),
    (
        "copy",
        "copy the last reply, the shown question or a code block",
        Some("[ask|code [n]]"),
    ),
    (
        "answer",
        "answer the shown question in words",
        Some("<words>"),
    ),
    ("login", "sign in to a model route", None),
    (
        "mcp",
        "MCP servers, their tools and sign-in",
        Some("[add <name> <url | -- command> | import <file>]"),
    ),
    (
        "done",
        "end the session after a closing turn that writes its note",
        None,
    ),
    (
        "end",
        "end the session now, with no closing turn; /quit and /quit! too",
        None,
    ),
    (
        "quit",
        "end the session now, with no closing turn; /end and /quit! too",
        None,
    ),
];
const WORDS: &str = "[words for the agent]";
/// `/help`'s rows: each command with its arguments and what it is for, then the keys.
pub fn help(width: usize) -> Vec<Line<'static>> {
    let mut rows = vec![rule("COMMANDS", None, width)];
    let skill = ("<skill>", "hand the agent that skill", Some(WORDS));
    for (name, about, args) in COMMANDS.into_iter().chain([skill]) {
        let head = format!("/{name}{}", args.map_or(String::new(), |a| format!(" {a}")));
        rows.extend(wrap(
            vec![span(head, CYAN), span(format!("  {about}"), GRAY)],
            width,
        ));
    }
    rows
}
// The kept candidates; a skill row's description keeps this many columns before its tag goes.
const CAP: usize = 60;
const ABOUT_MIN: usize = 12;

/// The list's memory: the draft it was for, the selected row, the first row shown, whether Esc
/// hid it until the draft changes, whether a command was being typed at the last edit, and when
/// the skills were last read.
#[derive(Default)]
pub struct Menu {
    text: String,
    index: usize,
    top: usize,
    hidden: bool,
    typing: bool,
    read: u64,
    ran: Vec<(u64, &'static str)>,
}

#[derive(Clone)]
struct Candidate {
    name: String,
    about: String,
    args: Option<&'static str>,
    skill: bool,
    // An agent's state glyph and its tone, drawn before its name.
    glyph: Option<(&'static str, Color)>,
}

struct List {
    matches: Vec<(Candidate, Vec<usize>)>,
    index: usize,
    top: usize,
}

/// Rows the list wants: five (three on a phone) while the draft is one line starting with `/`,
/// or ends at the cursor in a `/word` after whitespace that names a skill; none otherwise.
pub fn rows(app: &App) -> u16 {
    match subject(app) {
        Some(_) if app.columns < PHONE => 3,
        Some(_) => 5,
        None => 0,
    }
}

/// Draws the list one column in: each candidate's name with the typed letters lit, what it is
/// for, the selected row marked; once a space follows a whole name, that command's usage.
pub fn draw(app: &App, b: &mut Buffer, r: Rect, hits: &mut Vec<Hit>) {
    let Some((text, mid)) = subject(app) else {
        return;
    };
    let cols = r.width.saturating_sub(1).max(1) as usize;
    fill(b, r, style(DATA, RECESS));
    let rows: Vec<Line> = match (word(&text), list(app, &text, mid)) {
        (_, Some(list)) => {
            let (lines, start) = (
                lines(&list, cols, r.height as usize),
                start(&list, r.height as usize),
            );
            for n in 0..lines.len() {
                let at = Rect::new(r.x, r.y + n as u16, r.width, 1);
                hits.push((at, Action::Complete(start + n)));
            }
            lines
        }
        (Some(_), None) if hidden(app, &text) => Vec::new(),
        (Some(_), None) => vec![missing(&text, cols)],
        (None, _) => vec![usage(app, &text, cols)],
    };
    let inner = Rect::new(r.x + 1, r.y, r.width - 1, r.height);
    for (n, row) in rows.iter().take(r.height as usize).enumerate() {
        line(b, inner, r.y + n as u16, row, style(DATA, RECESS));
    }
}

/// Up, Down, Tab, Space, Enter and Esc while the list shows: Tab completes, Enter completes a
/// partial name and leaves a whole one to run, Space completes the only candidate. False leaves
/// the key to the editor.
pub fn captures(app: &App) -> bool {
    subject(app).is_some_and(|(text, mid)| list(app, &text, mid).is_some())
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    let Some((text, mid)) = subject(app) else {
        return false;
    };
    let Some(list) = list(app, &text, mid) else {
        return false;
    };
    let typed = text[text.rfind([' ', '/']).map_or(0, |at| at + 1)..].to_lowercase();
    let (choice, rows) = (list.matches[list.index].0.clone(), rows(app) as usize);
    let partial = choice.name.to_lowercase() != typed;
    match k.code {
        KeyCode::Up | KeyCode::Down => {
            let index = match k.code == KeyCode::Up {
                true => list.index.saturating_sub(1),
                false => (list.index + 1).min(list.matches.len() - 1),
            };
            let top = list.top.max((index + 1).saturating_sub(rows)).min(index);
            (app.menu.text, app.menu.index, app.menu.top) = (text, index, top);
        }
        KeyCode::Tab => complete(app, mid, &text, &choice),
        // Space completes the one candidate only when the letters typed start its name.
        KeyCode::Char(' ')
            if list.matches.len() == 1
                && partial
                && choice.name.to_lowercase().starts_with(&typed) =>
        {
            complete(app, mid, &text, &choice)
        }
        KeyCode::Enter if partial => complete(app, mid, &text, &choice),
        KeyCode::Esc => (app.menu.text, app.menu.hidden) = (text, true),
        _ => return false,
    }
    true
}

/// A click on row `n` of the list completes it, as Tab on that row does.
pub fn click(app: &mut App, n: usize) {
    let Some((text, mid)) = subject(app) else {
        return;
    };
    if let Some((choice, _)) = list(app, &text, mid).and_then(|l| l.matches.into_iter().nth(n)) {
        complete(app, mid, &text, &choice);
    }
}

/// After an edit: reads the addressed session's skills when a command starts being typed, at
/// most once a second.
pub fn edited(app: &mut App, now: u64) {
    let draft = &app.composer.text;
    let typing = draft.starts_with('/') && !draft.contains('\n') || token(app).is_some();
    let (opened, id) = (typing && !app.menu.typing, app.focus.clone());
    app.menu.typing = typing;
    let Some(id) = id.filter(|_| opened && now.saturating_sub(app.menu.read) >= 1000) else {
        return;
    };
    app.menu.read = now;
    app.spawn(|api| async move {
        let read = api
            .json(Method::GET, &format!("v1/sessions/{id}/skills"), None)
            .await;
        let invalid = "The server sent an invalid skills list; reconnect to the server.";
        let read = read.and_then(|v| {
            serde_json::from_value::<Vec<Skill>>(v).map_err(super::super::fail(invalid))
        });
        move |app: &mut App| match read {
            Ok(mut skills) => {
                skills.truncate(1024);
                app.skills = skills;
            }
            Err(e) => app.say(&e.to_string()),
        }
    });
}

/// Notes `word` as run when it names a command, for the list to show among the recent.
pub fn ran(app: &mut App, word: &str) {
    let word = word.strip_prefix('/');
    if let Some(&(name, ..)) = COMMANDS.iter().find(|c| word == Some(c.0)) {
        app.menu.ran.retain(|r| r.1 != name);
        app.menu.ran.insert(0, (now(), name));
        app.menu.ran.truncate(5);
    }
}

/// Whether `name` (without its `/`) names one of this terminal's commands.
pub(super) fn command(name: &str) -> bool {
    COMMANDS.iter().any(|c| c.0 == name)
}

/// The skill a draft starts with (`/writing words`) and the words after it, when it names one.
pub fn skill(app: &App, text: &str) -> Option<(String, String)> {
    let named = text
        .strip_prefix("/skill ")
        .map(|rest| format!("/{}", rest.trim_start()));
    let text = named.as_deref().unwrap_or(text);
    let (name, words) = text
        .strip_prefix('/')?
        .split_once(' ')
        .unwrap_or((&text[1..], ""));
    let known = app.skills.iter().any(|s| s.name == name);
    let command = COMMANDS.iter().any(|c| c.0 == name);
    (known && !command).then(|| (name.to_owned(), words.trim_start().to_owned()))
}

// What is completed and whether it is a skill token mid-draft.
fn subject(app: &App) -> Option<(String, bool)> {
    let draft = &app.composer.text;
    if draft.starts_with('/') && !draft.contains('\n') {
        return Some((draft.clone(), false));
    }
    let token = token(app)?;
    list(app, &token, true)?;
    Some((token, true))
}

// The `/partial` before the cursor, after whitespace, with the cursor at its end.
fn token(app: &App) -> Option<String> {
    let (draft, cursor) = (&app.composer.text, app.composer.cursor());
    if draft.starts_with('/') || draft[cursor..].starts_with(|c: char| !c.is_whitespace()) {
        return None;
    }
    let before = &draft[..cursor];
    let class = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-');
    let start = before.trim_end_matches(class).len();
    let name = &before[start..];
    let mut back = before[..start].chars().rev();
    let after_space = back.next() == Some('/') && back.next().is_some_and(char::is_whitespace);
    let first = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric());
    (after_space && first).then(|| format!("/{name}"))
}

// The word after `/`, lower case, while no whitespace follows it.
fn word(text: &str) -> Option<String> {
    let typed = text.strip_prefix('/')?;
    (!typed.contains(char::is_whitespace)).then(|| typed.to_lowercase())
}

fn hidden(app: &App, text: &str) -> bool {
    app.menu.hidden && app.menu.text == text
}

// The ranked candidates with the menu's selection, or none when nothing matches or Esc hid it:
// commands and skills for a `/word`, agents for the name after `/to ` or `/attach `.
fn list(app: &App, text: &str, mid: bool) -> Option<List> {
    let to = ["/to ", "/attach "]
        .iter()
        .find_map(|c| text.strip_prefix(c));
    let matches = match to.filter(|name| !name.contains(char::is_whitespace)) {
        Some(name) => ranked(&name.to_lowercase(), agents(app)),
        None => ranked(&word(text)?, named(app, mid)),
    };
    if matches.is_empty() || hidden(app, text) {
        return None;
    }
    let same = app.menu.text == text;
    let index = if same {
        app.menu.index.min(matches.len() - 1)
    } else {
        0
    };
    let top = if same { app.menu.top } else { 0 };
    Some(List {
        matches,
        index,
        top,
    })
}

// The commands, then the skills that are not commands, by name, with the five commands run or
// skills loaded last ahead, newest first; mid-draft, only skills.
fn named(app: &App, mid: bool) -> Vec<Candidate> {
    let commands = COMMANDS
        .iter()
        .filter(move |_| !mid)
        .map(|(name, about, args)| Candidate {
            name: (*name).into(),
            about: (*about).into(),
            args: *args,
            skill: false,
            glyph: None,
        });
    let command = |name: &str| !mid && COMMANDS.iter().any(|c| c.0 == name.to_lowercase());
    let mut more: Vec<&Skill> = app.skills.iter().filter(|s| !command(&s.name)).collect();
    more.sort_by_cached_key(|s| (s.name.to_lowercase(), s.name.clone()));
    let skills = more.into_iter().map(|s| Candidate {
        name: s.name.clone(),
        about: sentence(&s.description),
        args: Some(WORDS),
        skill: true,
        glyph: None,
    });
    let t = app.focus.as_ref().and_then(|s| app.transcripts.get(s));
    let loaded = t.into_iter().flat_map(|t| t.entries.values().rev());
    let loaded = loaded.filter_map(|e| match &e.body {
        Body::Input(i) if i.from.is_person() => Some((
            e.at,
            i.text.strip_prefix("<skill name=\"")?.split('"').next()?,
        )),
        _ => None,
    });
    let mut recent: Vec<_> = app.menu.ran.iter().copied().chain(loaded.take(5)).collect();
    recent.sort_by_key(|r| (r.1, std::cmp::Reverse(r.0)));
    recent.dedup_by_key(|r| r.1);
    recent.sort_by_key(|r| std::cmp::Reverse(r.0));
    let mut all: Vec<_> = commands.chain(skills).collect();
    all.sort_by_key(|c| {
        recent
            .iter()
            .take(5)
            .position(|r| r.1 == c.name)
            .unwrap_or(5)
    });
    all
}

// The agents in the status rows but the addressed one: each one's state glyph in its tone,
// its state, model, effort and headline.
fn agents(app: &App) -> impl Iterator<Item = Candidate> {
    let others = app
        .agents
        .values()
        .filter(|a| app.focus.as_ref() != Some(&a.session));
    others.map(|a| {
        let (card, model, effort) = (Card::of(app, a, now()), model(&a.model), effort(a.effort));
        let about = format!("{} · {model} · {effort} · {}", card.word, a.headline);
        Candidate {
            name: a.name.clone(),
            about,
            args: None,
            skill: false,
            glyph: Some((card.glyph, card.tone)),
        }
    })
}

// Best first: the whole name, a prefix, the start of a word inside it, anywhere inside, then its
// letters in order; within a rank, in listing order.
fn ranked(
    typed: &str,
    candidates: impl IntoIterator<Item = Candidate>,
) -> Vec<(Candidate, Vec<usize>)> {
    let mut ranked: Vec<_> = candidates
        .into_iter()
        .enumerate()
        .filter_map(|(order, c)| rank(&c.name, typed).map(|r| (r, order, c)))
        .collect();
    ranked
        .sort_by_key(|((tier, at, _), order, _)| (*tier, if *tier >= 2 { *at } else { 0 }, *order));
    ranked
        .into_iter()
        .take(CAP)
        .map(|((_, _, lit), _, c)| (c, lit))
        .collect()
}

// The tier, where the match starts, and the matched characters of `name`.
fn rank(name: &str, typed: &str) -> Option<(u8, usize, Vec<usize>)> {
    let (n, w): (Vec<char>, Vec<char>) = (
        name.to_lowercase().chars().collect(),
        typed.chars().collect(),
    );
    let span = |at: usize| (at..at + w.len()).collect::<Vec<_>>();
    let found: Vec<usize> = (0..n.len()).filter(|&i| n[i..].starts_with(&w)).collect();
    match found.first() {
        _ if w.is_empty() => return Some((1, 0, Vec::new())),
        _ if n == w => return Some((0, 0, span(0))),
        Some(0) => return Some((1, 0, span(0))),
        _ => {}
    }
    let boundary = found
        .iter()
        .find(|&&i| i > 0 && matches!(n[i - 1], '-' | ':' | '_' | '.' | '!'));
    if let Some(&at) = boundary.or(found.first()) {
        return Some((if boundary.is_some() { 2 } else { 3 }, at, span(at)));
    }
    if w.len() < 2 {
        return None;
    }
    let (mut lit, mut from) = (Vec::new(), 0);
    for ch in &w {
        let at = from + n[from..].iter().position(|c| c == ch)?;
        lit.push(at);
        from = at + 1;
    }
    Some((4, lit[0], lit))
}

// The window of rows around the selection; `↑` or `↓` ends a row when more lies that way.
fn lines(list: &List, cols: usize, max: usize) -> Vec<Line<'static>> {
    let total = list.matches.len();
    let edge = if total > max { 2 } else { 0 };
    let widest = list
        .matches
        .iter()
        .map(|(c, _)| c.name.chars().count())
        .max()
        .unwrap_or(0);
    let name_cols = 1 + widest.min(if cols < 60 { 18 } else { 26 });
    let start = start(list, max);
    let rows = list.matches.iter().enumerate().skip(start).take(max);
    rows.map(|(n, (c, lit))| {
        let on = n == list.index;
        let mark = ["  ", "▸ "][usize::from(on)];
        let mut row = vec![Span::styled(mark, style(CYAN, RECESS))];
        row.push(match c.glyph {
            Some((glyph, tone)) => Span::styled(format!("{glyph} "), style(tone, RECESS)),
            None => Span::styled("/", style(if on { CYAN } else { DATA }, RECESS)),
        });
        let name = clip(&c.name, name_cols - 1);
        let pad = format!("{} ", " ".repeat(name_cols.saturating_sub(width(&name))));
        let lead = 2 + width(&row[1].content);
        let room = cols.saturating_sub(edge + lead + width(&name) + width(&pad));
        let tag = c.skill && room >= width("  skill") + width(&c.about).min(ABOUT_MIN);
        let about = clip(&c.about, room.saturating_sub(if tag { 7 } else { 0 }));
        for (i, ch) in name.chars().enumerate() {
            let s = match (lit.contains(&i), on) {
                (true, _) => bold(WHITE, RECESS),
                (false, true) => style(CYAN, RECESS),
                (false, false) => style(DATA, RECESS),
            };
            row.push(Span::styled(ch.to_string(), s));
        }
        row.push(Span::styled(pad, style(DATA, RECESS)));
        row.push(Span::styled(about, style(GRAY, RECESS)));
        if tag {
            row.push(Span::styled("  skill", style(GRAY, RECESS)));
        }
        let arrow = match n - start {
            0 if start > 0 => "↑",
            i if i + 1 == max && start + max < total => "↓",
            _ => "",
        };
        if !arrow.is_empty() {
            let used: usize = row.iter().map(|s| width(&s.content)).sum();
            let gap = " ".repeat(cols.saturating_sub(1 + used).max(1));
            row.push(Span::styled(format!("{gap}{arrow}"), style(GRAY, RECESS)));
        }
        Line::from(row)
    })
    .collect()
}

// The first row shown: where the window was, moved only as far as keeps the selection in view.
fn start(list: &List, max: usize) -> usize {
    let start = list
        .top
        .max((list.index + 1).saturating_sub(max))
        .min(list.index);
    start.min(list.matches.len().saturating_sub(max))
}

// Once a space follows the name: its usage, or that no command has that name.
fn usage(app: &App, text: &str, cols: usize) -> Line<'static> {
    let name = text[1..]
        .split(char::is_whitespace)
        .next()
        .unwrap_or("")
        .to_lowercase();
    let found = ranked(&name, named(app, false))
        .into_iter()
        .find(|(c, _)| c.name.to_lowercase() == name);
    let Some((c, _)) = found else {
        return missing(text, cols);
    };
    let head = format!(
        "  /{}{}  ",
        c.name,
        c.args.map(|a| format!(" {a}")).unwrap_or_default()
    );
    let about = clip(&c.about, cols.saturating_sub(width(&head)));
    Line::from(vec![
        Span::styled(clip(&head, cols), style(DATA, RECESS)),
        Span::styled(about, style(GRAY, RECESS)),
    ])
}

fn missing(text: &str, cols: usize) -> Line<'static> {
    let name = text.split(char::is_whitespace).next().unwrap_or(text);
    let words = format!("  no command {name} · /help lists them");
    Line::from(Span::styled(clip(&words, cols), style(GRAY, RECESS)))
}

// The chosen name replaces what was typed of it; a command that takes words gets a space.
fn complete(app: &mut App, mid: bool, text: &str, choice: &Candidate) {
    let cursor = app.composer.cursor();
    let (from, space) = match mid {
        true => (cursor - text.len(), " "),
        false => (0, if choice.args.is_some() { " " } else { "" }),
    };
    let to = if mid { cursor } else { app.composer.text.len() };
    let (from, name) = match choice.glyph {
        Some(_) => (text.rfind(' ').map_or(0, |at| at + 1), choice.name.clone()),
        None => (from, format!("/{}{space}", choice.name)),
    };
    app.composer.swap(from..to, &name);
    app.edited();
}
