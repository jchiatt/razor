//! The person's saved views (docs/design/agents-view.md): kept in `views.json` by the server
//! `server.shared` names, the same in every home, and forwarded to it from every other.
use super::{App, Refusal, bad, other, peers};
use crate::protocol::{
    BODY_BYTES, Delivery, Live,
    views::{Views, ViewsStatus},
};
mod layout;
use axum::body::to_bytes;
use axum::extract::{Json, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use std::{collections::BTreeSet, path::Path, sync::Arc};
use tokio::{io::AsyncReadExt, sync::Mutex, sync::watch};

/// The largest saved set as stored, defaults written out, which is what a start reads back.
const BYTES: usize = 256 << 10;

/// This server's own views, none until one is saved, and the lock one write holds at a time.
pub(super) struct Kept {
    writing: Mutex<()>,
    pub(super) views: watch::Sender<Arc<Views>>,
    pub(super) published: watch::Sender<Arc<Published>>,
}

impl Kept {
    /// Reads `views.json`, at most 256 KiB; a missing or unreadable file is none.
    pub(super) async fn open(home: &Path) -> Self {
        let (path, mut bytes) = (home.join("views.json"), Vec::new());
        if let Ok(file) = tokio::fs::File::open(&path).await {
            let _ = file.take(BYTES as u64).read_to_end(&mut bytes).await;
        }
        let views = Arc::new(serde_json::from_slice(&bytes).unwrap_or_default());
        Self {
            writing: Mutex::default(),
            views: watch::channel(views).0,
            published: watch::channel(Arc::new(encode(ViewsStatus::default()))).0,
        }
    }
}

/// `GET /v1/views`: this server's views (version 0 and empty before any is saved). `PUT`: the
/// whole set with the version it was edited from, refused unless that version is current or
/// if the set breaks a rule; else stored as the next version by write-and-rename, announced to
/// every status stream, and answered. Either goes to the keeping server when it is another.
pub(super) async fn route(State(app): State<App>, request: Request) -> Result<Response, Refusal> {
    let shared = app.controller.config().await.server.shared;
    if !super::voice::owns(&app, &shared) {
        if request.headers().contains_key(peers::HOP) {
            return Err(bad("Views forward once; correct server.shared."));
        }
        let (device, _) = app.peers.owner(&app, &shared).await.ok_or_else(|| {
            Refusal(
                StatusCode::BAD_GATEWAY,
                "The server that keeps your views is unavailable; reconnect to it.".into(),
            )
        })?;
        return peers::relay(&app, &device, request, "/v1/views").await;
    }
    if request.method() == axum::http::Method::GET {
        let views = app.views.views.borrow().clone();
        return Ok(Json(&*views).into_response());
    }
    let large = "Saved views are at most 256 KiB; remove some.";
    let large = || Refusal(StatusCode::PAYLOAD_TOO_LARGE, large.into());
    let body = to_bytes(request.into_body(), BODY_BYTES)
        .await
        .map_err(|_| large())?;
    let mut views: Views =
        serde_json::from_slice(&body).map_err(|e| bad(format!("Invalid views: {e}.")))?;
    let mut names = BTreeSet::new();
    for view in &mut views.views {
        if !(1..=64).contains(&view.name.chars().count()) || !names.insert(&view.name) {
            return Err(bad("View names must be unique and 1 to 64 characters."));
        }
        let mut ids = BTreeSet::new();
        let mut pending: Vec<_> = view.groups.iter_mut().map(|g| (g, 0)).collect();
        while let Some((g, depth)) = pending.pop() {
            if depth > 2 {
                return Err(bad("Groups allow three levels; move a deeper group up."));
            }
            if g.why
                .as_ref()
                .is_some_and(|why| why.len() > 200 || why.contains(['\n', '\r']))
            {
                return Err(bad("Use one line of at most 200 bytes for why."));
            }
            if g.id.is_empty() {
                g.id = crate::devices::fresh(16)?;
            }
            if !ids.insert(g.id.clone()) {
                return Err(bad("Each group needs its own id; remove a duplicate."));
            }
            pending.extend(g.groups.iter_mut().map(|g| (g, depth + 1)));
        }
    }
    let _writing = app.views.writing.lock().await;
    if views.version != app.views.views.borrow().version {
        let changed = "Your views changed on another device; nothing was saved.";
        return Err(Refusal(StatusCode::CONFLICT, changed.into()));
    }
    views.version += 1;
    let bytes = serde_json::to_vec(&views).map_err(other("Encoding the views"))?;
    if bytes.len() > BYTES {
        return Err(large());
    }
    super::save(&app.home, "views.json", &bytes)
        .await
        .map_err(other("Writing views.json"))?;
    let answer = Json(&views).into_response();
    app.views.views.send_replace(Arc::new(views));
    Ok(answer)
}

/// One serialized complete line, bounded to 8 MiB, with layout slices for changed-only sends.
pub(super) struct Published {
    line: Arc<str>,
    layouts: std::collections::BTreeMap<String, std::ops::Range<usize>>,
    head: usize,
}
fn encode(mut status: ViewsStatus) -> Published {
    let mut line = format!(
        r#"{{"kind":"views","version":{},"names":{},"error":{},"layouts":["#,
        status.version,
        serde_json::json!(status.names),
        serde_json::json!(status.error)
    );
    let head = line.len();
    let mut layouts = std::collections::BTreeMap::new();
    for layout in &status.layouts {
        if !layouts.is_empty() {
            line.push(',');
        }
        let start = line.len();
        if let Ok(text) = serde_json::to_string(layout) {
            line.push_str(&text);
        }
        layouts.insert(layout.name.clone(), start..line.len());
    }
    line.push_str("]}");
    if line.len() >= 8 << 20 {
        status.layouts.clear();
        status.error = Some(
            "Your views contain too many session rows; remove some views or narrow them.".into(),
        );
        return encode(status);
    }
    Published {
        line: line.into(),
        layouts,
        head,
    }
}
/// Reconnect sends the whole line; subsequent sends reuse only changed layout slices.
pub(super) async fn next(
    received: &mut watch::Receiver<Arc<Published>>,
    previous: &mut Option<Arc<Published>>,
) -> Option<Arc<str>> {
    if previous.is_some() {
        received.changed().await.ok()?;
    }
    let latest = received.borrow_and_update().clone();
    Some(match previous.replace(latest.clone()) {
        None => latest.line.clone(),
        Some(old) => {
            let changed = latest
                .layouts
                .iter()
                .filter(|(name, range)| {
                    old.layouts
                        .get(*name)
                        .is_none_or(|r| old.line[r.clone()] != latest.line[(*range).clone()])
                })
                .map(|(_, r)| &latest.line[r.clone()])
                .collect::<Vec<_>>()
                .join(",");
            format!("{}{}]}}", &latest.line[..latest.head], changed).into()
        }
    })
}

/// Resolves once for all clients, at most once per second, from bounded controller/peer snapshots.
pub(super) async fn run(app: App) {
    let (mut local, mut peers, mut deliveries, mut saved) = (
        app.controller.status(),
        app.peers.subscribe(),
        app.deliveries.subscribe(),
        app.views.views.subscribe(),
    );
    let mut clock = tokio::time::interval(std::time::Duration::from_secs(1));
    let (mut definition, mut owner) = (None::<Arc<Views>>, String::new());
    let (mut dirty, mut next_expiry) = (true, u64::MAX);
    let mut delivered = None;
    loop {
        tokio::select! {
            line = local.recv() => { if line.is_none() { return; } dirty = true; continue; }
            _ = peers.recv() => { dirty = true; continue; }
            _ = deliveries.recv() => { delivered = None; dirty = true; continue; }
            _ = saved.changed() => { dirty = true; continue; }
            _ = clock.tick() => {}
        }
        let shared = app.controller.config().await.server.shared;
        if shared != owner {
            definition = None;
            owner = shared;
            dirty = true;
        }
        let remote = !super::voice::owns(&app, &owner);
        let paired = app.peers.owner(&app, &owner).await;
        let version = paired.as_ref().and_then(|(_, v)| *v);
        if remote
            && (definition.is_none()
                || version.is_some_and(|v| definition.as_ref().is_none_or(|d| d.version != v)))
        {
            if let Some((device, _)) = paired
                && let Some(views) = app
                    .devices
                    .fetch::<Views>(&device, "/v1/views", &(), BYTES)
                    .await
            {
                definition = Some(Arc::new(views));
                dirty = true;
            }
        } else if !remote {
            definition = Some(app.views.views.borrow().clone());
        }
        if !dirty && crate::protocol::now() < next_expiry {
            continue;
        }
        if delivered.is_none() {
            delivered = Some(
                app.deliveries
                    .list(None, 256, &[])
                    .await
                    .unwrap_or_default(),
            );
        }
        next_expiry = publish(
            &app,
            definition.as_deref(),
            delivered.as_deref().unwrap_or_default(),
        )
        .await;
        dirty = false;
        clock.reset();
    }
}

async fn publish(app: &App, definition: Option<&Views>, delivered: &[Delivery]) -> u64 {
    let Some(Live::Agents {
        agents: mut rows, ..
    }) = app.controller.status().recv().await
    else {
        return u64::MAX;
    };
    let config = app.controller.config().await;
    let servers = app
        .peers
        .extend(&mut rows, super::identity(app, &config))
        .await;
    app.controller.child_rows(&mut rows);
    let delivered = app.peers.deliveries(delivered.to_vec(), 256, &[]).await;
    let unread = delivered
        .iter()
        .filter(|d| !d.read && !d.archived)
        .map(|d| d.session.clone())
        .collect();
    let now = crate::protocol::now();
    let mut status = match definition {
        Some(d) => ViewsStatus {
            version: d.version,
            layouts: layout::resolve(d, &rows, &servers, &unread, now),
            ..Default::default()
        },
        None => ViewsStatus {
            error: Some("The server that keeps your views is unavailable; reconnect to it.".into()),
            ..Default::default()
        },
    };
    status.names = status.layouts.iter().map(|l| l.name.clone()).collect();
    app.residents
        .sort(definition, &status, rows.iter().map(|a| a.session.clone()));
    let published = Arc::new(encode(status));
    if app.views.published.borrow().line != published.line {
        app.views.published.send_replace(published);
    }
    rows.iter()
        .filter_map(|a| a.person_input_at)
        .map(|at| at.saturating_add(43_200_000))
        .filter(|at| *at > now)
        .min()
        .unwrap_or(u64::MAX)
}
