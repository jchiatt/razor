//! Speech both ways, each through a configured program: `speech` turns one `Text` entry into MP3,
//! kept atomically in the session's folder before serving it with ranges; `transcribe` turns a
//! voice note's audio into the words it is
//! admitted with (docs/design/voice-notes.md). Razor knows no voice, vendor or key.

use super::{App, Refusal, bad, files, other, voice};
use crate::protocol::{Body as Said, Entry, EntryId, Input, SessionId, Spoken};
use crate::shell::Group;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use std::sync::{Mutex, PoisonError};
use std::{collections::BTreeSet, io, os::unix::fs::MetadataExt, process::Stdio, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

/// The most audio one reply may come to: at 64 kbit/s, over half an hour.
const AUDIO: usize = 16 << 20;
/// The most words one voice note may come to: admission's limit for a person's input.
const WORDS: usize = 1 << 20;
/// How long the speech program may take over one reply, and the transcriber beyond the note's
/// own length, which counts up to three hours.
const LIMIT: Duration = Duration::from_secs(300);
/// What a second request for a file already being made or heard is told, and one for a fifth.
const MAKING_REPLY: &str = "This reply's audio is being made; try again.";
const HEARING: &str = "This voice note is still being transcribed.";
const SILENT: &str = "The reply's audio could not be kept; see the server log.";
const NONE: &str = "This server has no transcriber; set `transcribe`.";
const FULL: &str = "Four voice notes and replies are being processed; send it again in a moment.";
/// The files being made or heard now: at most four, each once.
static MAKING: Mutex<BTreeSet<(SessionId, String)>> = Mutex::new(BTreeSet::new());

/// `GET /v1/sessions/{id}/speech/{entry}`: complete audio, whole or one byte range of it.
pub(super) async fn speech(
    State(app): State<App>,
    Path((id, entry)): Path<(SessionId, EntryId)>,
    headers: HeaderMap,
) -> Result<Response, Refusal> {
    let kept = format!("speech/{}.mp3", entry.0);
    if let Ok(file) = app.controller.file(&id, &kept).await {
        return files::serve(file, &headers, "audio/mpeg").await;
    }
    let line = app.controller.get(&id, entry).await?;
    let said: Entry = serde_json::from_str(&line).map_err(other("Reading the reply"))?;
    let Said::Text { text } = said.body else {
        return Err(bad("Only a reply's text can be read aloud."));
    };
    let mut command = program(&app.controller.config().await.speech)?;
    let summary = app.controller.summary(&id).await?;
    let env = voice::ask(&app, None)
        .await
        .map(|g| voice::environment(&g.settings, family(&summary)))
        .unwrap_or_default();
    command.envs(env).arg(speaker(&summary));
    let claim = Claim::new(&id, kept.clone(), MAKING_REPLY)?;
    let controller = app.controller.clone();
    // Dropping the request detaches this task; the paid generation still finishes and is kept.
    let made = tokio::spawn(async move {
        let (session, name) = &claim.0;
        let made = run(command, Stdio::piped(), Some(text), AUDIO, LIMIT).await?;
        if let Err(why) = controller.keep(session, name, made).await {
            eprintln!("razor: speech failed on {session} at {name}: {why}");
        }
        io::Result::Ok(())
    })
    .await;
    let failed = |why: String| Refusal(StatusCode::INTERNAL_SERVER_ERROR, why);
    if let Ok(Err(why)) = made {
        return Err(failed(format!("Couldn't read it aloud: {why}")));
    }
    let file = (app.controller.file(&id, &kept).await).map_err(|_| failed(SILENT.into()))?;
    files::serve(file, &headers, "audio/mpeg").await
}

#[derive(serde::Deserialize)]
pub(super) struct Note {
    text: String,
    #[serde(default)]
    requested: bool,
}
/// `POST /v1/sessions/{id}/voice/{entry}`: `spoken`, for a client.
pub(super) async fn note(
    State(app): State<App>,
    Path((id, entry)): Path<(SessionId, EntryId)>,
    axum::Json(note): axum::Json<Note>,
) -> super::Reply<Spoken> {
    Ok(axum::Json(
        spoken(app, id, entry, note.text, note.requested).await?,
    ))
}

/// A root's recorded message becomes a voice delivery at its call's entry, after synthesis.
pub(super) async fn spoken(
    app: App,
    id: SessionId,
    entry: EntryId,
    text: String,
    requested: bool,
) -> Result<Spoken, Refusal> {
    let summary = app.controller.summary(&id).await?;
    if summary.child.is_some() {
        return Err(bad(
            "Only roots send the person voice notes; write to your parent instead.",
        ));
    }
    if text.contains(['\r', '\n']) {
        return Err(bad("A voice note must fit on one line; write instead."));
    }
    let mut command = program(&app.controller.config().await.speech)?;
    command.arg(speaker(&summary));
    let family = family(&summary);
    let kept = format!("speech/{}.mp3", entry.0);
    let claim = Claim::new(&id, kept.clone(), MAKING_REPLY)?;
    // A disconnected client does not cancel an already charged generation.
    tokio::spawn(async move {
        let _claim = claim;
        let granted = voice::ask(
            &app,
            Some(voice::Reserve {
                characters: text.chars().count() as u32,
                requested,
                seconds: 0.0,
            }),
        )
        .await?;
        let metadata = format!("speech/{}.json", entry.0);
        let path = app.home.join("sessions").join(id.as_str()).join(&metadata);
        let mut env = voice::environment(&granted.settings, family);
        env.push(("RAZOR_WORDS", path.to_string_lossy().into_owned()));
        command.envs(env);
        let made = run(command, Stdio::piped(), Some(text), AUDIO, LIMIT)
            .await
            .map_err(|e| bad(format!("Speech failed: {e}; write instead.")))?;
        let mut bytes = Vec::new();
        // A malformed or missing sidecar only flattens the waveform; retain at most 64 KiB.
        if let Ok(file) = app.controller.file(&id, &metadata).await {
            let _ = file.take(64 << 10).read_to_end(&mut bytes).await;
        }
        #[derive(Default, serde::Deserialize)]
        #[serde(default)]
        struct Metadata {
            duration_ms: u64,
            levels: Vec<u8>,
        }
        let mut meta: Metadata = serde_json::from_slice(&bytes).unwrap_or_default();
        meta.levels.truncate(64);
        app.controller.keep(&id, &kept, made).await?;
        let voice = crate::protocol::Voice {
            file: kept.into(),
            duration_ms: meta.duration_ms,
            levels: meta.levels,
        };
        Ok(Spoken {
            voice,
            sound: granted.sound,
            terminal: granted.terminal,
        })
    })
    .await
    .map_err(other("Making a voice note"))?
}

/// Admits `input`; a voice note only once the `transcribe` program has heard it, its words
/// following the input's text. A task of its own owns the file's claim through admission and
/// the request awaits it, so a client that leaves stops nothing. The claim refuses the same file
/// while its program runs (409), and the request id's repeat check, read before the claim by the
/// input route and again here, answers the first entry while that id is among the session's
/// newest 64. A refusal before admission admitted nothing; an error from admission itself is
/// uncertain, and the client resends with the same request id.
pub(super) async fn admit(app: &App, id: &SessionId, mut input: Input) -> Result<EntryId, Refusal> {
    let Some(voice) = &input.voice else {
        let c = &app.controller;
        return Ok(c.children().admit(c, id, input).await?);
    };
    if voice.levels.len() > 64 {
        return Err(bad("A voice note's waveform holds at most 64 levels."));
    }
    let command = app.controller.config().await.transcribe;
    if command.is_empty() {
        return Err(unavailable(NONE));
    }
    let command = program(&command)?;
    let limit = LIMIT + Duration::from_millis(voice.duration_ms.min(10_800_000));
    // Claimed by the file itself, so no other spelling of its path is a second note.
    let path = voice.file.to_string_lossy();
    let file = app.controller.file(id, &path).await?;
    let found = (file.metadata().await).map_err(other("Reading a note"))?;
    let claim = Claim::new(id, format!("{}:{}", found.dev(), found.ino()), HEARING);
    if let Some(entry) = (app.controller.summary(id).await?).repeat(input.request.as_deref()) {
        return Ok(entry);
    }
    let (claim, controller, id) = (claim?, app.controller.clone(), id.clone());
    let task = tokio::spawn(async move {
        let _claim = claim;
        let heard = run(command, file.into_std().await.into(), None, WORDS, limit).await;
        let why = match heard.map(String::from_utf8) {
            Ok(Ok(words)) if !words.trim().is_empty() => {
                let joint = if input.text.is_empty() { "" } else { "\n" };
                input.text += &format!("{joint}{}", words.trim());
                return Ok(controller.admit(&id, input).await?);
            }
            Ok(Ok(_)) => "no words were heard".into(),
            Ok(Err(_)) => "its words were not UTF-8".into(),
            Err(e) => e.to_string(),
        };
        let why = format!("Couldn't transcribe it: {}.", why.trim_end_matches('.'));
        Err(Refusal(StatusCode::INTERNAL_SERVER_ERROR, why))
    });
    task.await.map_err(other("Transcribing a voice note"))?
}

/// Who speaks: a resident's home session and its threads its name, any other the session's id.
fn speaker(summary: &crate::record::Summary) -> &str {
    let thread = summary.thread.as_ref();
    thread.map_or(&summary.session, |t| &t.resident).as_str()
}

fn family(summary: &crate::record::Summary) -> &'static str {
    crate::protocol::FAMILIES[crate::protocol::family(speaker(summary))]
}

/// Configured programs run in their own process group; an empty speech command refuses.
pub(super) fn program(command: &[String]) -> Result<Command, Refusal> {
    let Some((name, args)) = command.split_first() else {
        return Err(unavailable("No speech program is configured in `speech`."));
    };
    let mut command = Command::new(name);
    command.process_group(0).args(args);
    Ok(command)
}

/// Runs `command` in a process group of its own with `stdin`, writing `text` to it, and drains
/// standard output and standard error together with the writer under `limit`: at most `cap`
/// bytes of output, refused past it rather than cut, and the last 4 KiB of standard error, whose
/// last line goes to the server's log. The group is killed on every way out but a clean exit.
/// Ok only on exit 0 with some output; an error is one sentence: standard error's last line, or
/// how it ended.
pub(super) async fn run(
    mut command: Command,
    stdin: Stdio,
    text: Option<String>,
    cap: usize,
    limit: Duration,
) -> io::Result<Vec<u8>> {
    let mut child = command
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let (mut group, input) = (Group(child.id()), child.stdin.take());
    let (Some(output), Some(mut error)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(io::Error::other("its output could not be read"));
    };
    let (mut out, mut tail) = (Vec::new(), Vec::new());
    let over = format!("it wrote more than {} MiB", cap >> 20);
    let ran = tokio::time::timeout(limit, async {
        let write = async {
            let (Some(mut input), Some(text)) = (input, text) else {
                return Ok(());
            };
            // A program may answer without reading all of it; that is not a failure.
            input.write_all(text.as_bytes()).await.or(Ok(()))
        };
        let read = async {
            let n = output.take(cap as u64 + 1).read_to_end(&mut out).await?;
            (n <= cap).then_some(()).ok_or(io::Error::other(over))
        };
        let errors = async {
            let mut chunk = [0; 4 << 10];
            while let n @ 1.. = error.read(&mut chunk).await? {
                tail.extend_from_slice(&chunk[..n]);
                tail.drain(..tail.len().saturating_sub(4 << 10));
            }
            io::Result::Ok(())
        };
        tokio::try_join!(write, read, errors)?;
        child.wait().await
    })
    .await;
    let tail = String::from_utf8_lossy(&tail);
    let last = tail.lines().map(str::trim).rfind(|l| !l.is_empty());
    last.inspect(|line| eprintln!("razor: {line}"));
    let why = match ran {
        Ok(Ok(status)) if status.success() && !out.is_empty() => {
            group.0 = None;
            return Ok(out);
        }
        Ok(Ok(status)) if status.success() => "it wrote nothing".into(),
        Ok(Ok(status)) => last.map_or(format!("the program ended with {status}"), str::to_owned),
        Ok(Err(e)) => e.to_string(),
        Err(_) => format!("it took longer than {} seconds", limit.as_secs()),
    };
    Err(io::Error::other(why))
}

/// One program at work on one file of one session (a reply's audio by its path, a voice note by
/// its device and inode); dropping it frees the place.
struct Claim((SessionId, String));

impl Claim {
    /// Refuses with `busy` while the file is claimed, and with 429 while four programs run.
    fn new(session: &SessionId, file: String, busy: &str) -> Result<Claim, Refusal> {
        let mut all = MAKING.lock().unwrap_or_else(PoisonError::into_inner);
        let key = (session.clone(), file);
        if all.contains(&key) {
            return Err(Refusal(StatusCode::CONFLICT, busy.into()));
        }
        if all.len() >= 4 {
            return Err(Refusal(StatusCode::TOO_MANY_REQUESTS, FULL.into()));
        }
        all.insert(key.clone());
        Ok(Claim(key))
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        // A panic while the lock is held is already a bug; the set is still whole.
        let mut all = MAKING.lock().unwrap_or_else(PoisonError::into_inner);
        all.remove(&self.0);
    }
}

fn unavailable(sentence: &str) -> Refusal {
    Refusal(StatusCode::SERVICE_UNAVAILABLE, sentence.into())
}
