//! Phone meetings: durable acknowledgments in meetrec's folder, call facts in the session.

// One writer lock per session, taken after a request's body has arrived, serializes uploads with
// completion, so an acknowledgment always names a synced whole-frame prefix or a synced final
// piece. The folder is meetrec's answer, never built here; only call facts and folder files
// survive a restart, and readers and their lag are disposable.

mod reader;

pub(super) use reader::run;

use super::{App, Refusal, devices, files};
use crate::protocol::{BODY_BYTES, Body, CallEvent, CallReason, SessionId, Source};
use crate::record::MeetingState;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Extension, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use base64::Engine;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    path::Path as FilePath,
    process::Stdio,
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    sync::{Mutex, OwnedMutexGuard},
};

type Result<T> = std::result::Result<T, Error>;
type Reply = Result<Json<Value>>;
type At = Path<(SessionId, String)>;

/// A refusal keeps its status and sentence; anything else is the folder or meetrec failing.
enum Error {
    Refused(Refusal),
    Io(std::io::Error),
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<Refusal> for Error {
    fn from(e: Refusal) -> Self {
        Self::Refused(e)
    }
}
impl From<crate::controller::Error> for Error {
    fn from(e: crate::controller::Error) -> Self {
        Self::Refused(e.into())
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(e) => f.write_str(&e.1),
            Self::Io(e) => write!(
                f,
                "The meeting folder or meetrec failed ({e}); check the meeting folder and meetrec, then send again."
            ),
        }
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        match self {
            Self::Refused(e) => e,
            Self::Io(_) => Refusal(StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
        }
        .into_response()
    }
}
fn refused(status: StatusCode, sentence: impl Into<String>) -> Error {
    Error::Refused(Refusal(status, sentence.into()))
}
fn bad(sentence: impl Into<String>) -> Error {
    refused(StatusCode::BAD_REQUEST, sentence)
}

/// Disposable state only: each session's writer lock while held, and each reader's latest lag.
pub(super) struct Kept {
    pub(super) available: bool,
    writers: Mutex<HashMap<SessionId, Weak<Mutex<()>>>>,
    readers: Mutex<HashMap<(SessionId, String), Option<f64>>>,
}

impl Kept {
    pub(super) async fn open() -> Self {
        let ingest = command(&["capabilities"]).await;
        Self {
            available: ingest.is_ok_and(|v| v["commands"].get("ingest").is_some()),
            writers: Mutex::default(),
            readers: Mutex::default(),
        }
    }

    /// The session's writer lock; the map keeps only locks some request or completion holds.
    async fn write(&self, id: &SessionId) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.writers.lock().await;
            locks.retain(|_, lock| lock.strong_count() != 0);
            let lock = locks.get(id).and_then(Weak::upgrade).unwrap_or_default();
            locks.insert(id.clone(), Arc::downgrade(&lock));
            lock
        };
        lock.lock_owned().await
    }
}

pub(super) fn routes() -> Router<App> {
    let at = |tail: &str| format!("/v1/sessions/{{id}}/meetings/{{meeting}}{tail}");
    Router::new()
        .route(&at(""), put(create).get(status).delete(cancel))
        .route(&at("/live"), put(live))
        .route(&at("/pieces/{n}"), put(piece))
        .route(&at("/speakers/{label}"), put(speaker))
        .route(&at("/pauses/{k}"), put(pause))
        .route(&at("/end"), put(end))
        .route(&at("/retry"), post(retry))
        .route(&at("/{file}"), get(output))
}

/// One meetrec command's JSON answer, at most 64 KiB, within 15 s so no request waits longer.
async fn command(args: &[&str]) -> Result<Value> {
    let run = async {
        let mut child = tokio::process::Command::new("meetrec")
            .args(args)
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or(std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
        let value: Value = parse(stdout, 64 << 10).await?;
        if !child.wait().await?.success() || value["ok"] == false {
            return Err(bad(format!(
                "meetrec could not finish: {}; check meetrec on this machine.",
                value["error"]["message"]
                    .as_str()
                    .unwrap_or("the command failed")
            )));
        }
        Ok(value)
    };
    tokio::time::timeout(Duration::from_secs(15), run)
        .await
        .unwrap_or_else(|_| {
            Err(bad(
                "meetrec did not answer; check it on this machine and try again.",
            ))
        })
}

/// JSON from at most `cap` bytes; anything longer is refused, never parsed in part.
async fn parse<T: DeserializeOwned>(from: impl AsyncRead + Unpin, cap: u64) -> Result<T> {
    let mut bytes = Vec::new();
    from.take(cap + 1).read_to_end(&mut bytes).await?;
    if bytes.len() as u64 > cap {
        return Err(bad(
            "A meeting file or meetrec's answer is too large; check the meeting folder.",
        ));
    }
    Ok(serde_json::from_slice(&bytes).map_err(std::io::Error::from)?)
}

/// A folder file of at most 1 MiB, or the default while it does not exist.
async fn read<T: DeserializeOwned + Default>(path: &FilePath) -> Result<T> {
    match tokio::fs::File::open(path).await {
        Ok(file) => parse(file, 1 << 20).await,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e.into()),
    }
}

async fn save(dir: &FilePath, name: &str, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(std::io::Error::from)?;
    if bytes.len() > 1 << 20 {
        return Err(bad(
            "Meeting metadata is at most 1 MiB; send fewer names, pauses or pieces.",
        ));
    }
    Ok(super::save(dir, name, &bytes).await?)
}

async fn fact(app: &App, id: &SessionId, meeting: &str, event: CallEvent) -> Result<()> {
    let call = meeting.into();
    let body = Body::VoiceCall {
        call,
        source: Source::Meeting,
        event,
    };
    app.controller.append(id, body).await?;
    Ok(())
}

/// The live file's audio bytes in whole frames: a crash mid-append can leave half of one.
async fn recorded(folder: &FilePath) -> Result<u64> {
    let len = tokio::fs::metadata(folder.join("room.live.wav"))
        .await?
        .len();
    Ok(len.saturating_sub(44) & !1)
}

fn sha256(bytes: &[u8]) -> aws_lc_rs::digest::Digest {
    aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, bytes)
}

/// A held piece's size and digest, using one 64 KiB buffer regardless of its length.
async fn hash_file(path: &FilePath) -> Result<(u64, String)> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hash = aws_lc_rs::digest::Context::new(&aws_lc_rs::digest::SHA256);
    let mut buffer = vec![0u8; 64 << 10];
    let mut size = 0;
    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        size += n as u64;
    }
    Ok((size, hex(hash.finish().as_ref())))
}

/// A request's hold on a listed meeting: the session's writer lock and the summary's facts.
struct Listed {
    meeting: crate::record::Meeting,
    reply: String,
    _writing: OwnedMutexGuard<()>,
}

/// The session's writer lock and summary, for a session on this machine and a valid id.
async fn enter(
    app: &App,
    id: &SessionId,
    meeting: &str,
) -> Result<(OwnedMutexGuard<()>, crate::record::Summary)> {
    if id.address().is_some() {
        return Err(bad(
            "A meeting goes to the session's own machine; connect to it directly.",
        ));
    }
    super::request(Some(meeting))?;
    let writing = app.meetings.write(id).await;
    Ok((writing, app.controller.summary(id).await?))
}

/// A meeting among the session's newest; the server starts following it if it is open.
async fn listed(app: &App, id: &SessionId, meeting: &str) -> Result<Listed> {
    let (_writing, summary) = enter(app, id, meeting).await?;
    let Some(m) = summary.meetings.into_iter().find(|m| m.id == meeting) else {
        return Err(refused(
            StatusCode::NOT_FOUND,
            "That meeting is no longer listed; open one of the session's recent meetings.",
        ));
    };
    let listed = Listed {
        meeting: m,
        reply: summary.last_reply,
        _writing,
    };
    if listed.meeting.state != MeetingState::Closed {
        reader::ensure(app, id, &listed.meeting.id, &listed.meeting.folder).await;
    }
    Ok(listed)
}

/// The meeting's status after advancing its folder, as every answer carries it.
async fn answer(app: &App, id: &SessionId, m: &Listed) -> Result<Value> {
    let mut status = advance(&m.meeting.folder).await?;
    let readers = app.meetings.readers.lock().await;
    status["lag_s"] = json!(
        readers
            .get(&(id.clone(), m.meeting.id.clone()))
            .copied()
            .flatten()
    );
    status["reply"] = json!(m.reply.chars().take(160).collect::<String>());
    Ok(status)
}

/// The end manifest; the phone's end request is its `live_bytes` and pieces, Razor adds the rest.
#[derive(Serialize, Deserialize, PartialEq)]
struct Manifest {
    #[serde(default)]
    v: u8,
    #[serde(default)]
    rate: Value,
    live_bytes: u64,
    pieces: Vec<Listing>,
}

#[derive(Serialize, Deserialize, PartialEq)]
struct Listing {
    #[serde(default)]
    name: String,
    bytes: u64,
    sha256: String,
}

/// The folder's progress: `meetrec stop` once the live file holds every byte the end declared,
/// and `complete.json` once every listed piece is also held at its listed size and digest.
async fn advance(name: &str) -> Result<Value> {
    let folder = FilePath::new(name);
    let pieces = folder.join("pieces");
    // A failed upload may leave visible bytes or a renamed piece without its sync barrier.
    // Every status acknowledgment re-establishes both barriers before reporting what is held.
    tokio::fs::File::open(folder.join("room.live.wav"))
        .await?
        .sync_all()
        .await?;
    tokio::fs::File::open(&pieces).await?.sync_all().await?;
    let live = recorded(folder).await?;
    let mut held = Vec::new();
    let mut dir = tokio::fs::read_dir(&pieces).await?;
    while let Some(entry) = dir.next_entry().await? {
        let name = entry.file_name();
        let n = name.to_str().and_then(|s| {
            s.strip_prefix("room.")?
                .strip_suffix(".wav")?
                .parse::<usize>()
                .ok()
        });
        held.extend(n);
    }
    held.sort_unstable();
    let expected: Option<Manifest> = read(&pieces.join("expected.json")).await?;
    let session: Value = read(&folder.join("session.json")).await?;
    let done: Value = read(&folder.join("final.json")).await?;
    let missing: Vec<usize> = (expected.iter().flat_map(|e| 0..e.pieces.len()))
        .filter(|n| held.binary_search(n).is_err())
        .collect();
    let canceled = session["phase"] == "canceled";
    if let Some(e) = expected
        .as_ref()
        .filter(|e| e.live_bytes == live && !canceled)
    {
        if session["phase"] == "recording" {
            command(&["stop", name]).await?;
        }
        if missing.is_empty() && !tokio::fs::try_exists(pieces.join("complete.json")).await? {
            for p in &e.pieces {
                verify_piece(&pieces, p).await?;
            }
            save(&pieces, "complete.json", e).await?;
        }
    }
    let phase = match &expected {
        _ if canceled => "canceled",
        _ if done["ok"] == true => "ready",
        _ if !done.is_null() => "failed",
        None => "recording",
        Some(e) if live < e.live_bytes => "stopping",
        Some(_) if !missing.is_empty() => "uploading",
        Some(_) => "transcribing",
    };
    Ok(json!({"phase": phase, "live": live, "pieces": held, "missing": missing, "final": done}))
}

/// Check held bytes before either accepting a new manifest or publishing its completion.
async fn verify_piece(dir: &FilePath, piece: &Listing) -> Result<()> {
    if hash_file(&dir.join(&piece.name)).await? != (piece.bytes, piece.sha256.clone()) {
        return Err(bad(
            "A held piece does not match the end manifest; correct its size or digest and send the end again.",
        ));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Deserialize)]
struct Start {
    started_at: String,
    rate: u32,
    #[serde(default)]
    attendees: Vec<String>,
}

async fn create(
    State(app): State<App>,
    Path((id, meeting)): At,
    Extension(devices::Enrolled(device)): Extension<devices::Enrolled>,
    Json(start): Json<Start>,
) -> Reply {
    let (writing, summary) = enter(&app, &id, &meeting).await?;
    // A repeat answers with the meeting it made; a new one is listed once its start is recorded.
    if !summary.meetings.iter().any(|m| m.id == meeting) {
        if !app.meetings.available {
            return Err(bad(
                "This machine cannot take meetings; install meetrec with ingest support.",
            ));
        }
        if summary
            .meetings
            .iter()
            .any(|m| m.state != MeetingState::Closed)
        {
            return Err(refused(
                StatusCode::CONFLICT,
                "This chat already has an open meeting; finish or discard it first.",
            ));
        }
        let dir = summary
            .child
            .as_ref()
            .and_then(|c| c.worktree.as_ref())
            .map_or(&summary.cwd, |w| &w.origin);
        let dir = std::path::absolute(dir)?;
        let dir = dir.to_string_lossy();
        let (owner, rate) = (format!("razor:{id}"), start.rate.to_string());
        let mut args = vec![
            "start",
            "--ingest",
            "--dir",
            &dir,
            "--owner",
            &owner,
            "--request",
            &meeting,
            "--name",
            &summary.name,
        ];
        args.extend(["--rate", &rate, "--started-at", &start.started_at]);
        for name in &start.attendees {
            args.extend(["--attendee", name]);
        }
        let started = command(&args).await?;
        let Some(folder) = started["session"]["path"].as_str() else {
            return Err(bad(
                "meetrec returned no meeting folder; check its version.",
            ));
        };
        let folder = Some(folder.into());
        fact(&app, &id, &meeting, CallEvent::Started { folder, device }).await?;
    }
    drop(writing);
    status(State(app), Path((id, meeting))).await
}

async fn status(State(app): State<App>, Path((id, meeting)): At) -> Reply {
    let m = listed(&app, &id, &meeting).await?;
    Ok(Json(answer(&app, &id, &m).await?))
}

#[derive(Deserialize)]
struct Offset {
    offset: u64,
}

async fn live(
    State(app): State<App>,
    Path((id, meeting)): At,
    Query(at): Query<Offset>,
    body: Bytes,
) -> Reply {
    let m = listed(&app, &id, &meeting).await?;
    if at.offset % 2 != 0 || !body.len().is_multiple_of(2) {
        return Err(bad(
            "Live audio needs whole PCM16 frames; resend an even byte offset and length.",
        ));
    }
    let folder = FilePath::new(&m.meeting.folder);
    let end: Option<Manifest> = read(&folder.join("pieces/expected.json")).await?;
    if end.is_some_and(|e| at.offset.saturating_add(body.len() as u64) > e.live_bytes) {
        return Err(bad(
            "This audio runs past the meeting's end; resend only its declared live bytes.",
        ));
    }
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(folder.join("room.live.wav"))
        .await?;
    let held = recorded(folder).await?;
    if file.metadata().await?.len() > 44 + held {
        file.set_len(44 + held).await?;
    }
    if at.offset <= held {
        file.write_all(&body[(held - at.offset).min(body.len() as u64) as usize..])
            .await?;
    }
    file.flush().await?;
    file.sync_all().await?;
    Ok(Json(answer(&app, &id, &m).await?))
}

async fn piece(
    State(app): State<App>,
    Path((id, meeting, n)): Path<(SessionId, String, u16)>,
    headers: HeaderMap,
    body: Bytes,
) -> Reply {
    let m = listed(&app, &id, &meeting).await?;
    let sent = (headers.get("content-digest").and_then(|h| h.to_str().ok()))
        .and_then(|s| s.strip_prefix("sha-256=:")?.strip_suffix(':'));
    let digest = sha256(&body);
    if sent != Some(&base64::engine::general_purpose::STANDARD.encode(digest.as_ref())) {
        return Err(bad(
            "The piece did not match Content-Digest; send it again.",
        ));
    }
    let pieces = FilePath::new(&m.meeting.folder).join("pieces");
    let name = pieces.join(format!("room.{n:04}.wav"));
    if !tokio::fs::try_exists(&name).await? {
        let expected: Option<Manifest> = read(&pieces.join("expected.json")).await?;
        if let Some(expected) = expected {
            let matches = expected
                .pieces
                .get(usize::from(n))
                .is_some_and(|p| p.bytes == body.len() as u64 && p.sha256 == hex(digest.as_ref()));
            if !matches {
                return Err(bad(
                    "This piece differs from the end manifest; resend the declared piece.",
                ));
            }
        }
        let part = pieces.join(format!(".room.{n:04}.wav.part"));
        let mut file = tokio::fs::File::create(&part).await?;
        file.write_all(&body).await?;
        file.sync_all().await?;
        tokio::fs::rename(part, name).await?;
    } else if hash_file(&name).await? != (body.len() as u64, hex(digest.as_ref())) {
        return Err(refused(
            StatusCode::CONFLICT,
            "That piece already holds different audio; resend the original piece.",
        ));
    }
    let mut status = answer(&app, &id, &m).await?;
    (status["piece"], status["bytes"]) = (json!(n), json!(body.len()));
    Ok(Json(status))
}

#[derive(Deserialize)]
struct Name {
    name: String,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Speakers {
    v: u8,
    attendees: Vec<String>,
    names: BTreeMap<String, String>,
}

async fn speaker(
    State(app): State<App>,
    Path((id, meeting, label)): Path<(SessionId, String, String)>,
    Json(Name { name }): Json<Name>,
) -> Reply {
    let m = listed(&app, &id, &meeting).await?;
    if name.trim().is_empty() || name.len() > 512 {
        return Err(bad("Use a nonempty speaker name of at most 512 bytes."));
    }
    let folder = FilePath::new(&m.meeting.folder);
    let mut speakers: Speakers = read(&folder.join("speakers.json")).await?;
    if speakers.names.get(&label) != Some(&name) {
        // The fact goes first: a failed save is sent again, and a repeated fact names again.
        let event = CallEvent::Named {
            label: label.clone(),
            name: name.clone(),
        };
        fact(&app, &id, &meeting, event).await?;
        speakers.v = 1;
        speakers.names.insert(label, name);
        save(folder, "speakers.json", &speakers).await?;
    }
    let mut status = answer(&app, &id, &m).await?;
    status["speakers"] = json!(speakers);
    Ok(Json(status))
}

/// One interruption in `pauses.json`; the phone sends only its times.
#[derive(Serialize, Deserialize, PartialEq)]
struct Pause {
    #[serde(default)]
    k: u64,
    at_s: f64,
    missing_s: f64,
    #[serde(default)]
    reason: String,
}

async fn pause(
    State(app): State<App>,
    Path((id, meeting, k)): Path<(SessionId, String, u64)>,
    Json(mut pause): Json<Pause>,
) -> Reply {
    let m = listed(&app, &id, &meeting).await?;
    if pause.at_s < 0.0 || pause.missing_s < 0.0 {
        return Err(bad("A pause needs nonnegative at_s and missing_s seconds."));
    }
    (pause.k, pause.reason) = (k, "interruption".into());
    let folder = FilePath::new(&m.meeting.folder);
    let mut pauses: Vec<Pause> = read(&folder.join("pauses.json")).await?;
    match pauses.iter().find(|p| p.k == k) {
        Some(old) if *old != pause => {
            return Err(bad(
                "That pause already has different times; send its original times.",
            ));
        }
        Some(_) => {}
        None => {
            pauses.push(pause);
            save(folder, "pauses.json", &pauses).await?;
        }
    }
    let mut status = answer(&app, &id, &m).await?;
    status["pauses"] = json!(pauses);
    Ok(Json(status))
}

async fn end(
    State(app): State<App>,
    Path((id, meeting)): At,
    Json(mut end): Json<Manifest>,
) -> Reply {
    let m = listed(&app, &id, &meeting).await?;
    if m.meeting.state == MeetingState::Closed {
        return Ok(Json(answer(&app, &id, &m).await?));
    }
    let digest =
        |s: &str| s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if end.live_bytes % 2 != 0
        || (end.pieces.iter()).any(|p| p.bytes > BODY_BYTES as u64 || !digest(&p.sha256))
    {
        return Err(bad(
            "The end needs an even live_bytes and each piece's byte count and lowercase SHA-256 digest.",
        ));
    }
    let folder = FilePath::new(&m.meeting.folder);
    if end.live_bytes < recorded(folder).await? {
        return Err(bad(
            "The end omits live bytes already held; resend the full live byte count.",
        ));
    }
    let clock: Value = read(&folder.join("clock.json")).await?;
    (end.v, end.rate) = (1, clock["tracks"]["room"]["sample_rate"].clone());
    for (n, p) in end.pieces.iter_mut().enumerate() {
        p.name = format!("room.{n:04}.wav");
    }
    let pieces = folder.join("pieces");
    for piece in &end.pieces {
        if tokio::fs::try_exists(pieces.join(&piece.name)).await? {
            verify_piece(&pieces, piece).await?;
        }
    }
    match read::<Option<Manifest>>(&pieces.join("expected.json")).await? {
        None => save(&pieces, "expected.json", &end).await?,
        Some(old) if old != end => {
            return Err(bad(
                "This meeting already has a different end; resend its original manifest.",
            ));
        }
        Some(_) => {}
    }
    if m.meeting.state == MeetingState::Recording {
        fact(
            &app,
            &id,
            &meeting,
            CallEvent::Ended {
                reason: CallReason::Ended,
            },
        )
        .await?;
    }
    Ok(Json(answer(&app, &id, &m).await?))
}

async fn cancel(State(app): State<App>, Path((id, meeting)): At) -> Reply {
    let m = listed(&app, &id, &meeting).await?;
    if m.meeting.state == MeetingState::Closed {
        return Ok(Json(answer(&app, &id, &m).await?));
    }
    let session: Value = read(&FilePath::new(&m.meeting.folder).join("session.json")).await?;
    if session["phase"] != "canceled" {
        command(&["cancel", &m.meeting.folder]).await?;
    }
    fact(
        &app,
        &id,
        &meeting,
        CallEvent::Ended {
            reason: CallReason::Canceled,
        },
    )
    .await?;
    Ok(Json(answer(&app, &id, &m).await?))
}

async fn retry(State(app): State<App>, Path((id, meeting)): At) -> Reply {
    let m = listed(&app, &id, &meeting).await?;
    let done: Value = read(&FilePath::new(&m.meeting.folder).join("final.json")).await?;
    if done["ok"] == false {
        command(&["ingest", &m.meeting.folder, "--retry"]).await?;
    }
    Ok(Json(answer(&app, &id, &m).await?))
}

async fn output(
    State(app): State<App>,
    Path((id, meeting, file)): Path<(SessionId, String, String)>,
    headers: HeaderMap,
) -> Result<Response> {
    let (name, kind) = match file.as_str() {
        "transcript" => ("transcript.json", "application/json"),
        "audio" => ("room.wav", "audio/wav"),
        _ => {
            return Err(refused(
                StatusCode::NOT_FOUND,
                "There is no such meeting file; use transcript or audio.",
            ));
        }
    };
    let m = listed(&app, &id, &meeting).await?;
    let opened = tokio::fs::File::open(FilePath::new(&m.meeting.folder).join(name)).await;
    let opened = opened.map_err(|e| {
        refused(
            StatusCode::NOT_FOUND,
            format!("The meeting's {file} is not ready: {e}; try again when it is."),
        )
    })?;
    Ok(files::serve(opened, &headers, kind).await?)
}
