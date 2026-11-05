//! Keys, pastes and clicks act on the app directly; their API work is spawned beside the loop
//! or queued behind earlier writes, and its answer is applied between draws.
use super::inputs::{admit_answer, answer, history, send, take_back};
use super::transcript::{Direction, select};
use super::{App, Composer, activity, bindings, clipboard, composer, data, mcp, panel, picker};
use super::{Transcript, recap, view::Action};
use crate::protocol::{Answer, Body, EntryId, SessionId, now};
use composer::{completion, emoji, shortcodes};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers as Mods};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use reqwest::Method;

/// One terminal event; true for a second Ctrl-C in a row, which quits without stopping a session
/// or its jobs. An open picker, panel or new-agent chooser takes every key and drops pastes, as
/// the emoji grid drops them, and only a panel's or the chooser's own rows take clicks.
pub fn event(app: &mut App, event: Event) -> bool {
    if let Event::Key(k) = &event
        && app.program.answer(k)
    {
        app.dirty |= app.program.answered;
        return false;
    }
    (app.dirty, app.immediate) = (true, true);
    let modal = app.picker.is_some() || app.panel.is_some() || app.start.is_some();
    match event {
        Event::Key(k) if k.kind != KeyEventKind::Release => {
            activity::report(app);
            let Some((k, context, action)) = bindings::translate(app, k) else {
                return false;
            };
            // Ctrl-C outside the emoji grid, which takes it to close, arms quitting.
            let quits = k.code == KeyCode::Char('c') && k.modifiers == Mods::CONTROL;
            let quits = quits && app.grid.is_none();
            if app.quitting && quits {
                return true;
            }
            // The stage hears each key, and a long draft any key but Enter empties.
            let long = app.composer.text.chars().nth(39).is_some();
            key(app, k, context, action, quits);
            let cleared = long && app.composer.text.is_empty();
            app.stage.key(now(), cleared && k.code != KeyCode::Enter);
            return false;
        }
        Event::FocusLost => {
            recap::leave(app);
            app.presence.report(false, now());
            return false;
        }
        Event::FocusGained if !app.presence.focused => {
            app.presence.report(true, now());
            recap::back(app);
            recap::check(app);
        }
        Event::Paste(text) => {
            activity::report(app);
            app.quitting = false;
            if !modal && app.grid.is_none() {
                composer::paste(app, text);
            }
        }
        Event::Mouse(m) => {
            activity::report(app);
            if app.panel.is_some() {
                mcp::click(app, m);
            } else if app.start.is_some() {
                let at = (m.column, m.row).into();
                let hit = app.hits.iter().rev().find(|(r, _)| r.contains(at));
                if let (Some((_, a @ Action::Choose(..))), MouseEventKind::Down(_)) = (hit, m.kind)
                {
                    act(app, a.clone())
                }
            } else if !modal {
                mouse(app, m);
            }
        }
        _ => {}
    }
    // Focus, a click, a scroll or a paste wakes a resting room as a key does.
    app.stage.key(now(), false);
    false
}

/// Applies one resolved key with the binding table's explicit context and action.
fn key(app: &mut App, k: KeyEvent, context: &str, action: &str, quits: bool) {
    // Ctrl-C arms quitting; while armed, Esc only disarms and any other key also does its own work.
    let armed = std::mem::replace(&mut app.quitting, quits);
    if app.quitting || armed && k.code == KeyCode::Esc {
        return;
    }
    if (context == "voice" || action == "global.record") && super::audio::key(app, k) {
        return;
    }
    (app.selection, app.recap) = (None, None);
    app.transcript_mut()
        .into_iter()
        .for_each(|t| t.ended = None);
    if app.panel.is_some() {
        return if matches!(context, "panel" | "mcp") {
            panel::key(app, k)
        };
    }
    // PageUp and PageDown still scroll the transcript behind the picker.
    if app.picker.is_some() && !matches!(k.code, KeyCode::PageUp | KeyCode::PageDown) {
        return if context == "picker" {
            picker::key(app, k)
        };
    }
    if k.code == KeyCode::Char('s') && k.modifiers == Mods::CONTROL {
        return super::start::open(app);
    }
    if app.start.is_some() {
        return super::start::key(app, k);
    }
    if context == "list" {
        return super::agents::key(app, k);
    }
    if app.grid.is_some() {
        return emoji::key(app, k);
    }
    if k.code == KeyCode::Char('g') && k.modifiers == Mods::CONTROL {
        return emoji::open(app, shortcodes::typed(app));
    }
    // A question closed while its answer was being written: every key says so and the words stay
    // a draft. Only the Enter or Esc meant for the answer ends it, doing nothing else, so words
    // typed after the close never leave as an input.
    let open = |id: &super::MailId| app.asks().iter().any(|a| &a.id == id);
    if app.sending.is_none() && app.answering.as_ref().is_some_and(|id| !open(id)) {
        app.say("That question closed; your words stay in the draft.");
        if matches!(k.code, KeyCode::Enter | KeyCode::Esc) {
            app.answering = None;
            return;
        }
    }
    if matches!(action, "global.copy" | "global.cut") {
        return copy(app, action == "global.cut");
    }
    if context != "attachments" {
        app.composer.chosen = None;
    }
    match context {
        "attachments" => attachments(app, k),
        "completion" if completion::key(app, k) => {}
        "shortcode" if shortcodes::key(app, k) => {}
        "completion" | "shortcode" => {
            if k.code == KeyCode::Enter {
                enter(app)
            } else if app.composer.key(k) {
                app.edited();
            }
        }
        "global" | "transcript" | "skill" | "children" => browse(app, k),
        _ => edit(app, k, context),
    }
}

fn copy(app: &mut App, cut: bool) {
    if app.copying {
        return app.say("A clipboard write is pending; wait before copying or cutting again.");
    }
    // With an empty draft, a copy takes the shown question.
    if !cut && app.composer.text.is_empty() && app.shown_ask.is_some() {
        return clipboard::question(app);
    }
    // A cut's words leave only when its copy can start.
    if cut && app.pending >= 8 {
        return app.say("Eight API operations are pending; wait for an answer before cutting.");
    }
    let Some(text) = app.composer.copy_selection(cut) else {
        return app.say("Nothing is selected to cut.");
    };
    if cut {
        app.edited();
    }
    clipboard::draft(app, text);
}

fn browse(app: &mut App, k: KeyEvent) {
    let ctrl = k.modifiers.contains(Mods::CONTROL);
    match k.code {
        KeyCode::Esc => escape(app),
        KeyCode::Char('x') if ctrl => stop(app),
        KeyCode::Char(c @ ('n' | 'p')) if ctrl && app.composer.text.is_empty() => {
            children(app, c == 'n')
        }
        KeyCode::PageUp | KeyCode::PageDown => {
            if let Some(t) = app.transcript_mut() {
                let page = t.detail_height.saturating_sub(2).max(3) as usize;
                match k.code == KeyCode::PageUp {
                    true => t.page_up(page),
                    false => t.page_down(page),
                }
            }
        }
        KeyCode::Char('t') if ctrl => act(app, Action::Status),
        // Alt and a digit opens that lamp's skill card, never typing the digit.
        KeyCode::Char(c @ '1'..='9') if k.modifiers == Mods::ALT => {
            let slot = usize::from(c as u8 - b'1');
            let t = app.transcript_mut();
            match t
                .as_ref()
                .and_then(|t| super::tools::lamps::slot_entry(t, slot))
            {
                Some(id) => t.into_iter().for_each(|t| t.toggle(id)),
                None => app.say(&format!("No skill is lit in slot {c}.")),
            }
        }
        KeyCode::Char('o') if ctrl => {
            if let Some(said) = app.transcript_mut().map(|t| t.open_all()) {
                app.say(said);
            }
        }
        KeyCode::Home if ctrl => {
            if let Some(id) = app.focus.clone() {
                super::transcript::jump(app, &id, EntryId(1));
            }
        }
        KeyCode::End if ctrl => {
            if let Some(t) = app.transcript_mut() {
                match t.opened {
                    Some(_) => t.detail_scroll = usize::MAX,
                    None => t.bottom(),
                }
            }
        }
        _ => {}
    }
}

// The transcript the wheel turns: a split's main half when this frame drew it under the pointer,
// else the addressed one, its opened card included.
fn wheeled(app: &mut App, at: ratatui::layout::Position) -> Option<&mut super::Transcript> {
    let root = app.root.clone().filter(|_| app.split());
    match root.filter(|id| app.transcripts.get(id).is_some_and(|t| t.holds(at))) {
        Some(id) => app.transcripts.get_mut(&id),
        None => app.transcript_mut(),
    }
}

// Esc backs out one layer: the selected child card, the opened card, the answer, the split view, a
// floating status pane, the pinned reply, and only then stops the session (Trey, 2026-10-30).
fn escape(app: &mut App) {
    if app.selected.take().is_some() {
        app.revision = app.revision.wrapping_add(1);
        return;
    }
    let opened = app
        .transcript_mut()
        .is_some_and(|t| t.opened.take().is_some());
    let left = !opened && app.answering.take().is_some();
    if left && !app.composer.text.trim().is_empty() {
        app.say("Left the answer; its words stay in the draft.");
    }
    if opened || left {
        return;
    }
    if app.split_view() {
        return focus(app, None);
    }
    if app.pane_open == Some(true) && !super::view::docked(app) {
        app.pane_open = Some(false);
        return;
    }
    match app.transcript_mut().filter(|t| t.pin.is_some()) {
        Some(t) => t.pin = None,
        // A new agent's composer goes back to the session addressed before it.
        None => match app.back.take().filter(|_| app.focus.is_none()) {
            Some(id) => super::agents::press(app, Action::Agent(id)),
            None => stop(app),
        },
    }
}

// Ctrl-N and Ctrl-P: in a split, the next or previous sibling's split; on a phone, the next
// child's split at once; else the next child card, the pane opened for it.
fn children(app: &mut App, forward: bool) {
    let split = app.split();
    let parent = match split {
        true => app.row().and_then(|a| a.parent.clone()),
        false => app.focus.clone(),
    };
    let mut kin: Vec<_> = (app.agents.values())
        .filter(|a| parent.is_some() && a.parent == parent)
        .map(|a| (a.launched, a.session.clone()))
        .collect();
    kin.sort();
    let n = kin.len();
    if n < 1 + usize::from(split) {
        return app.say(["No children out.", "No other child at this level."][usize::from(split)]);
    }
    let here = if split {
        app.focus.clone()
    } else {
        app.selected_child().cloned()
    };
    let at = kin.iter().position(|(_, id)| Some(id) == here.as_ref());
    let next = kin[step(at, n, forward)].1.clone();
    if split || app.columns < super::view::PHONE {
        return focus(app, Some(next));
    }
    // The selection is the next child of the list, whether or not its card is drawn yet.
    (app.pane_open, app.selected) = (Some(true), Some(Action::Child(next)));
    app.revision = app.revision.wrapping_add(1);
}

// The place after `at` round a ring of `n`, or before it; with none, the first or the last.
fn step(at: Option<usize>, n: usize, forward: bool) -> usize {
    match forward {
        true => at.map_or(0, |i| (i + 1) % n),
        false => at.map_or(n - 1, |i| (i + n - 1) % n),
    }
}

// The browse ring: every drawn click target in the ring's order; Enter on one does what a
// click does.
fn select(app: &mut App, forward: bool) {
    let ring = app.targets.clone();
    let n = ring.len();
    if n == 0 {
        return app.say("nothing to open yet");
    }
    let at = (app.selected.as_ref()).and_then(|a| ring.iter().position(|t| t == a));
    app.selected = Some(ring[step(at, n, forward)].clone());
    app.revision = app.revision.wrapping_add(1);
}

fn edit(app: &mut App, k: KeyEvent, context: &str) {
    app.list.keys = false;
    match context {
        // A digit past the ask's options is the draft's.
        "question" => match k.code {
            KeyCode::Char(n @ '1'..='9') => {
                let folded = app.hits.iter().find(|(_, a)| matches!(a, Action::Ask(_)));
                if let Some((_, open)) = folded.cloned() {
                    return act(app, open);
                }
                let option = n as usize - '1' as usize;
                match app.visible_options.contains(&option) {
                    _ if fresh(app) => {}
                    true => answer(app, Answer::Option(option)),
                    false => app.say(CUT),
                }
            }
            KeyCode::Left | KeyCode::Right => {
                let ids: Vec<_> = app.asks().iter().map(|a| a.id.clone()).collect();
                let at = ids.iter().position(|id| Some(id) == app.ask.as_ref());
                let step = match k.code {
                    KeyCode::Left => ids.len().saturating_sub(1),
                    _ => 1,
                };
                app.ask = ids
                    .get((at.unwrap_or(0) + step) % ids.len().max(1))
                    .cloned();
            }
            _ => {}
        },
        "line_start" if app.list.open => app.list.keys = true,
        "line_start" => super::agents::toggle(app),
        "targets" => select(app, matches!(k.code, KeyCode::Tab | KeyCode::Down)),
        "input_history" => history(app, k.code == KeyCode::Up),
        "queued_input" => {
            let t = app.transcript();
            let newest = t.and_then(|t| t.queued.difference(&t.taking).last().copied());
            newest.into_iter().for_each(|input| take_back(app, input));
        }
        "attached_images" => app.composer.chosen = Some(app.composer.active_images().count() - 1),
        "composer" if k.code == KeyCode::Enter => enter(app),
        "composer" => escape(app),
        "editor" | "text" => {
            let chip = match k.code {
                KeyCode::Backspace => app.composer.image_by_cursor(false),
                KeyCode::Delete => app.composer.image_by_cursor(true),
                _ => None,
            };
            if context == "editor"
                && k.modifiers.is_empty()
                && let Some(number) = chip
            {
                return remove_image(app, number);
            }
            if app.composer.key(k) {
                if app.selected.take().is_some() {
                    app.revision = app.revision.wrapping_add(1);
                }
                app.edited();
            }
        }
        _ => {}
    }
}

fn enter(app: &mut App) {
    // A trailing backslash is a new line: the phone's, which has no Shift-Enter.
    let end = app.composer.text.len();
    if app.composer.cursor() == end && app.composer.text.ends_with('\\') {
        app.composer.swap(end - 1..end, "\n");
        return app.edited();
    }
    if app.sending.is_some() {
        return app.say(&super::bindings::text(
            app,
            "The earlier input is still being admitted; press <composer.send> once it answers.",
        ));
    }
    let empty = app.composer.text.is_empty();
    if let (true, Some(target)) = (empty, app.selected.take()) {
        return act(app, target);
    }
    if app.answering.is_some() && !app.composer.text.trim().is_empty() {
        return answer(app, Answer::Text(app.composer.text.clone()));
    }
    if let (true, Some(ask)) = (empty, app.shown_ask.as_ref().filter(|_| !app.split_view())) {
        app.answering = Some(ask.id.clone());
    } else if app.composer.text.starts_with('/')
        && completion::skill(app, &app.composer.text).is_none()
    {
        super::commands::run(app);
    } else {
        let text = data::expand(&app.composer.text, &app.emoji);
        app.composer.replace(text);
        send(app);
    }
}

fn mouse(app: &mut App, m: MouseEvent) {
    let at = (m.column, m.row).into();
    if let MouseEventKind::ScrollUp | MouseEventKind::ScrollDown = m.kind {
        let down = m.kind == MouseEventKind::ScrollDown;
        let step = if down { 3 } else { -3 };
        app.selection = None;
        if app.list.area.is_some_and(|r| r.contains(at)) {
            app.list.scroll = app.list.scroll.saturating_add_signed(step);
        } else if app.status_area.is_some_and(|r| r.contains(at)) {
            app.status_scroll = app.status_scroll.saturating_add_signed(step);
        } else if let Some(t) = wheeled(app, at) {
            // One row per event: a terminal sends several for each notch of the wheel.
            match down {
                true => t.page_down(1),
                false => t.page_up(1),
            }
        }
        return;
    }
    let cell = (m.column, m.row);
    // A held list edge follows the mouse until it lets go, which keeps the width.
    if app.list.dragging {
        let left = app.list.area.map_or(0, |a| a.x);
        let columns = (m.column + 1).saturating_sub(left);
        match m.kind {
            MouseEventKind::Drag(MouseButton::Left) => super::agents::resize(app, columns, false),
            MouseEventKind::Up(MouseButton::Left) => super::agents::resize(app, columns, true),
            _ => {}
        }
        return;
    }
    if draft(app, m, cell) {
        app.list.keys = false;
        return;
    }
    match m.kind {
        MouseEventKind::Drag(MouseButton::Left) => return select::drag(app, cell),
        MouseEventKind::Up(MouseButton::Left) => {
            return select::release(app, cell, now())
                .into_iter()
                .for_each(|a| act(app, a));
        }
        _ => {}
    }
    let hit = app.hits.iter().rev().find(|(area, _)| area.contains(at));
    let grid = matches!(hit, Some((_, Action::Grid(_) | Action::Emoji)));
    if let (Some(_), MouseEventKind::Down(_), false) = (&app.grid, m.kind, grid) {
        app.grid = None;
    }
    if let (Some((_, Action::Grid(h))), MouseEventKind::Down(MouseButton::Right)) = (hit, m.kind) {
        return emoji::click(app, *h, true);
    }
    if m.kind != MouseEventKind::Down(MouseButton::Left) {
        return;
    }
    let action = hit.map(|(_, action)| action.clone());
    if !select::press(app, cell, action.clone()) {
        action.into_iter().for_each(|a| act(app, a));
    }
}

// The mouse in the draft, as the cockpit's: a press puts the cursor there, a drag selects, Shift
// extends, a double click takes the word, and a selection the mouse made is copied on release.
// True when the draft took the event.
fn draft(app: &mut App, m: MouseEvent, cell: (u16, u16)) -> bool {
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let at = now();
            let double = matches!(app.clicks, (last, c, 1) if c == cell && at < last + 500);
            // Anything drawn over the words with a click of its own, such as the emoji grid, keeps it.
            let covered = app.hits.iter().any(|(r, _)| r.contains(cell.into()));
            let shift = m.modifiers.contains(Mods::SHIFT);
            if covered || !app.composer.press(cell, shift, double) {
                return false;
            }
            // A third press on the cell starts over rather than counting as another double; a
            // press in the words lets go of a chosen image chip.
            (app.clicks, app.selection) = ((at, cell, u8::from(!double)), None);
            app.composer.chosen = None;
        }
        MouseEventKind::Drag(MouseButton::Left) => return app.composer.drag(cell),
        MouseEventKind::Up(MouseButton::Left) => match app.composer.release() {
            Some(Some(words)) => clipboard::draft(app, words),
            Some(None) => {}
            None => return false,
        },
        _ => return false,
    }
    true
}

// What a click on a hit does.
fn act(app: &mut App, action: Action) {
    match action {
        Action::Record => super::audio::record(app),
        Action::Edge => app.list.dragging = true,
        Action::Play(session, entry) => super::audio::play(app, session, entry),
        Action::Child(id) => focus(app, Some(id)),
        Action::Option(_, _) if fresh(app) => {}
        Action::Option(id, n) => admit_answer(app, id, Answer::Option(n)),
        Action::Cut => app.say(CUT),
        Action::Call(session, id) => held(app, &session, |t| t.toggle(id)),
        Action::Chip(at) => app.composer.chosen = Some(at),
        Action::Unfold(session, id) => held(app, &session, |t| t.expand([id], true)),
        Action::Back => focus(app, None),
        Action::Archive => app.archive = !app.archive,
        a @ (Action::Section(_) | Action::Agent(_) | Action::New | Action::Thread(_)) => {
            super::agents::press(app, a)
        }
        Action::Choose(server, cwd) => super::start::choose(app, Some((server, cwd))),
        Action::Inbox(offset) => {
            (app.inbox, app.pane_open, app.to_inbox) = (offset, Some(true), true)
        }
        Action::Delivery(session, entry, _) => delivery(app, session, entry),
        Action::Strip => app.strip = !app.strip,
        Action::Complete(n) => completion::click(app, n),
        Action::Emoji => emoji::open(app, shortcodes::typed(app)),
        Action::Grid(h) => emoji::click(app, h, false),
        Action::Code(n) => shortcodes::click(app, n),
        Action::Model => picker::open(app, false),
        Action::Image => app.say("Paste or drop an image's file path to attach it."),
        Action::Send if app.composer.text.trim().is_empty() => app.say("Nothing to send yet."),
        Action::Send => enter(app),
        Action::HandBack(session, name) => super::inputs::hand_back(app, session, name),
        Action::Status => {
            // Beside the agents list the pane is closed until asked for.
            let wide = app.columns >= 140 && app.list.area.is_none();
            app.pane_open = Some(!app.pane_open.unwrap_or(wide));
        }
        Action::Copy(text) => clipboard::copy(app, Some(text)),
        Action::CopyText(session, entry) => {
            let entry = app
                .transcripts
                .get(&session)
                .and_then(|t| t.entries.get(&entry));
            match entry.map(|e| &e.body) {
                Some(Body::Text { text } | Body::Thinking { text }) => {
                    clipboard::copy(app, Some(text.clone()))
                }
                _ => app.say(
                    "That reply is no longer in the transcript buffer; use recall to copy it.",
                ),
            }
        }
        Action::RemoveImage(number) => remove_image(app, number),
        Action::TakeBack(input) => take_back(app, input),
        Action::Quote => select::quote(app),
        Action::Jump(session, id) => super::transcript::jump(app, &session, id),
        Action::Latest(session) => held(app, &session, Transcript::bottom),
        Action::Pin(session, id) => held(app, &session, |t| t.pin(Some(id))),
        // Only an open `/mcp` draws these, and it takes its own clicks.
        Action::Mcp(_) => {}
        Action::Link(url) => clipboard::copy(app, Some(url.to_string())),
        Action::Ask(id) => {
            let n = app.shown_ask.as_ref().map_or(0, |a| a.options.len());
            (app.ask_open, app.ask_at) = (Some(id), now());
            app.say(&format!("Press 1-{n} to answer."));
        }
    }
}

// A delivery opens as a link does, its address, its path or a note's line copied; the copy
// starting marks it read.
fn delivery(app: &mut App, session: SessionId, entry: EntryId) {
    if app.copying {
        return app.say("A clipboard write is pending; wait before opening another delivery.");
    }
    let Some(delivery) = app
        .delivered
        .iter()
        .find(|d| (&d.session, d.entry) == (&session, entry))
    else {
        return;
    };
    let Some(target) = delivery
        .url
        .clone()
        .or_else(|| delivery.path.as_ref().map(|p| p.display().to_string()))
        // A note, with neither file nor url, opens to its line as a link does to its address.
        .or_else(|| delivery.file.is_none().then(|| delivery.line.clone()))
    else {
        return app.say("The delivery has no opening path; reconnect to its owning server.");
    };
    let read = delivery.read;
    clipboard::copy(app, Some(target));
    if !read && app.copying {
        let path = format!("v1/sessions/{session}/deliveries/{}/read", entry.0);
        app.write(|api| async move {
            let result = api.json(Method::PUT, &path, None).await;
            move |app: &mut App| result.err().iter().for_each(|e| app.say(&e.to_string()))
        });
    }
}

// Acts on `session`'s transcript while the app holds it.
fn held(app: &mut App, session: &SessionId, act: impl FnOnce(&mut Transcript)) {
    if let Some(t) = app.transcripts.get_mut(session) {
        act(t);
    }
}

// A question that just took the band is read before a digit or click can answer it.
const CUT: &str = "That option is not shown in full; answer its words with /answer instead.";

fn fresh(app: &mut App) -> bool {
    let fresh = now() < app.ask_at.saturating_add(600);
    if fresh {
        app.say("New question: read it first.");
    }
    fresh
}

// With an image chip chosen: Left and Right move along the chips, Backspace or Delete removes it
// and chooses its neighbour, Esc or Down lets go, and any other key lets go and is typed. True
// when the tray took the key.
fn attachments(app: &mut App, k: KeyEvent) {
    let n = app.composer.active_images().count();
    let Some(at) = app.composer.chosen.take().filter(|&at| at < n) else {
        return;
    };
    let chosen = match k.code {
        KeyCode::Left => Some((at + n - 1) % n),
        KeyCode::Right => Some((at + 1) % n),
        KeyCode::Backspace | KeyCode::Delete => {
            let number = app.composer.active_images().nth(at).map(|im| im.number);
            number
                .into_iter()
                .for_each(|number| remove_image(app, number));
            (n > 1).then(|| at.min(n - 2))
        }
        KeyCode::Esc | KeyCode::Down => None,
        _ => return,
    };
    app.composer.chosen = chosen;
}

fn remove_image(app: &mut App, number: usize) {
    if app.sending.is_none() && app.composer.remove_image(number) {
        app.edits += 1;
        app.save();
    }
}

fn stop(app: &mut App) {
    if let Some(id) = app.focus.clone() {
        app.spawn(|api| async move {
            let stopped = api
                .json(Method::POST, &format!("v1/sessions/{id}/stop"), None)
                .await;
            move |app: &mut App| stopped.err().iter().for_each(|e| app.say(&e.to_string()))
        });
    }
}

/// Addresses `next`, or the addressed child's parent; the old draft is saved first.
pub(super) fn focus(app: &mut App, next: Option<SessionId>) {
    if next.is_some() && next == app.focus || next.is_none() && !app.split() {
        return;
    }
    if app.sending.is_some() || super::audio::unsent(app) {
        return app.say("A note or input is pending; wait before changing the addressed session.");
    }
    app.save();
    recap::leave(app);
    let old = app.focus.clone();
    if let (true, Some(old)) = (app.split(), app.focus.clone()) {
        app.streams
            .remove(&old)
            .iter()
            .for_each(|task| task.abort());
        // The child's map goes with it; one left paged into older history becomes a
        // newest-entry card again.
        if let Some(t) = app.transcripts.get_mut(&old) {
            t.marks = None;
            if t.retain_older {
                t.prepare_page(Direction::Newest);
                app.previews.remove(&old);
            }
        }
    }
    app.focus = match next {
        Some(id) => {
            app.root.get_or_insert(id.clone());
            app.status_scroll = 0;
            Some(id)
        }
        None => {
            let parent = app
                .focus
                .as_ref()
                .and_then(|id| app.agents.get(id)?.parent.clone());
            parent.or_else(|| app.root.clone())
        }
    };
    (app.selected, app.panel, app.history, app.initial) = (None, None, None, None);
    app.revision = app.revision.wrapping_add(1);
    app.composer = Composer::default();
    super::split::start(app, old, now());
    recap::back(app);
    if let Some(id) = app.focus.clone() {
        app.open(id);
    }
}
