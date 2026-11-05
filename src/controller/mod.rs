//! The session controller (docs/design/controller.md): every input enters through one admission
//! path, each loaded session is one task that owns its record and its turn, and both model paths
//! speak the same conversation messages.

mod files;
mod questions;
mod session;
mod surface;

use crate::children;
use crate::config::{Config, Settings};
use crate::protocol::{
    AgentView, Answer, Body, Brand, Child, ChildRequest, Effort, EntryId, Image, Input, Listed,
    Live, ModelSettings, Origin, Phone, SessionId, Speed, State, Thread, Trigger, Ui, UsageWindow,
};
use crate::record::{self, Record, Replay, Summary};
use crate::residents::Deliveries;
use crate::responses::Responses;
use crate::tools::{Auth, Tools};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use session::Session;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use std::{error, fmt, io};
use tokio::io::AsyncReadExt;
use tokio::sync::{Notify, broadcast, mpsc, oneshot};

/// Every session this server has loaded. Cheap to clone; any task may call any method.
#[derive(Clone)]
pub struct Controller {
    inner: Arc<Inner>,
}

struct Inner {
    home: PathBuf,
    /// The last good config; holding its lock also serializes reloads.
    config: tokio::sync::Mutex<Settings>,
    mcp_auth: Auth,
    mcp_revision: AtomicU64,
    address: Option<String>,
    endpoint: String,
    responses: Arc<Responses>,
    bridge: crate::claude::SharedBridge,
    children: children::Children,
    devices: Arc<crate::devices::Devices>,
    memory: crate::memory::Memory,
    deliveries: Deliveries,
    plugins: crate::extensions::Plugins,
    /// Each loaded session's queue of work, 256 deep; cold idle tasks remove their sender.
    sessions: Mutex<HashMap<SessionId, mpsc::Sender<Work>>>,
    /// One resumable fold per configured resident; no entry-offset indexes survive unloading.
    summaries: record::Summaries,
    /// Sessions idle with a conversation open, longest idle first; at most `sessions.idle`.
    idle: Mutex<VecDeque<SessionId>>,
    status: Mutex<Status>,
    /// Held by `create` from naming a root to its row being listed, so two never share a name.
    naming: tokio::sync::Mutex<()>,
    /// Woken whenever a session answers inputs, so an internal input waiting for room retries.
    space: Notify,
    /// When this server started: a live call recorded as starting earlier is no longer running.
    started: crate::protocol::Millis,
}

/// The controller-wide stream: a row per agent, with its own readings, and the config notice.
struct Status {
    agents: BTreeMap<SessionId, AgentView>,
    remote: BTreeMap<SessionId, (SessionId, record::ChildSummary)>,
    config: Option<String>,
    /// The brand, `[ui]` and `[phone]` settings last sent.
    shown: (Brand, Ui, Phone),
    /// 1,024 lines; a client further behind gets a fresh snapshot.
    lines: broadcast::Sender<Live>,
}

/// Work for a session's task: it runs with the session to itself, in queue order, and an error
/// it returns ends the session's turn.
type Work = Box<dyn for<'a> FnOnce(&'a mut Session) -> BoxFuture<'a, Result<(), Error>> + Send>;

impl Controller {
    /// Start on the state directory `home`. Each session `record::live` names is loaded: calls
    /// with no result get `Unknown`, lost jobs end `Unknown` with their notices held for the
    /// next input, a session whose turn was open is rebuilt (`Restart`) and goes on, and one
    /// with idle maintenance due stays loaded for it.
    pub async fn start(home: PathBuf, responses: Arc<Responses>) -> Result<Controller, Error> {
        let settings = Settings::open(home.clone())?;
        let config = settings.config.clone();
        let deliveries = Deliveries::open(&home)
            .await
            .map_err(other("opening deliveries"))?;
        let status = Status {
            agents: BTreeMap::new(),
            remote: BTreeMap::new(),
            config: None,
            shown: (config.brand, config.ui, config.phone),
            lines: broadcast::channel(1024).0,
        };
        let controller = Controller {
            inner: Arc::new(Inner {
                address: config.server.address.clone(),
                endpoint: config.responses.base_url.clone(),
                children: children::Children::open(home.clone()),
                devices: Arc::new(
                    crate::devices::Devices::open(&home)
                        .await
                        .map_err(|e| Error::Invalid(e.to_string()))?,
                ),
                plugins: crate::extensions::Plugins::new(home.clone()),
                config: tokio::sync::Mutex::new(settings),
                mcp_auth: Auth::new(home.clone()),
                mcp_revision: AtomicU64::new(0),
                bridge: crate::claude::SharedBridge::new(&home),
                memory: crate::memory::Memory::new(&home),
                home,
                responses,
                deliveries,
                sessions: Mutex::default(),
                summaries: record::Summaries::default(),
                idle: Mutex::default(),
                status: Mutex::new(status),
                naming: tokio::sync::Mutex::new(()),
                started: crate::protocol::now(),
                space: Notify::new(),
            }),
        };
        controller.inner.children.post.start(&controller);
        for session in record::live(&controller.inner.home).await? {
            let _ = controller.tell(&session, |s| Box::pin(s.restart())).await;
        }
        let rows = controller.clone();
        tokio::spawn(async move { rows.recent().await });
        let ticker = controller.clone();
        tokio::spawn(async move {
            for tick in (1..=60).cycle() {
                tokio::time::sleep(Duration::from_secs(1)).await;
                ticker.config().await;
                // Recheck cold tasks after a client or the last queued caller leaves.
                for task in lock(&ticker.inner.sessions).values() {
                    let _ = task.try_send(Box::new(|s| Box::pin(s.tick())));
                }
                if tick == 60 {
                    ticker.stuck().await;
                }
            }
        });
        Ok(controller)
    }

    /// An MCP operation in the session's project, or `cwd`'s without one; a file change reaches
    /// every loaded session at its next call.
    pub(crate) async fn mcp(
        &self,
        cwd: &Path,
        session: Option<&SessionId>,
        operation: crate::protocol::McpOperation,
    ) -> Result<Value, Error> {
        use crate::protocol::McpOperation::*;
        let (settings, tools) = match session {
            Some(id) => self.session_settings(id).await.map(|(s, t)| (s, Some(t)))?,
            None => (self.inner.config.lock().await.project(cwd)?, None),
        };
        let auth = &self.inner.mcp_auth;
        let result =
            crate::config::manage::run(&settings, auth, tools.as_deref(), &operation).await?;
        if matches!(
            operation,
            Add { .. } | Remove { .. } | Enable { .. } | Disable { .. }
        ) {
            self.inner.mcp_revision.fetch_add(1, Ordering::Release);
        }
        // The sign-in's question is recorded and tied to its attempt before the address is
        // returned, so no callback can arrive first; no agent turn starts.
        if let (SignIn { name }, Some(id)) = (operation, session) {
            let url = result["url"].as_str().unwrap_or_default();
            let text = crate::profiles::mcp_text("sign_in", &[("name", &name), ("url", url)]);
            let entry = questions::ask(self, id, text, Vec::new(), true).await?;
            auth.asked(&name, (id.clone(), entry)).await;
        }
        Ok(result)
    }

    /// The machine's settings as last read good, read again first; no project's files.
    pub(crate) async fn machine_settings(&self) -> Settings {
        self.config().await;
        self.inner.config.lock().await.clone()
    }

    /// A session's settings as its files now read; credential references stay unresolved.
    pub(crate) async fn settings(&self, session: &SessionId) -> Result<Settings, Error> {
        Ok(self.session_settings(session).await?.0)
    }

    async fn session_settings(&self, id: &SessionId) -> Result<(Settings, Arc<Tools>), Error> {
        self.ask(id, |s| {
            Box::pin(async move {
                s.settings.read();
                Ok((s.settings.clone(), s.tools.clone()))
            })
        })
        .await
    }

    /// The callback's landed address: a sign-in a session asked for is answered in its question
    /// there, as a pasted address is; any other completes here. Either consumes the same
    /// state-bound, single-use attempt.
    pub(crate) async fn mcp_complete(&self, redirect: &str) -> Result<(), Error> {
        if let Some((session, ask)) = self.inner.mcp_auth.question(redirect).await {
            let answer = Answer::Text(redirect.into());
            return self.answer(&session, ask, answer, None).await.map(drop);
        }
        let completed = self.inner.mcp_auth.complete(redirect).await;
        completed.map_err(|e| Error::Invalid(e.to_string()))
    }

    /// The child actions; the input route admits a video through them, to start its watcher.
    pub(crate) fn children(&self) -> &children::Children {
        &self.inner.children
    }

    /// Current enrollment and paired transport shared by the API and tools.
    pub(crate) fn devices(&self) -> &Arc<crate::devices::Devices> {
        &self.inner.devices
    }

    /// Memory, which the sessions and the residents' phone list share.
    pub(crate) fn memory(&self) -> &crate::memory::Memory {
        &self.inner.memory
    }

    /// The delivery index and subscription the session actions and the API share.
    pub fn deliveries(&self) -> Deliveries {
        self.inner.deliveries.clone()
    }

    /// The admission path: every input to every session enters here. The input is appended at
    /// once and reaches the model at its next turn or tool boundary (SPEC section 3 says which),
    /// after the rebuilt context during a rebuild. At most 64 wait unread by the model: past that
    /// a person's input or an answer is refused `Full`, and any other input waits for room.
    pub(crate) async fn admit(&self, session: &SessionId, input: Input) -> Result<EntryId, Error> {
        loop {
            let space = self.inner.space.notified();
            tokio::pin!(space);
            space.as_mut().enable();
            let given = input.clone();
            // A person's input continues a closed child as `continue` does (Trey, 2026-10-31).
            match self.ask(session, |s| Box::pin(s.take(given, false))).await {
                Err(Error::Full(_))
                    if !matches!(input.from, Origin::Person | Origin::Answer { .. }) =>
                {
                    space.await
                }
                admitted => return admitted,
            }
        }
    }

    /// Escape: kill the turn's foreground jobs and interrupt the model; the turn ends `Stopped`,
    /// and inputs waiting for it open the next turn.
    pub(crate) async fn stop(&self, session: &SessionId) -> Result<(), Error> {
        self.tell(session, |s| Box::pin(s.stop())).await
    }

    /// The session's stream from `after`, as lines ready to send: `attached`, the entries after
    /// the cursor, then each line as it happens. A client 1,024 lines behind is attached again
    /// from its cursor, with no gap.
    pub(crate) async fn attach(
        &self,
        session: &SessionId,
        after: EntryId,
        terminal: i32,
    ) -> Result<mpsc::Receiver<Arc<str>>, Error> {
        let mut next = Ok(self.attachment(session, after).await?);
        let (tx, rx) = mpsc::channel(64);
        let (controller, session) = (self.clone(), session.clone());
        lock(&self.inner.status).open(&session, terminal, None);
        tokio::spawn(async move {
            'attach: while let Ok((attached @ Live::Attached { last, .. }, mut replay, mut lines)) =
                next
            {
                if tx.send(live(&attached)).await.is_err() {
                    break 'attach;
                }
                while let Some(line) = replay.next_line().await {
                    let Ok(line) = line else { break 'attach };
                    if tx.send(line.into()).await.is_err() {
                        break 'attach;
                    }
                }
                drop(replay);
                let mut cursor = last;
                let seen = |(id, line): (Option<EntryId>, _)| {
                    cursor = id.unwrap_or(cursor);
                    line
                };
                if !relay(&mut lines, &tx, seen).await {
                    break 'attach;
                }
                next = controller.attachment(&session, cursor).await;
            }
            lock(&controller.inner.status).open(&session, -terminal, None);
        });
        Ok(rx)
    }

    /// A reader saw `session` through `entry`: its status row says so, and when, until this
    /// server restarts.
    pub(crate) fn seen(&self, session: &SessionId, entry: EntryId) {
        lock(&self.inner.status).open(session, 0, Some(entry));
    }

    /// Pending inputs, last entry, replay and subscription, taken in one session operation.
    async fn attachment(&self, session: &SessionId, after: EntryId) -> Result<Attachment, Error> {
        self.ask(session, move |s| Box::pin(s.attachment(after)))
            .await
    }

    /// The agents snapshot, then every change; the snapshot and subscription share one lock,
    /// and a client 1,024 lines behind gets a fresh snapshot.
    pub(crate) fn status(&self) -> mpsc::Receiver<Live> {
        let (tx, rx) = mpsc::channel(64);
        let controller = self.clone();
        tokio::spawn(async move {
            loop {
                let (snapshot, mut lines) = {
                    let status = lock(&controller.inner.status);
                    (status.snapshot(), status.lines.subscribe())
                };
                if tx.send(snapshot).await.is_err() || !relay(&mut lines, &tx, |l| l).await {
                    return;
                }
            }
        });
        rx
    }

    /// A fresh bounded read; an invalid edit keeps the last good value, and a changed notice or
    /// brand or ui reaches the status stream once.
    pub async fn config(&self) -> Config {
        let mut settings = self.inner.config.lock().await;
        settings.read();
        let config = &settings.config;
        self.inner
            .summaries
            .configure(config.residents.keys().cloned());
        let mut changed = Vec::new();
        if config.server.address != self.inner.address {
            changed.push("server.address");
        }
        if config.responses.base_url != self.inner.endpoint {
            changed.push("responses.base_url");
        }
        let notice = if !settings.errors.is_empty() {
            Some(settings.errors.join(" "))
        } else {
            (!changed.is_empty()).then(|| {
                format!(
                    "{} changed; restart Razor to apply it.",
                    changed.join(" and ")
                )
            })
        };
        let plugins = self.inner.plugins.load(&config.plugins).await;
        let notice = notice
            .into_iter()
            .chain(plugins)
            .reduce(|a, b| format!("{a} {b}"));
        let mut status = lock(&self.inner.status);
        if status.config != notice {
            let shown = notice.as_deref().unwrap_or("config.toml is valid again.");
            eprintln!("razor: {shown}");
        }
        let shown = (
            config.brand.clone(),
            config.ui.clone(),
            config.phone.clone(),
        );
        if status.config != notice || status.shown != shown {
            (status.config, status.shown) = (notice, shown);
            let _ = status.lines.send(status.snapshot());
        }
        config.clone()
    }

    /// Runs `work` on the session's task and returns its answer.
    async fn ask<T: Send + 'static>(
        &self,
        session: &SessionId,
        work: impl for<'a> FnOnce(&'a mut Session) -> BoxFuture<'a, Result<T, Error>> + Send + 'static,
    ) -> Result<T, Error> {
        let (reply, answer) = oneshot::channel();
        self.tell(session, move |s| {
            Box::pin(async move {
                let result = work(s).await;
                let missing = match &result {
                    Err(Error::NoSession(gone)) => Err(Error::NoSession(gone.clone())),
                    _ => Ok(()),
                };
                let _ = reply.send(result);
                missing
            })
        })
        .await?;
        answer
            .await
            .map_err(|_| Error::NoSession(session.clone()))?
    }

    /// Queues `work` on the session's task, loading the session first if it is not loaded.
    async fn tell(
        &self,
        session: &SessionId,
        work: impl for<'a> FnOnce(&'a mut Session) -> BoxFuture<'a, Result<(), Error>> + Send + 'static,
    ) -> Result<(), Error> {
        let task = self.task(session, None);
        task.send(Box::new(work))
            .await
            .map_err(|_| Error::NoSession(session.clone()))
    }

    /// The session's queue, its task started (its record opened, unless given) if not loaded.
    fn task(&self, session: &SessionId, record: Option<Record>) -> mpsc::Sender<Work> {
        let mut sessions = lock(&self.inner.sessions);
        if let Some(task) = sessions.get(session) {
            return task.clone();
        }
        let (task, queue) = mpsc::channel(256);
        sessions.insert(session.clone(), task.clone());
        tokio::spawn(session::run(self.clone(), session.clone(), record, queue));
        task
    }

    /// Notes a session going idle with its conversation open, or leaving that; past `cap` such
    /// sessions, the longest idle closes its conversation.
    fn mark(&self, session: &SessionId, idle: bool, cap: usize) {
        let mut list = lock(&self.inner.idle);
        list.retain(|s| s != session);
        if idle {
            list.push_back(session.clone());
        }
        let (over, sessions) = (list.len().saturating_sub(cap), lock(&self.inner.sessions));
        for session in list.drain(..over) {
            if let Some(task) = sessions.get(&session) {
                let _ = task.try_send(Box::new(|s| Box::pin(s.unload())));
            }
        }
    }

    /// Marks `Stuck` each working session that has waited `stuck.after` with inputs unanswered,
    /// no job running, no question open, no `agent wait` before its deadline and no working
    /// descendant.
    async fn stuck(&self) {
        let after = self.inner.config.lock().await.config.stuck.after;
        let after = after.as_millis() as u64;
        let sessions: Vec<_> = lock(&self.inner.sessions).keys().cloned().collect();
        for id in sessions {
            let Ok(s) = self.summary(&id).await else {
                continue;
            };
            if s.unanswered.is_empty()
                || !s.jobs.is_empty()
                || !s.questions.is_empty()
                || !s.mail_questions.is_empty()
                || crate::protocol::now().saturating_sub(s.at) < after
                || self.ask(&id, Session::waiting).await.unwrap_or(true)
            {
                continue;
            }
            let mut status = lock(&self.inner.status);
            let descendant = status.agents.values().any(|row| {
                let up = |p: &&SessionId| status.agents.get(*p).and_then(|r| r.parent.as_ref());
                row.state == State::Working
                    && std::iter::successors(row.parent.as_ref(), up).any(|p| *p == id)
            });
            let Status { agents, lines, .. } = &mut *status;
            if let Some(row) = agents.get_mut(&id)
                && row.state == State::Working
                && !descendant
            {
                row.state = State::Stuck;
                let _ = lines.send(Live::Agent(Box::new(row.clone())));
            }
        }
    }

    fn forget(&self, session: &SessionId) {
        lock(&self.inner.sessions).remove(session);
        self.inner.space.notify_waiters();
        lock(&self.inner.idle).retain(|id| id != session);
        let mut status = lock(&self.inner.status);
        status.remote.retain(|_, (parent, _)| parent != session);
        if status.agents.remove(session).is_some() {
            let _ = status.lines.send(status.snapshot());
        }
    }

    fn dir(&self, session: &SessionId) -> PathBuf {
        self.inner.home.join("sessions").join(session.as_str())
    }
}

/// The captured attached line, replay and subscription for starting or resuming a stream.
type Attachment = (
    Live,
    Replay,
    broadcast::Receiver<(Option<EntryId>, Arc<str>)>,
);

impl Status {
    fn snapshot(&self) -> Live {
        Live::Agents {
            agents: self.agents.values().cloned().collect(),
            config: self.config.clone(),
            brand: self.shown.0.clone(),
            ui: Box::new(self.shown.1.clone()),
            phone: self.shown.2.clone(),
            server: Default::default(),
            servers: Vec::new(),
        }
    }

    /// Stores `row`, sending it when anything but its activity time changed.
    fn row(&mut self, row: AgentView) {
        let changed = self.agents.get_mut(&row.session).is_none_or(|old| {
            old.last_activity = row.last_activity;
            row != *old
        });
        if changed {
            let _ = self.lines.send(Live::Agent(Box::new(row.clone())));
        }
        self.agents.insert(row.session.clone(), row);
    }

    /// A stream to `session` opened or closed, a terminal app's counting 1 or -1, or a reader
    /// reported the last entry it saw: each is the person looking.
    fn open(&mut self, session: &SessionId, change: i32, seen: Option<EntryId>) {
        if let Some(mut row) = self.agents.get(session).cloned() {
            row.terminals = row.terminals.saturating_add_signed(change);
            row.looked_at = Some(crate::protocol::now());
            row.seen = seen.or(row.seen);
            self.row(row);
        }
    }

    fn usage(&mut self, session: &SessionId, mut windows: Vec<UsageWindow>) {
        if let Some(mut row) = self.agents.get(session).cloned() {
            windows.truncate(32);
            row.usage = windows;
            self.row(row);
        }
    }

    /// Builds agent rows from the same fold without requiring a loaded session, with the
    /// model's window and threshold as `profiles::limits` gives them; a session's first row
    /// also projects its children's.
    fn record(
        &mut self,
        s: &Summary,
        state: State,
        context: Option<u64>,
        closing: bool,
        (window, threshold): (Option<u64>, f64),
    ) {
        let (child, old) = (s.child.as_ref(), self.agents.get(&s.session));
        let first = old.is_none();
        self.row(AgentView {
            session: s.session.clone(),
            parent: child.map(|c| c.parent.clone()),
            launched: child.map(|c| c.launched),
            name: s.name.clone(),
            model: s.model.clone(),
            effort: s.settings.effort,
            speed: s.settings.speed,
            state,
            closing,
            close_reason: s.close_reason,
            headline: s.headline.clone(),
            headline_at: s.headline_at,
            last_activity: s.activity,
            person_input_at: s.person_input_at,
            spoke_at: s.spoke_at,
            last_reply: s.last_reply.clone(),
            looked_at: old.and_then(|r| r.looked_at),
            seen: old.and_then(|r| r.seen),
            context: context.or(s.context),
            window,
            threshold,
            branch: child
                .and_then(|c| c.worktree.as_ref())
                .map(|_| children::branch(&s.session)),
            changed_files: s.changed_files.clone(),
            calls: s.unresolved.iter().map(|(_, at)| *at).collect(),
            calls_made: s.calls_made,
            failed: s.failed,
            asks: s.questions.clone(),
            report: old.and_then(|r| r.report.clone()),
            cache: s.cache,
            usage: old.map_or_else(Vec::new, |r| r.usage.clone()),
            settled: old.and_then(|r| r.settled),
            project: children::project(s),
            repo: children::repo(s),
            rebuilt: s.last_rebuild.map(|(_, at)| at),
            error: s.error().map(String::from),
            interrupted: s.interrupted(),
            resident: s.thread.as_ref().map(|t| t.resident.clone()),
            resident_project: s.thread.as_ref().and_then(|t| t.resident_project.clone()),
            terminals: old.map_or(0, |r| r.terminals),
        });
        if first {
            self.child_rows(s);
        }
    }

    /// A parent's record decides its children's reports and whether they are done.
    fn child_rows(&mut self, parent: &Summary) {
        for child in &parent.children {
            if child.session.address().is_some() {
                let fact = (parent.session.clone(), child.clone());
                if self.remote.insert(child.session.clone(), fact.clone()) != Some(fact) {
                    let _ = self.lines.send(self.snapshot());
                }
            }
            let Some(row) = self.agents.get_mut(&child.session) else {
                continue;
            };
            let before = row.clone();
            row.report = child.report.clone().filter(|_| !child.stopped);
            row.settled = child.settled;
            if !child.working() {
                row.state = State::Done;
            } else if before.state == State::Done {
                row.state = State::NeedsYou;
            }
            if *row != before {
                let _ = self.lines.send(Live::Agent(Box::new(row.clone())));
            }
        }
    }
}

/// Forwards `lines`, as `seen` makes them, until the client leaves (false) or falls 1,024
/// lines behind (true).
async fn relay<T: Clone, U>(
    lines: &mut broadcast::Receiver<T>,
    tx: &mpsc::Sender<U>,
    mut seen: impl FnMut(T) -> U,
) -> bool {
    loop {
        let received = tokio::select! {
            line = lines.recv() => line,
            _ = tx.closed() => return false,
        };
        let sent = match received {
            Ok(line) => tx.send(seen(line)).await,
            Err(broadcast::error::RecvError::Lagged(_)) => return true,
            Err(broadcast::error::RecvError::Closed) => return false,
        };
        if sent.is_err() {
            return false;
        }
    }
}

/// A lock whose holder panicked still guards whole data: no update spans an unlock.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A live line as sent to clients; it holds only strings, numbers and plain enums.
fn live(line: &Live) -> Arc<str> {
    serde_json::to_string(line).unwrap_or_default().into()
}

/// Runs blocking file work on a thread of its own.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(other("accessing the session directory"))?
}

fn invalid<T>(sentence: &str) -> Result<T, Error> {
    Err(Error::Invalid(sentence.into()))
}

fn other<E: error::Error + Send + Sync + 'static>(
    context: &'static str,
) -> impl FnOnce(E) -> Error {
    move |source| Error::Other {
        source: Box::new(source),
        context,
    }
}

#[derive(Debug)]
pub enum Error {
    /// No session with this id.
    NoSession(SessionId),
    /// The session already holds 64 inputs it has not answered.
    Full(SessionId),
    /// The session is working, so this heartbeat was skipped.
    Busy(SessionId),
    /// The request cannot be met as asked; the sentence says why.
    Invalid(String),
    Other {
        source: Box<dyn error::Error + Send + Sync>,
        context: &'static str,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoSession(s) => write!(f, "There is no session {s}."),
            Error::Full(s) => write!(
                f,
                "Session {s} holds 64 messages the model has not read yet; wait for it, or stop it."
            ),
            Error::Busy(s) => write!(f, "Session {s} is busy; this heartbeat was skipped."),
            Error::Invalid(sentence) => f.write_str(sentence),
            Error::Other { source, context } => write!(f, "Razor failed {context}: {source}."),
        }
    }
}

impl error::Error for Error {}

impl From<record::Error> for Error {
    fn from(e: record::Error) -> Error {
        match e {
            record::Error::Missing(id) => Error::NoSession(id),
            record::Error::NoEntry(id) => {
                Error::Invalid(format!("The record has no entry {}.", id.0))
            }
            e => other("accessing the record")(e),
        }
    }
}

impl From<crate::config::Error> for Error {
    fn from(e: crate::config::Error) -> Error {
        match e {
            crate::config::Error::Invalid(sentence) => Error::Invalid(sentence),
            e => other("reading the config")(e),
        }
    }
}

impl From<crate::profiles::Error> for Error {
    fn from(e: crate::profiles::Error) -> Error {
        match e {
            crate::profiles::Error::Other { source, context } => Error::Other { source, context },
            e => Error::Invalid(e.to_string()),
        }
    }
}

impl From<crate::extensions::Error> for Error {
    fn from(e: crate::extensions::Error) -> Error {
        Error::Invalid(e.to_string())
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        other("accessing the session directory")(e)
    }
}
