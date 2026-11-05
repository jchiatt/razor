//! Voice notes and reading aloud. One note and one player at a time run this machine's `capture`
//! and `play` programs in the terminal's own process group, so a hangup reaches them; each is
//! killed and reaped before the next starts, and their audio stays in one private folder removed
//! at quit. A note sits above the composer, which stays the person's, and reaches the transcript
//! only through the stream, once the server admits it.
use super::{App, Done, Error, error, fail, inputs, text::*, transcript, view::Action, view::Hit};
use crate::config::Config;
use crate::protocol::{Body, Entry, EntryId, Input, SessionId, State, Voice, now};
use crate::terminal::bindings;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{buffer::Buffer, layout::Rect, text::Line};
use reqwest::Method;
use serde_json::json;
use std::sync::{LazyLock, atomic::AtomicU64, atomic::Ordering::Relaxed};
use std::{future::Future, path::PathBuf, process::Stdio};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::{process::Command, sync::mpsc::Sender, sync::oneshot, task::JoinHandle, time::*};

/// 100 ms of 16 kHz mono 16-bit PCM; a note keeps at most ten minutes of them.
const WINDOW: usize = 3200;
const SILENT: &str = "The microphone gave no sound; check its device and permission.";
const STOPPED: &str = "The microphone program stopped; check `capture` and try again.";
const SAVING: &str = "Local audio could not be saved; check the temporary folder.";
const INVALID: &str = "The server's answer is invalid; reconnect to a matching server.";
const QUIT: &str = " QUITTING WILL DISCARD AUDIO. <composer.back> stays.";
/// This process and its start, which name its private audio folder and its notes' request ids.
pub(super) static RUN: LazyLock<String> =
    LazyLock::new(|| format!("{}-{}", std::process::id(), now()));

/// At most one note and one player; the newest program's task, which the next awaits so one
/// program runs at a time; and whether this machine's `capture` was set when last read.
#[derive(Default)]
pub struct Audio {
    home: PathBuf,
    tx: Option<Sender<Done>>,
    capture: bool,
    pub(super) play: bool,
    pub(super) reported: Option<Instant>,
    note: Option<Note>,
    player: Option<Player>,
    last: Option<JoinHandle<()>>,
    next: u64,
}

struct Note {
    id: u64,
    phase: Phase,
    // Enter sends `true` to the capture, Esc `false`; dropping it stops the capture too.
    stop: Option<oneshot::Sender<bool>>,
    file: Option<Temp>,
    voice: Voice,
}

#[derive(PartialEq)]
enum Phase {
    Recording,
    Held,
    Sending,
    Failed(String),
}

struct Player {
    id: u64,
    target: (SessionId, EntryId),
    speech: bool,
    // Ready to hear once its first file is here; why it failed, kept for Enter to try again.
    state: Result<bool, String>,
    // Dropping it kills and reaps the program.
    _stop: oneshot::Sender<bool>,
}

/// A private audio file, removed when dropped.
struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        tokio::spawn(tokio::fs::remove_file(std::mem::take(&mut self.0)));
    }
}

/// What one sending step's answer made: the session, the uploaded file or the admitted entry.
#[derive(serde::Deserialize)]
struct Made {
    session: Option<SessionId>,
    path: Option<PathBuf>,
    entry: Option<EntryId>,
}

/// Gives the loop's audio its home and channel, and reads this machine's settings beside it.
pub fn start(app: &mut App, home: PathBuf, tx: Sender<Done>) {
    (app.audio.home, app.audio.tx) = (home.clone(), Some(tx.clone()));
    tokio::spawn(async move { settings(home, &tx).await.ok() });
}

// This machine's own settings, read beside the loop; a project's file never names argv here.
async fn settings(home: PathBuf, tx: &Sender<Done>) -> Result<Config, Error> {
    let unread = "This machine's settings could not be read; check its config.";
    let read = tokio::task::spawn_blocking(move || crate::config::Settings::open(home));
    let read = read.await.map_err(fail(unread))?;
    let config = read.map_err(fail(unread))?.config;
    let (capture, play) = (!config.capture.is_empty(), !config.play.is_empty());
    let _ = tx
        .send(Box::new(move |app| {
            (app.audio.capture, app.audio.play) = (capture, play)
        }))
        .await;
    Ok(config)
}

// Why this terminal cannot record for the addressed session, whatever `capture` says.
fn unavailable(app: &App) -> Option<&'static str> {
    let local = || app.servers.iter().find(|s| s.name.is_empty());
    let addressed = |id| super::status::server(app, id);
    let server = app.focus.as_ref().map_or_else(local, addressed);
    let ssh = "Recording is unavailable over SSH; open Razor on the microphone's Mac.";
    let none = "This server has no transcriber; set `transcribe`.";
    let unanswered = "The server has not answered yet; try again in a moment.";
    match std::env::var_os("SSH_CONNECTION") {
        Some(_) => Some(ssh),
        // Every status line names a server: none yet is a status that has not arrived.
        None if app.servers.is_empty() => Some(unanswered),
        None => (!server.is_some_and(|s| s.transcribe)).then_some(none),
    }
}

/// Whether Voice shows: this machine can record for the addressed session.
pub fn available(app: &App) -> bool {
    app.audio.capture && unavailable(app).is_none()
}

/// Whether a note is recording, held or being sent, which keeps its session addressed.
pub fn unsent(app: &App) -> bool {
    app.audio.note.is_some()
}

/// Starts a note for the addressed session, stopping the player; the editor's words, images and
/// answer stay as they are. A held note must be sent or discarded first.
pub fn record(app: &mut App) {
    let waiting = "A note or input is waiting; send or discard it before recording again.";
    if app.audio.note.is_some() || app.sending.is_some() {
        return app.say(waiting);
    }
    if let Some(why) = unavailable(app) {
        return app.say(why);
    }
    app.audio.player = None;
    let (stop, rx) = oneshot::channel();
    let work = |config: Config, tx, id| async move { capture(&config.capture, id, &tx, rx).await };
    let id = program(app, work, captured);
    let voice = Voice {
        file: PathBuf::new(),
        duration_ms: 0,
        levels: Vec::new(),
    };
    let note = Note {
        id,
        phase: Phase::Recording,
        stop: Some(stop),
        file: None,
        voice,
    };
    app.audio.note = Some(note);
}

/// Takes Ctrl-R, and Enter and Esc while a note or the player owns them, ahead of answers and
/// Esc's stop: a recording note takes Enter to send and Esc to hold it for review.
pub fn captures(app: &App, k: KeyEvent) -> bool {
    matches!(k.code, KeyCode::Enter | KeyCode::Esc)
        && (app.audio.note.is_some()
            || app
                .audio
                .player
                .as_ref()
                .is_some_and(|p| k.code == KeyCode::Esc || p.state.is_err()))
}

pub fn key(app: &mut App, k: KeyEvent) -> bool {
    let (enter, esc) = (k.code == KeyCode::Enter, k.code == KeyCode::Esc);
    if k.code == KeyCode::Char('r') && k.modifiers == KeyModifiers::CONTROL {
        record(app);
    } else if let Some(note) = app.audio.note.as_mut().filter(|_| enter || esc) {
        match note.phase {
            Phase::Recording => {
                let _ = note.stop.take().map(|stop| stop.send(enter));
            }
            Phase::Sending if esc => {
                app.audio.note = None;
                app.say("The voice note may still arrive.");
            }
            Phase::Sending => {}
            _ if esc => app.audio.note = None,
            _ if note.file.is_some() => send(app),
            _ => {
                app.audio.note = None;
                record(app);
            }
        }
    } else if let Some(p) = app.audio.player.as_ref().filter(|_| enter || esc) {
        let ((session, entry), failed) = (p.target.clone(), p.state.is_err());
        match esc {
            true => app.audio.player = None,
            false if failed => play(app, session, entry),
            false => return false,
        }
    } else {
        return false;
    }
    true
}

// Runs `work` with this machine's settings read again, once the newest program's task has ended
// and so reaped its program; `done` applies the outcome to the note or player `id`, if it is
// still there. Returns `id`.
fn program<T: Send + 'static, F: Future<Output = Result<T, Error>> + Send>(
    app: &mut App,
    work: impl FnOnce(Config, Sender<Done>, u64) -> F + Send + 'static,
    done: fn(&mut App, u64, Result<T, Error>),
) -> u64 {
    app.audio.next += 1;
    let (id, home, tx) = (app.audio.next, app.audio.home.clone(), app.audio.tx.clone());
    let last = app.audio.last.take();
    app.audio.last = Some(tokio::spawn(async move {
        let Some(tx) = tx else { return };
        let _ = futures_util::future::OptionFuture::from(last).await;
        let result = async { work(settings(home, &tx).await?, tx.clone(), id).await }.await;
        let _ = tx.send(Box::new(move |app| done(app, id, result))).await;
    }));
    id
}

// `argv` without a shell, in the terminal's process group, killed if dropped; `unset` says why an
// empty one cannot run.
fn command(argv: &[String], unset: &str) -> Result<Command, Error> {
    let (name, args) = argv.split_first().ok_or_else(|| error(unset))?;
    let mut command = Command::new(name);
    command.args(args).kill_on_drop(true).stdin(Stdio::null());
    command.stdout(Stdio::null()).stderr(Stdio::null());
    Ok(command)
}

// Records until Enter (`true`), Esc or a dropped note (`false`), or ten minutes of PCM; the WAV
// keeps every whole sample read, a partial last window included. Two seconds with no samples, or
// of exact zeros, is a microphone that gave nothing. The program is reaped on every way out.
async fn capture(
    argv: &[String],
    id: u64,
    tx: &Sender<Done>,
    mut stop: oneshot::Receiver<bool>,
) -> Result<(Temp, u64, Vec<u8>, bool), Error> {
    let mut program = command(argv, "No local capture program is set.")?;
    let start = fail("The microphone program could not start; check `capture`.");
    let mut child = program.stdout(Stdio::piped()).spawn().map_err(start)?;
    let pcm = child.stdout.take();
    let captured = async {
        let (mut pcm, (temp, mut file)) = (pcm.ok_or_else(|| error(STOPPED))?, temp().await?);
        file.write_all(&[0; 44]).await.map_err(fail(SAVING))?;
        let (mut window, mut filled, mut levels, mut zeros) = ([0; WINDOW], 0, Vec::new(), 0);
        let mut send = false;
        while levels.len() < 6000 {
            let read = pcm.read(&mut window[filled..]);
            let read = tokio::time::timeout(Duration::from_secs(2), read);
            filled += tokio::select! {
                sent = &mut stop => { send = sent.unwrap_or(false); break }
                read = read => match read.map_err(fail(SILENT))?.map_err(fail(STOPPED))? {
                    0 => return Err(error(STOPPED)),
                    n => n,
                },
            };
            if filled < WINDOW {
                continue;
            }
            filled = 0;
            zeros = (zeros + 1) * usize::from(window.iter().all(|b| *b == 0));
            if zeros == 20 {
                return Err(error(SILENT));
            }
            levels.push(level(&window));
            file.write_all(&window).await.map_err(fail(SAVING))?;
            let recent = levels[levels.len().saturating_sub(48)..].to_vec();
            let ms = levels.len() as u64 * 100;
            let _ = tx.try_send(Box::new(move |app| {
                if let Some(n) = app.audio.note.as_mut().filter(|n| n.id == id) {
                    (n.voice.duration_ms, n.voice.levels) = (ms, recent);
                }
            }));
        }
        let tail = &window[..filled & !1];
        let size = (levels.len() * WINDOW + tail.len()) as u32;
        levels.extend((!tail.is_empty()).then(|| level(tail)));
        if levels.is_empty() {
            return Err(error(SILENT));
        }
        let mut wav = *b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x80\x3e\0\0\0\x7d\0\0\x02\0\x10\0data\0\0\0\0";
        wav[4..8].copy_from_slice(&(size + 36).to_le_bytes());
        wav[40..].copy_from_slice(&size.to_le_bytes());
        file.write_all(tail).await.map_err(fail(SAVING))?;
        file.rewind().await.map_err(fail(SAVING))?;
        file.write_all(&wav).await.map_err(fail(SAVING))?;
        file.flush().await.map_err(fail(SAVING))?;
        Ok((temp, u64::from(size) / 32, resample(&levels, 48), send))
    };
    let captured = captured.await;
    let _ = child.kill().await;
    captured
}

// A recorded note is held for review, or sent at once after Enter; a failed one holds its why.
fn captured(app: &mut App, id: u64, result: Result<(Temp, u64, Vec<u8>, bool), Error>) {
    if let Some(note) = app.audio.note.as_mut().filter(|n| n.id == id) {
        match result {
            Ok((file, ms, levels, send)) => {
                (note.phase, note.file) = (Phase::Held, Some(file));
                (note.voice.duration_ms, note.voice.levels) = (ms, levels);
                if send {
                    self::send(app);
                }
            }
            Err(e) => note.phase = Phase::Failed(e.to_string()),
        }
    }
}

// Takes the held note its next step, each one request whose result is kept so Enter resumes
// there after a refusal: the session when there is none, the note's file, uploaded once, then
// its admission, asked again with the same request id and file until the server answers.
fn send(app: &mut App) {
    let working = app.row().is_some_and(|a| a.state == State::Working);
    let mut input = app.initial.clone().unwrap_or_default();
    let session = json!({"cwd": app.cwd, "resident": app.resident, "model": input.model, "effort": input.effort, "speed": input.speed});
    let (focus, Some(note)) = (app.focus.clone(), app.audio.note.as_ref()) else {
        return;
    };
    (input.text, input.images) = Default::default();
    input.request = Some(format!("voice-{}-{}", *RUN, note.id));
    input.voice = Some(note.voice.clone());
    let id = note.id;
    let wav = note.file.as_ref().map(|f| f.0.clone()).unwrap_or_default();
    let uploaded = !note.voice.file.as_os_str().is_empty();
    // The server transcribes for the note's length, at most three hours, and five minutes more.
    let bound = Duration::from_millis(note.voice.duration_ms.min(10_800_000) + 330_000);
    let until = Instant::now() + bound * u32::from(uploaded);
    let (path, json) = match focus {
        None => (inputs::creation(app), Some(session)),
        Some(id) if uploaded => (format!("v1/sessions/{id}/input"), Some(json!(input))),
        Some(id) => (format!("v1/sessions/{id}/files/voice.wav"), None),
    };
    let kind = ["audio/wav", "application/json"][usize::from(json.is_some())];
    let sending = app.spawn(move |api| async move {
        let made: Result<Made, _> = async {
            let unread = fail("The recording could not be read; record the note again.");
            let body = match json {
                Some(json) => json.to_string().into_bytes(),
                None => tokio::fs::read(wav).await.map_err(unread)?,
            };
            let mut answer = Vec::new();
            let posted = api.audio(Method::POST, &path, Some((kind, body)), until, &mut answer);
            posted.await?;
            serde_json::from_slice(&answer).map_err(fail(INVALID))
        }
        .await;
        move |app: &mut App| sent(app, id, made, working)
    });
    if let Some(note) = app.audio.note.as_mut().filter(|_| sending) {
        note.phase = Phase::Sending;
    }
}

// Keeps what a step made and takes the next: the session, now addressed; the uploaded file; or
// the admitted entry, queued as a typed input is, which ends the note. A refusal holds the note.
fn sent(app: &mut App, id: u64, made: Result<Made, Error>, working: bool) {
    if let Some(note) = app.audio.note.as_mut().filter(|n| n.id == id) {
        match made.map(|m| (m.entry, m.path, m.session)) {
            Ok((Some(entry), ..)) => {
                if let Some(session) = app.focus.clone().filter(|_| working) {
                    inputs::queue(app, &session, entry);
                }
                (app.audio.note, app.initial) = (None, None);
            }
            Ok((None, Some(path), _)) => {
                note.voice.file = path;
                send(app);
            }
            Ok((None, None, Some(session))) => {
                let session = session.on(&app.server);
                (app.root, app.focus) = (Some(session.clone()), Some(session.clone()));
                app.stream(session, 0);
                app.save();
                send(app);
            }
            Ok(_) => note.phase = Phase::Failed(INVALID.into()),
            Err(e) => note.phase = Phase::Failed(e.to_string()),
        }
    }
}

/// Reads the addressed session's newest completed reply aloud.
pub fn listen(app: &mut App) {
    let newest = |s: SessionId| Some((*reply(app, &s).first()?, s));
    match app.focus.clone().and_then(newest) {
        Some((entry, session)) => play(app, session, entry),
        None => app.say("There is no completed reply to read aloud yet."),
    }
}

/// Plays a saved note, or reads the newest completed reply aloud, in place of what was playing;
/// the same again stops it. Recorded inputs play at their own speed, agent speech at 1.2.
pub fn play(app: &mut App, session: SessionId, entry: EntryId) {
    if app.audio.note.as_ref().is_some_and(|n| n.stop.is_some()) {
        return app.say("Stop the recording before playing audio.");
    }
    let target = (session, entry);
    let player = app.audio.player.take();
    if player.is_some_and(|p| p.target == target && p.state.is_ok()) {
        return;
    }
    let transcript = app.transcripts.get(&target.0);
    let body = transcript.and_then(|t| t.entries.get(&entry));
    let delivery = app
        .delivered
        .iter()
        .find(|d| (&d.session, d.entry) == (&target.0, entry));
    let delivery = delivery.filter(|d| d.voice.is_some());
    let note = match body.map(|e| &e.body) {
        Some(Body::Input(Input { voice: Some(v), .. })) => Some(v.file.display().to_string()),
        _ => delivery
            .and_then(|d| d.voice.as_ref())
            .map(|v| v.file.display().to_string()),
    };
    let (speech, base) = (
        note.is_none() || delivery.is_some(),
        format!("v1/sessions/{}/", target.0),
    );
    let read = delivery.map(|_| format!("{base}deliveries/{}/read", entry.0));
    let spoken = |e: EntryId| format!("{base}speech/{}", e.0);
    let files: Vec<_> = match note {
        Some(file) => vec![format!("{base}files/{file}")],
        None => reply(app, &target.0).into_iter().map(spoken).collect(),
    };
    let Some(api) = app.api.clone().filter(|_| !files.is_empty()) else {
        return app.say("There is no completed reply to read aloud yet.");
    };
    let (stop, mut rx) = oneshot::channel();
    let rate = ["1.0", "1.2"][usize::from(speech)];
    let work = move |config: Config, tx: Sender<Done>, id| async move {
        let rated = |a: &String| a.replace("{rate}", rate);
        let argv: Vec<_> = config.play.iter().map(rated).collect();
        for file in files {
            let (temp, mut audio) = temp().await?;
            let until = Instant::now() + Duration::from_secs(330);
            let got = api.audio(Method::GET, &file, None, until, &mut audio);
            tokio::select! { _ = &mut rx => return Ok(false), got = got => got? }
            audio.flush().await.map_err(fail(SAVING))?;
            let ready: Done = Box::new(move |app| played(app, id, Ok(true)));
            let _ = tx.send(ready).await;
            let mut program = command(&argv, "No local play program is set.")?;
            let start = fail("The local player could not start; check `play`.");
            let mut child = program.arg(&temp.0).spawn().map_err(start)?;
            let failed = "The local player failed; check `play`.";
            tokio::select! {
                _ = &mut rx => { let _ = child.kill().await; return Ok(false) }
                ended = child.wait() => if !ended.is_ok_and(|s| s.success()) {
                    return Err(error(failed));
                }
            }
        }
        if let Some(path) = read {
            api.json(Method::PUT, &path, None).await?;
        }
        Ok(false)
    };
    let id = program(app, work, played);
    let player = Player {
        id,
        target,
        speech,
        state: Ok(false),
        _stop: stop,
    };
    app.audio.player = Some(player);
}

// The player `id`'s news: its first file ready to hear (`Ok(true)`), its end (`Ok(false)`), which
// drops it, or its failure, kept for Enter to try again.
fn played(app: &mut App, id: u64, news: Result<bool, Error>) {
    if let Some(p) = app.audio.player.as_mut().filter(|p| p.id == id) {
        match news {
            Ok(false) => app.audio.player = None,
            news => p.state = news.map_err(|e| e.to_string()),
        }
    }
}

// The newest completed reply's text entries in order: those before the last turn end, back to
// the first entry that is neither text nor thinking.
fn reply(app: &App, session: &SessionId) -> Vec<EntryId> {
    let text = |e: &&Entry| matches!(e.body, Body::Text { .. });
    let thinking = |e: &&Entry| matches!(e.body, Body::Thinking { .. });
    let ended = |e: &&Entry| !matches!(e.body, Body::TurnEnd { .. });
    let entries = app.transcripts.get(session).into_iter();
    let entries = entries.flat_map(|t| t.entries.values().rev().skip_while(ended).skip(1));
    let words = entries.skip_while(|e| !text(e));
    let words = words.take_while(|e| text(e) || thinking(e));
    let mut group: Vec<_> = words.filter(text).map(|e| e.id).collect();
    group.reverse();
    group
}

/// Stops the note and the player, waits until their programs are reaped, and removes the private
/// folder; what the server holds goes on.
pub async fn close(app: &mut App) {
    if app.audio.reported.is_some()
        && let Some(api) = &app.api
    {
        let body = json!({"terminal": *RUN, "active": false});
        let _ = timeout(
            Duration::from_secs(1),
            api.json(Method::PUT, "v1/voice/activity", Some(body)),
        )
        .await;
    }
    (app.audio.note, app.audio.player) = (None, None);
    let _ = futures_util::future::OptionFuture::from(app.audio.last.take()).await;
    let _ = tokio::fs::remove_dir_all(folder()).await;
}

fn folder() -> PathBuf {
    std::env::temp_dir().join(format!("razor-audio-{}", *RUN))
}

// A new file in this terminal's private audio folder, which only its owner can read.
async fn temp() -> Result<(Temp, tokio::fs::File), Error> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut dir = tokio::fs::DirBuilder::new();
    let made = dir.recursive(true).mode(0o700).create(folder()).await;
    made.map_err(fail(SAVING))?;
    let path = folder().join(NEXT.fetch_add(1, Relaxed).to_string());
    let mut open = tokio::fs::OpenOptions::new();
    open.write(true).create_new(true).mode(0o600);
    let file = open.open(&path).await;
    Ok((Temp(path), file.map_err(fail(SAVING))?))
}

// A window's loudness as the phone measures it: its RMS from −55 to −15 dBFS onto 0 to 255.
fn level(pcm: &[u8]) -> u8 {
    let square = |b: &[u8; 2]| (f64::from(i16::from_le_bytes(*b)) / 32768.).powi(2);
    let sum: f64 = pcm.as_chunks::<2>().0.iter().map(square).sum();
    let db = 20. * ((sum / (pcm.len() / 2) as f64).sqrt() + 1e-7).log10();
    (255. * ((db + 55.) / 40.).clamp(0., 1.)) as u8
}

// Measured levels averaged into `n` equal-time cells; fewer repeat, and none draw flat.
fn resample(levels: &[u8], n: usize) -> Vec<u8> {
    let len = levels.len();
    let cell = |i: usize| levels.get(i * len / n..((i + 1) * len / n).max(i * len / n + 1));
    let mean = |c: &[u8]| c.iter().map(|v| usize::from(*v)).sum::<usize>() / c.len().max(1);
    let cells = (0..n).map(|i| mean(cell(i).unwrap_or_default()) as u8);
    cells.collect()
}

// The one waveform every note draws: measured levels as at most 48 bars in `columns`.
fn waveform(levels: &[u8], columns: usize) -> String {
    let bar = |v: u8| ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'][(usize::from(v) * 7 + 127) / 255];
    let levels = resample(levels, columns.min(48));
    levels.into_iter().map(bar).collect()
}

/// A voice note's rows, sent from here or another device: `YOU` and its length, then its
/// waveform, which plays it; its words follow as any input's do. None for another input.
pub fn note(input: &Input, entry: EntryId, columns: usize) -> Vec<transcript::Row> {
    let Some(voice) = &input.voice else {
        return Vec::new();
    };
    let head = format!("   YOU  voice note · {}", timer(voice.duration_ms));
    let bars = waveform(&voice.levels, columns.saturating_sub(10));
    let bars = format!("        ▶ {bars}");
    let head = transcript::Row::on(Line::styled(head, bold(CYAN, YOU_BG)), YOU_BG);
    let bars = transcript::Row::on(Line::styled(bars, style(CYAN, YOU_BG)), YOU_BG);
    let action = Some(transcript::Action::Play(entry));
    vec![head, transcript::Row { action, ..bars }]
}

/// A delivered agent note's play control and waveform on the reply's violet line.
pub fn spoken(voice: &Voice, entry: EntryId, columns: usize) -> transcript::Row {
    let bars = waveform(&voice.levels, columns.saturating_sub(16));
    let words = format!("  ▌ ▶ {bars} {}", timer(voice.duration_ms));
    transcript::Row {
        action: Some(transcript::Action::Play(entry)),
        ..transcript::Row::on(Line::styled(words, style(VIOLET, AGENT_BG)), AGENT_BG)
    }
}

/// Draws, from the top of `r`, the quit warning, the note, and the player's state or failure;
/// returns the rows drawn.
pub fn draw(app: &App, b: &mut Buffer, r: Rect) -> u16 {
    let rows = band(app, r.width.into());
    for (y, row) in (r.y..r.bottom()).zip(&rows) {
        let ground = style(DATA, GLASS).patch(row.style);
        fill(b, Rect::new(r.x, y, r.width, 1), ground);
        line(b, r, y, row, ground);
    }
    rows.len().min(r.height.into()) as u16
}

/// The rows `draw` takes for `columns`.
pub fn band(app: &App, columns: usize) -> Vec<Line<'static>> {
    let mut band = Vec::new();
    if let Some(note) = &app.audio.note {
        if app.quitting {
            band = wrap(vec![span(bindings::text(app, QUIT), WHITE)], columns);
        }
        band.extend(rows(app, note, columns));
    }
    if let Some(p) = &app.audio.player {
        let doing = match (p.speech, &p.state) {
            (_, Err(why)) => {
                return [band, failed(app, why, columns, "<voice.cancel> dismisses")].concat();
            }
            (false, _) => "Voice note",
            (true, Ok(false)) => "Getting it ready to read aloud",
            (true, Ok(true)) => "Reading aloud",
        };
        band.push(gray(&bindings::text(
            app,
            &format!("{doing} · <voice.cancel> stops it"),
        )));
    }
    band
}

// A note's rows: `YOU`, what it is doing and its length, its waveform, and what Enter and Esc do
// now; a failure's sentence instead, and how to go on.
fn rows(app: &App, note: &Note, columns: usize) -> Vec<Line<'static>> {
    let local = app.servers.iter().find(|s| s.name.is_empty());
    let keys = match &note.phase {
        Phase::Failed(why) => return failed(app, why, columns, "<voice.cancel> discards"),
        Phase::Recording => "<voice.accept> sends · <voice.cancel> stops it",
        Phase::Held => "<voice.accept> sends · <voice.cancel> discards",
        Phase::Sending if note.voice.file.as_os_str().is_empty() => "◐ Sending voice note…",
        Phase::Sending => "◐ Transcribing…",
    };
    let doing = match local.filter(|_| note.phase == Phase::Recording) {
        Some(_) if note.voice.levels.is_empty() => "Opening microphone…".into(),
        Some(server) => format!("Recording on {}", server.machine),
        None => "voice note".into(),
    };
    let head = clip(&format!(" YOU · {doing}"), columns.saturating_sub(6));
    let time = timer(note.voice.duration_ms);
    let title = format!("{head}{time:>w$}", w = columns.saturating_sub(width(&head)));
    let bars = waveform(&note.voice.levels, columns.saturating_sub(2));
    let bars = Line::styled(format!(" {bars}"), style(CYAN, YOU_BG));
    let keys = bindings::text(app, keys);
    vec![Line::styled(title, bold(CYAN, YOU_BG)), bars, gray(&keys)]
}

// A failure's sentence in amber, wrapped, then that Enter tries again.
fn failed(app: &App, why: &str, columns: usize, esc: &str) -> Vec<Line<'static>> {
    let mut rows = wrap(vec![span(format!(" ✗ {why}"), AMBER)], columns);
    rows.push(gray(&bindings::text(
        app,
        &format!("<voice.accept> tries again · {esc}"),
    )));
    rows
}

fn gray(words: &str) -> Line<'static> {
    Line::styled(format!(" {words}"), style(GRAY, GLASS))
}

/// Marks drawn notes, queued ones included, and the newest completed reply's cap with the
/// player's state; a queued note's ▶ and the cap's play them.
pub fn controls(app: &App, b: &mut Buffer, hits: &mut Vec<Hit>) {
    let Some(focus) = app.focus.as_ref() else {
        return;
    };
    let player = app.audio.player.as_ref().filter(|p| p.state.is_ok());
    let (newest, mut plays) = (reply(app, focus).first().map(|e| (focus, *e)), Vec::new());
    for (r, action) in hits.iter() {
        let (target, cap) = match action {
            Action::Pin(session, entry) => ((session, *entry), true),
            Action::Play(session, entry) => ((session, *entry), false),
            Action::TakeBack(entry) => ((focus, *entry), false),
            _ => continue,
        };
        let playing = player.filter(|p| (&p.target.0, p.target.1) == target);
        let lit = playing.is_some() || newest == Some(target);
        let shown = |x: &u16| b.cell((*x, r.y)).is_some_and(|c| c.symbol() == "▶");
        let x = match cap {
            true => lit.then_some(r.right().saturating_sub(2)),
            false => (r.x..r.right()).find(shown),
        };
        let Some(x) = x else { continue };
        let mark = playing.map_or("▶", |p| ["◐", "■"][usize::from(p.state == Ok(true))]);
        if let Some(cell) = b.cell_mut((x, r.y)) {
            let agent = cap
                || app
                    .delivered
                    .iter()
                    .any(|d| (&d.session, d.entry) == target && d.voice.is_some());
            cell.set_symbol(mark)
                .set_fg([CYAN, VIOLET][usize::from(agent)]);
        }
        if !matches!(action, Action::Play(..)) {
            let play = Action::Play(target.0.clone(), target.1);
            plays.push((Rect::new(x, r.y, 2, 1), play));
        }
    }
    hits.extend(plays);
}
