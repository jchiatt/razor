//! One session's task. It alone owns the record, admission, the turn and the conversation;
//! action futures never own it, and both model paths reach it through the same messages.

mod actions;
mod conversation;
mod idle;
mod inputs;
mod turn;

use conversation::next;

use super::{Controller, Error, Ordering, Settings, Work, invalid, lock, other};
use crate::children;
use crate::claude::Output;
use crate::extensions::{Before, Plugins};
use crate::profiles::{self, Profile, Transport, limits};
use crate::protocol::{
    Answer, Body, BridgeInput, CallId, CloseReason, Command, End, EntryId, FromBridge, Input,
    JobEvent, JobId, Live, MailKind, Millis, ModelSettings, Native, Origin, Outcome, Resume,
    SessionId, State, ToBridge, Trigger, Usage, now,
};
use crate::record::{self, Record};
use crate::{code_mode::FAILED, shell::Shell, tools::Tools};
use futures_util::{StreamExt, future::BoxFuture, stream::FuturesUnordered};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Component;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, broadcast, mpsc, oneshot, watch};

/// The most inputs a session holds that the model has not read.
const UNREAD: usize = 64;
/// The most news waiting calls hold that the model has not read (`taken`); a wait at it is refused.
const TAKEN: usize = 64;

pub(super) struct Session {
    c: Controller,
    id: SessionId,
    pub(super) record: Record,
    /// The transcript's live lines, 1,024 deep; a client further behind attaches again.
    pub(super) lines: broadcast::Sender<(Option<EntryId>, Arc<str>)>,
    shell: Arc<Shell>,
    pub(super) tools: Arc<Tools>,
    plugins: Plugins,
    /// Two events for each of the shell's 64 handles.
    events: mpsc::Receiver<JobEvent>,
    /// The session's settings files, read again at each turn's start and MCP operation.
    pub(super) settings: Settings,
    /// The MCP revision these settings were read at.
    mcp_revision: u64,
    /// The servers' availability the model last heard of, and whether the registry changed since.
    mcp_key: String,
    news: bool,
    profile: Arc<Profile>,
    model: String,
    model_settings: ModelSettings,
    reported: Option<u64>,
    context: Option<u64>,
    state: State,
    /// A finished, stopped or closed child holds its inputs until it is continued.
    blocked: bool,
    conversation: Option<Conversation>,
    turn: children::Turn,
    /// Running actions, at most one model step's calls; dropping one cancels it.
    actions: FuturesUnordered<BoxFuture<'static, Ran>>,
    /// FIFO: read-only actions hold it together, every other action alone, in call order.
    order: Arc<RwLock<()>>,
    /// Admitted inputs not yet sent to the model, in arrival order but for put-backs; at most 64
    /// but for news put back (see `unknown_results`).
    pending: VecDeque<(EntryId, Input)>,
    /// Inputs sent to the model path and not yet read, in the order sent; at most UNREAD, since
    /// `fit` caps each delivery.
    sent: Vec<EntryId>,
    /// The inputs that opened the turn, which only its end answers; at most one message's.
    opening: Vec<EntryId>,
    /// Inputs sent to the conversation or covered by its rebuilt context, and not answered.
    delivered: Vec<EntryId>,
    /// Waiting `agent wait` calls, each with its Call entry, at most 256.
    waiters: VecDeque<(EntryId, oneshot::Sender<EntryId>)>,
    /// News waiting calls took, with the waiting Call's entry, or once a cell's call has returned
    /// it to the code, the cell's exec Call's (see `took`); at most TAKEN.
    taken: Vec<(EntryId, EntryId)>,
    /// This turn's jobs, each with its end notice once it ends; at most the shell's 64 handles.
    foreground: BTreeMap<JobId, Option<Input>>,
    /// Results that showed their job's end; once mirrored, that job's notice is dropped.
    shown: Vec<(CallId, JobId)>,
    /// End notices for the next turn, one per shell handle (64).
    notices: VecDeque<(JobId, Input)>,
    /// Jobs the person's stop killed (the record's after a restart), until their end notices are
    /// admitted or a Result shows their end; at most the shell's 64 handles, which either releases.
    stopped: BTreeSet<JobId>,
    /// The output size each running job's tail last showed.
    tails: BTreeMap<JobId, u64>,
    /// The newest thought's number since the session loaded.
    thoughts: u64,
    planned: Option<Trigger>,
    /// `/done`'s input: the turn that answers it is the session's last.
    done: Option<EntryId>,
}

/// An open model conversation: messages in, lines out, and what its batch must settle.
struct Conversation {
    send: mpsc::Sender<ToBridge>,
    /// 64 lines; a Claude line keeps its share of the bridge budget until it is handled.
    lines: mpsc::Receiver<Output>,
    profile: watch::Sender<Arc<Profile>>,
    model: String,
    /// The memory page and skills, sent ahead of a fresh conversation's first input.
    page: Option<String>,
    /// A rebuild's context and videos, sent ahead of the next batch.
    ahead: Vec<BridgeInput>,
    /// Resumed from native history; closing before it says anything rebuilds instead.
    resuming: bool,
    /// The rebuilt context's size, until the turn carrying it ends.
    rebuilt: Option<usize>,
    /// The threshold input sent to this conversation, and the rebuild it asks for: `Handover`
    /// when the person asked.
    threshold: Option<(EntryId, Trigger)>,
    /// The note of the handover call in the batch being settled; empty for the night's.
    handover: Option<String>,
    /// Escape interrupted this turn; the path has not yet ended it.
    interrupted: bool,
    /// The session's thoughts before this conversation's, which its own numbers follow.
    thoughts: u64,
    /// A finished child's parent and report, admitted once the batch settles.
    closing: Option<(SessionId, Input)>,
}

/// A finished action: its answer to the model, whether the batch must settle, a finished
/// child's report, and the job it left with whether its end was shown.
struct Ran {
    call: CallId,
    output: String,
    error: bool,
    settle: bool,
    close: Option<(SessionId, Input)>,
    job: Option<(JobId, bool)>,
    refresh: bool,
}

/// The session's task: load, then handle job events, registry changes, finished actions, queued
/// work, the conversation's lines and the tail timer, in that priority, until the server stops.
pub(super) async fn run(
    c: Controller,
    id: SessionId,
    record: Option<Record>,
    mut queue: mpsc::Receiver<Work>,
) {
    let created = record.is_some();
    let loaded = async {
        let record = match record {
            Some(record) => record,
            None => Record::open(&c.inner.home, &id).await?,
        };
        Session::load(c.clone(), id.clone(), record).await
    };
    let (mut s, lost) = match loaded.await {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!("razor: session {id}: {e}");
            c.forget(&id);
            return;
        }
    };
    let parent = match s.record.summary().child.clone() {
        Some(child) if child.parent.address().is_none() => c.summary(&child.parent).await.ok(),
        _ => None,
    };
    s.blocked = s.record.summary().done();
    if let Some(parent) = &parent {
        s.blocked |= parent
            .children
            .iter()
            .any(|f| f.session == id && !f.working());
    }
    s.publish_row();
    children::recover_loaded(c.clone(), s.record.summary());
    if let Some(parent) = &parent {
        lock(&c.inner.status).child_rows(parent);
    }
    // Only `create` hands over a record, whose first entry is the `Start` it just wrote.
    if created && let Ok(start) = s.record.get(EntryId(1)).await {
        let cwd = &s.record.summary().cwd;
        s.plugins.after(&s.id, cwd, &start.into(), &s.lines);
    }
    // Jobs lost in a restart are recorded, but their notices wait for the next input; a child
    // with nothing unanswered hears of them at once, since no person's input will come.
    let wake = s.record.summary().child.is_some() && s.record.summary().unanswered.is_empty();
    for event in lost {
        if let Err(e) = s.job_event(event, wake).await {
            s.fail(&e).await;
        }
    }
    let mut registry = s.tools.subscribe();
    let mut tail = tokio::time::Instant::now();
    loop {
        let done = tokio::select! {
            biased;
            Some(event) = s.events.recv() => s.job_event(event, true).await,
            Ok(()) = registry.changed() => {
                s.news = true;
                s.refresh().await
            }
            Some(ran) = s.actions.next(), if !s.actions.is_empty() => s.ran(ran).await,
            work = queue.recv() => match work {
                Some(work) => work(&mut s).await,
                None => break,
            },
            line = next(&mut s.conversation) => s.line(line).await,
            () = tokio::time::sleep_until(tail), if !s.record.summary().jobs.is_empty() => {
                s.tails().await;
                tail = tokio::time::Instant::now() + Duration::from_millis(250);
                Ok(())
            }
        };
        if matches!(&done, Err(Error::NoSession(gone)) if *gone == id) {
            c.forget(&id);
            break;
        }
        if let Err(e) = done {
            eprintln!("razor: session {}: {e}", s.id);
            s.fail(&e).await;
        }
        if s.idle(&queue) {
            if let Err(e) = c.inner.summaries.unload(s.record).await {
                eprintln!("razor: session {id}'s summary: {e}");
            }
            break;
        }
    }
}

impl Session {
    /// Captures withdrawable inputs, record cursor and subscription without interleaving work.
    pub(super) async fn attachment(&mut self, after: EntryId) -> Result<super::Attachment, Error> {
        let attached = Live::Attached {
            session: self.id.clone(),
            last: self.record.summary().last,
            pending: self.pending.iter().map(|(id, _)| *id).collect(),
        };
        let replay = self.record.replay(after).await?;
        Ok((attached, replay, self.lines.subscribe()))
    }

    async fn load(
        c: Controller,
        id: SessionId,
        record: Record,
    ) -> Result<(Session, Vec<JobEvent>), Error> {
        let settings = c.inner.config.lock().await.project(&record.summary().cwd)?;
        let mcp_revision = c.inner.mcp_revision.load(Ordering::Acquire);
        let (events, receiver) = mpsc::channel(128);
        let (s, dir) = (record.summary(), record.dir().to_path_buf());
        let (shell, lost) = Shell::open(dir.clone(), s.next_job.0, &s.jobs, events)
            .await
            .map_err(other("opening shell jobs"))?;
        let shell = Arc::new(shell);
        // Razor's own keys stay with Razor: a command or MCP server that printed its environment
        // would put them in the record (Trey, 2026-10-28, for the web keys).
        let names = crate::config::setup::names();
        let keys: Vec<_> = names.chain(crate::claude::KEYS).collect();
        let env = std::env::vars()
            .filter(|(name, _)| !keys.contains(&name.as_str()))
            .collect();
        let plugins = c.inner.plugins.clone();
        let tools = Tools::open(
            dir,
            s.cwd.clone(),
            env,
            s.started,
            shell.clone(),
            crate::tools::Config {
                plugins: plugins.tools(),
                ..settings.tools()
            },
            (
                c.inner.mcp_auth.clone(),
                super::questions::questions(c.clone(), id.clone()),
            ),
            c.inner.children.post.clone(),
        );
        let (config, child) = (&settings.config, s.child.is_some());
        let profile = config.profile(&c.inner.endpoint, &s.model, tools.actions(), child)?;
        // An admitted `/done` survives a crash until answered or withdrawn.
        let mut done = s.job();
        for &entry in &s.unanswered {
            if let Body::Input(input) = record.entry(entry).await?.body
                && input.command == Some(Command::Done)
            {
                done = Some(entry);
            }
        }
        let session = Session {
            c,
            id,
            lines: broadcast::channel(1024).0,
            shell,
            tools: Arc::new(tools),
            plugins,
            events: receiver,
            profile: Arc::new(profile),
            model: s.model.clone(),
            model_settings: s.settings,
            reported: s.window,
            context: s.context,
            stopped: s.stopped.iter().copied().collect(),
            record,
            settings,
            mcp_revision,
            mcp_key: String::new(),
            news: false,
            state: State::NeedsYou,
            blocked: false,
            conversation: None,
            turn: Default::default(),
            actions: FuturesUnordered::new(),
            order: Arc::default(),
            pending: VecDeque::new(),
            sent: Vec::new(),
            opening: Vec::new(),
            delivered: Vec::new(),
            waiters: VecDeque::new(),
            taken: Vec::new(),
            foreground: BTreeMap::new(),
            shown: Vec::new(),
            notices: VecDeque::new(),
            tails: BTreeMap::new(),
            thoughts: 0,
            planned: None,
            done,
        };
        Ok((session, lost))
    }

    /// Appends `body`, sends its line to attached clients and refreshes the agent row.
    pub(super) async fn append(
        &mut self,
        body: Body,
        native: Option<Native>,
    ) -> Result<EntryId, Error> {
        let failed = match &body {
            Body::TurnEnd { inputs, end, .. } if *end != End::Done => {
                Some((inputs.clone(), end.clone()))
            }
            Body::Withdrawn { inputs } => Some((inputs.clone(), End::Stopped)),
            _ => None,
        };
        let children = record::ChildSummary::changed_by(&body);
        // This task alone appends, so the next committed id is last + 1.
        let mut turn = self.turn;
        turn.recorded(&body, EntryId(self.record.summary().last.0 + 1));
        let (id, line) = self.record.append(body, native).await?;
        self.record.mark(self.due()).await?;
        self.turn = turn;
        if let Some((inputs, end)) = failed {
            crate::residents::failed(&self.settings.config, &self.id, &self.record, &inputs, &end)
                .await;
        }
        let cwd = &self.record.summary().cwd;
        self.plugins.after(&self.id, cwd, &line, &self.lines);
        let _ = self.lines.send((Some(id), line));
        self.publish_row();
        if children {
            lock(&self.c.inner.status).child_rows(self.record.summary());
        }
        Ok(id)
    }

    fn publish(&self, line: &Live) {
        let _ = self.lines.send((None, super::live(line)));
    }

    fn set_state(&mut self, state: State) {
        self.state = state;
        self.publish_row();
        let idle = state == State::NeedsYou && self.conversation.is_some();
        self.c
            .mark(&self.id, idle, self.settings.config.sessions.idle);
    }

    /// The session's agent row; its children's rows follow only an entry that changes them.
    fn publish_row(&self) {
        let limits = limits(&self.model, &self.settings.config, self.reported);
        let state = match (self.blocked, self.record.summary().questions.is_empty()) {
            (true, _) => State::Done,
            (_, true) => self.state,
            _ => State::NeedsYou,
        };
        lock(&self.c.inner.status).record(
            self.record.summary(),
            state,
            self.context,
            self.done.is_some() && self.record.summary().job().is_none(),
            limits,
        );
    }

    /// UNREAD inputs held or sent unread: a person's input or an answer is refused, any other
    /// input waits for room, and end notices, the threshold input and MCP news wait.
    fn full(&self) -> bool {
        self.pending.len() + self.sent.len() >= UNREAD
    }

    /// The model has read `latest` and every input sent before it; an id no longer sent was read
    /// already. Inputs waiting for room wake.
    fn read(&mut self, latest: Option<EntryId>) {
        if let Some(at) = latest.and_then(|l| self.sent.iter().position(|id| *id == l)) {
            self.sent.drain(..=at);
            self.c.inner.space.notify_waiters();
        }
    }

    /// The last turn is ending on `answered`: withdraw every other unanswered input;
    /// no turn starts after it, and the session closes as a finished child does.
    pub(super) async fn close_out(&mut self, answered: &[EntryId]) -> Result<(), Error> {
        (self.done, self.planned, self.blocked) = (None, None, true);
        self.c.inner.children.post.closed(self.id.as_str());
        self.c.inner.memory.closed(&self.c, self.record.summary());
        self.c.inner.children.closed(&self.c, self.record.summary());
        self.pending.clear();
        self.delivered.clear();
        self.sent.clear();
        let mut inputs = self.record.summary().unanswered.clone();
        inputs.retain(|id| !answered.contains(id));
        if !inputs.is_empty() {
            self.append(Body::Withdrawn { inputs }, None).await?;
        }
        Ok(())
    }

    /// Gives every call without a mirrored result `Unknown`. No Result handed the news they took
    /// to the model, so it goes back to the front of `pending`, in record order, for the next
    /// turn. This turn's jobs leave the foreground, those already ended queueing their notices.
    async fn unknown_results(&mut self) -> Result<(), Error> {
        for (call, _) in self.record.summary().unresolved.clone() {
            let unknown = Body::Result {
                call,
                outcome: Outcome::Unknown,
                output: record::UNKNOWN.into(),
                inputs: Vec::new(),
            };
            self.append(unknown, None).await?;
        }
        self.taken.sort_unstable_by_key(|(_, news)| *news);
        let taken = std::mem::take(&mut self.taken).into_iter();
        self.put_back(taken.map(|(_, news)| news).collect()).await?;
        self.shown.clear();
        self.release(false);
        Ok(())
    }

    /// Returns inputs the model never read to the front of `pending`, in the order given, and
    /// out of `delivered`; any other entry, a rebuild's, stays for the turn's end to answer.
    async fn put_back(&mut self, inputs: Vec<EntryId>) -> Result<(), Error> {
        let before = self.pending.len();
        for entry in inputs.into_iter().rev() {
            if let Body::Input(input) = self.record.entry(entry).await?.body {
                self.delivered.retain(|id| *id != entry);
                self.pending.push_front((entry, input));
            }
        }
        if self.pending.len() != before {
            self.publish(&Live::Pending {
                inputs: self.pending.iter().map(|(id, _)| *id).collect(),
            });
        }
        Ok(())
    }

    /// The news this Result answers, as the first the model reads after it was taken: what the
    /// call took as it waited, or for an exec or a `wait`, what the calls of the cell it reads
    /// took, since a call a cell makes hands its Result only to the cell's code. With `error`, a
    /// Result that delivered no news and showed no cell (a refusal), it answers and moves nothing.
    /// None when the call is not open.
    fn took(&mut self, call: &CallId, error: bool) -> Option<Vec<EntryId>> {
        let s = self.record.summary();
        let at =
            |ids: &[(CallId, EntryId)], id: &str| ids.iter().find(|c| c.0.0 == id).map(|c| c.1);
        let own = at(&s.unresolved, &call.0)?;
        // A call a cell made has its exec's id, a colon and a number; its news moves to the cell.
        let cell = call.0.rsplit_once(':').and_then(|(e, _)| at(&s.cells, e));
        let reads = s.reads(&call.0).unwrap_or(own);
        let leaves = |t: &mut (_, _)| !error && t.0 == reads && cell.map(|e| t.0 = e).is_none();
        Some(self.taken.extract_if(.., leaves).map(|t| t.1).collect())
    }

    /// Takes this turn's jobs out of the foreground: an ended job's notice is queued, and with
    /// `kill` a running job is killed.
    fn release(&mut self, kill: bool) {
        for (job, notice) in std::mem::take(&mut self.foreground) {
            match notice {
                Some(notice) => self.notices.push_back((job, notice)),
                None if kill => {
                    let shell = self.shell.clone();
                    tokio::spawn(async move {
                        if let Err(e) = shell.kill(job).await {
                            eprintln!("razor: could not stop job-{}: {e}", job.0);
                        }
                    });
                }
                None => {}
            }
        }
    }

    /// Escape: stops the running actions and kills this turn's foreground jobs, then has the
    /// model path interrupt the turn and end it `Stopped`, keeping the conversation. A second
    /// Escape, or one with no conversation, drops it and ends the turn here; held inputs open
    /// the next turn.
    pub(super) async fn stop(&mut self) -> Result<(), Error> {
        let quit = self.done == Some(EntryId(0));
        if self.state != State::Working && !quit {
            return Ok(());
        }
        self.actions.clear();
        // A job a dropped call started has queued its Started: it is this turn's to stop, marked
        // before the end the shell gives it, which may follow at once, is recorded.
        self.shell.started().await;
        while let Ok(event) = self.events.try_recv() {
            if let JobEvent::Started { job, .. } = event {
                self.stopped.insert(job);
            }
            self.job_event(event, true).await?;
        }
        let running = self.foreground.iter().filter(|j| j.1.is_none());
        self.stopped.extend(running.map(|(job, _)| *job));
        self.release(true);
        if let Some(c) = self.conversation.as_mut().filter(|c| !c.interrupted) {
            c.interrupted = true;
            return self.say(|session| ToBridge::Interrupt { session }).await;
        }
        self.conversation = None;
        // Inputs sent and not yet read were in the dropped conversation's queue: as for a finished
        // child (`settled`), they wait for the next turn, or `/quit`'s close_out withdraws them.
        let unread = std::mem::take(&mut self.sent);
        self.put_back(unread).await?;
        self.end_turn(self.delivered.clone(), End::Stopped, Usage::default(), None)
            .await?;
        self.start_turn().await
    }

    /// Holds later inputs, stops the turn and kills every job of the session.
    pub(super) async fn close(&mut self) -> Result<(), Error> {
        self.blocked = true;
        self.c.inner.children.post.closed(self.id.as_str());
        self.conversation = None;
        self.stop().await?;
        for event in self.record.summary().jobs.clone() {
            if let JobEvent::Started { job, .. } = event {
                let killed = self.shell.kill(job).await;
                killed.map_err(other("closing a session's jobs"))?;
            }
        }
        self.drain().await?;
        self.set_state(State::Done);
        Ok(())
    }

    /// Ends whatever is open with `error`, so a failure never leaves the session working.
    pub(super) async fn fail(&mut self, error: &Error) {
        self.conversation = None;
        if let Err(e) = self.abandon(error.to_string()).await {
            eprintln!("razor: session {}: {e}", self.id);
        }
    }

    /// Work that keeps both a child unfinished and a cold session task loaded, beside the inputs
    /// a turn ending now `answers`.
    fn pending(&self, answers: &[EntryId]) -> bool {
        self.unfinished(answers) || self.record.summary().children.iter().any(|c| c.working())
    }

    /// The session's own unfinished work: its inputs, jobs, calls, notices and open questions.
    fn unfinished(&self, answers: &[EntryId]) -> bool {
        let s = self.record.summary();
        s.unanswered.iter().any(|id| !answers.contains(id))
            || !s.jobs.is_empty()
            || !self.actions.is_empty()
            || !self.notices.is_empty()
            || !s.questions.is_empty()
            || !s.mail_questions.is_empty()
    }

    /// Removes a cold, idle task only with no caller holding its queue sender. The map lock
    /// makes that check and removal atomic with `task`, so admission cannot lose queued work.
    fn idle(&self, queue: &mpsc::Receiver<Work>) -> bool {
        if self.state == State::Working
            || self.conversation.is_some()
            || self.due()
            || self.pending(&[])
            || (!self.blocked && self.turn.waiting())
            || self.lines.receiver_count() != 0
        {
            return false;
        }
        let mut sessions = lock(&self.c.inner.sessions);
        if queue.is_empty()
            && sessions
                .get(&self.id)
                .is_some_and(|tx| tx.strong_count() == 1)
        {
            sessions.remove(&self.id);
            return true;
        }
        false
    }

    /// Closes an idle conversation, to keep within `sessions.idle`.
    pub(super) async fn unload(&mut self) -> Result<(), Error> {
        if self.state == State::NeedsYou {
            self.conversation = None;
        }
        Ok(())
    }

    /// The night's rebuild, at once when idle. A turn the night's `input` opened ends first, its
    /// tidying done; a turn that input waits behind settles its current batch (Trey, 2026-11-04).
    pub(super) async fn night(&mut self, input: Option<EntryId>) -> Result<(), Error> {
        if self.state != State::Working {
            return self.rebuild(Trigger::Night, None, Vec::new()).await;
        }
        self.planned = Some(Trigger::Night);
        let behind = input.is_none_or(|entry| self.pending.iter().any(|(id, _)| *id == entry));
        if let Some(c) = self.conversation.as_mut().filter(|_| behind) {
            c.handover.get_or_insert_default();
        }
        Ok(())
    }

    /// After a restart, an open turn resumes within an hour of activity, else holds for input;
    /// an idle handover's turn ends at its rebuild.
    /// A closed child holds its unanswered inputs again for its next task, and a turn a child's
    /// lost job opened at load (`run`) goes on.
    pub(super) async fn restart(&mut self) -> Result<(), Error> {
        self.record.mark(self.due()).await?;
        let summary = self.record.summary();
        if self.blocked {
            return self.put_back(summary.unanswered.clone()).await;
        }
        if summary.unanswered.is_empty() || summary.closed || self.state == State::Working {
            return Ok(());
        }
        // An idle note turn cut off ends at its rebuild, as it would have: nobody asked for more.
        if summary.idle_handover {
            return self.rebuild(Trigger::Idle, None, Vec::new()).await;
        }
        let gap = crate::protocol::now().saturating_sub(self.record.last_activity);
        if gap > 3_600_000 {
            self.unknown_results().await?;
            self.append(record::interrupted(gap), None).await?;
            return Ok(());
        }
        // A switch cut off in its note turn: the inputs before the note request, never sent, open
        // their model behind the rebuilt context, as `start_turn` holds them; later ones wait.
        let held = (summary.unanswered.iter()).filter(|id| **id > summary.cut);
        self.put_back(held.copied().collect()).await?;
        let note = (self.pending.iter()).position(|(_, i)| i.trigger == Some(Trigger::Change));
        if let Some(at) = note {
            self.pending.remove(at);
            self.opening = self.pending.iter().take(at).map(|(id, _)| *id).collect();
            let model = (self.pending.iter().take(at).rev()).find_map(|(_, i)| i.model.clone());
            self.model = model.unwrap_or_else(|| self.model.clone());
            self.refresh().await?;
            return self.rebuild(Trigger::Change, None, Vec::new()).await;
        }
        self.pending.clear();
        self.rebuild(Trigger::Restart, None, Vec::new()).await
    }
}

/// Closes `session` and its descendants on a task of its own, then sends a finished child's
/// recorded report to `parent`. A plain function, so the type of the future it spawns is not
/// part of the turn's end that calls it, which closing and admission also reach.
fn close(c: Controller, session: SessionId, parent: Option<SessionId>) {
    tokio::spawn(async move {
        let closed = match parent {
            Some(parent) => children::deliver(&c, &session, parent)
                .await
                .map_err(|e| Error::Invalid(e.to_string())),
            None => c.close(&session).await,
        };
        if let Err(e) = closed {
            eprintln!("razor: closing session {session}: {e}");
        }
    });
}

fn sentence(words: &str) -> String {
    match words.trim() {
        "" => "The turn failed, and the model path did not say why.".into(),
        w if w.ends_with('.') => w.into(),
        w => format!("{w}."),
    }
}
