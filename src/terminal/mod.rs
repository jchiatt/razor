//! The terminal app is a client of the API server. Everything it holds is a capped cache of the
//! streams, disposable at quit; only the server owns a record.
mod activity;
mod agents;
mod api;
mod audio;
mod bindings;
mod brand;
mod cards;
mod children;
mod clipboard;
mod commands;
mod composer;
mod data;
mod gauge;
mod inputs;
mod keys;
mod markdown;
mod mcp;
mod motion;
mod output;
mod panel;
mod picker;
mod questions;
mod recap;
mod screen;
mod servers;
mod split;
mod stage;
mod start;
mod status;
mod streams;
mod strip;
mod text;
mod tools;
mod topbar;
mod transcript;
mod tty;
mod view;

use crate::protocol::{AgentView, Ask, Brand, ClientInput, EntryId, Event as ApiEvent};
use crate::protocol::{Live, MailId, Models, SessionId, State, now};
use api::{Api, Incoming, Line};
use composer::Composer;
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};
use std::{collections::BTreeMap, future::Future, path::PathBuf, pin::Pin, sync::Arc};
use tokio::{
    sync::{Semaphore, mpsc, oneshot},
    task::JoinHandle,
    time::{Duration, Instant},
};
use transcript::Transcript;

/// A finished operation's effect on the app, applied on the loop between draws.
type Done = Box<dyn FnOnce(&mut App) + Send>;
type Job = Pin<Box<dyn Future<Output = Done> + Send>>;

/// Renders a session and one opened child through the versioned API, never a record file.
/// Main supplies the socket-bound or remote client, first input and the Razor home whose machine
/// settings name the audio programs; a session is made on admission. A second Ctrl-C in a row, or
/// a hangup, restores the terminal and quits without stopping a session or its jobs.
pub async fn run(
    client: reqwest::Client,
    server: reqwest::Url,
    device_key: String,
    session: Option<SessionId>,
    initial: ClientInput,
    home: PathBuf,
    agents: bool,
) -> Result<(), Error> {
    let unreadable =
        "The working directory could not be read; start Razor in an existing directory.";
    let cwd = tokio::task::spawn_blocking(std::env::current_dir).await;
    let cwd = cwd.map_err(fail(unreadable))?.map_err(fail(unreadable))?;
    let (done, mut finished) = mpsc::channel(16);
    // At most 16 lines in flight and 8 MiB of their bytes, or one larger whole entry alone.
    let (lines, mut incoming) = mpsc::channel(16);
    let (api, budget) = (
        Api::new(client, server, device_key),
        Arc::new(Semaphore::new(api::BYTES)),
    );
    api.stream(None, 0, lines.clone(), budget.clone());
    let mut app = App::new(api, done, lines, budget, cwd);
    // At most eight audio effects wait; a recording's levels skip a draw rather than wait.
    let (sender, mut audio) = mpsc::channel(8);
    #[cfg(target_os = "macos")]
    activity::start(sender.clone());
    app.bindings = bindings::compile(&app.ui.keys)?;
    app.list.kept = home.join("list-width");
    let width = std::fs::read_to_string(&app.list.kept).ok();
    app.list.width = width.and_then(|w| w.trim().parse().ok());
    audio::start(&mut app, home, sender);
    app.emoji = data::load()?;
    composer::emoji::read(&mut app);
    app.model = initial.model.clone().unwrap_or_default();
    app.composer.replace(initial.text.clone());
    app.composer.initial_images(initial.images.clone());
    (app.root, app.focus, app.initial) = (session.clone(), session.clone(), Some(initial));
    picker::request(&mut app, false);
    if let Some(id) = session {
        app.open(id);
    }
    if agents {
        agents::toggle(&mut app);
    }
    let mut recent = agents && app.focus.is_none() && app.composer.is_empty();
    let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
        .map_err(Error::Terminal)?;
    let mut modes = Some(screen::Modes::enter()?);
    let ran = async {
        let backend = CrosstermBackend::new(std::io::stdout());
        let terminal = &mut Terminal::new(backend).map_err(Error::Terminal)?;
        // A terminal under 30 rows starts with the gauge folded.
        app.gauge_folded = terminal.size().is_ok_and(|size| size.height < 30);
        if !app.composer.is_empty() {
            inputs::send(&mut app);
        }
        let mut events = tty::Events::default();
        let mut last_draw = Instant::now() - motion::FRAME;
        loop {
            if app.leaving {
                return app.quit(finished).await;
            }
            if std::mem::take(&mut app.list.editor) {
                drop(events);
                drop(modes.take());
                let edited = agents::edit(&mut app).await;
                modes = Some(screen::Modes::enter()?);
                *terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))
                    .map_err(Error::Terminal)?;
                app.painted = Default::default();
                events = tty::Events::default();
                if let Err(e) = edited {
                    app.say(&e.to_string());
                }
                app.dirty = true;
            }
            stage::remember(&mut app);
            clipboard::write(&mut app, terminal.backend_mut())?;
            let pace = motion::pace(app.cost, app.moving.is_some());
            if app.dirty && (app.immediate || last_draw.elapsed() >= pace) {
                let start = Instant::now();
                screen::draw(terminal, &mut app)?;
                (last_draw, app.cost) = (Instant::now(), start.elapsed());
                children::read_visible(&mut app, now());
                output::read_visible(&mut app);
                transcript::read_visible(&mut app);
                (app.dirty, app.immediate) = (false, false);
            }
            let wake = app.wake(last_draw);
            tokio::select! {
                event = events.next() => {
                    let Some(event) = event else { return Ok(()) };
                    if keys::event(&mut app, event.map_err(Error::Terminal)?) {
                        return app.quit(finished).await;
                    }
                }
                Some(line) = incoming.recv() => {
                    let snapshot = matches!(&line.line, Line::Event(ApiEvent::Live(Live::Agents { .. })));
                    app.fold_line(line);
                    if snapshot && std::mem::take(&mut recent) {
                        agents::recent(&mut app);
                    }
                },
                _ = hangup.recv() => return app.quit(finished).await,
                Some(done) = audio.recv() => {
                    done(&mut app);
                    app.dirty = true;
                }
                Some(done) = finished.recv() => {
                    app.pending = app.pending.saturating_sub(1);
                    done(&mut app);
                    app.dirty = true;
                }
                _ = tokio::time::sleep_until(wake) => app.tick(),
            }
        }
    }
    .await;
    drop(audio);
    audio::close(&mut app).await;
    drop(modes);
    if let (Ok(()), Some(line)) = (&ran, app.farewell()) {
        println!("{line}");
    }
    ran
}

/// A terminal failure, or another cause retained with a sentence of context.
#[derive(Debug)]
pub enum Error {
    /// Terminal setup or painting failed; main can offer --print.
    Terminal(std::io::Error),
    /// An HTTP refusal retains its status so background reads never retry a 4xx.
    Refused { status: u16, sentence: String },
    /// An API, data or task failure with its underlying cause.
    Other {
        context: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Terminal(e) => write!(f, "The terminal failed: {e}; use --print to continue."),
            Self::Refused { status, sentence } => write!(f, "API {status}: {sentence}"),
            Self::Other { context, .. } => f.write_str(context),
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Terminal(e) => Some(e),
            Self::Refused { .. } => None,
            Self::Other { source, .. } => Some(source.as_ref()),
        }
    }
}
impl Error {
    /// A 4xx: the request itself was refused, so a background read never retries it.
    fn refused(&self) -> bool {
        matches!(
            self,
            Self::Refused {
                status: 400..=499,
                ..
            }
        )
    }
}
/// Wraps a cause under `context`, the sentence a person reads.
fn fail<E: std::error::Error + Send + Sync + 'static>(context: &str) -> impl FnOnce(E) -> Error {
    let context = context.to_owned();
    move |source| Error::Other {
        context,
        source: Box::new(source),
    }
}
/// An error that is its own cause.
fn error(context: impl Into<String>) -> Error {
    let context = context.into();
    Error::Other {
        source: context.clone().into(),
        context,
    }
}

/// What the app shows. Status rows, transcripts and every list are bounded caches of the
/// streams; drafts and emoji history belong to the API, and the rest is disposable.
#[derive(Default)]
struct App {
    // At most one voice note and one player, and their programs' newest task.
    audio: audio::Audio,
    bindings: bindings::Bindings,
    api: Option<Api>,
    done: Option<mpsc::Sender<Done>>,
    writes: Option<mpsc::Sender<Job>>,
    lines: Option<mpsc::Sender<Incoming>>,
    // Stream lines in flight hold their bytes of this 8 MiB budget until folded.
    budget: Option<Arc<Semaphore>>,
    // Spawned operations not yet applied; at most eight, plus eight queued writes.
    pending: usize,
    streams: BTreeMap<SessionId, JoinHandle<()>>,
    dirty: bool,
    immediate: bool,
    draft_at: Option<Instant>,
    root: Option<SessionId>,
    focus: Option<SessionId>,
    cwd: PathBuf,
    // The resident a new session is a thread of (its native name); read only while no session is
    // addressed, which `start::aim` alone brings about.
    resident: Option<SessionId>,
    model: String,
    models: Option<Models>,
    models_at: u64,
    picker: Option<picker::Picker>,
    composer: Composer,
    menu: composer::completion::Menu,
    // The emoji table, the open grid, the shortcode strip, and the person's emoji history as
    // last read: at most 32 emoji and their tones.
    emoji: data::Table,
    grid: Option<composer::emoji::Grid>,
    codes: composer::shortcodes::Codes,
    used: crate::protocol::Emoji,
    // The addressed session's skills, at most 1,024, read when a command starts being typed.
    skills: Vec<crate::protocol::Skill>,
    // The person's own draft while an earlier input is recalled into the editor.
    history: Option<(EntryId, Composer)>,
    history_loading: bool,
    transcripts: BTreeMap<SessionId, Transcript>,
    // Wire rows whole, at most 1,024 of them and 8 MiB; a snapshot replaces the map.
    agents: BTreeMap<SessionId, AgentView>,
    agent_bytes: usize,
    servers: Vec<crate::protocol::Server>,
    config: Option<String>,
    brand: Brand,
    // The person's display settings; `ui.motion` is the motion switch every drawer reads.
    ui: crate::protocol::Ui,
    // What the last draw asked for, its cost for the pace, and the person's comings and goings.
    next: motion::Wake,
    cost: Duration,
    presence: motion::Presence,
    // From his return to a session until its recap is asked, that session, the entry to recap
    // after and when it was last looked at. Then the recap's session, that look, its words (empty
    // until they land) and when they landed.
    away: Option<(SessionId, EntryId, u64)>,
    recap: Option<(SessionId, u64, String, u64)>,
    // When the main session's place and headline last changed while attached.
    changed: [Option<u64>; 2],
    lights: tools::rim::Lights,
    // The frame moving between a child's card and the split, while one does.
    moving: Option<split::Move>,
    stage: stage::Stage,
    // The focused folder's branch (empty when detached) and uncommitted files, read by the
    // project route on a focus change and on the focused session's turn ends.
    folder: Option<(SessionId, String, u64)>,
    // The newest 256 deliveries, ordered by their original (at, session, entry).
    delivered: std::collections::VecDeque<crate::protocol::Delivery>,
    inbox: Option<usize>,
    list: agents::List,
    // The new-agent chooser while open; the server a new session starts on (empty for this
    // one), the request id of its creation until it is answered, and the session addressed
    // before, which Escape returns to.
    start: Option<start::Start>,
    server: String,
    request: Option<String>,
    back: Option<SessionId>,
    answering: Option<MailId>,
    shown_ask: Option<Ask>,
    visible_options: std::collections::BTreeSet<usize>,
    // The drawn targets Tab and Ctrl-N cycle, in the cockpit's ring order and without repeats;
    // Enter on an empty composer does what a click on one does.
    targets: Vec<view::Action>,
    selected: Option<view::Action>,
    // The ask the person moved the band to, held by its id; when it closes the band shows the
    // first. A digit or click within 600 ms of the shown ask changing or opening is refused.
    ask: Option<MailId>,
    ask_at: u64,
    // The last option picked and when: the same pick again within 1.5 s is a double press.
    picked: Option<(MailId, u64)>,
    // The ask whose one-row band the person opened to its options, by a click or a first digit.
    ask_open: Option<MailId>,
    notice: Option<(String, u64)>,
    // A first Ctrl-C and no key or paste since: the notice row says so and the next Ctrl-C quits.
    quitting: bool,
    // `/detach` was typed, or the session `/done` or `/quit` closed has ended: the loop quits as a
    // second Ctrl-C does.
    leaving: bool,
    // The session `/done` or `/quit` closed, until its status row reads it closed.
    closing: Option<SessionId>,
    panel: Option<panel::Panel>,
    panel_scroll: usize,
    login: Option<tokio::task::AbortHandle>,
    pane_open: Option<bool>,
    // The archived children are listed in full; the children strip is open.
    archive: bool,
    strip: bool,
    status_scroll: usize,
    // The pane's next draw scrolls to its inbox: after the unread chip or an inbox page.
    to_inbox: bool,
    status_area: Option<Rect>,
    columns: u16,
    // Started inside tmux, which re-sends each cell through its host's width table.
    tmux: bool,
    zone: Option<jiff::tz::TimeZone>,
    gauge_folded: bool,
    // The focused tape's last move: whose, the share it moved from and when (`gauge::tide`).
    tide: Option<(SessionId, f64, u64)>,
    busy_at: Option<u64>,
    // The words of an input being admitted, moved out of the editor at Enter.
    sending: Option<Composer>,
    loading_images: usize,
    output_loading: bool,
    copying: bool,
    // An encoded copy for the frame loop to write, and its toast.
    clipboard: Option<(String, String)>,
    selection: Option<transcript::select::Selection>,
    // The last click's time, cell and count, for double and triple clicks.
    clicks: (u64, (u16, u16), u8),
    // At most one link per visible cell; destinations share the parsed paragraph's bytes.
    links: Vec<(Rect, Arc<str>)>,
    painted: screen::Painted,
    hits: Vec<view::Hit>,
    // Launch choices ride on the first admitted input, never on a bare launch.
    initial: Option<ClientInput>,
    revision: u64,
    // One per status row: last activity, retry-after (MAX on success/refusal), in flight, failure said.
    previews: BTreeMap<SessionId, (u64, u64, bool, bool)>,
    edits: u64,
    // The tab's title as last written.
    title: String,
    program: screen::ProgramStatus,
}

impl App {
    fn new(
        api: Api,
        done: mpsc::Sender<Done>,
        lines: mpsc::Sender<Incoming>,
        budget: Arc<Semaphore>,
        cwd: PathBuf,
    ) -> Self {
        let (writes, mut queue) = mpsc::channel::<Job>(8);
        let finished = done.clone();
        // Drafts, admissions and opens run one at a time in order, so an earlier draft never
        // lands after the admission that cleared it; past quit, their effects go unapplied.
        tokio::spawn(async move {
            while let Some(job) = queue.recv().await {
                let _ = finished.send(job.await).await;
            }
        });
        Self {
            api: Some(api),
            done: Some(done),
            writes: Some(writes),
            lines: Some(lines),
            budget: Some(budget),
            cwd,
            zone: Some(jiff::tz::TimeZone::system()),
            tmux: std::env::var_os("TMUX").is_some(),
            dirty: true,
            presence: motion::Presence::new(now()),
            ..Self::default()
        }
    }

    /// Runs `work` beside the loop and applies its effect when done; refused past eight.
    fn spawn<W, F, G>(&mut self, work: W) -> bool
    where
        W: FnOnce(Api) -> F,
        F: Future<Output = G> + Send + 'static,
        G: FnOnce(&mut App) + Send + 'static,
    {
        if self.pending >= 8 {
            self.say("Eight API operations are pending; wait for an answer before trying again.");
            return false;
        }
        let (Some(api), Some(done)) = (self.api.clone(), self.done.clone()) else {
            return false;
        };
        let work = work(api);
        self.pending += 1;
        tokio::spawn(async move { done.send(Box::new(work.await)).await });
        true
    }

    /// Queues `work` behind every earlier draft, admission and open; refused past eight.
    fn write<W, F, G>(&mut self, work: W) -> bool
    where
        W: FnOnce(Api) -> F,
        F: Future<Output = G> + Send + 'static,
        G: FnOnce(&mut App) + Send + 'static,
    {
        let Some(work) = self.api.clone().map(work) else {
            return false;
        };
        let job: Job = Box::pin(async move { Box::new(work.await) as Done });
        let queued = self
            .writes
            .as_ref()
            .is_some_and(|w| w.try_send(job).is_ok());
        match queued {
            true => self.pending += 1,
            false => {
                self.say("Eight draft or input writes are pending; keep the unsent words and wait.")
            }
        }
        queued
    }

    /// Saves the addressed session's unsent words.
    fn save(&mut self) {
        if let (None, Some(id)) = (&self.sending, self.focus.clone()) {
            let text = self.composer.text.clone();
            self.write(|api| async move {
                let saved = api.draft(&id, Some(text)).await;
                move |app: &mut App| saved.err().iter().for_each(|e| app.say(&e.to_string()))
            });
        }
    }

    /// Ctrl-C: waits up to 30 seconds for the last draft save, then quits. Behind a pending
    /// admission, words typed since go after the draft it left: the sent words, if refused.
    /// Effects still `finished` are dropped unapplied, freeing what they hold: a read page holds
    /// the one page permit an open queued ahead of the save waits for.
    async fn quit(&mut self, finished: mpsc::Receiver<Done>) -> Result<(), Error> {
        drop(finished);
        let (pending, saved) = (self.sending.is_some(), self.focus.clone());
        let (api, writes) = (self.api.clone(), self.writes.take());
        let mut text = self.composer.text.clone();
        let (tx, rx) = oneshot::channel::<Result<(), Error>>();
        let job: Job = Box::pin(async move {
            let result = match (saved, api) {
                (Some(_), Some(_)) if pending && text.is_empty() => Ok(()),
                (Some(id), Some(api)) => {
                    async {
                        let kept = match pending {
                            true => api.draft(&id, None).await?,
                            false => String::new(),
                        };
                        if !kept.is_empty() {
                            text = format!("{kept}\n{text}");
                        }
                        api.draft(&id, Some(text)).await.map(drop)
                    }
                    .await
                }
                _ => Ok(()),
            };
            let _ = tx.send(result);
            Box::new(|_: &mut App| {}) as Done
        });
        let closed = "The draft writer closed; keep the unsent words before quitting.";
        let saving = async {
            let writes = writes.ok_or_else(|| error(closed))?;
            writes.send(job).await.map_err(|_| error(closed))?;
            rx.await.map_err(fail(closed))?
        };
        let late = "Saving the draft timed out; check its saved words before sending again.";
        let saved = tokio::time::timeout(Duration::from_secs(30), saving).await;
        saved.map_err(fail(late))?
    }

    /// The line left on the normal screen once the app has quit: the root session ended, or it
    /// stays open on the server and `--resume` reopens it; none before a session started.
    fn farewell(&self) -> Option<String> {
        let id = self.root.as_ref()?;
        let row = self.agents.get(id);
        let name = row.map_or("", |a| a.name.as_str());
        let name = if name.is_empty() { id.as_str() } else { name };
        let ended = row.is_some_and(|a| a.state == State::Done || a.settled.is_some());
        Some(match ended {
            true => format!("Session ended: {name}."),
            false => format!("Left {name}; it stays open. Reopen it with razor --resume {id}"),
        })
    }

    fn tick(&mut self) {
        self.dirty = true;
        if self.draft_at.is_some_and(|at| at <= Instant::now()) && self.pending < 8 {
            self.draft_at = None;
            self.save();
        }
    }

    /// Marks the editor changed; its words are saved half a second after the last change.
    fn edited(&mut self) {
        self.edits += 1;
        self.draft_at = Some(Instant::now() + Duration::from_millis(500));
        composer::completion::edited(self, now());
    }

    // The drawn screen's next change, paced; while anything works, the clocks' seconds, of the wall
    // and of the focused turn; a draft save and the notice's end; else the minute.
    fn wake(&self, last_draw: Instant) -> Instant {
        let now = now();
        let working = self.agents.values().any(|a| a.state == State::Working);
        let seconds = [Some(0), self.busy_at].map(|since| {
            since
                .filter(|_| working)
                .map(|since| now + 1000 - now.saturating_sub(since) % 1000)
        });
        // Unfocused, the drawn screen asks at most once a second; data still draws on arrival.
        let floor = |at: u64| at.max(now + if self.presence.focused { 0 } else { 1000 });
        let asks = [self.next.get().map(floor), seconds[0], seconds[1]];
        let at = (asks.into_iter().flatten().min()).unwrap_or(now + 60_000 - now % 60_000);
        let moving = self.moving.is_some();
        let mut wake = motion::deadline(at, now, last_draw, self.cost, moving);
        if self.dirty {
            wake = wake.min(last_draw + motion::pace(self.cost, moving));
        }
        if let Some(at) = self.draft_at {
            wake = wake.min(at);
        }
        if let Some((_, until)) = self.notice.as_ref().filter(|(_, until)| *until > now) {
            wake = wake.min(Instant::now() + Duration::from_millis(until - now));
        }
        wake
    }

    /// The model and effort the next input would carry: a staged choice, the agent's row,
    /// then the configured defaults.
    fn choice(&self) -> (&str, Option<crate::protocol::Effort>) {
        let row = self.row();
        let initial = self.initial.as_ref();
        let model = initial
            .and_then(|v| v.model.as_deref())
            .or_else(|| row.map(|v| v.model.as_str()));
        let model = model.or_else(|| self.models.as_ref().map(|v| v.model.as_str()));
        let effort = initial
            .and_then(|v| v.effort)
            .or_else(|| row.map(|v| v.effort));
        (
            model.unwrap_or(&self.model),
            effort.or_else(|| self.models.as_ref().map(|v| v.effort)),
        )
    }
    fn speed(&self) -> crate::protocol::Speed {
        self.initial
            .as_ref()
            .and_then(|i| i.speed)
            .or_else(|| self.row().map(|r| r.speed))
            .or_else(|| self.models.as_ref().map(|m| m.speed))
            .unwrap_or_default()
    }
    /// The addressed session's status row.
    fn row(&self) -> Option<&AgentView> {
        self.agents.get(self.focus.as_ref()?)
    }
    fn split(&self) -> bool {
        self.focus != self.root
    }
    /// A child's split in full screen, where Esc goes back; with the agents list open, at any
    /// width, the addressed session is the conversation itself, spoken to and stopped as in full screen.
    fn split_view(&self) -> bool {
        self.split() && !self.list.open
    }
    /// Octant pictures only from `PHONE` columns and outside tmux: a phone reaches Razor
    /// through mosh and tmux, which drop or mismeasure a glyph their host's width table lacks
    /// (macOS's lacks the octants), and a phone font may not draw them.
    fn octant(&self) -> bool {
        let plain = self.columns < view::PHONE || self.tmux;
        self.ui.blocks == crate::protocol::Blocks::Octant && !plain
    }
    fn transcript(&self) -> Option<&Transcript> {
        self.transcripts.get(self.focus.as_ref()?)
    }
    fn transcript_mut(&mut self) -> Option<&mut Transcript> {
        self.transcripts.get_mut(self.focus.as_ref()?)
    }
    /// The child whose card Tab or Ctrl-N selected.
    fn selected_child(&self) -> Option<&SessionId> {
        match &self.selected {
            Some(view::Action::Child(id)) => Some(id),
            _ => None,
        }
    }
    /// A toast for 2.6 seconds, Loom's `TOAST_MS`.
    fn say(&mut self, text: &str) {
        self.notice = Some((text::clip(text, 512), now() + 2600));
    }
    /// Every open ask, the addressed session's first, oldest first within a session.
    fn asks(&self) -> Vec<&Ask> {
        let mine = |a: &&Ask| self.focus.as_ref() == Some(&a.id.0);
        let all = self.agents.values().flat_map(|row| &row.asks);
        // The focused session's own asks first, then the others' in the order they opened.
        let (mut asks, mut rest): (Vec<_>, Vec<_>) = all.partition(mine);
        rest.sort_by_key(|a| a.at);
        asks.extend(rest);
        asks
    }
    /// The shown ask: the one being answered in words, else the one the person moved to, while
    /// it is open; else the first.
    fn question(&self) -> Option<(SessionId, EntryId, Ask)> {
        let asks = self.asks();
        let mut held = [&self.answering, &self.ask].into_iter().flatten();
        let held = held.find_map(|id| asks.iter().find(|a| &a.id == id));
        let ask = held.or(asks.first())?;
        Some((ask.id.0.clone(), ask.id.1, (*ask).clone()))
    }
}
