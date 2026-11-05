//! What the person admits to the API: inputs and answers, each saved as the draft first and
//! posted behind every earlier write, and earlier inputs recalled into the editor.
use super::api::Api;
use super::{App, Composer, Error, clipboard, composer, error, fail};
use crate::protocol::{Answer, Body, ClientInput, EntryId, Event as ApiEvent, MailId};
use crate::protocol::{SessionId, State, now};
use composer::completion;
use reqwest::Method;
use serde_json::{Value, json};
use std::time::Duration;

/// Admits the composer's words and images to the addressed session, or makes the session first.
pub fn send(app: &mut App) {
    if app.sending.is_some() || app.composer.is_empty() {
        return;
    }
    if let Err(e) = app.composer.validate_images() {
        return app.say(&e.to_string());
    }
    if app.loading_images > 0 {
        return app.say("An image is still loading; wait for its attachment before sending.");
    }
    let initial = app.initial.as_ref();
    let (model, effort) = (
        initial.and_then(|i| i.model.clone()),
        initial.and_then(|i| i.effort),
    );
    let Some(id) = app.focus.clone() else {
        return create(app);
    };
    let (text, images) = (app.composer.text.clone(), app.composer.images_for_send());
    let (skill, text) = match completion::skill(app, &text) {
        Some((name, words)) => (Some(name), words),
        None => (None, text),
    };
    let from = initial.and_then(|i| i.from.clone());
    let input = ClientInput {
        text,
        images,
        model,
        effort,
        speed: initial.and_then(|i| i.speed),
        handover: initial.and_then(|i| i.handover),
        from,
        skill,
        ..Default::default()
    };
    let body = serde_json::to_value(input).unwrap_or_default();
    // Queued only behind a turn already running: an input that starts its own is taken as it is
    // admitted, possibly before this session's stream attaches to hear it.
    let working = app
        .agents
        .get(&id)
        .is_some_and(|a| a.state == State::Working);
    admit(
        app,
        format!("v1/sessions/{id}/input"),
        (body, false),
        move |app, admitted| {
            let missing = "The server's admission answer had no entry id; check the transcript before sending again.";
            let Some(entry) = admitted["entry"].as_u64().map(EntryId) else {
                let signed_in = admitted["signed_in"] == true;
                return app.say(if signed_in {
                    "MCP sign-in completed."
                } else {
                    missing
                });
            };
            if working {
                queue(app, &id, entry);
            }
            (app.history, app.initial) = (None, None);
            app.edited();
        },
    );
}

// Makes the session the new-session composer is aimed at, its words moved out of the editor
// meanwhile; `created` sends them to it.
fn create(app: &mut App) {
    // One random request id until answered: sending again after a lost answer makes no second.
    if app.request.is_none() {
        let mut random = [0u8; 16];
        // aws-lc's RAND_bytes aborts rather than fail, so `fill` cannot return an error.
        let _ = aws_lc_rs::rand::fill(&mut random);
        app.request = Some(format!("{:032x}", u128::from_be_bytes(random)));
    }
    let i = app.initial.as_ref();
    let body = json!({
        "cwd": app.cwd, "resident": app.resident, "request": app.request,
        "model": i.and_then(|i| i.model.as_ref()), "effort": i.and_then(|i| i.effort),
        "speed": i.and_then(|i| i.speed),
    });
    let (server, path) = (app.server.clone(), creation(app));
    if app.spawn(|api| async move {
        let made = api.json(Method::POST, &path, Some(body)).await;
        let invalid = "The server returned an invalid session id; reconnect before sending.";
        let made = made.and_then(|v| {
            let id = serde_json::from_value::<SessionId>(v["session"].clone());
            id.map(|id| id.on(&server)).map_err(fail(invalid))
        });
        move |app: &mut App| created(app, made)
    }) {
        app.sending = Some(std::mem::take(&mut app.composer));
    }
}

/// The route that makes the session the new-session composer is aimed at, on the server it names;
/// the id it returns is addressed as `.on(&app.server)`.
pub fn creation(app: &App) -> String {
    format!("v1/sessions?server={}", app.server)
}

/// Whether a session can take an input the person hands it: it is neither done nor settled.
pub fn takes(agent: Option<&crate::protocol::AgentView>) -> bool {
    agent.is_some_and(|a| a.state != State::Done && a.settled.is_none())
}

/// Sends one input to each interrupted root on this server, retaining its saved draft.
pub fn continue_all(app: &mut App) {
    let sessions: Vec<_> = app
        .agents
        .values()
        .filter(|a| a.parent.is_none() && a.interrupted)
        .map(|a| a.session.clone())
        .collect();
    app.write(|api| async move {
        let (mut count, mut errors) = (0, Vec::new());
        for id in sessions {
            let sent = async {
                let draft = api.draft(&id, None).await?;
                let path = format!("v1/sessions/{id}/input");
                api.json(Method::POST, &path, Some(json!({"text": "Continue."})))
                    .await?;
                count += 1;
                if !draft.is_empty() {
                    api.draft(&id, Some(draft)).await?;
                }
                Ok::<_, Error>(())
            }
            .await;
            if let Err(e) = sent {
                errors.push(format!("{id}: {e}"));
            }
        }
        move |app: &mut App| {
            let unit = if count == 1 { "session" } else { "sessions" };
            let mut text = format!("Continued {count} interrupted {unit}.");
            if !errors.is_empty() {
                text.push_str(&format!(" {}", errors.join(" ")));
            }
            app.say(&text);
        }
    });
}

/// Hands skill `name` to session `id` as an input of its own, behind every earlier write: never
/// through the draft, an open answer or a command. The admission clears the session's saved
/// draft, so the draft read before it is saved again after.
pub fn hand_back(app: &mut App, id: SessionId, name: String) {
    let agent = app.agents.get(&id);
    if !takes(agent) {
        return app.say(&format!(
            "That session has ended; {name} cannot go back to it."
        ));
    }
    let working = agent.is_some_and(|a| a.state == State::Working);
    let body = json!({"text": "", "skill": name});
    app.write(|api| async move {
        let path = format!("v1/sessions/{id}/input");
        let draft = api.draft(&id, None).await;
        let admitted = api.json(Method::POST, &path, Some(body)).await;
        if let (Ok(_), Ok(draft)) = (&admitted, draft)
            && !draft.is_empty()
        {
            let _ = api.draft(&id, Some(draft)).await;
        }
        move |app: &mut App| match admitted.map(|v| v["entry"].as_u64().map(EntryId)) {
            Ok(Some(entry)) if working => queue(app, &id, entry),
            Ok(_) => {}
            Err(e) => app.say(&e.to_string()),
        }
    });
}

/// Takes the queued `input` back into the editor, after a newline when the editor has words,
/// and queues its withdrawal behind earlier writes. A refusal puts its band row back unless
/// the model took it meanwhile, keeps the words, and shows the server's sentence.
pub fn take_back(app: &mut App, input: EntryId) {
    let Some(id) = app.focus.clone() else { return };
    let entry = app.transcripts.get(&id).and_then(|t| t.entries.get(&input));
    let Some(Body::Input(sent)) = entry.map(|e| &e.body) else {
        return;
    };
    let lead = ["\n", ""][usize::from(app.composer.text.is_empty())];
    if !app.composer.append(&format!("{lead}{}", sent.text)) {
        return app.say("Those words and the editor's exceed 1 MiB together; nothing was taken.");
    }
    if let Some(t) = app.transcripts.get_mut(&id) {
        t.taking.insert(input);
        t.left = Some((input, now(), true));
    }
    app.edited();
    app.write(|api| async move {
        let path = format!("v1/sessions/{id}/input/{}", input.0);
        let gone = api.json(Method::DELETE, &path, None).await;
        move |app: &mut App| {
            if let Err(e) = gone {
                app.transcripts
                    .get_mut(&id)
                    .map(|t| t.taking.remove(&input));
                app.say(&e.to_string());
            }
        }
    });
}

// The made session gets the moved words; anything typed meanwhile is the editor again after.
fn created(app: &mut App, made: Result<SessionId, Error>) {
    let sent = app.sending.take().unwrap_or_default();
    let typed = std::mem::replace(&mut app.composer, sent);
    match made {
        Ok(id) => {
            (app.root, app.focus, app.request) = (Some(id.clone()), Some(id.clone()), None);
            app.list.selected = Some(super::view::Action::Agent(id.clone()));
            app.open(id);
            send(app);
        }
        Err(e) => app.say(&e.to_string()),
    }
    merge(app, typed);
}

// Words typed while an admission was pending go after the editor's, one undo step apart.
fn merge(app: &mut App, typed: Composer) {
    if app.composer.text.is_empty() && app.composer.active_images().next().is_none() {
        app.composer = typed;
    } else if !typed.text.is_empty() && !app.composer.append(&format!("\n{}", typed.text)) {
        clipboard::copy(app, Some(typed.text));
        app.say("The returned and new words exceed 1 MiB together; the new ones were copied.");
    }
    app.edited();
}

pub fn answer(app: &mut App, answer: Answer) {
    let shown = app.shown_ask.as_ref().map(|q| q.id.clone());
    match app.answering.clone().or(shown) {
        Some(id) => admit_answer(app, id, answer),
        None => app.say("There is no shown question to answer; nothing was sent."),
    }
}

pub fn admit_answer(app: &mut App, (id, ask): MailId, answer: Answer) {
    if app.sending.is_some() {
        return;
    }
    // An option leaves the editor's words and chips where they are; words answer with them.
    let words = matches!(answer, Answer::Text(_));
    let chips = app.loading_images > 0 || app.composer.active_images().next().is_some();
    if words && chips {
        return app.say("A question answer takes words only; wait for its image reads, then remove the chips or leave this answer to send an ordinary input.");
    }
    // A second pick of the same ask within 1.5 s is a double press, dropped.
    let pick = ((id.clone(), ask), now());
    let again = |(p, at): (MailId, u64)| p == pick.0 && pick.1 < at + 1500;
    if !words && app.picked.replace(pick.clone()).is_some_and(again) {
        return;
    }
    let body = serde_json::to_value(answer).unwrap_or_default();
    admit(
        app,
        format!("v1/sessions/{id}/asks/{}", ask.0),
        (body, !words),
        |app, _| {
            (app.history, app.answering) = (None, None);
            app.edited();
        },
    );
}

// Saves the draft and then posts behind every earlier write: the server clears a draft only
// when it admits the input, so refused or uncertain words survive. The words leave the editor
// meanwhile; refused, they come back ahead of anything typed since.
fn admit<F>(app: &mut App, path: String, (body, keep): (Value, bool), then: F)
where
    F: FnOnce(&mut App, Value) + Send + 'static,
{
    let draft = app.focus.clone().map(|id| (id, app.composer.text.clone()));
    let queued = app.write(|api| async move {
        let saved = match draft {
            Some((id, text)) => api.draft(&id, Some(text)).await.map(drop),
            None => Ok(()),
        };
        let admitted = match saved {
            Ok(()) => api.json(Method::POST, &path, Some(body)).await,
            Err(e) => Err(e),
        };
        move |app: &mut App| {
            let sent = app.sending.take().unwrap_or_default();
            match admitted {
                Ok(v) => then(app, v),
                Err(e) => {
                    let typed = std::mem::replace(&mut app.composer, sent);
                    merge(app, typed);
                    app.say(&e.to_string());
                }
            }
        }
    });
    // The person's own words come on screen whether or not a turn is running.
    if queued {
        app.sending = Some(match keep {
            true => Composer::default(),
            false => std::mem::take(&mut app.composer),
        });
        app.transcript_mut().into_iter().for_each(|t| t.bottom());
    }
}

// Up and Down at the editor's edges recall the person's earlier inputs, one API page at a
// time; the person's own draft waits in `history` until they come back to it.
pub fn history(app: &mut App, older: bool) {
    if app.history_loading || app.loading_images > 0 {
        return app
            .say("An input or image is still being read; wait before browsing input history.");
    }
    if !older && app.history.is_none() {
        return;
    }
    if app.history.is_some() && app.composer.active_images().next().is_some() {
        return app.say("This edited input has an image; send it or remove its chip before browsing another input.");
    }
    let Some(id) = app.focus.clone() else { return };
    let (current, edits) = (app.history.as_ref().map(|(entry, _)| *entry), app.edits);
    app.history_loading = app.spawn(|api| async move {
        let read = tokio::time::timeout(Duration::from_secs(30), inputs(&api, &id, current, older));
        let late = "Reading input history timed out; your draft is unchanged, so try again.";
        let read = read.await.map_err(fail(late)).and_then(|read| read);
        move |app: &mut App| {
            app.history_loading = false;
            if app.focus.as_ref() != Some(&id) || app.edits != edits || app.sending.is_some() {
                return;
            }
            match read {
                Ok(Some((entry, text))) => {
                    let saved = app.history.take().map(|(_, draft)| draft);
                    let saved = saved.unwrap_or_else(|| std::mem::take(&mut app.composer));
                    app.composer = Composer::default();
                    app.composer.replace(text);
                    app.history = Some((entry, saved));
                    app.edited();
                }
                Ok(None) if !older => {
                    if let Some((_, draft)) = app.history.take() {
                        app.composer = draft;
                        app.edited();
                    }
                }
                Ok(None) => app.say(
                    "There is no earlier person input in this session; your draft is unchanged.",
                ),
                Err(e) => app.say(&e.to_string()),
            }
        }
    });
}

// The person input before `current`, or the nearest one after it: a newer search rescans from
// the newest page and keeps only the latest candidate.
async fn inputs(
    api: &Api,
    id: &SessionId,
    current: Option<EntryId>,
    older: bool,
) -> Result<Option<(EntryId, String)>, Error> {
    let mut before = current.filter(|_| older).map(|e| e.0);
    let mut found = None;
    loop {
        let page = api.page(id, before, 256).await?;
        let mut oldest = None;
        for event in page.events.into_iter().rev() {
            let ApiEvent::Entry(entry) = event else {
                continue;
            };
            oldest = Some(oldest.map_or(entry.id.0, |v: u64| v.min(entry.id.0)));
            if !older && current.is_some_and(|c| entry.id <= c) {
                return Ok(found);
            }
            let Body::Input(input) = entry.body else {
                continue;
            };
            if !input.from.is_person() {
                continue;
            }
            if input.text.len() > composer::DRAFT_BYTES {
                return Err(error(
                    "That input exceeds the 1 MiB editor limit; copy a smaller part from the transcript instead.",
                ));
            }
            found = Some((entry.id, input.text));
            if older {
                return Ok(found);
            }
        }
        let Some(oldest) = oldest.filter(|id| *id > 1) else {
            return Ok(found);
        };
        if before.is_some_and(|old| oldest >= old) {
            return Err(error(
                "The history page did not move backward; reconnect to a matching server.",
            ));
        }
        before = Some(oldest);
    }
}

// Marks an admitted input queued behind the running turn, unless the model already took it.
pub(super) fn queue(app: &mut App, id: &SessionId, entry: EntryId) {
    if let Some(t) = app
        .transcripts
        .get_mut(id)
        .filter(|t| !t.taken.contains(&entry))
    {
        t.queued.insert(entry);
        t.invalidate();
    }
}
