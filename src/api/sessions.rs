//! Session creation, input and transcript routes.

use super::*;
use crate::config::Settings;
use crate::protocol::{
    Answer, ClientInput, Effort, Found, IMAGE_BASE64, IMAGES_PER_INPUT, Input, Listed, Origin,
    Search, Skill, Speed,
};
use base64::Engine;
use serde_json::value::RawValue;

pub(super) fn routes() -> Router<App> {
    Router::new()
        .route("/v1/sessions", get(list).post(create))
        .route("/v1/models", get(models))
        .route("/v1/search", get(search))
        .route("/v1/sessions/{id}/input", post(input))
        .route("/v1/sessions/{id}/effort", post(effort))
        .route("/v1/sessions/{id}/input/{entry}", delete(withdraw))
        .route("/v1/sessions/{id}/stop", post(stop))
        .route("/v1/sessions/{id}/done", post(done))
        .route("/v1/sessions/{id}/quit", post(quit))
        .route("/v1/sessions/{id}/handover", post(handover))
        .route("/v1/sessions/{id}/skills", get(skills))
        .route("/v1/sessions/{id}/asks/{entry}", post(answer))
        .route("/v1/sessions/{id}/events", get(events))
        .route("/v1/sessions/{id}/entries", get(entries))
        .route("/v1/sessions/{id}/children", get(children))
        .route("/v1/sessions/{id}/tasks", get(tasks))
        .route(
            "/v1/sessions/{id}/draft",
            (get(draft).put(set_draft)).layer(DefaultBodyLimit::max(1 << 20)),
        )
        .route("/v1/sessions/{id}/settings", get(settings))
        .route("/v1/sessions/{id}/name", put(rename))
        .route("/v1/sessions/{id}/seen", put(seen))
}

/// Readiness reflects the configured Responses endpoint, applied at the next server start.
async fn models(State(app): State<App>, Query(q): Query<Page>) -> Reply<crate::protocol::Models> {
    let config = app.controller.config().await;
    let mut models = profiles::models();
    models.extend(config.models.keys().cloned());
    models.sort();
    models.dedup();
    let (mut windows, mut efforts) = (BTreeMap::new(), BTreeMap::new());
    let mut unavailable = BTreeMap::new();
    let mut speeds = BTreeMap::new();
    let checks = if q.ready {
        let sign_in = config.claude.sign_in.is_empty().then_some(&[][..]);
        ready::credentials(&app.home, sign_in, Some(Duration::from_secs(2))).await
    } else {
        BTreeMap::new()
    };
    for name in &models {
        let (window, _) = profiles::limits(name, &config, None);
        windows.extend(window.map(|window| (name.clone(), window)));
        efforts.insert(name.clone(), profiles::model(name).efforts().to_vec());
        let speed = |s| profiles::speed_choice(name, s, &config);
        speeds.insert(name.clone(), Speed::ALL.into_iter().map(speed).collect());
        if q.ready
            && let Some(why) = ready::unready(&checks, &config, name)
        {
            unavailable.insert(name.clone(), why);
        }
    }
    let switch = match &q.session {
        Some(id) => app.controller.switch(id, models.clone()).await?,
        None => BTreeMap::new(),
    };
    Ok(Json(crate::protocol::Models {
        switch,
        models,
        windows,
        efforts,
        unavailable,
        model: config.model,
        effort: config.effort,
        call_model: config.call_model,
        call_effort: config.call_effort,
        speed: config.speed,
        speeds,
    }))
}

async fn list(State(app): State<App>, Query(q): Query<Page>) -> Reply<Vec<Listed>> {
    let limit = q.limit.unwrap_or(50).min(1_000);
    let jobs = q.jobs.unwrap_or(q.resident.is_some());
    let listed = app.controller.list(q.cwd, q.resident.as_ref(), jobs, limit);
    Ok(Json(listed.await?))
}

/// Immediate children not settled, retaining the controller's complete rows and remote facts.
async fn children(State(app): State<App>, Path(id): Path<SessionId>) -> Reply {
    let mut rows = app.controller.tree(&id).await?;
    rows.retain(|row| row.parent.as_ref() == Some(&id));
    Ok(Json(json!(rows)))
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Create {
    /// Unused when `resident` is given: a thread starts in its resident's folder, or its
    /// resident project's.
    cwd: PathBuf,
    resident: Option<SessionId>,
    resident_project: Option<String>,
    model: Option<String>,
    effort: Option<Effort>,
    speed: Option<Speed>,
    request: Option<String>,
    /// Started for a call: a model or effort left out is the call's (`call_model`, `call_effort`).
    call: bool,
}

/// A repeat of one of the newest 64 creations' request ids since the server started answers the
/// session that creation made; with a request id, the list's lock is held from the lookup until
/// it is remembered.
async fn create(State(app): State<App>, Json(c): Json<Create>) -> Reply {
    request(c.request.as_deref())?;
    let mut made = match c.request {
        Some(_) => Some(app.creations.lock().await),
        None => None,
    };
    let mut repeat = made.iter().flat_map(|m| m.iter());
    if let Some((_, session)) = repeat.find(|(r, _)| c.request.as_ref() == Some(r)) {
        let name = app.controller.summary(session).await?.name;
        return Ok(Json(json!({ "session": session, "name": name })));
    }
    let config = app.controller.config().await;
    let model = (c.model.or(c.call.then(|| config.call_model.clone()))).map(profiles::id);
    let effort = c.effort.or(c.call.then_some(config.call_effort));
    let (cwd, thread) = match c.resident {
        Some(name) => (app.residents.thread(&name, None, c.resident_project)).await?,
        None => (c.cwd, None),
    };
    let _ = tokio::fs::read_dir(&cwd).await.map_err(|e| {
        bad(format!(
            "Razor cannot read {}: {e}; choose an existing readable folder.",
            cwd.display()
        ))
    })?;
    let session = app
        .controller
        .create(cwd, model, (effort, c.speed), None, None, thread);
    let session = session.await?;
    if let (Some(made), Some(r)) = (&mut made, c.request) {
        made.truncate(63);
        made.push_front((r, session.clone()));
    }
    let name = app.controller.summary(&session).await?.name;
    Ok(Json(json!({ "session": session, "name": name })))
}

/// Pictures are decoded and stored one at a time, so at most one decoded picture is held beside
/// the body.
async fn input(
    State(app): State<App>,
    Path(id): Path<SessionId>,
    Extension(devices::Enrolled(enrolled)): Extension<devices::Enrolled>,
    Json(c): Json<ClientInput>,
) -> Reply {
    request(c.request.as_deref())?;
    if let Some(request) = &c.request
        && let Some(entry) = app.controller.summary(&id).await?.repeat(Some(request))
    {
        return Ok(Json(json!({ "entry": entry })));
    }
    if c.from.is_none() && landed(&app, &id, &c.text).await? {
        return Ok(Json(json!({ "signed_in": true })));
    }
    let n = c.images.len() + c.files.len();
    if n > IMAGES_PER_INPUT {
        return Err(bad(format!(
            "An input holds at most {IMAGES_PER_INPUT} pictures and files, not {n}."
        )));
    }
    let mut images = Vec::with_capacity(c.images.len());
    for (n, data) in (1..).zip(c.images) {
        if data.len() > IMAGE_BASE64 {
            return Err(bad(format!(
                "Picture {n} is over 10 MB of base64; send it smaller."
            )));
        }
        let bytes = (base64::engine::general_purpose::STANDARD.decode(&data))
            .map_err(|e| bad(format!("Picture {n} is not base64: {e}")))?;
        images.push(app.controller.store(&id, bytes).await?);
    }
    let (from, device) = match c.from {
        Some(name) => (Origin::Program { name }, None),
        None => (Origin::Person, enrolled),
    };
    let mut text = c.text;
    if let Some(name) = c.skill {
        let dirs = app.controller.settings(&id).await?.skills();
        let found = crate::tools::find_skill(dirs, &name).await;
        let (_, skill) = found.map_err(|e| bad(e.to_string()))?;
        text = format!("<skill name=\"{name}\">\n{skill}\n</skill>\n\n{text}");
    }
    let input = Input {
        from,
        device,
        text,
        model: c.model.map(profiles::id),
        effort: c.effort,
        speed: c.speed,
        handover: c.handover,
        images,
        files: c.files,
        request: c.request,
        voice: c.voice.map(Box::new),
        video: c.video.map(Box::new),
        ..Input::default()
    };
    let entry = speech::admit(&app, &id, input).await?;
    Ok(Json(json!({ "entry": entry })))
}

async fn withdraw(State(app): State<App>, Path((id, entry)): Path<(SessionId, EntryId)>) -> Reply {
    app.controller.withdraw(&id, entry).await?;
    Ok(Json(json!({})))
}

async fn effort(
    State(app): State<App>,
    Path(id): Path<SessionId>,
    Json(c): Json<EffortChoice>,
) -> Reply {
    let entry = app.controller.effort(&id, c.effort).await?;
    Ok(Json(
        entry.map_or(json!({}), |entry| json!({ "entry": entry })),
    ))
}

#[derive(Deserialize)]
struct EffortChoice {
    effort: Effort,
}

async fn stop(State(app): State<App>, Path(id): Path<SessionId>) -> Reply {
    app.controller.stop(&id).await?;
    Ok(Json(json!({})))
}

/// `/done`: answers once the closing input is admitted; its turn runs on in the server.
async fn done(State(app): State<App>, Path(id): Path<SessionId>) -> Reply {
    let entry = app.controller.done(&id, true).await?;
    Ok(Json(json!({ "entry": entry })))
}

async fn quit(State(app): State<App>, Path(id): Path<SessionId>) -> Reply {
    app.controller.done(&id, false).await?;
    Ok(Json(json!({})))
}

async fn skills(State(app): State<App>, Path(id): Path<SessionId>) -> Reply<Vec<Skill>> {
    let dirs = app.controller.settings(&id).await?.skills();
    Ok(Json(crate::tools::list_skills(dirs, 1024).await))
}

async fn handover(State(app): State<App>, Path(id): Path<SessionId>) -> Reply {
    let entry = app.controller.handover(&id).await?;
    Ok(Json(entry.map_or_else(
        || json!({}),
        |entry| json!({ "entry": entry }),
    )))
}

async fn answer(
    State(app): State<App>,
    Path((id, ask)): Path<(SessionId, EntryId)>,
    Extension(devices::Enrolled(device)): Extension<devices::Enrolled>,
    Json(a): Json<Answer>,
) -> Reply {
    let entry = app.controller.answer(&id, ask, a, device).await?;
    Ok(Json(json!({ "entry": entry })))
}

/// The stream holds its device's mark on the session while it is open, which the push sender
/// reads.
async fn events(
    State(app): State<App>,
    Extension(gone): Extension<Gone>,
    Extension(devices::Enrolled(device)): Extension<devices::Enrolled>,
    Path(id): Path<SessionId>,
    Query(q): Query<Page>,
) -> Result<Response, Refusal> {
    let after = EntryId(q.after.unwrap_or(0));
    let lines = app.controller.attach(&id, after, q.terminal.into()).await?;
    let open = app.devices.stream(device, &id);
    let lines = received(lines).map(move |line| {
        let _held = &open;
        sent(line, q.native)
    });
    Ok(ndjson(lines, gone))
}

/// A page is read in one pass and ends by itself, so it does not watch its device.
async fn entries(
    State(app): State<App>,
    Path(id): Path<SessionId>,
    Query(q): Query<Page>,
) -> Result<Response, Refusal> {
    let limit = q.limit.unwrap_or(256).min(1_024);
    let before = q.before.map(EntryId);
    let replay = app.controller.page(&id, before, limit).await?;
    let lines = stream::unfold(replay, move |mut replay| async move {
        let line = replay
            .next_line()
            .await?
            .map(|line| sent(line, q.native) + "\n");
        Some((line, replay))
    });
    Ok(([(header::CONTENT_TYPE, NDJSON)], Body::from_stream(lines)).into_response())
}

/// A stored or live line as a reader gets it: an entry's `native`, the model path's replay data
/// that no client draws (about 100 KB on a GPT response), is left out unless `native`.
fn sent<T: AsRef<str> + From<String>>(line: T, native: bool) -> T {
    #[derive(Deserialize)]
    struct Line<'a> {
        #[serde(borrow)]
        native: Option<&'a RawValue>,
    }
    const KEY: &str = ",\"native\":";
    let text = line.as_ref();
    // Only a top-level `native` is read; the record writes it compact, after the entry's id.
    let parsed = (!native && text.contains(KEY)).then(|| serde_json::from_str(text));
    let Some(Ok(Line { native: Some(raw) })) = parsed else {
        return line;
    };
    let start = raw.get().as_ptr() as usize - text.as_ptr() as usize;
    match text[..start].strip_suffix(KEY) {
        Some(head) => format!("{head}{}", &text[start + raw.get().len()..]).into(),
        None => line,
    }
}

async fn draft(State(app): State<App>, Path(id): Path<SessionId>) -> Result<String, Refusal> {
    Ok(app.controller.draft(&id).await?)
}

async fn set_draft(State(app): State<App>, Path(id): Path<SessionId>, text: String) -> Reply {
    app.controller.set_draft(&id, text).await?;
    Ok(Json(json!({})))
}

/// The person's past conversations, here and on paired servers; 404 for a resident not in the
/// config.
async fn search(State(app): State<App>, Query(q): Query<Search>) -> Reply<Found> {
    if let Some(name) = &q.resident {
        app.residents.resident(name).await?;
    }
    Ok(Json(crate::children::search(&app.controller, q).await?))
}

/// The session's project's tasks, as its `task list` answers them for `filter`.
async fn tasks(State(app): State<App>, Path(id): Path<SessionId>, Query(q): Query<Page>) -> Reply {
    let (c, cwd) = (&app.controller, app.controller.summary(&id).await?.cwd);
    let tasks = c.memory().tasks(c, &cwd, q.filter).await;
    Ok(Json(tasks.map_err(|e| bad(e.to_string()))?))
}

/// The person's rename; the body is the new name.
async fn rename(State(app): State<App>, Path(id): Path<SessionId>, name: String) -> Reply {
    crate::children::rename(&app.controller, &id, &name).await?;
    Ok(Json(json!({})))
}

/// The last entry a reader saw, at most the last recorded, on the status row in memory with when
/// it was reported.
async fn seen(State(app): State<App>, Path(id): Path<SessionId>, Query(q): Query<Page>) -> Reply {
    let entry = q
        .entry
        .ok_or_else(|| bad("Name the last entry seen: ?entry=N."))?;
    let last = app.controller.summary(&id).await?.last;
    app.controller.seen(&id, last.min(EntryId(entry)));
    Ok(Json(json!({})))
}

async fn settings(State(app): State<App>, Path(id): Path<SessionId>) -> Reply<Settings> {
    Ok(Json(app.controller.settings(&id).await?))
}
