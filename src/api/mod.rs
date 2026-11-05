//! The API server (docs/design/api.md): HTTP and NDJSON streams under `/v1`, on the local socket
//! and an optional tailnet address, every request carrying a device key. Also the client half of
//! `razor login` and `razor devices`.

mod call;
mod client;
mod devices;
mod emoji;
mod files;
mod load;
mod marks;
pub mod mcp;
mod meetings;
mod peers;
mod project;
mod push;
pub mod ready;
mod recap;
mod repos;
mod residents;
mod sessions;
mod speech;
mod views;
mod voice;

use crate::controller::{self, Controller};
pub use crate::devices::Error;
use crate::devices::{Devices, Gone, other};
use crate::protocol::{BODY_BYTES, Delivery, EntryId, Live, Login, Mail, Server, SessionId};
use crate::residents::{Deliveries, Error as ResidentError, Residents};
use crate::responses::Responses;
use crate::{config, profiles};
use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Extension, Json, Path, Query, RawQuery, Request, State};
use axum::http::{Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::serve::ListenerExt;
pub use client::{DEVICES_USE, devices, enroll, login, revoke};
use futures_util::{FutureExt, Stream, StreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::convert::Infallible;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, UnixListener};
use tokio::sync::{Mutex, mpsc, oneshot, watch};

/// The local socket and the local device's key (mode 0600), in the state directory.
pub const SOCKET: &str = "server.sock";
pub use crate::devices::KEY;
/// This home's id as a phone knows it (mode 0600), in the state directory.
const ID: &str = "server.id";
/// The base URL of a client bound to `SOCKET`; the host is not used.
pub const LOCAL: &str = "http://localhost";
/// Maximum time to finish live calls after shutdown starts.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);
/// The largest `POST /v1/mail` body: mail's 64 KiB of text, and 8 KiB for the sender, the
/// project, the ids and the text's JSON escapes.
const MAIL_BYTES: usize = (64 << 10) + (8 << 10);

#[derive(Clone)]
struct App {
    home: Arc<PathBuf>,
    peers: Arc<peers::Peers>,
    controller: Controller,
    residents: Residents,
    deliveries: Deliveries,
    responses: Arc<Responses>,
    devices: Arc<Devices>,
    emoji: Arc<emoji::History>,
    views: Arc<views::Kept>,
    meetings: Arc<meetings::Kept>,
    /// This home's id.
    id: Arc<str>,
    /// Serializes settings and the daily count on their owner through durable replacement.
    voice: Arc<Mutex<()>>,
    /// On the voice owner, the latest input of at most 64 terminals on any server naming it,
    /// expired after a minute on the next sample.
    activity: Arc<Mutex<BTreeMap<String, std::time::Instant>>>,
    /// The newest 64 creations that carried a request id, since the server started.
    creations: Arc<Mutex<VecDeque<(String, SessionId)>>>,
    /// True once the server stops; each live call holds a receiver until it has ended.
    calls: Arc<watch::Sender<bool>>,
}

/// Serve until this future is dropped.
/// - Listens on `home/server.sock`, replacing a stale file (the caller holds `server.lock`), and
///   on `address` (`server.address`, the tailnet address) when one is given.
/// - Makes `server.id` at the first start and writes `server.json`: `{pid, address}`. The
///   controller has already opened device enrollment and keys through `crate::devices`.
/// - Every request must carry `Authorization: Bearer <key>`: the local key or an enrolled
///   device's. A body is at most 32 MiB.
/// - ChatGPT sign-in shares the controller's Responses client and authentication mutex.
/// - Sends to Apple's push service beside serving.
/// - Returns once `shutdown` does and the live calls have ended, recorded and charged, or 20
///   seconds have passed.
pub async fn serve(
    home: PathBuf,
    address: Option<String>,
    controller: Controller,
    residents: Residents,
    deliveries: Deliveries,
    responses: Arc<Responses>,
    shutdown: impl Future<Output = ()>,
) -> Result<(), Error> {
    let devices = controller.devices().clone();
    let id = crate::devices::kept(&home.join(ID), 16, "Reading or making server.id").await?;
    let socket = home.join(SOCKET);
    // A stale socket file: the caller holds `server.lock`, so no server answers on it.
    let _ = tokio::fs::remove_file(&socket).await;
    let local = UnixListener::bind(&socket).map_err(|e| {
        Error::Refused(format!("Razor cannot listen on {}: {e}.", socket.display()))
    })?;
    let remote = match address {
        Some(address) => Some(TcpListener::bind(&address).await.map_err(|e| {
            Error::Refused(format!(
                "Razor cannot listen on server.address {address}: {e}. Set it to this machine's tailnet address, or remove it."
            ))
        })?),
        None => None,
    };
    let bound = remote.as_ref().map(TcpListener::local_addr).transpose();
    let bound = bound.map_err(other("Reading the bound address"))?;
    let found = json!({ "pid": std::process::id(), "address": bound.map(|a| a.to_string()) });
    tokio::fs::write(home.join("server.json"), found.to_string())
        .await
        .map_err(other("Writing server.json"))?;
    let app = App {
        home: Arc::new(home.clone()),
        peers: Arc::default(),
        controller,
        residents,
        deliveries,
        responses,
        devices: devices.clone(),
        emoji: Arc::new(emoji::History::open(&home).await),
        views: Arc::new(views::Kept::open(&home).await),
        meetings: Arc::new(meetings::Kept::open().await),
        id: id.into(),
        creations: Arc::default(),
        voice: Arc::default(),
        activity: Arc::default(),
        calls: Arc::new(watch::channel(false).0),
    };
    let speaking = app.clone();
    app.controller
        .children()
        .speak(Box::new(move |id, entry, text, requested| {
            let spoken = speech::spoken(speaking.clone(), id, entry, text, requested);
            Box::pin(spoken.map(|spoken| spoken.map_err(|r| crate::children::Error::Refused(r.1))))
        }));
    let router = Router::new()
        .merge(sessions::routes())
        .route("/v1/repos", get(repos::repos))
        .route("/v1/sessions/{id}/recap", post(recap::recap))
        .route("/v1/sessions/{id}/project", get(project::project))
        .route("/v1/sessions/{id}/marks", get(marks::marks))
        .route("/v1/sessions/{id}/files/{*name}", get(files::file))
        .route("/v1/sessions/{id}/files/{*name}", post(files::upload))
        .route("/v1/sessions/{id}/speech/{entry}", get(speech::speech))
        .route("/v1/voice/activity", put(voice::activity))
        .route(
            "/v1/sessions/{id}/deliveries/{entry}/read",
            put(delivery_read).delete(delivery_read),
        )
        .route(
            "/v1/sessions/{id}/deliveries/{entry}/archive",
            put(delivery_archive).delete(delivery_archive),
        )
        .route_layer(middleware::from_fn_with_state(app.clone(), peers::forward))
        .route("/v1/sessions/{id}/call", get(call::upgrade))
        .merge(meetings::routes())
        .route("/v1/sessions/{id}/voice/{entry}", post(speech::note))
        .route("/v1/views", get(views::route).put(views::route))
        .route("/v1/status", get(status))
        .merge(residents::routes())
        .route(
            "/v1/children",
            post(peers::child).layer(DefaultBodyLimit::max(512 << 10)),
        )
        .route(
            "/v1/permissions",
            get(async || Json(crate::residents::permissions().await)),
        )
        .route("/v1/emoji", get(emoji::history).post(emoji::used))
        .route("/v1/deliveries", get(delivery_list))
        .route("/v1/voice", get(voice::get).put(voice::put))
        .route("/v1/voice/voices", get(voice::voices))
        .route("/v1/voice/reserve", post(voice::reserve))
        .route("/v1/devices", get(devices::list).post(devices::enroll))
        .route("/v1/devices/{name}", delete(devices::revoke))
        .route(
            "/v1/devices/self/push",
            put(devices::register).delete(devices::unregister),
        )
        .route(
            "/v1/devices/self/notifications",
            get(devices::notifications).put(devices::set_notifications),
        )
        .route(
            "/v1/sessions/{id}/follow",
            put(devices::follow).delete(devices::unfollow),
        )
        .route(
            "/v1/mail",
            post(mail).layer(DefaultBodyLimit::max(MAIL_BYTES)),
        )
        .route("/v1/login", post(sign_in))
        .route("/v1/mcp", post(mcp))
        .route("/v1/mcp/callback", get(mcp_callback))
        .fallback(|| async { Refusal(StatusCode::NOT_FOUND, "There is no such route.".into()) })
        .layer(DefaultBodyLimit::max(BODY_BYTES))
        .layer(middleware::from_fn(refusals))
        .layer(middleware::from_fn_with_state(devices, devices::authorize))
        .with_state(app.clone())
        .into_make_service();
    let local = axum::serve(local, router.clone()).into_future();
    let served = async {
        let Some(remote) = remote else {
            return local.await;
        };
        // Without TCP_NODELAY a streamed answer's last small write waits for the peer's delayed
        // ACK, 40 ms or more per paged or streamed read a paired server forwards.
        let remote = remote.tap_io(|tcp| drop(tcp.set_nodelay(true)));
        tokio::try_join!(local, axum::serve(remote, router).into_future()).map(drop)
    };
    tokio::select! {
        served = served => served.map_err(other("Serving the API")),
        () = push::send(app.clone()) => Ok(()),
        () = peers::run(app.clone()) => Ok(()),
        () = views::run(app.clone()) => Ok(()),
        () = meetings::run(app.clone()) => Ok(()),
        () = shutdown => {
            app.calls.send_replace(true);
            let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, app.calls.closed()).await;
            Ok(())
        }
    }
}

/// A client's own id for an input or a creation: 1 to 64 ASCII letters, digits and `-`.
fn request(request: Option<&str>) -> Result<(), Refusal> {
    match request.filter(|r| {
        !(1..=64).contains(&r.len()) || r.bytes().any(|b| !b.is_ascii_alphanumeric() && b != b'-')
    }) {
        Some(r) => Err(bad(format!(
            "A request id is 1 to 64 ASCII letters, digits and -, not {r:?}."
        ))),
        _ => Ok(()),
    }
}

/// A refusal: a status and one sentence, sent as `{"error": "…"}`.
struct Refusal(StatusCode, String);
type Reply<T = Value> = Result<Json<T>, Refusal>;

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<controller::Error> for Refusal {
    fn from(e: controller::Error) -> Refusal {
        let status = match &e {
            controller::Error::NoSession(_) => StatusCode::NOT_FOUND,
            // A file the client named that is not there.
            controller::Error::Other { source, .. }
                if (source.downcast_ref::<io::Error>())
                    .is_some_and(|e| e.kind() == io::ErrorKind::NotFound) =>
            {
                StatusCode::NOT_FOUND
            }
            controller::Error::Full(_) | controller::Error::Busy(_) => StatusCode::CONFLICT,
            controller::Error::Invalid(_) => StatusCode::BAD_REQUEST,
            controller::Error::Other { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Refusal(status, e.to_string())
    }
}

impl From<ResidentError> for Refusal {
    fn from(e: ResidentError) -> Refusal {
        let status = match &e {
            ResidentError::NoResident(_) | ResidentError::NoProject(_) => StatusCode::NOT_FOUND,
            ResidentError::Refused(_) => StatusCode::BAD_REQUEST,
            ResidentError::Other { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Refusal(status, e.to_string())
    }
}

impl From<crate::children::Error> for Refusal {
    fn from(e: crate::children::Error) -> Refusal {
        let status = match &e {
            crate::children::Error::Refused(_) => StatusCode::BAD_REQUEST,
            crate::children::Error::Other { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Refusal(status, e.to_string())
    }
}

fn bad(sentence: impl Into<String>) -> Refusal {
    Refusal(StatusCode::BAD_REQUEST, sentence.into())
}

/// The sign-in runs on its own task; leaving or removing its device cancels it.
async fn sign_in(State(app): State<App>, Extension(gone): Extension<Gone>) -> Response {
    let (show, shown) = mpsc::channel(8);
    let (stop, stopped) = watch::channel(false);
    let (mut end, ended) = oneshot::channel();
    tokio::spawn(async move {
        let login = app.responses.login(show, stopped);
        tokio::pin!(login);
        let result = tokio::select! {
            result = &mut login => result,
            () = end.closed() => {
                let _ = stop.send(true);
                login.await
            }
        };
        let _ = end.send(result);
    });
    let last = ended.map(|result| match result {
        Ok(Ok(())) => Login::Done,
        Ok(Err(e)) => Login::Failed {
            error: e.to_string(),
        },
        Err(_) => Login::Failed {
            error: "The sign-in stopped before it ended.".into(),
        },
    });
    let lines = received(shown)
        .map(|text| Login::Show { text })
        .chain(stream::once(last))
        .filter_map(|line| async move { line_of(&line) });
    ndjson(lines, gone)
}

#[derive(Deserialize)]
struct McpRequest {
    cwd: PathBuf,
    session: Option<SessionId>,
    #[serde(flatten)]
    operation: crate::protocol::McpOperation,
}

async fn mcp(State(app): State<App>, Json(request): Json<McpRequest>) -> Reply {
    let answer = app
        .controller
        .mcp(&request.cwd, request.session.as_ref(), request.operation)
        .await?;
    Ok(Json(answer))
}

async fn mcp_callback(State(app): State<App>, RawQuery(query): RawQuery) -> Reply {
    let query = query.unwrap_or_default();
    let parsed = reqwest::Url::parse(&format!("http://localhost/?{query}")).map_err(|_| {
        bad("The MCP callback query is invalid; paste the complete redirect address.")
    })?;
    let redirect = parsed
        .query_pairs()
        .find(|(key, _)| key == "redirect")
        .map(|(_, value)| value.into_owned());
    let redirect = match redirect {
        Some(redirect) => redirect,
        None => {
            let mut url = callback(&app).await.map_err(|e| bad(e.to_string()))?;
            url.set_query(Some(&query));
            url.into()
        }
    };
    app.controller.mcp_complete(&redirect).await?;
    Ok(Json(json!({"signed_in": true})))
}

async fn callback(app: &App) -> Result<reqwest::Url, crate::tools::Error> {
    crate::tools::callback(app.controller.config().await.server.https_url.as_deref())
}

/// A person's input that is one landed sign-in address completes its sign-in as the callback
/// does, answering the ask of whichever session began it; its code reaches neither the record
/// nor the model.
async fn landed(app: &App, id: &SessionId, text: &str) -> Result<bool, Refusal> {
    let text = text.trim();
    let Ok(mut url) = reqwest::Url::parse(text) else {
        return Ok(false);
    };
    url.set_query(None);
    if text.contains(char::is_whitespace) || callback(app).await.ok() != Some(url) {
        return Ok(false);
    }
    app.controller.mcp_complete(text).await?;
    // The terminal saved the address, code and all, as the draft before sending it.
    app.controller.set_draft(id, String::new()).await?;
    Ok(true)
}

/// axum's own refusals (a malformed body, path or query; a body over its limit) are plain text:
/// send them as every other refusal is sent.
async fn refusals(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    let plain = (response.headers().get(header::CONTENT_TYPE))
        .is_some_and(|t| t.as_bytes().starts_with(b"text/plain"));
    if response.status().is_success() || !plain {
        return response;
    }
    let status = response.status();
    let text = axum::body::to_bytes(response.into_body(), 4096).await;
    let text = String::from_utf8_lossy(&text.unwrap_or_default()).into_owned();
    let text = text.trim_end_matches('.');
    Refusal(status, format!("The request was refused: {text}.")).into_response()
}

/// The query of every listing and stream; each route reads its own fields.
#[derive(Deserialize)]
struct Page {
    cwd: Option<PathBuf>,
    /// The sessions list: only this resident's threads.
    resident: Option<SessionId>,
    /// The sessions list: fresh runs too; by default only with `resident`.
    jobs: Option<bool>,
    session: Option<SessionId>,
    before: Option<u64>,
    after: Option<u64>,
    /// The last entry a reader saw.
    entry: Option<u64>,
    limit: Option<usize>,
    #[serde(default)]
    local: bool,
    #[serde(default)]
    heartbeat: bool,
    #[serde(default)]
    views: bool,
    #[serde(default)]
    terminal: bool,
    #[serde(default)]
    ready: bool,
    /// A session's stream and pages: entries keep their `native`.
    #[serde(default)]
    native: bool,
    filter: Option<String>,
    q: Option<String>,
}

/// Mail from a paired server, or a local program, to the one agent here its `to` names.
async fn mail(
    State(app): State<App>,
    Extension(devices::Enrolled(server)): Extension<devices::Enrolled>,
    Json(mail): Json<Mail>,
) -> Reply {
    let (session, entry) =
        crate::children::receive(&app.controller, mail, server.as_deref()).await?;
    Ok(Json(json!({ "session": session, "entry": entry })))
}

/// The display label is shared by status, push and device enrollment.
fn label(server: &config::Server) -> String {
    let label = [&server.label, &server.name]
        .into_iter()
        .find(|label| !label.is_empty())
        .cloned()
        .unwrap_or_else(profiles::machine);
    label.chars().take(64).collect()
}

/// Identity shared by layouts and status, without sampling load or the residents' calendars.
fn identity(app: &App, config: &config::Config) -> Server {
    Server {
        id: app.id.to_string(),
        label: label(&config.server),
        machine: if config.server.name.is_empty() {
            profiles::machine()
        } else {
            config.server.name.clone()
        },
        realm: config.server.realm.clone(),
        residents: crate::residents::homes(config),
        ..Default::default()
    }
}

/// Local metadata is independent of labels; remote config never replaces local display choices.
async fn server(app: &App) -> Server {
    let config = app.controller.config().await;
    Server {
        transcribe: !config.transcribe.is_empty(),
        call: config.live.url.is_some()
            || (tokio::fs::metadata(app.home.join("key")).await).is_ok_and(|key| key.len() > 0),
        shared: voice::owns(app, &config.server.shared),
        video: true,
        meetings: app.meetings.available,
        load: load::load().await,
        residents: app.residents.days().await,
        views_version: Some(app.views.views.borrow().version),
        ..identity(app, &config)
    }
}

/// Local lines plus transient peer snapshots; a lapped stream ends so the client reloads its page.
async fn status(
    State(app): State<App>,
    Extension(gone): Extension<Gone>,
    Query(q): Query<Page>,
) -> Response {
    let heartbeat = tokio::time::interval_at(
        tokio::time::Instant::now() + std::time::Duration::from_secs(15),
        std::time::Duration::from_secs(15),
    );
    let sources = (
        app.controller.status(),
        app.deliveries.subscribe(),
        app.peers.subscribe(),
        heartbeat,
        app.views.published.subscribe(),
        app.views.views.subscribe(),
        None,
        app,
        q,
    );
    let lines = stream::unfold(sources, |mut sources| async move {
        let (local, deliveries, peers, heartbeat, views, saved, previous, app, q) = &mut sources;
        loop {
            let mut line = tokio::select! {
                line = views::next(views, previous), if q.views => return Some((line?, sources)),
                _ = saved.changed(), if q.heartbeat => Live::Server(server(app).await.into()),
                line = local.recv() => line?,
                d = deliveries.recv() => Live::Delivery(d.ok()?.0.into()),
                peer = peers.recv(), if !q.local => match peer.ok()? {
                    Live::Server(s) if s.name.is_empty() => { *local = app.controller.status(); continue; },
                    line => line,
                },
                _ = heartbeat.tick(), if q.heartbeat => Live::Server(server(app).await.into()),
            };
            if let Live::Agents {
                server: home,
                servers,
                agents,
                ..
            } = &mut line
            {
                **home = server(app).await;
                *servers = if q.local {
                    vec![*home.clone()]
                } else {
                    app.peers.extend(agents, *home.clone()).await
                };
            }
            if !q.local {
                let rows = match &mut line {
                    Live::Agents { agents, .. } => agents.as_mut_slice(),
                    Live::Agent(row) => std::slice::from_mut(&mut **row),
                    _ => &mut [],
                };
                app.controller.child_rows(rows);
            }
            return Some((line_of(&line)?, sources));
        }
    });
    ndjson(lines, gone)
}

/// Newest first; cursors retain each delivery's original position when read.
async fn delivery_list(State(app): State<App>, Query(q): Query<Page>) -> Reply<Vec<Delivery>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 256);
    let words: Vec<_> =
        q.q.unwrap_or_default()
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
    let rows = app
        .deliveries
        .list(q.session.zip(q.before.map(EntryId)), limit, &words)
        .await?;
    Ok(Json(if q.local || q.before.is_some() {
        rows
    } else {
        app.peers.deliveries(rows, limit, &words).await
    }))
}

async fn delivery_read(
    State(app): State<App>,
    Path((id, entry)): Path<(SessionId, EntryId)>,
    method: Method,
) -> Reply {
    app.deliveries
        .read(&id, entry, method == Method::PUT)
        .await?;
    Ok(Json(json!({})))
}

async fn delivery_archive(
    State(app): State<App>,
    Path((id, entry)): Path<(SessionId, EntryId)>,
    method: Method,
) -> Reply {
    app.deliveries
        .archive(&id, entry, method == Method::PUT)
        .await?;
    Ok(Json(json!({})))
}

const NDJSON: &str = "application/x-ndjson";

/// A channel's lines as a stream.
fn received<T: Send + 'static>(mut lines: mpsc::Receiver<T>) -> impl Stream<Item = T> + Send {
    stream::poll_fn(move |cx| lines.poll_recv(cx))
}

/// Replaces one home file only after its contents and the containing directory are synced.
async fn save(home: &std::path::Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let path = home.join(name);
    let new = path.with_extension("json.new");
    tokio::fs::write(&new, bytes).await?;
    tokio::fs::File::open(&new).await?.sync_all().await?;
    tokio::fs::rename(&new, path).await?;
    tokio::fs::File::open(home).await?.sync_all().await
}
/// One JSON line; `None` only for a value JSON cannot hold (a path that is not UTF-8).
fn line_of(value: &impl Serialize) -> Option<Arc<str>> {
    serde_json::to_string(value).ok().map(Arc::from)
}

/// An NDJSON response of `lines`, which ends when the device that asked is removed.
fn ndjson(lines: impl Stream<Item = Arc<str>> + Send + 'static, mut gone: Gone) -> Response {
    let gone = async move { gone.changed().await };
    let body = (lines.take_until(gone)).map(|line| Ok::<_, Infallible>(format!("{line}\n")));
    ([(header::CONTENT_TYPE, NDJSON)], Body::from_stream(body)).into_response()
}

impl From<crate::devices::Error> for Refusal {
    fn from(e: crate::devices::Error) -> Self {
        let status = match &e {
            crate::devices::Error::Enrollment(status, _) => *status,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Refusal(status, e.to_string())
    }
}
