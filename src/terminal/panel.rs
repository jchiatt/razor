//! A card over the transcript: `/status`, `/tasks`, `/login` with the server's sign-in
//! instructions, or `/mcp`.
use super::{App, mcp, text::*, view};
use crate::protocol::Login;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
};
use std::collections::VecDeque;

pub enum Panel {
    Status,
    /// The addressed session's project's open tasks, a line each.
    Tasks(Vec<Line<'static>>),
    Help,
    Keys,
    // Sign-in instructions are transient API output, at most 64 KiB in 128 lines.
    Login {
        lines: VecDeque<String>,
        running: bool,
    },
    /// `/mcp`, which lays out and keys its own views inside the box.
    Mcp(mcp::Mcp),
}

/// Opens the panel `word` names (`/status`, `/help`, `/keys`, `/tasks` or `/login`); false for
/// another command.
pub fn command(app: &mut App, word: &str) -> bool {
    match word {
        "/status" => (app.panel, app.panel_scroll) = (Some(Panel::Status), 0),
        "/help" => (app.panel, app.panel_scroll) = (Some(Panel::Help), 0),
        "/keys" => (app.panel, app.panel_scroll) = (Some(Panel::Keys), 0),
        "/tasks" => tasks(app),
        "/login" => start(app),
        _ => return false,
    }
    true
}

/// Starts the server's sign-in; Escape cancels it by dropping its stream.
fn start(app: &mut App) {
    let (lines, running) = (VecDeque::new(), true);
    (app.panel, app.panel_scroll) = (Some(Panel::Login { lines, running }), 0);
    let (Some(api), Some(tx), Some(budget)) =
        (app.api.clone(), app.lines.clone(), app.budget.clone())
    else {
        return;
    };
    let task = tokio::spawn(async move { api.login(tx, budget).await });
    app.login = Some(task.abort_handle());
    app.spawn(|_| async move {
        let ended = task.await;
        // A cancelled sign-in leaves the handle of any later one alone.
        move |app: &mut App| match ended {
            Ok(Ok(())) => {
                app.login = None;
                if let Some(Panel::Login { running, .. }) = &mut app.panel {
                    *running = false;
                }
            }
            Ok(Err(e)) => {
                app.login = None;
                show(app, &e.to_string(), true);
            }
            Err(_) => {}
        }
    });
}

/// One sign-in line from the server.
pub fn login(app: &mut App, line: Login) {
    match line {
        Login::Show { text } => show(app, &text, false),
        Login::Done => show(app, "The server login completed.", true),
        Login::Failed { error } => show(app, &error, true),
    }
}

fn show(app: &mut App, text: &str, ended: bool) {
    let Some(Panel::Login { lines, running }) = &mut app.panel else {
        return;
    };
    *running = !ended;
    for line in screen_safe(text).lines() {
        lines.push_back(clip(line, 8192));
        while lines.len() > 128 || lines.iter().map(String::len).sum::<usize>() > 64 << 10 {
            lines.pop_front();
        }
    }
}

/// One key: an open `/mcp` takes it first, and what it leaves scrolls or closes the panel.
pub fn key(app: &mut App, key: KeyEvent) {
    if matches!(app.panel, Some(Panel::Mcp(_))) && mcp::key(app, key) {
        return;
    }
    app.panel_scroll = match key.code {
        KeyCode::Esc => {
            if let (Some(Panel::Login { .. }), Some(task)) = (&app.panel, app.login.take()) {
                task.abort();
            }
            app.panel = None;
            0
        }
        KeyCode::Up => app.panel_scroll.saturating_sub(1),
        KeyCode::PageUp => app.panel_scroll.saturating_sub(20),
        KeyCode::Down => app.panel_scroll.saturating_add(1),
        KeyCode::PageDown => app.panel_scroll.saturating_add(20),
        KeyCode::Home => 0,
        KeyCode::End => usize::MAX,
        _ => app.panel_scroll,
    };
}

/// The card in a rule box over `r`, two columns in from each side and edge to edge below
/// `view::PHONE`, scrolled, with its keys on the last row inside.
pub fn draw(app: &mut App, b: &mut Buffer, r: Rect, now: u64, hits: &mut Vec<view::Hit>) {
    if r.width < 8 || r.height < 4 {
        return;
    }
    let inset = if app.columns < view::PHONE { 0 } else { 2 };
    let r = Rect::new(r.x + inset, r.y, r.width - 2 * inset, r.height);
    let width = r.width.saturating_sub(4) as usize;
    let (title, rows) = match &app.panel {
        Some(Panel::Status) => ("status", super::status::rows(app, width, now)),
        Some(Panel::Tasks(rows)) => ("tasks", rows.clone()),
        Some(Panel::Help) => ("help", super::composer::completion::help(width)),
        Some(Panel::Keys) => ("keys", super::bindings::rows(app, width)),
        Some(Panel::Login { lines, running, .. }) => {
            let rows = lines
                .iter()
                .flat_map(|s| wrap(vec![Span::raw(s.clone())], width))
                .collect();
            (
                if *running {
                    "login · waiting for sign-in"
                } else {
                    "login"
                },
                rows,
            )
        }
        Some(Panel::Mcp(_)) => return mcp::draw(app, b, r, now, hits),
        None => return,
    };
    let words = frame(b, r, title);
    let body = Rect {
        height: words.height.saturating_sub(1),
        ..words
    };
    scroll(app, b, body, &rows);
    let keys = super::bindings::text(app, "<panel.up>/<panel.down> scroll · <panel.cancel> back");
    put(
        b,
        words.x,
        body.bottom(),
        &keys,
        words.width,
        style(GRAY, GLASS),
    );
}

/// Fills `r` and draws its rule box with `title` in the top edge; returns the area inside for
/// the words, two columns in from each side.
pub fn frame(b: &mut Buffer, r: Rect, title: &str) -> Rect {
    frame_colour(b, r, title, BEZEL_HI)
}

/// Draws the same panel with the requested border colour.
pub fn frame_colour(b: &mut Buffer, r: Rect, title: &str, colour: ratatui::style::Color) -> Rect {
    fill(b, r, style(DATA, GLASS));
    let (edge, rule) = (style(colour, GLASS), "─".repeat(r.width as usize - 2));
    put(b, r.x, r.y, &format!("┌{rule}┐"), r.width, edge);
    let title = format!(" {title} ");
    put(b, r.x + 2, r.y, &title, r.width - 4, style(WHITE, GLASS));
    for y in r.y + 1..r.bottom() - 1 {
        put(b, r.x, y, "│", 1, edge);
        put(b, r.right() - 1, y, "│", 1, edge);
    }
    put(b, r.x, r.bottom() - 1, &format!("└{rule}┘"), r.width, edge);
    Rect::new(r.x + 2, r.y + 1, r.width - 4, r.height - 2)
}

/// Draws `rows` into `r` from the panel's scroll, first clamped so the last row reaches the
/// bottom and no further.
pub fn scroll(app: &mut App, b: &mut Buffer, r: Rect, rows: &[Line<'_>]) {
    let last = rows.len().saturating_sub(r.height as usize);
    app.panel_scroll = app.panel_scroll.min(last);
    for (words, y) in rows.iter().skip(app.panel_scroll).zip(r.y..r.bottom()) {
        line(b, r, y, words, style(DATA, GLASS));
    }
}

/// `/tasks`: the addressed session's project's open tasks, as the server lists them, in a panel
/// opened when they arrive.
fn tasks(app: &mut App) {
    let Some(id) = app.focus.clone().or_else(|| app.root.clone()) else {
        return;
    };
    app.spawn(move |api| async move {
        let path = format!("v1/sessions/{id}/tasks?filter=open");
        let read = api.json(reqwest::Method::GET, &path, None).await;
        let row = |t: &serde_json::Value| {
            let [state, title, claim] = ["state", "title", "claim"].map(|k| t[k].as_str());
            let due = t["due"].as_str().map(|d| format!("due {d}"));
            let words = [state, title, claim, due.as_deref()].into_iter().flatten();
            let line = format!("{}  {}", t["id"], words.collect::<Vec<_>>().join("  "));
            Line::raw(screen_safe(&line).into_owned())
        };
        let rows = match read.map(|v| v["tasks"].as_array().cloned().unwrap_or_default()) {
            Ok(tasks) => tasks.iter().map(row).collect(),
            Err(e) => vec![Line::raw(e.to_string())],
        };
        move |app: &mut App| (app.panel, app.panel_scroll) = (Some(Panel::Tasks(rows)), 0)
    });
}
