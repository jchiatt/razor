//! The slash commands typed into the composer: each acts at Enter and, once it has, the draft
//! is cleared; a command its owner reads (`agents`, `panel`, `mcp`, `clipboard`) goes there.
use super::keys::focus;
use super::{App, Composer, agents, audio, clipboard, composer, inputs, mcp, output, panel};
use super::{picker, view::Action};
use crate::protocol::{Answer, EntryId, SessionId, State};
use composer::{completion, emoji};
use reqwest::Method;
use serde_json::json;

/// Runs the command the draft holds.
pub fn run(app: &mut App) {
    let text = app.composer.text.clone();
    let (word, args) = text.split_once(' ').unwrap_or((&text, ""));
    completion::ran(app, word);
    let args = args.trim();
    if agents::command(app, word, args) || panel::command(app, word) {
        clear(app);
        return;
    }
    match word {
        "/name" if args.is_empty() => app.say("Use /name <new name> to rename this session."),
        "/name" => rename(app, args.to_owned()),
        "/fast" | "/ultrafast" => {
            picker::fast(app, word, args);
            clear(app);
        }
        "/record" => audio::record(clear(app)),
        "/listen" => audio::listen(clear(app)),
        "/model" | "/effort" if args.is_empty() => {
            clear(app);
            picker::open(app, false);
        }
        // A last word that is an effort level is the effort: `/model opus high`.
        "/model" => {
            let effort = args
                .rsplit_once(' ')
                .map(|(m, e)| (m, serde_json::from_value(json!(e))));
            let (model, level) = match effort {
                Some((model, Ok(level))) => (model.trim(), Some(level)),
                _ => (args, None),
            };
            picker::stage(app, Some(model.into()), level, None, None);
            clear(app);
        }
        "/effort" => match serde_json::from_value(json!(args)) {
            Ok(level) => {
                picker::stage(app, None, Some(level), None, None);
                clear(app);
            }
            Err(_) => app.say("That effort is not known; choose low, medium, high, xhigh or max."),
        },
        "/resume" => {
            let shown = |id| app.root.as_ref() == Some(id) || app.focus.as_ref() == Some(id);
            let others = (app.agents.values()).any(|a| a.parent.is_none() && !shown(&a.session));
            clear(app);
            match others {
                true if app.list.open => {}
                true => super::agents::toggle(app),
                false => app.say("There is no other session to open."),
            }
        }
        "/skill" => app.say("That skill is not in this session's list; /help lists commands."),
        "/detach" => {
            clear(app);
            app.leaving = true;
        }
        "/pin" => {
            app.transcript_mut().into_iter().for_each(|t| t.pin(None));
            clear(app);
        }
        "/unattach" => {
            app.composer = Composer::default();
            focus(app, None);
        }
        "/attach" | "/to" => {
            let found = app
                .agents
                .iter()
                .find(|(id, a)| id.as_str() == args || a.name == args);
            match found.map(|(id, _)| id.clone()) {
                Some(id) => {
                    app.composer = Composer::default();
                    focus(app, Some(id));
                }
                None => app.say("No such agent is in the status view; use its session id or name."),
            }
        }
        "/answer" if args.is_empty() => app.say("/answer <words>"),
        "/answer" => inputs::answer(app, Answer::Text(args.into())),
        "/copy" => clipboard::command(app, args),
        "/mcp" => {
            if mcp::command(app, args) {
                clear(app);
            }
        }
        "/done" => done(app, "done"),
        "/continue" if args == "all" => {
            clear(app);
            inputs::continue_all(app);
        }
        "/continue" => app.say("/continue all"),
        "/end" | "/quit" | "/quit!" => done(app, "quit"),
        "/emoji" => {
            clear(app);
            emoji::open(app, None);
        }
        "/gauge" => {
            clear(app);
            app.gauge_folded = match args {
                "fold" => true,
                "open" => false,
                _ => !app.gauge_folded,
            };
        }
        "/handover" => handover(app),
        "/startup" => {
            clear(app);
            app.transcript_mut()
                .into_iter()
                .for_each(|t| t.toggle(output::STARTUP.0));
        }
        _ => app.say("Unknown command; /help lists commands, and no words were sent."),
    }
}

fn clear(app: &mut App) -> &mut App {
    app.composer = Composer::default();
    app.edited();
    app
}

// `/name`: the addressed session takes `name` now.
fn rename(app: &mut App, name: String) {
    let Some(id) = app.focus.clone() else {
        return app.say("No session has started yet, so there is nothing to rename.");
    };
    if app.write(|api| async move {
        let renamed = api.rename(&id, name).await;
        move |app: &mut App| renamed.err().iter().for_each(|e| app.say(&e.to_string()))
    }) {
        clear(app);
    }
}

// `/done` and `/quit` (`route`): the addressed session ends, after a last turn that runs on in
// the server or at once. Once the server takes it, unless the person has moved on, the draft
// clears; once its row reads it closed, `ended` leaves the app or goes back to the root.
fn done(app: &mut App, route: &'static str) {
    let Some(id) = app.focus.clone() else {
        return app.say("No session has started yet, so there is nothing to close out.");
    };
    let working = app.row().is_some_and(|a| a.state == State::Working);
    app.write(|api| async move {
        let done = api
            .json(
                Method::POST,
                &format!("v1/sessions/{id}/{route}"),
                Some(json!({})),
            )
            .await;
        move |app: &mut App| match done {
            Ok(v) => {
                if let Some(entry) = v["entry"].as_u64().filter(|_| working) {
                    inputs::queue(app, &id, EntryId(entry));
                }
                if app.focus.as_ref() == Some(&id) {
                    app.composer = Composer::default();
                }
                app.closing = Some(id);
                ended(app);
            }
            Err(e) => app.say(&e.to_string()),
        }
    });
}

/// Once the session `/done` or `/quit` closed reads closed, the agents list addresses its next
/// open row (Trey, 2026-11-02); otherwise the app leaves, or a split goes back to the root.
pub fn ended(app: &mut App) {
    let closed = |id: &SessionId| app.agents.get(id).is_some_and(|a| a.state == State::Done);
    let Some(id) = app.closing.take_if(|id| closed(id)) else {
        return;
    };
    let rows = &app.list.rows;
    let at = (rows.iter().position(|a| a == &Action::Agent(id.clone()))).unwrap_or(0);
    let next = (rows[at..].iter().chain(rows[..at].iter().rev())).find_map(|a| match a {
        Action::Agent(n) if app.list.open && !closed(n) => Some(n.clone()),
        _ => None,
    });
    match next {
        Some(next) => {
            app.list.selected = Some(Action::Agent(next.clone()));
            focus(app, Some(next.clone()));
            if app.focus.as_ref() == Some(&next) && app.root.as_ref() == Some(&id) {
                app.streams.remove(&id).iter().for_each(|task| task.abort());
                app.root = Some(next);
            }
        }
        None if app.root.as_ref() == Some(&id) => app.leaving = true,
        None => focus(app, app.root.clone()),
    }
}

// `/handover`: the addressed session writes its continuation note and rebuilds, as at its
// threshold; the transcript shows both.
fn handover(app: &mut App) {
    let Some(id) = app.focus.clone() else {
        return app.say("No session has started yet, so there is nothing to hand over.");
    };
    let working = app.row().is_some_and(|a| a.state == State::Working);
    clear(app);
    app.spawn(|api| async move {
        let asked = api
            .json(Method::POST, &format!("v1/sessions/{id}/handover"), None)
            .await;
        move |app: &mut App| match asked {
            Ok(v) => match v["entry"].as_u64().map(EntryId) {
                Some(entry) if working => inputs::queue(app, &id, entry),
                entry => app.say(match entry {
                    Some(_) => {
                        "Handing over: the agent writes its note, then its context rebuilds."
                    }
                    None => "A handover or rebuild is already under way; nothing more was asked.",
                }),
            },
            Err(e) => app.say(&e.to_string()),
        }
    });
}
