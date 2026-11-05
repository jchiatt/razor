//! Live calls (docs/design/call.md): the phone's audio relayed to GPT-Live and back, the call's
//! words recorded as facts, the person's words admitted as their input at the voice's hand-off or,
//! while the agent works, at their pause, and the agent's progress passed to the voice. One loop owns the two sockets and the clocks; one worker does
//! every write in order, so the loop never waits on the record or the day's spending.

mod writes;

use super::{App, Gone, voice};
use crate::controller;
use crate::protocol::{Body, CallEvent, CallReason, Entry, Origin, SessionId, Speaker};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, Path, State};
use axum::http::header::AUTHORIZATION;
use axum::response::Response;
use base64::Engine;
use futures_util::{SinkExt, StreamExt, future::OptionFuture};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};
use writes::{Report, Work, work};

type Live =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const URL: &str = "wss://api.openai.com/v1/live/sessions";
const PCM: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;
/// GPT-Live's limits in tokens, kept in bytes, since a token is at least one byte: the opening
/// history, which also bounds the agent's words kept while the voice is held, and one update.
const SEED: usize = 8_192;
const PIECE: usize = 500;
/// A line is cut here, so the 256 unread facts a record keeps hold about 1 MiB of words.
const LINE: usize = 4 << 10;
/// A pause that ends a speaker's line; quiet before the latest step is said, and after it before
/// the one reminder that the agent is still on it; quiet that ends a call with no turn running.
const PAUSE: Duration = Duration::from_secs(2);
const STEP: Duration = Duration::from_secs(7);
const AGAIN: Duration = Duration::from_secs(20);
const IDLE: Duration = Duration::from_secs(180);
/// The voice's time to say the cap's sentence, and either socket's time to take a message.
const GRACE: Duration = Duration::from_secs(5);
/// GPT-Live's time to start a call, or to close one with its final usage.
const CLOSING: Duration = Duration::from_secs(15);
/// Words that ask the voice for quiet, and words that may stand beside them: "hold on a second",
/// "no, wait", "stop talking". "Don't stop" and "wait, which file?" ask for more.
const QUIET: &str = "stop wait hold hang pause shh quiet enough sec";
const BESIDE: &str = "on up a one second moment minute just please okay no sorry talking";
/// Words that only keep the person's side of the line going: "mm-hm", "yeah, okay", "uh-huh".
const BACKCHANNEL: &str = "mm hm mhm uh huh yeah okay ok right";

/// `call.toml`.
#[derive(serde::Deserialize)]
struct Words {
    prompt: String,
    developer: String,
    cap: String,
    still: String,
    again: String,
    ack: String,
}

static WORDS: LazyLock<Words> = LazyLock::new(|| {
    // Invariant: the file is compiled into the binary, so it parses in every run of a build or in
    // none, and the first call of any flow shows which.
    toml::from_str(include_str!("call.toml")).expect("src/api/call.toml parses")
});

/// `GET /v1/sessions/{id}/call`, under the device's usual authorization: a refusal after the
/// upgrade is a `closed` line, and removing the device ends the call, as the server's stop ends it
/// lost; the call holds its stop receiver until it has ended.
pub(super) async fn upgrade(
    State(app): State<App>,
    Path(id): Path<SessionId>,
    Extension(gone): Extension<Gone>,
    Extension(super::devices::Enrolled(device)): Extension<super::devices::Enrolled>,
    ws: WebSocketUpgrade,
) -> Response {
    let stop = app.calls.subscribe();
    ws.max_message_size(64 << 10)
        .on_upgrade(move |mut phone| async move {
            match start(&app, &id, device).await {
                Ok((call, ready)) => call.run(phone, ready, gone, stop).await,
                Err((reason, sentence)) => hang_up(&mut phone, reason, &sentence).await,
            }
        })
}

/// GPT-Live started for session `id`, after its key, the day's spending and the opening history,
/// and the phone's `ready`; or why the call is refused. The key goes wherever the call goes; only
/// `live.url`, a stand-in or the person's own relay, lets a call start without one.
async fn start(
    app: &App,
    id: &SessionId,
    device: Option<String>,
) -> Result<(Call, Value), (CallReason, String)> {
    let failed = |e: controller::Error| (CallReason::Error, e.to_string());
    let s = app.controller.summary(id).await.map_err(failed)?;
    let config = app.controller.config().await.live;
    let url = config.url.as_deref().unwrap_or(URL);
    let mut request = url
        .into_client_request()
        .map_err(|_| refused(CallReason::Error))?;
    let mut key = String::new();
    if let Ok(file) = tokio::fs::File::open(app.home.join("key")).await {
        let _ = file.take(4 << 10).read_to_string(&mut key).await;
    }
    let key = Some(key.trim()).filter(|key| !key.is_empty());
    match key.and_then(|key| format!("Bearer {key}").parse().ok()) {
        Some(bearer) => drop(request.headers_mut().insert(AUTHORIZATION, bearer)),
        None if config.url.is_none() => return Err(refused(CallReason::Key)),
        None => {}
    }
    let owner = voice::ask(app, None).await;
    let settings = owner
        .map_err(|refusal| (CallReason::Error, refusal.1))?
        .settings;
    if settings.call_used_today >= settings.call_daily_dollars {
        return Err(refused(CallReason::Cap));
    }
    let lines = app.controller.attach(id, s.last, 0).await.map_err(failed)?;
    let input = seed(app, id, &s).await.map_err(failed)?;
    let prompt =
        (WORDS.prompt.replace("{agent}", &s.name)).replace("{names}", &config.names.join(", "));
    let voice = Some(settings.call_voice.as_str()).filter(|v| !v.is_empty());
    let session = json!({"model": "gpt-live-1", "instructions": prompt, "store": false,
        "audio": {"format": {"type": "audio/pcm", "rate": 24000}, "output": {"voice": voice.unwrap_or("marin")}},
        "delegation": {"type": "client", "acknowledgement_delay_ms": 800}, "input": input});
    let started = tokio::time::timeout(CLOSING, open(request, session)).await;
    let (live, call) = started.unwrap_or(Err(refused(CallReason::Lost)))?;
    let (worker, queue) = mpsc::channel(64);
    let (reports, reported) = mpsc::channel(16);
    tokio::spawn(work(app.clone(), id.clone(), call, queue, reports, device));
    let call = Call {
        phone: None,
        live,
        lines,
        worker,
        reported,
        delegation: None,
        utterance: None,
        held: None,
        working: !s.unanswered.is_empty() && !s.done() && !s.interrupted(),
        unsent: None,
        waiting: None,
        over: false,
        quiet: Instant::now(),
        step: None,
        said: HashSet::new(),
        ending: None,
        closed: false,
        unmetered: Some(Instant::now()),
    };
    let (today, cap) = (settings.call_used_today, settings.call_daily_dollars);
    let ready = json!({"type": "ready", "today": today, "cap": cap,
        "model": s.model, "effort": s.settings.effort});
    Ok((call, ready))
}

/// Connects and sends `session.start`; the call's id is GPT-Live's for the session it started. A
/// handshake refused 401 or 403 is the key's fault; an endpoint never reached is lost.
async fn open(
    request: tungstenite::handshake::client::Request,
    session: Value,
) -> Result<(Live, String), (CallReason, String)> {
    let (mut live, _) = (tokio_tungstenite::connect_async(request).await).map_err(|e| {
        refused(match e {
            tungstenite::Error::Http(r) if matches!(r.status().as_u16(), 401 | 403) => {
                CallReason::Key
            }
            _ => CallReason::Lost,
        })
    })?;
    let start = json!({"type": "session.start", "session": session}).to_string();
    let sent = live.send(tungstenite::Message::Text(start.into())).await;
    sent.map_err(|_| refused(CallReason::Lost))?;
    while let Some(Ok(message)) = live.next().await {
        let event: Value =
            serde_json::from_str(message.to_text().unwrap_or("")).unwrap_or_default();
        match event["type"].as_str() {
            Some("session.started") => {
                let call = event["session"]["id"].as_str().unwrap_or_default();
                return Ok((live, call.into()));
            }
            Some("error") => {
                eprintln!(
                    "razor: GPT-Live refused a call: {}",
                    event["error"]["message"]
                );
                return Err(refused(CallReason::Error));
            }
            _ => {}
        }
    }
    Err(refused(CallReason::Lost))
}

/// The voice's opening history: a developer line naming the agent and its work, then the newest of
/// the person's words and the agent's replies and questions, at most 128 messages and SEED bytes.
async fn seed(
    app: &App,
    id: &SessionId,
    s: &crate::record::Summary,
) -> Result<Vec<Value>, controller::Error> {
    let work = [&s.headline, &s.title]
        .into_iter()
        .find(|work| !work.is_empty());
    let developer = WORDS.developer.replace("{agent}", &s.name);
    let developer = developer.replace("{work}", work.map_or("", String::as_str));
    let mut page = app.controller.page(id, None, 256).await?;
    // At most the page's 256 entries.
    let mut said = Vec::new();
    while let Some(Ok(line)) = page.next_line().await {
        let message = match serde_json::from_str::<Entry>(&line).map(|e| e.body) {
            Ok(Body::Input(i)) if matches!(i.from, Origin::Person | Origin::Answer { .. }) => {
                Some(("user", "input_text", i.text))
            }
            Ok(Body::Text { text }) => Some(("assistant", "output_text", text)),
            Ok(Body::Call { name, input, .. }) => crate::record::ask(&name, &input).map(|ask| {
                (
                    "assistant",
                    "output_text",
                    ask["text"].as_str().unwrap_or("").into(),
                )
            }),
            _ => None,
        };
        said.extend(message.filter(|(_, _, text)| !text.is_empty()));
    }
    let message =
        |role, kind, text| json!({"role": role, "content": [{"type": kind, "text": text}]});
    let developer = cut(&developer, SEED);
    let mut room = SEED - developer.len();
    let newest = said.iter().rev().take(127);
    let mut input: Vec<_> = (newest.map_while(|(role, kind, text)| {
        let text = cut(text, room);
        room -= text.len();
        (!text.is_empty()).then(|| message(*role, *kind, text))
    }))
    .collect();
    input.push(message("developer", "input_text", developer));
    input.reverse();
    Ok(input)
}

/// One live call, from GPT-Live's start.
struct Call {
    /// Until its socket fails.
    phone: Option<WebSocket>,
    live: Live,
    /// The session's lines from the call's start: the agent's progress.
    lines: mpsc::Receiver<Arc<str>>,
    worker: mpsc::Sender<Work>,
    reported: mpsc::Receiver<Report>,
    /// The latest hand-off's, which every update to the voice names.
    delegation: Option<String>,
    /// The line being said: when its latest words came, who says it, its words, its start and end.
    utterance: Option<(Instant, Speaker, String, u64, u64)>,
    /// While the person's words only ask for quiet, the voice is not heard. The agent's words
    /// wait here, at most SEED bytes, to be given silently once they say anything else.
    held: Option<String>,
    /// From a hand-off or the agent's text or step until a turn ends.
    working: bool,
    /// While the agent works, when the person last said more than a request for quiet or a
    /// backchannel that has not gone to it yet.
    unsent: Option<Instant>,
    /// A hand-off came before the words it hands off, at this offset in GPT-Live's audio: the next
    /// line of the person's begun by then goes to the agent; one begun later does not.
    waiting: Option<u64>,
    /// The agent's turn ended while the person was speaking: the call keeps working only if their
    /// line goes to the agent.
    over: bool,
    /// When someone last spoke, the voice was given words, or a turn ended.
    quiet: Instant,
    /// While the agent works, its latest described step: said after STEP of quiet, once a call,
    /// and after AGAIN more, or AGAIN when it was said before, the voice says it is still on it.
    step: Option<String>,
    /// The steps the voice has said are still running.
    said: HashSet<String>,
    /// Why the call ends, when its next step is due (`session.close`, then giving up on GPT-Live's
    /// answer), and whether `session.close` went.
    ending: Option<(CallReason, Instant, bool)>,
    /// GPT-Live's session is over.
    closed: bool,
    /// When the call opened, until GPT-Live reports its usage.
    unmetered: Option<Instant>,
}

impl Call {
    async fn run(
        mut self,
        phone: WebSocket,
        ready: Value,
        mut gone: Gone,
        mut stop: watch::Receiver<bool>,
    ) {
        self.phone = Some(phone);
        let (folder, device) = (None, None);
        let started = CallEvent::Started { folder, device };
        self.queue(Work::Fact(started, None)).await;
        self.tell(ready).await;
        self.tell(json!({"type": "working", "on": self.working}))
            .await;
        while !self.closed {
            let deadline = self.deadline();
            // The phone's audio runs until `session.close` goes, so the voice can finish speaking.
            let listening = !matches!(self.ending, Some((.., true)));
            let phone = OptionFuture::from(self.phone.as_mut().map(WebSocket::recv));
            tokio::select! {
                Some(frame) = phone, if listening => self.on_phone(frame).await,
                event = self.live.next() => self.on_live(event).await,
                line = self.lines.recv(), if self.ending.is_none() => self.on_session(line).await,
                Some(report) = self.reported.recv() => self.report(report).await,
                _ = gone.changed(), if self.ending.is_none() => self.end(CallReason::Ended),
                _ = stop.changed(), if self.ending.is_none() => self.end(CallReason::Lost),
                () = tokio::time::sleep_until(deadline) => self.tick().await,
            }
        }
        self.flush().await;
        // A call GPT-Live never metered is charged its length by the clock.
        if let Some(opened) = self.unmetered {
            self.queue(Work::Usage(opened.elapsed().as_secs_f64()))
                .await;
        }
        let reason = self.ending.map_or(CallReason::Error, |(reason, ..)| reason);
        self.queue(Work::Fact(CallEvent::Ended { reason }, None))
            .await;
        // The worker reports its last charges, then stops once the end is recorded.
        while let Ok(Some(report)) = tokio::time::timeout(CLOSING, self.reported.recv()).await {
            self.report(report).await;
        }
        if let Some(phone) = &mut self.phone {
            hang_up(phone, reason, sentence(reason)).await;
        }
    }

    /// The phone's audio goes on to GPT-Live a frame at a time, an empty frame dropped; `mute` and
    /// `unmute` go on unacknowledged, `choose` goes to the worker, and `end` ends the call.
    async fn on_phone(&mut self, frame: Option<Result<Message, axum::Error>>) {
        match frame {
            Some(Ok(Message::Binary(pcm))) if !pcm.is_empty() => {
                let audio = PCM.encode(&pcm);
                let append =
                    format!(r#"{{"type":"session.input_audio.append","audio":"{audio}"}}"#);
                self.send_live(append).await
            }
            Some(Ok(Message::Text(command))) => {
                let command: Value = serde_json::from_str(&command).unwrap_or_default();
                match command["type"].as_str() {
                    Some(kind @ ("mute" | "unmute")) => {
                        let input = format!(r#"{{"type":"session.input_audio.{kind}"}}"#);
                        self.send_live(input).await
                    }
                    Some("choose") => self.queue(Work::Choose(command)).await,
                    Some("end") => self.end(CallReason::Ended),
                    _ => {}
                }
            }
            Some(Ok(Message::Close(_))) => self.end(CallReason::Ended),
            Some(Ok(_)) => {}
            Some(Err(_)) | None => {
                self.phone = None;
                self.end(CallReason::Lost);
            }
        }
    }

    /// GPT-Live's audio goes to the phone as it comes, its words become captions and lines, a
    /// delegation is a hand-off, usage is charged, and its close or loss ends the call.
    async fn on_live(&mut self, event: Option<Result<tungstenite::Message, tungstenite::Error>>) {
        let event: Value = match event {
            Some(Ok(tungstenite::Message::Text(event))) => {
                serde_json::from_str(&event).unwrap_or_default()
            }
            Some(Ok(tungstenite::Message::Close(_)) | Err(_)) | None => {
                self.closed = true;
                return self.end(CallReason::Lost);
            }
            Some(Ok(_)) => return,
        };
        if let Some(seconds) = event["usage"]["seconds"].as_f64() {
            self.unmetered = None;
            self.queue(Work::Usage(seconds)).await;
        }
        match event["type"].as_str().unwrap_or_default() {
            "session.output_audio.delta" if self.held.is_none() => {
                if let Some(pcm) = event["delta"].as_str().and_then(|d| PCM.decode(d).ok()) {
                    self.send_phone(Message::Binary(pcm.into())).await
                }
            }
            "session.input_transcript.delta" => self.transcript(Speaker::Person, &event).await,
            "session.output_transcript.delta" if self.held.is_none() => {
                self.transcript(Speaker::Voice, &event).await
            }
            "session.delegation.created" => self.hand_off(&event).await,
            "session.closed" => {
                self.closed = true;
                // A close the call asked for keeps its own reason.
                self.end(CallReason::Lost);
            }
            "error" => eprintln!("razor: GPT-Live on a call: {}", event["error"]["message"]),
            _ => {}
        }
    }

    /// Each fragment is a caption at once; one speaker's fragments join into a line until the other
    /// speaks, a hand-off, a pause, the line's size or the end. The person asking for quiet holds
    /// the voice, and the phone drops its audio not yet played; anything else they say lifts the
    /// hold, and the voice is given silently what the agent said meanwhile.
    async fn transcript(&mut self, who: Speaker, event: &Value) {
        let delta = event["delta"].as_str().unwrap_or_default();
        let start_ms = event["start_ms"].as_u64().unwrap_or_default();
        let end_ms = event["end_ms"].as_u64().unwrap_or_default();
        let caption = json!({"type": "caption", "who": who, "text": delta,
            "start_ms": start_ms, "end_ms": end_ms});
        self.tell(caption).await;
        let (now, mut rest) = (Instant::now(), delta);
        while !rest.is_empty() {
            let piece = cut(rest, LINE);
            rest = &rest[piece.len()..];
            let joins = matches!(&self.utterance, Some((_, said, text, ..))
                if *said == who && text.len() + piece.len() <= LINE);
            if !joins {
                self.flush().await;
            }
            let line = (self.utterance).get_or_insert((now, who, String::new(), start_ms, end_ms));
            line.2.push_str(piece);
            (line.0, line.4) = (now, end_ms);
        }
        self.quiet = now;
        // GPT-Live sends the odd blank fragment, which is not the person speaking again.
        let line = (self.utterance.as_ref()).filter(|l| l.2.contains(char::is_alphanumeric));
        let hold = line.map_or(self.held.is_some(), |l| {
            l.1 == Speaker::Person && only(&l.2, QUIET, BESIDE)
        });
        if hold && self.held.is_none() {
            self.tell(json!({"type": "hush"})).await;
            self.held = Some(String::new());
        } else if !hold && let Some(words) = self.held.take() {
            for piece in pieces(&words) {
                self.update("session.thinking.append", piece).await;
            }
        }
    }

    /// Records the words being said as one line, unless they are only spaces; the person's words
    /// for the agent, unless they only ask the voice for quiet, which is no news to the agent. The
    /// line a hand-off is waiting for goes at once; while the agent works, any but a backchannel
    /// goes at the person's pause. A turn that ended while they spoke then ends the work.
    async fn flush(&mut self) -> Option<String> {
        let said = self.line().await;
        if std::mem::take(&mut self.over) && self.unsent.is_none() {
            self.set_working(false).await;
        }
        said
    }

    async fn line(&mut self) -> Option<String> {
        let (at, who, text, start_ms, end_ms) = self.utterance.take()?;
        let text = Some(text.trim().to_owned()).filter(|text| !text.is_empty())?;
        let said = (who == Speaker::Person && !only(&text, QUIET, BESIDE)).then(|| text.clone());
        let backchannel = only(&text, BACKCHANNEL, "");
        let line = CallEvent::Line {
            who,
            name: None,
            label: None,
            text,
            start_ms,
            end_ms,
        };
        self.queue(Work::Fact(line, said.clone())).await;
        if said.is_some() && self.waiting.take().is_some_and(|offset| start_ms <= offset) {
            self.queue(Work::HandOff).await;
            self.set_working(true).await;
        } else if said.is_some() && self.working && !backchannel {
            self.unsent = Some(at);
        }
        said
    }

    /// GPT-Live handed the request on: the voice's updates name this delegation. With the agent
    /// idle, the person's words go to it as their input, the voice acknowledges them by name, and
    /// they are what it is still working on until it says more; while it works, `tick` sends them.
    async fn hand_off(&mut self, event: &Value) {
        let asked = self.flush().await;
        self.delegation = event["delegation"]["id"].as_str().map(str::to_owned);
        self.quiet = Instant::now();
        if self.working {
            return;
        }
        let Some(asked) = asked else {
            self.waiting = Some(event["offset_ms"].as_u64().unwrap_or(u64::MAX));
            return;
        };
        let asked = cut(&asked, PIECE - WORDS.ack.len());
        self.say(&WORDS.ack.replace("{asked}", asked)).await;
        self.step = Some(asked.to_owned());
        self.queue(Work::HandOff).await;
        self.set_working(true).await;
    }

    /// The agent's progress: its text is said, its question to the person asked, and each step's
    /// description kept for a quiet stretch; a turn's end ends the work.
    async fn on_session(&mut self, line: Option<Arc<str>>) {
        let Some(line) = line else {
            return self.end(CallReason::Error);
        };
        let Ok(entry) = serde_json::from_str::<Entry>(&line) else {
            return;
        };
        match entry.body {
            Body::Text { text } => {
                self.set_working(true).await;
                self.say(&text).await
            }
            Body::Call { name, input, .. } => {
                self.set_working(true).await;
                if let Some(ask) = crate::record::ask(&name, &input) {
                    return self.say(ask["text"].as_str().unwrap_or_default()).await;
                }
                if let Some(step) = input["description"].as_str() {
                    self.step = Some(cut(step.trim(), PIECE - WORDS.still.len()).to_owned());
                }
            }
            Body::TurnEnd { .. } => {
                (self.quiet, self.step) = (Instant::now(), None);
                // Words said during the turn and not yet sent are its successor's input: at once,
                // or with the person still speaking, at their pause, with what `flush` sends.
                if self.talking() {
                    self.over = true;
                } else if self.unsent.take().is_some() {
                    self.queue(Work::HandOff).await;
                } else {
                    self.set_working(false).await;
                }
            }
            _ => {}
        }
    }

    /// Words for the voice to say, in pieces it accepts, or kept while it is held; any step not yet
    /// said is then old news.
    async fn say(&mut self, words: &str) {
        if let Some(held) = &mut self.held {
            held.push_str(cut(&format!("\n{words}"), SEED.saturating_sub(held.len())));
        } else {
            for piece in pieces(words) {
                self.update("session.commentary.append", piece).await;
            }
        }
        (self.quiet, self.step) = (Instant::now(), None);
    }

    async fn update(&mut self, kind: &str, content: &str) {
        let update = json!({"type": kind, "delegation_id": self.delegation, "content": content});
        self.send_live(update.to_string()).await;
    }

    async fn set_working(&mut self, on: bool) {
        if self.working != on {
            self.working = on;
            self.tell(json!({"type": "working", "on": on})).await;
        }
    }

    /// The worker's line for the phone: a choice's answer, or a charge's with the day's total,
    /// when at the cap the voice says so before the call ends.
    async fn report(&mut self, (line, capped): Report) {
        self.tell(line).await;
        if capped && self.ending.is_none() {
            self.say(&WORDS.cap).await;
            self.ending = Some((CallReason::Cap, Instant::now() + GRACE, false));
        }
    }

    /// The next moment a clock runs out: a pause ending a line, the latest step's turn to be said,
    /// three quiet minutes with no turn running, or the ending's next step.
    fn deadline(&self) -> Instant {
        let clocks = [
            self.utterance.as_ref().map(|line| line.0 + PAUSE),
            self.unsent.filter(|_| !self.talking()).map(|at| at + PAUSE),
            self.due(),
            (!self.working && self.ending.is_none()).then_some(self.quiet + IDLE),
            self.ending.map(|(_, at, _)| at),
        ];
        let next = clocks.into_iter().flatten().min();
        next.unwrap_or_else(|| Instant::now() + IDLE)
    }

    async fn tick(&mut self) {
        let now = Instant::now();
        if self
            .utterance
            .as_ref()
            .is_some_and(|line| now >= line.0 + PAUSE)
        {
            self.flush().await;
        }
        // While the agent works, the person's words go to it once they pause, as a hand-off would
        // take them: a correction reaches the agent whether or not GPT-Live hands it on, in one
        // piece when the voice talks over it.
        if !self.talking() && self.unsent.is_some_and(|at| now >= at + PAUSE) {
            self.unsent = None;
            self.queue(Work::HandOff).await;
        }
        if self.due().is_some_and(|due| now >= due)
            && let Some(step) = self.step.take()
        {
            if self.said.insert(step.clone()) {
                self.say(&WORDS.still.replace("{step}", &step)).await;
                self.step = Some(step);
            } else {
                self.say(&WORDS.again).await;
            }
        }
        if !self.working && now >= self.quiet + IDLE {
            self.end(CallReason::Idle);
        }
        match self.ending {
            Some((reason, at, false)) if now >= at => {
                self.send_live(r#"{"type":"session.close"}"#.into()).await;
                self.ending = Some((reason, now + CLOSING, true));
            }
            Some((_, at, true)) if now >= at => self.closed = true,
            _ => {}
        }
    }

    /// When the latest step is next said, never while the voice is held: STEP after the quiet
    /// began, or AGAIN once it was said.
    fn due(&self) -> Option<Instant> {
        let step = self.step.as_deref().filter(|_| self.held.is_none())?;
        let again = self.said.contains(step);
        Some(self.quiet + if again { AGAIN } else { STEP })
    }

    fn talking(&self) -> bool {
        matches!(self.utterance, Some((_, Speaker::Person, ..)))
    }

    /// Ends the call for `reason` unless it is already ending; `session.close` goes at once.
    fn end(&mut self, reason: CallReason) {
        self.ending.get_or_insert((reason, Instant::now(), false));
    }

    /// A phone that takes nothing for GRACE is gone: the call ends lost, and nothing more is sent.
    async fn send_phone(&mut self, message: Message) {
        let Some(phone) = &mut self.phone else {
            return;
        };
        let sent = tokio::time::timeout(GRACE, phone.send(message)).await;
        if !matches!(sent, Ok(Ok(()))) {
            self.phone = None;
            self.end(CallReason::Lost);
        }
    }

    async fn tell(&mut self, line: Value) {
        self.send_phone(Message::Text(line.to_string().into()))
            .await;
    }

    /// GPT-Live taking nothing for GRACE is lost, and its session with it.
    async fn send_live(&mut self, event: String) {
        let sent = self.live.send(tungstenite::Message::Text(event.into()));
        if !matches!(tokio::time::timeout(GRACE, sent).await, Ok(Ok(()))) {
            self.closed = true;
            self.end(CallReason::Lost);
        }
    }

    async fn queue(&mut self, work: Work) {
        // The worker stops only after the call's end, the last work queued.
        let _ = self.worker.send(work).await;
    }
}

/// Whether each of the words is in `first` or `beside`, and one in `first`.
fn only(text: &str, first: &str, beside: &str) -> bool {
    let among = |list: &str, word: &str| list.split(' ').any(|w| w.eq_ignore_ascii_case(word));
    let mut words = text.split(|c: char| !c.is_alphanumeric());
    let asks = words.clone().any(|w| among(first, w));
    asks && words.all(|w| w.is_empty() || among(first, w) || among(beside, w))
}

/// `text` in pieces of at most PIECE bytes, each ending at a sentence's end where one fits.
fn pieces(mut text: &str) -> impl Iterator<Item = &str> {
    std::iter::from_fn(move || {
        text = text.trim_start();
        let mut piece = cut(text, PIECE);
        if piece.len() < text.len()
            && let Some(end) = piece.rfind(['.', '!', '?', '\n'])
        {
            piece = &piece[..=end];
        }
        text = &text[piece.len()..];
        Some(piece).filter(|piece| !piece.is_empty())
    })
}

/// The longest start of `text` within `max` bytes that ends at a character's boundary.
fn cut(text: &str, max: usize) -> &str {
    &text[..text.floor_char_boundary(max)]
}

fn refused(reason: CallReason) -> (CallReason, String) {
    (reason, sentence(reason).into())
}

/// What the phone tells the person as each kind of ending closes the call.
fn sentence(reason: CallReason) -> &'static str {
    match reason {
        CallReason::Ended | CallReason::Canceled => "The call ended.",
        CallReason::Idle => "The call ended after three quiet minutes.",
        CallReason::Cap => "The daily call limit is reached; continue by message.",
        CallReason::Key => "Calls need a valid OpenAI API key in this server's key file.",
        CallReason::Lost => "The connection to the voice was lost; the agent's work goes on.",
        CallReason::Error => "The call could not go on; see the server log, then call again.",
    }
}

/// Tells the phone why the call closed, then reads to its own close, so that its socket ends
/// cleanly rather than being reset with its audio unread and the reason lost.
async fn hang_up(phone: &mut WebSocket, reason: CallReason, sentence: &str) {
    let closed = json!({"type": "closed", "reason": reason, "sentence": sentence});
    let _ = tokio::time::timeout(GRACE, async {
        let _ = phone.send(Message::Text(closed.to_string().into())).await;
        let _ = phone.send(Message::Close(None)).await;
        while let Some(Ok(_)) = phone.recv().await {}
    })
    .await;
}
