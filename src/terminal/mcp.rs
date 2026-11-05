//! `/mcp`: the MCP servers the focused session can use, and one server's connection,
//! operations and tools, in the panel over the transcript. Every list is a snapshot answer of
//! `POST /v1/mcp`; the panel keeps the newest and where the person is in it.
use super::{App, Error, cards::PULSE, fail, panel, panel::Panel, text::*, view};
use crate::protocol::{Connection, McpRow, McpScope, Reference, now};
use crate::terminal::bindings;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect, text::Line, text::Span};
use reqwest::Method;
use serde_json::{Map, Value, json};
use std::{path::Path, time::Duration};

/// The open `/mcp` panel: the newest list answer and where the person is in it.
#[derive(Default)]
pub struct Mcp {
    /// The newest list answer, ordered by the file each server wins from.
    rows: Vec<McpRow>,
    /// The chosen server, marked in the list.
    chosen: String,
    /// The chosen server's view is open over the list.
    open: bool,
    /// The foot's sentence until the next key: `✓` an operation's outcome, `✗` a refusal, `?`
    /// what waits for its confirming key.
    foot: Option<(char, String)>,
    /// Operations waiting for their confirming key: a second `x` for a removal, Enter for a
    /// previewed import's additions.
    confirm: Option<(KeyCode, Vec<Value>)>,
    /// Reads asked, counted from the instant the panel opened: only the newest read's answer
    /// is applied, and only to this panel.
    asked: u64,
}

/// The files a server can win from, in the order they win, with their rules' labels.
const FILES: [(Option<McpScope>, &str); 4] = [
    (Some(McpScope::Private), "PRIVATE"),
    (Some(McpScope::Project), "PROJECT"),
    (Some(McpScope::Global), "GLOBAL"),
    (None, "PACK"),
];
type Rows = Vec<Line<'static>>;
const USAGE: &str = "Use /mcp add <name> <url> [--header NAME=env:VAR] [--scope global|project|private], /mcp add <name> -- <command…>, or /mcp import <file>.";
const BY_REFERENCE: &str =
    "A header is read from Razor's environment or a file: --header NAME=env:VAR or NAME=file:PATH.";
const SESSIONLESS: &str = "Send a first message to start a session, then open /mcp.";

/// `/mcp` typed in the composer. Alone it opens the list. `add NAME URL [--header
/// NAME=env:VAR|NAME=file:PATH]… [--scope global|project|private]` and `add NAME [--scope …]
/// -- COMMAND ARGS…` add a server, to the private file unless a scope is given; `import FILE
/// [--scope …]` previews another client's servers for Enter to import. Each opens the list,
/// whose foot shows the outcome. True when the line was taken and the composer clears; a
/// malformed line is refused in the band and kept, but a header given other than by reference
/// clears the composer after its refusal.
pub fn command(app: &mut App, args: &str) -> bool {
    if app.focus.is_none() {
        return refuse(app, SESSIONLESS, false);
    }
    let mut words = args.split_whitespace();
    let (verb, mut scope, mut headers) = (words.next(), "private", Map::new());
    let (mut plain, mut config) = (Vec::new(), Value::Null);
    while let Some(word) = words.next() {
        match word {
            "--scope" => scope = words.next().unwrap_or_default(),
            "--" => {
                let (command, args) = (words.next(), words.by_ref().collect::<Vec<_>>());
                config = json!({"command": command, "args": args});
            }
            url if url.contains("://") => config = json!({"url": url}),
            "--header" => {
                let header = words.next().and_then(|h| h.split_once('='));
                let (name, value) = header.unwrap_or_default();
                let reference = match value.split_once(':') {
                    Some(("env", var)) => json!({"env": var}),
                    Some(("file", path)) => json!({"file": typed(app, path)}),
                    _ => return refuse(app, BY_REFERENCE, true),
                };
                headers.insert(name.into(), reference);
            }
            word => plain.push(word),
        }
    }
    let operation = match (verb, &plain[..]) {
        (None, []) => None,
        (Some("add"), [name]) => {
            config["headers"] = Value::Object(headers);
            Some(json!({"op": "add", "name": name, "scope": scope, "config": config}))
        }
        (Some("import"), [file]) => {
            Some(json!({"op": "import", "source": typed(app, file), "scope": scope}))
        }
        _ => return refuse(app, USAGE, false),
    };
    let mcp = Mcp {
        asked: now(),
        ..Mcp::default()
    };
    (app.panel, app.panel_scroll) = (Some(Panel::Mcp(mcp)), 0);
    read(app, Duration::ZERO);
    post(app, operation.into_iter().collect());
    true
}

fn refuse(app: &mut App, words: &str, taken: bool) -> bool {
    app.say(words);
    taken
}

/// Reads the list for the open `/mcp` panel, if there is one, after `after`. It reads when it
/// opens, after operations' answers, when the focused session starts working, and a second after
/// an answer in which a server is still connecting; each answer is a snapshot of its moment. A
/// newer read makes this one's answer stale, so one chain of reads a second apart at most.
pub fn read(app: &mut App, after: Duration) {
    let Some(Panel::Mcp(mcp)) = &mut app.panel else {
        return;
    };
    mcp.asked += 1;
    let asked = mcp.asked;
    let body = json!({"op": "list", "cwd": app.cwd, "session": app.focus});
    app.spawn(move |api| async move {
        tokio::time::sleep(after).await;
        let read = api.json(Method::POST, "v1/mcp", Some(body)).await;
        let invalid = "The server sent an invalid MCP list; reconnect to the server.";
        let rows = read.and_then(|v| serde_json::from_value(v).map_err(fail(invalid)));
        move |app: &mut App| listed(app, asked, rows)
    });
}

fn listed(app: &mut App, asked: u64, rows: Result<Vec<McpRow>, Error>) {
    let Some(Panel::Mcp(mcp)) = &mut app.panel else {
        return;
    };
    let mut rows = match rows {
        _ if mcp.asked != asked => return,
        Ok(rows) => rows,
        Err(e) => return mcp.foot = Some(('✗', e.to_string())),
    };
    rows.sort_by_key(|row| FILES.iter().position(|f| f.0 == row.scope));
    if !rows.iter().any(|row| row.name == mcp.chosen) {
        let first = rows.first().map(|row| row.name.clone());
        (mcp.chosen, mcp.open) = (first.unwrap_or_default(), false);
    }
    let connecting = rows.iter().any(|row| row.state == "connecting");
    mcp.rows = rows;
    if connecting {
        read(app, Duration::from_secs(1));
    }
}

/// Posts `operations` in order for the focused session, past any refusal, then reads the list.
/// One operation's answer is the foot's outcome, or for an import the preview waiting for
/// Enter; several are counted, with the first refusal.
fn post(app: &mut App, operations: Vec<Value>) {
    if operations.is_empty() {
        return;
    }
    let (cwd, session) = (json!(app.cwd), json!(app.focus));
    app.spawn(|api| async move {
        let mut answers = Vec::new();
        for mut operation in operations {
            (operation["cwd"], operation["session"]) = (cwd.clone(), session.clone());
            let answer = api.json(Method::POST, "v1/mcp", Some(operation.clone()));
            answers.push((operation, answer.await));
        }
        move |app: &mut App| {
            if let Some(Panel::Mcp(mcp)) = &mut app.panel {
                answered(mcp, answers);
            }
            read(app, Duration::ZERO);
        }
    });
}

fn answered(mcp: &mut Mcp, answers: Vec<(Value, Result<Value, Error>)>) {
    let path = |v: &Value| home(Path::new(v.as_str().unwrap_or_default()));
    mcp.foot = Some(match &answers[..] {
        [(operation, Ok(preview))] if operation["op"] == "import" => {
            let operations = preview["operations"].as_array().cloned();
            mcp.confirm = Some((KeyCode::Enter, operations.unwrap_or_default()));
            ('?', imports(preview, &path(&preview["destination"])))
        }
        [(operation, Ok(answer))] => ('✓', said(operation, &path(&answer["file"]))),
        [(_, Err(e))] => ('✗', e.to_string()),
        _ => match answers.iter().find_map(|(_, answer)| answer.as_ref().err()) {
            Some(e) => ('✗', e.to_string()),
            None => ('✓', format!("Imported {} servers.", answers.len())),
        },
    });
}

fn said(operation: &Value, file: &str) -> String {
    let name = operation["name"].as_str().unwrap_or_default();
    match operation["op"].as_str().unwrap_or_default() {
        "add" => format!("Added {name} to {file}."),
        "remove" => format!("Removed {name} from {file}."),
        "enable" => format!("Enabled {name} in {file}."),
        "disable" => format!("Disabled {name} in {file}."),
        "reconnect" => format!("{name} is reconnecting."),
        "sign_out" => format!("Signed out of {name}."),
        _ => format!(
            "The link to sign in to {name} is in the ask above the composer; answer it with the \
             address the browser lands on."
        ),
    }
}

/// What Enter on an import's preview does: the servers it adds and the file it writes, which
/// it replaces there, the credential fields it leaves out, what it cannot read.
fn imports(preview: &Value, into: &str) -> String {
    let mut words = format!("<mcp.accept> imports into {into}:");
    let says = [" ", "; replacing ", "; leaving out ", "; unable to read "];
    let keys = ["operations", "replacements", "unresolved", "errors"];
    for (says, key) in says.iter().zip(keys) {
        let mut each = Vec::new();
        for v in preview[key].as_array().into_iter().flatten() {
            // An addition or an error by its name, a replacement as itself, a field after its name.
            let parts = [&v["name"], &v["field"], v].map(Value::as_str);
            each.push(parts.into_iter().flatten().collect::<Vec<_>>().join(" "));
        }
        if !each.is_empty() {
            words += &format!("{says}{}", each.join(", "));
        }
    }
    words + "."
}

/// One key while the panel shows `/mcp`; false leaves it to the panel, which scrolls or
/// closes. Any key ends the foot's sentence, and any but its own a confirmation.
pub fn key(app: &mut App, key: KeyEvent) -> bool {
    let Some(Panel::Mcp(mcp)) = &mut app.panel else {
        return false;
    };
    mcp.foot = None;
    if let Some((_, operations)) = mcp.confirm.take().filter(|c| c.0 == key.code) {
        post(app, operations);
        return true;
    }
    let at = mcp.rows.iter().position(|row| row.name == mcp.chosen);
    match (mcp.open, key.code, at) {
        (false, KeyCode::Up | KeyCode::Down, Some(at)) => {
            let step = if key.code == KeyCode::Up { -1 } else { 1 };
            let at = at.saturating_add_signed(step).min(mcp.rows.len() - 1);
            mcp.chosen = mcp.rows[at].name.clone();
        }
        (false, KeyCode::Enter, Some(_)) => (mcp.open, app.panel_scroll) = (true, 0),
        (true, KeyCode::Esc, _) => (mcp.open, app.panel_scroll) = (false, 0),
        (true, KeyCode::Char(c), Some(at)) => {
            let row = &mcp.rows[at];
            let Some((_, words)) = operations(row).find(|o| o.0 == c) else {
                return true;
            };
            let (op, scope) = (
                words.replace(' ', "_"),
                row.scope.unwrap_or(McpScope::Global),
            );
            let operation = vec![json!({"op": op, "name": row.name, "scope": scope})];
            if c != 'x' {
                post(app, operation);
                return true;
            }
            let file = row.file.as_deref().map(home).unwrap_or_default();
            let asking = format!(
                "Press <mcp.remove> again to remove {} from {file}.",
                row.name
            );
            (mcp.foot, mcp.confirm) = (Some(('?', asking)), Some((key.code, operation)));
        }
        _ => return false,
    }
    true
}

/// A press on a server's row in the list chooses and opens it; every other mouse event is
/// dropped while a panel is open.
pub fn captures(app: &App, key: KeyEvent) -> bool {
    matches!(&app.panel, Some(Panel::Mcp(mcp)) if key.code == KeyCode::Esc || if mcp.open { matches!(key.code, KeyCode::Char(_)) } else { true })
}

pub fn click(app: &mut App, m: MouseEvent) {
    let at = (m.column, m.row).into();
    let hit = app.hits.iter().rev().find(|(area, _)| area.contains(at));
    let pressed = m.kind == MouseEventKind::Down(MouseButton::Left);
    if let (true, Some((_, view::Action::Mcp(name)))) = (pressed, hit)
        && let Some(Panel::Mcp(mcp)) = &mut app.panel
    {
        mcp.chosen = name.clone();
        key(app, KeyCode::Enter.into());
    }
}

/// Lays the list or the chosen server out in the panel's box over `r`, keeps the chosen row
/// on screen, and makes each server row of the list a click target. Asks the frame clock for
/// the pulse only while a drawn server connects and motion is on.
pub fn draw(app: &mut App, b: &mut Buffer, r: Rect, now: u64, hits: &mut Vec<view::Hit>) {
    let Some(Panel::Mcp(mcp)) = &app.panel else {
        return;
    };
    let width = r.width.saturating_sub(4) as usize;
    let open = |row: &&McpRow| mcp.open && row.name == mcp.chosen;
    let open = mcp.rows.iter().find(open);
    let (rows, targets) = match open {
        Some(row) => (server(app, row, width, now), Vec::new()),
        None => servers(app, mcp, width, now),
    };
    let keys = match (&mcp.confirm, open) {
        (Some((KeyCode::Enter, _)), _) => "<mcp.accept> imports · <mcp.cancel> back",
        (_, Some(_)) => "<panel.up>/<panel.down> scroll · <mcp.cancel> back",
        _ => "<mcp.up>/<mcp.down> choose · <mcp.accept> open · <mcp.cancel> back",
    };
    let mut foot = match &mcp.foot {
        Some((mark, words)) => {
            let tones = [('✓', GREEN), ('✗', AMBER), ('?', CYAN)];
            let tone = tones.iter().find(|t| t.0 == *mark).map_or(CYAN, |t| t.1);
            let mark = span(format!("{mark} "), tone);
            wrap(vec![mark, span(bindings::text(app, words), DATA)], width)
        }
        None => Vec::new(),
    };
    foot.push(Line::from(span(bindings::text(app, keys), GRAY)));
    let title = open.map_or("mcp".into(), |row| format!("mcp · {}", row.name));
    let at = targets.iter().find(|t| t.1 == mcp.chosen).map(|t| t.0);
    let words = panel::frame(b, r, &title);
    let height = words.height.saturating_sub(foot.len() as u16);
    let body = Rect::new(words.x, words.y, words.width, height);
    if let Some(at) = at {
        let first = (at + 1).saturating_sub(height as usize);
        app.panel_scroll = app.panel_scroll.min(at).max(first);
    }
    panel::scroll(app, b, body, &rows);
    let row = |y| Rect::new(words.x, y, words.width, 1);
    for (foot, y) in foot.iter().zip(body.bottom()..words.bottom()) {
        line(b, words, y, foot, style(DATA, GLASS));
    }
    let shown = app.panel_scroll..app.panel_scroll + height as usize;
    for (at, name) in targets.into_iter().filter(|t| shown.contains(&t.0)) {
        let y = body.y + (at - app.panel_scroll) as u16;
        hits.push((row(y), view::Action::Mcp(name)));
    }
}

/// The servers under a rule for each file they win from, the chosen one marked, then how to
/// add one; with each server's row, for its click target.
fn servers(app: &App, mcp: &Mcp, width: usize, now: u64) -> (Rows, Vec<(usize, String)>) {
    let (mut rows, mut targets, mut last) = (Vec::new(), Vec::new(), None);
    let names = mcp.rows.iter().map(|row| row.name.chars().count()).max();
    for row in &mcp.rows {
        if last != Some(row.scope) {
            let label = FILES.iter().find(|f| f.0 == row.scope).map_or("", |f| f.1);
            let file = row.file.as_deref().map(home).filter(|_| width >= 48);
            let file = file.map(|file| tail(&file, width.saturating_sub(label.len() + 6)));
            rows.extend([Line::default(), rule(label, file.as_deref(), width)]);
            last = Some(row.scope);
        }
        targets.push((rows.len(), row.name.clone()));
        let chosen = row.name == mcp.chosen;
        let (mark, words) = state(app, row, now);
        let name = format!(" {:1$}  ", row.name, names.unwrap_or(0));
        let name = match chosen {
            true => Span::styled(name, bold(WHITE, GLASS)),
            false => span(name, DATA),
        };
        let chosen = span(if chosen { "▸ " } else { "  " }, CYAN);
        rows.push(Line::from(vec![chosen, mark, name, span(words, GRAY)]));
    }
    let hints = ["/mcp add <name> <url | -- command>", "/mcp import <file>"];
    rows.push(Line::default());
    rows.extend(hints.map(|hint| Line::from(span(hint, GRAY_DIM))));
    (rows, targets)
}

/// One server: its state and error, what it connects to, the operations that apply, and its
/// tools with the first line of each one's description.
fn server(app: &App, row: &McpRow, width: usize, now: u64) -> Rows {
    let (mark, words) = state(app, row, now);
    let bold = bold(WHITE, GLASS);
    let name = Span::styled(format!(" {}  ", row.name), bold);
    let mut rows = vec![
        Line::default(),
        Line::from(vec![mark, name, span(words, GRAY)]),
    ];
    if let Some(error) = &row.error {
        rows.extend(wrap(vec![span(error.clone(), GRAY)], width));
    }
    rows.push(Line::default());
    for (label, value) in fields(row) {
        let (label, value) = (format!("{label:9}"), tail(&value, width.saturating_sub(9)));
        rows.push(Line::from(vec![span(label, GRAY), span(value, DATA)]));
    }
    rows.extend([Line::default(), rule("OPERATIONS", None, width)]);
    for (_, words) in operations(row) {
        let chord = bindings::hint(app, &format!("mcp.{}", words.replace(' ', "_")));
        let key = span(format!("{chord}  "), WHITE);
        rows.push(Line::from(vec![key, span(words, GRAY)]));
    }
    let count = row.tools.len().to_string();
    rows.extend([Line::default(), rule("TOOLS", Some(&count), width)]);
    let names = row.tools.iter().map(|tool| tool.0.chars().count()).max();
    let names = names.unwrap_or(0).min(width / 2);
    for (name, about) in &row.tools {
        let about = span(clip(about, width.saturating_sub(names + 2)), GRAY);
        let name = span(format!("{name:names$}  "), DATA);
        rows.push(Line::from(vec![name, about]));
    }
    rows
}

/// What a server connects to, never a credential: an address's scheme and host, a command's
/// executable, its environment's names and its headers' references; then its sign-in and file.
fn fields(row: &McpRow) -> Vec<(&'static str, String)> {
    let mut fields = Vec::new();
    match &row.config.connection {
        Some(Connection::Http { url, headers, .. }) => {
            let origin = reqwest::Url::parse(url).map(|u| u.origin().ascii_serialization());
            fields.push(("url", origin.unwrap_or_default()));
            for (name, at) in headers {
                let at = match at {
                    Reference::Env(var) => format!("env:{var}"),
                    Reference::File(path) => format!("file:{}", home(path)),
                };
                fields.push(("header", format!("{name}={at}")));
            }
        }
        Some(Connection::Stdio { command, env, .. }) => {
            fields.push(("command", command.clone()));
            let names: Vec<_> = env.keys().map(String::as_str).collect();
            fields.extend((!names.is_empty()).then(|| ("env", names.join(", "))));
        }
        None => {}
    }
    fields.extend(row.signed_in.then(|| ("sign-in", "signed in".into())));
    fields.extend(row.file.as_deref().map(|file| ("file", home(file))));
    fields
}

/// A server's mark and the words for its state; a connecting server's mark turns, with motion
/// on.
fn state(app: &App, row: &McpRow, now: u64) -> (Span<'static>, String) {
    let tools = row.tools.len();
    // The files decide enabled and disabled at once; the session catches up at its next turn.
    let state = match (row.config.enabled, row.state.as_str()) {
        (false, _) => "disabled",
        (true, "disabled") => "configured",
        (true, state) => state,
    };
    let (mark, tone, words) = match state {
        "connected" => {
            let s = plural(tools);
            ("●", GREEN, format!("connected · {tools} tool{s}"))
        }
        "connecting" if super::motion::on(app) => {
            app.next.every(now, PULSE.0);
            let turn = PULSE.1[(now / PULSE.0 % 4) as usize];
            (turn, GREEN, "connecting".into())
        }
        "connecting" => ("◐", GREEN, "connecting".into()),
        "failed" => ("✗", AMBER, "failed".into()),
        "needs_sign_in" => ("?", CYAN, "needs sign-in".into()),
        "configured" => ("○", GRAY, "starts next turn".into()),
        other => ("○", GRAY_DIM, other.into()),
    };
    (span(mark, tone), words)
}

/// The operations that apply to a server: each one's key and words, whose `_` form is the
/// API's name for it.
fn operations(row: &McpRow) -> impl Iterator<Item = (char, &'static str)> {
    let http = matches!(row.config.connection, Some(Connection::Http { .. }));
    let on = row.config.enabled;
    [
        (http, 's', "sign in"),
        (row.signed_in, 'o', "sign out"),
        (on, 'r', "reconnect"),
        (on, 'd', "disable"),
        (!on, 'e', "enable"),
        (row.scope.is_some(), 'x', "remove"),
    ]
    .into_iter()
    .filter_map(|(applies, key, words)| applies.then_some((key, words)))
}

/// The end of `s` within `columns` cells, after `…` when cut: a path's end names its file.
fn tail(s: &str, columns: usize) -> String {
    let fits = |at: &usize| width(&s[*at..]) < columns;
    match s.char_indices().map(|(at, _)| at).find(fits) {
        _ if width(s) <= columns => s.into(),
        Some(at) => format!("…{}", &s[at..]),
        None => "…".into(),
    }
}

/// `path` with the home directory written `~`.
pub(super) fn home(path: &Path) -> String {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty());
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// A typed path: `~/` is the home directory, and a relative one is under the terminal's.
fn typed(app: &App, path: &str) -> std::path::PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => Path::new(&home).join(rest),
        _ => app.cwd.join(path),
    }
}
