//! The same fold recovers a record and advances it after a committed append.

use crate::protocol::*;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// A direct child's latest facts; the report keeps the API's first-line, 200-character
/// preview (at most 800 UTF-8 bytes), with its entry id for the whole report.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ChildSummary {
    pub(crate) session: SessionId,
    pub(crate) child: Child,
    pub(crate) task: EntryId,
    pub(crate) report: Option<Report>,
    pub(crate) stopped: bool,
    pub(crate) settled: Option<Decision>,
}

impl ChildSummary {
    /// Whether folding `body` can change a parent's child rows: a child event or a report.
    pub(crate) fn changed_by(body: &Body) -> bool {
        match body {
            Body::Child { .. } => true,
            Body::Input(input) => matches!(input.from, Origin::Report { .. }),
            _ => false,
        }
    }

    /// A report is admissible only while this task is still outstanding.
    pub(crate) fn working(&self) -> bool {
        self.report.is_none() && !self.stopped && self.settled.is_none()
    }
}

/// A meeting recorded on the phone, from its start fact: the phone's id for it, the folder
/// meetrec made, the phone that recorded it, and how far it got.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Meeting {
    pub(crate) id: String,
    pub(crate) folder: String,
    pub(crate) device: Option<String>,
    pub(crate) state: MeetingState,
}

/// Recording from its start, stopped by the person's end, and closed by its notes input or a
/// cancel; it only moves forward. The batch pass's outcome is the folder's `final.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum MeetingState {
    Recording,
    Stopped,
    Closed,
}

/// Facts retained without entry bodies or Responses items; historical ids cost eight bytes
/// each. A turn's unanswered inputs are what its last two conversations read (the controller's
/// handover answers what the previous rebuild carried), bounded by the context window, plus 64
/// unread; jobs are bounded by the shell's 64 handles and calls by one model step.
/// Children/reports and open questions retain only their latest recorded facts.
#[derive(Debug, Clone)]
pub(crate) struct Summary {
    pub(crate) session: SessionId,
    pub(crate) cwd: PathBuf,
    pub(crate) model: String,
    pub(crate) settings: ModelSettings,
    /// The latest input naming a model chose a handover note from a conversation a model has
    /// read (after an unread rebuild there is nothing to hand over); a rebuild clears it.
    pub(crate) handover: bool,
    pub(crate) profile_version: String,
    pub(crate) started: Millis,
    pub(crate) at: Millis,
    /// Last activity visible to clients; idle maintenance preserves it.
    pub(crate) activity: Millis,
    /// Completed work's idle start; an idle handover preserves it until external input.
    pub(crate) idle_since: Option<Millis>,
    pub(crate) idle_handover: bool,
    pub(crate) person_input_at: Option<Millis>,
    /// When the model last showed reply text.
    pub(crate) spoke_at: Option<Millis>,
    pub(crate) last_reply: String,
    pub(crate) cut: EntryId,
    pub(crate) last: EntryId,
    pub(crate) unanswered: Vec<EntryId>,
    /// Call facts no turn has answered and no hand-off carried, each with whether it is a line: the
    /// newest 256 lines (each under 4 KiB) and every start and end among the newest 512 facts,
    /// with how many older ones were left out since a turn last answered any.
    pub(crate) voice_calls: Vec<(EntryId, bool)>,
    pub(crate) voice_calls_left_out: usize,
    /// The newest 16 meetings started, in the order they started.
    pub(crate) meetings: Vec<Meeting>,
    pub(crate) unresolved: Vec<(CallId, EntryId)>,
    /// This turn's code-mode exec Calls. A cell is named by its exec's call id, and a call the
    /// cell makes has that id, a colon and a number. Cells stop when their turn ends, which
    /// clears this and `waits`, so each holds one conversation's calls at most.
    pub(crate) cells: Vec<(CallId, EntryId)>,
    /// This turn's `wait` Calls whose `cell_id` names one of `cells`, each with that exec Call.
    waits: Vec<(CallId, EntryId)>,
    pub(crate) last_end: Option<End>,
    /// Each list keeps four ids through the latest rebuild cut, plus ids after that cut.
    pub(crate) asks: Vec<EntryId>,
    pub(crate) parent_inputs: Vec<EntryId>,
    /// Inputs whose video a model may still be shown, each with whether it is kept for the
    /// session's life (a watcher's own, at most one): the kept one and those unanswered, whose
    /// count admission bounds; a rebuild carries them wherever they sit.
    pub(super) videos: Vec<(EntryId, bool)>,
    pub(crate) last_rebuild: Option<(EntryId, Millis)>,
    pub(crate) native: Option<Native>,
    /// Native-bearing entry ids since the last rebuild, bounded by the model's context.
    pub(crate) responses: Vec<EntryId>,
    pub(crate) jobs: Vec<JobEvent>,
    /// Live jobs whose call a stopped turn ended with an unknown outcome: those the person's stop
    /// killed, recovered after a restart. `unknown` holds such calls until their turn's end.
    pub(crate) stopped: Vec<JobId>,
    unknown: Vec<EntryId>,
    pub(crate) next_job: JobId,
    pub(crate) child: Option<Child>,
    /// Indexed outgoing report and latest tagged input; no report text is retained.
    pub(crate) reported: Option<EntryId>,
    pub(crate) child_task: Option<EntryId>,
    /// The name the session answers to, fixed by its Start: a child's own, a root's stored one,
    /// else its id's (`SessionId::name`), so a record from before keeps the name it showed.
    pub(crate) name: String,
    /// One small row per child ever started; settled rows remain addressable in the tree.
    pub(crate) children: Vec<ChildSummary>,
    pub(crate) headline: String,
    pub(crate) headline_at: Option<Millis>,
    pub(crate) changed_files: Vec<PathBuf>,
    /// Tool calls over the whole record, including unresolved calls.
    pub(crate) calls_made: u32,
    /// Calls whose result was an error, over the whole record.
    pub(crate) failed: u32,
    /// The newest turn's cache reads over its request input, once a turn has reported usage.
    pub(crate) cache: Option<f64>,
    /// The latest turn's measured context and resolved model window, retained across restarts.
    pub(crate) context: Option<u64>,
    pub(crate) window: Option<u64>,
    pub(crate) title: String,
    title_input: EntryId,
    pub(crate) questions: Vec<Ask>,
    pub(crate) mail_questions: Vec<MailId>,
    /// Latest due time per schedule name, at most 64 names; a new name evicts the stalest due.
    pub(crate) scheduled: BTreeMap<String, Millis>,
    /// The latest turn end closed the session and no input has come since: it stays closed
    /// until its next input, across a reload too.
    pub(crate) closed: bool,
    pub(crate) close_reason: Option<CloseReason>,
    /// That turn end was an interruption: closed, an error, answering no input.
    interrupted: bool,
    /// The newest 64 request ids this session's inputs carried, with their entries.
    requests: Vec<(String, EntryId)>,
    /// From `Start`: the resident this session is a thread of.
    pub(crate) thread: Option<Thread>,
    pub(crate) runs: super::runs::Runs,
}

impl Summary {
    /// The root's recorded listing facts with the repository resolved by the caller.
    pub(crate) fn listed(self, repo: Option<String>) -> Listed {
        let thread = self.thread.as_ref();
        Listed {
            session: self.session,
            cwd: self.cwd,
            model: self.model,
            title: self.title,
            at: self.activity,
            name: self.name,
            repo,
            effort: Some(self.settings.effort),
            speed: Some(self.settings.speed),
            resident: thread.map(|t| t.resident.clone()),
            resident_project: thread.and_then(|t| t.resident_project.clone()),
            job: thread.and_then(|t| t.job.clone()),
        }
    }

    pub(super) fn new(session: &SessionId, cwd: &Path, model: &str, choice: ModelSettings) -> Self {
        Self {
            session: session.clone(),
            cwd: cwd.into(),
            model: model.into(),
            settings: choice,
            handover: false,
            profile_version: String::new(),
            started: 0,
            at: 0,
            activity: 0,
            idle_since: None,
            idle_handover: false,
            person_input_at: None,
            spoke_at: None,
            last_reply: String::new(),
            cut: EntryId(0),
            last: EntryId(0),
            unanswered: Vec::new(),
            voice_calls: Vec::new(),
            voice_calls_left_out: 0,
            meetings: Vec::new(),
            unresolved: Vec::new(),
            cells: Vec::new(),
            waits: Vec::new(),
            last_end: None,
            asks: Vec::new(),
            parent_inputs: Vec::new(),
            videos: Vec::new(),
            last_rebuild: None,
            native: None,
            responses: Vec::new(),
            jobs: Vec::new(),
            stopped: Vec::new(),
            unknown: Vec::new(),
            next_job: JobId(1),
            child: None,
            reported: None,
            child_task: None,
            name: String::new(),
            children: Vec::new(),
            headline: String::new(),
            headline_at: None,
            changed_files: Vec::new(),
            calls_made: 0,
            failed: 0,
            cache: None,
            context: None,
            window: None,
            title: String::new(),
            title_input: EntryId(0),
            questions: Vec::new(),
            mail_questions: Vec::new(),
            scheduled: BTreeMap::new(),
            closed: false,
            close_reason: None,
            interrupted: false,
            requests: Vec::new(),
            thread: None,
            runs: Default::default(),
        }
    }

    /// The entry of the input that carried `request`, among the newest 64 that carried one.
    pub(crate) fn repeat(&self, request: Option<&str>) -> Option<EntryId> {
        let request = request?;
        let repeat = self.requests.iter().find(|(r, _)| r == request);
        repeat.map(|(_, entry)| *entry)
    }

    /// Open work: unanswered inputs, jobs, or, while this session is open and has not reported,
    /// a child at work, so a restart recovers a report its parent never admitted.
    pub(super) fn live(&self) -> bool {
        let open = !self.closed && self.reported.is_none();
        !self.unanswered.is_empty()
            || !self.jobs.is_empty()
            || (open && self.children.iter().any(ChildSummary::working))
    }

    /// Closed by `/done`, `/quit` or a settle, and no input since, or a child that reported:
    /// it reads done, loaded or not, across a restart too.
    pub(crate) fn done(&self) -> bool {
        (self.closed && !self.interrupted) || self.reported.is_some()
    }

    /// An interrupted turn holds its unanswered inputs, until any input reopens it.
    pub(crate) fn interrupted(&self) -> bool {
        self.interrupted
    }

    /// The latest turn end's error sentence, until a later turn ends.
    pub(crate) fn error(&self) -> Option<&str> {
        match &self.last_end {
            Some(End::Error(why)) => Some(why),
            _ => None,
        }
    }

    pub(super) fn fold(&mut self, entry: &Entry) {
        self.last = entry.id;
        self.at = entry.at;
        if let Body::Input(input) = &entry.body {
            self.idle_handover = input.trigger == Some(Trigger::Idle);
            if !self.idle_handover {
                self.idle_since = None;
            }
        }
        let idle_close = matches!(
            entry.body,
            Body::TurnEnd {
                close_reason: Some(CloseReason::Idle),
                ..
            }
        );
        if !self.idle_handover
            && !idle_close
            && (!self.closed || matches!(entry.body, Body::Input(_)))
        {
            self.activity = entry.at;
        }
        match &entry.native {
            Some(native @ Native::Claude { .. }) => self.native = Some(native.clone()),
            Some(Native::Responses { .. }) => self.responses.push(entry.id),
            None => {}
        }
        match &entry.body {
            Body::Start {
                profile_version,
                child,
                name,
                thread,
                ..
            } => {
                self.profile_version = profile_version.clone();
                self.started = entry.at;
                self.child = child.clone();
                self.thread = thread.as_deref().cloned();
                self.name = match child {
                    Some(child) => child.name.clone(),
                    None if name.is_empty() && thread.is_none() => self.session.name(),
                    None => name.clone(),
                };
            }
            Body::Input(input) => self.input(entry.id, input),
            Body::VoiceCall {
                call,
                source,
                event,
            } => {
                if *source == Source::Meeting {
                    self.meeting(call, event);
                }
                let facts = &mut self.voice_calls;
                facts.push((entry.id, matches!(event, CallEvent::Line { .. })));
                let lines = facts.iter().filter(|(_, line)| *line).count();
                let oldest = facts
                    .iter()
                    .position(|(_, line)| *line)
                    .filter(|_| lines > 256);
                if let Some(at) = oldest.or((facts.len() > 512).then_some(0)) {
                    facts.remove(at);
                    self.voice_calls_left_out += 1;
                }
            }
            Body::Name { name } => self.name = name.clone(),
            Body::Effort { effort } => self.settings.effort = *effort,
            Body::Fresh { schedule, due, .. } => self.schedule(schedule, *due),
            Body::Withdrawn { inputs } => {
                for ids in [
                    &mut self.unanswered,
                    &mut self.asks,
                    &mut self.parent_inputs,
                ] {
                    ids.retain(|id| !inputs.contains(id));
                }
                self.videos.retain(|(id, _)| !inputs.contains(id));
                if inputs.contains(&self.title_input) {
                    self.title.clear();
                    self.title_input = EntryId(0);
                }
            }
            Body::Call { call, name, input } => {
                self.calls_made += 1;
                self.unresolved.push((call.clone(), entry.id));
                let action =
                    (name.strip_prefix("functions.")).or_else(|| name.strip_prefix("mcp__razor__"));
                match action.unwrap_or(name) {
                    "exec" => self.cells.push((call.clone(), entry.id)),
                    "wait" => self
                        .waits
                        .extend(self.cell(input).map(|c| (call.clone(), c))),
                    _ => {}
                }
                self.question(entry.id, entry.at, name, input);
            }
            Body::Result {
                call,
                outcome,
                inputs,
                ..
            } => {
                self.unanswered.retain(|id| !inputs.contains(id));
                self.failed += u32::from(*outcome == Outcome::Error);
                if *outcome == Outcome::Error
                    && let Some((_, id)) = self.unresolved.iter().find(|(c, _)| c == call)
                {
                    self.questions.retain(|ask| ask.id.1 != *id);
                }
                if *outcome == Outcome::Unknown {
                    let calls = self.unresolved.iter().filter(|(c, _)| c == call);
                    self.unknown.extend(calls.map(|(_, at)| *at));
                }
                self.unresolved.retain(|(c, _)| c != call);
            }
            Body::Job(event @ JobEvent::Started { job, .. }) => {
                self.next_job = JobId(self.next_job.0.max(job.0 + 1));
                self.jobs.push(event.clone());
            }
            Body::Job(JobEvent::Ended { job, .. }) => {
                self.jobs
                    .retain(|e| !matches!(e, JobEvent::Started { job: j, .. } if j == job));
                self.stopped.retain(|j| j != job);
            }
            Body::TurnEnd {
                inputs,
                end,
                usage,
                closed,
                close_reason,
                context,
                window,
                report,
                ..
            } => {
                self.context = *context;
                self.window = *window;
                // A turn's report is always of the task it worked on, the current one.
                self.reported = report.as_ref().map(|_| entry.id).or(self.reported);
                self.turn_end(inputs, end, usage, *closed);
                self.close_reason = *close_reason;
                if !self.idle_handover && !closed {
                    self.idle_since = Some(entry.at);
                }
            }
            Body::Rebuild {
                profile_version,
                cut,
                ..
            } => {
                self.profile_version = profile_version.clone();
                (self.cut, self.handover) = (*cut, false);
                self.native = None;
                self.responses.clear();
                self.unknown.clear();
                for asks in [&mut self.asks, &mut self.parent_inputs] {
                    let old = asks.partition_point(|id| id <= cut).saturating_sub(4);
                    asks.drain(..old);
                    asks.shrink_to_fit();
                }
                self.unanswered.push(entry.id);
                self.last_rebuild = Some((entry.id, entry.at));
            }
            Body::Child { session, event } => self.child_event(entry.id, session, event),
            Body::Headline { text } => {
                self.headline = text.clone();
                self.headline_at = Some(entry.at);
            }
            Body::Text { text } if !self.idle_handover => {
                self.spoke_at = Some(entry.at);
                self.last_reply.clear();
                for word in text.split_whitespace() {
                    if !self.last_reply.is_empty() {
                        self.last_reply.push(' ');
                    }
                    let room = 512 - self.last_reply.len();
                    self.last_reply
                        .push_str(&word[..word.floor_char_boundary(room.min(word.len()))]);
                    if word.len() >= room || word.ends_with(['.', '!', '?']) {
                        break;
                    }
                }
                // Until a child sets a headline, its newest reply's first line with content
                // stands for one, cut to the headline tool's 64 KiB.
                if self.child.is_some()
                    && self.headline_at.is_none()
                    && let Some(line) = text.lines().map(str::trim).find(|l| !l.is_empty())
                {
                    self.headline = line[..line.floor_char_boundary(64 << 10)].into();
                }
            }
            Body::ChangedFiles { files } => self.changed_files = files.clone(),
            Body::MailSent {
                id,
                kind: MailKind::Question,
                ..
            } => self.mail_questions.push(id.clone()),
            Body::Notice { .. }
            | Body::Text { .. }
            | Body::Thinking { .. }
            | Body::Response { .. }
            | Body::MailSent { .. } => {}
            Body::Question { text, options, .. } => self.questions.push(Ask {
                id: (self.session.clone(), entry.id),
                text: text.clone(),
                options: options.clone(),
                at: entry.at,
            }),
        }
        self.runs.fold(&self.session, entry, &self.unanswered);
    }

    /// A schedule's latest due time, among at most 64 names, evicting the stalest for a new name.
    fn schedule(&mut self, name: &str, due: Millis) {
        if self.scheduled.len() == 64 && !self.scheduled.contains_key(name) {
            let oldest = self.scheduled.iter().min_by_key(|(_, due)| **due);
            // A map at its nonzero capacity always has an oldest entry.
            let name = oldest.expect("full schedule history").0.clone();
            self.scheduled.remove(&name);
        }
        let previous = self.scheduled.entry(name.into()).or_default();
        *previous = (*previous).max(due);
    }

    /// The open run of a job thread, whose turn end closes the thread.
    pub(crate) fn job(&self) -> Option<EntryId> {
        self.thread.as_ref()?.job.as_ref()?;
        self.runs.open()
    }

    // A turn's end answers its inputs; its cache reads over its whole request input become the
    // share reported, once a turn has reported any input.
    fn turn_end(&mut self, inputs: &[EntryId], end: &End, usage: &Usage, closed: bool) {
        self.unanswered.retain(|id| !inputs.contains(id));
        let facts = self.voice_calls.len();
        self.voice_calls.retain(|(id, _)| !inputs.contains(id));
        if self.voice_calls.len() < facts {
            self.voice_calls_left_out = 0;
        }
        let unanswered = &self.unanswered;
        self.videos
            .retain(|(id, kept)| *kept || unanswered.contains(id));
        self.cells.clear();
        self.waits.clear();
        let unknown = std::mem::take(&mut self.unknown);
        if *end == End::Stopped {
            let stopped = self.jobs.iter().filter_map(|event| match event {
                JobEvent::Started { job, call, .. } if unknown.contains(call) => Some(*job),
                _ => None,
            });
            self.stopped.extend(stopped);
        }
        self.closed = closed;
        self.interrupted = closed && inputs.is_empty() && matches!(end, End::Error(_));
        self.last_end = Some(end.clone());
        let all = (usage.input.saturating_add(usage.cache_read)).saturating_add(usage.cache_write);
        if all > 0 {
            self.cache = Some(usage.cache_read as f64 / all as f64);
        }
    }

    /// The exec Call of the cell `call` reads: its own for an exec, the one it names for a
    /// `wait`; None for any other call.
    pub(crate) fn reads(&self, call: &str) -> Option<EntryId> {
        let mut calls = self.cells.iter().chain(&self.waits);
        calls.find(|c| c.0.0 == call).map(|c| c.1)
    }

    /// The exec Call of this turn's cell a `wait` input's `cell_id` names.
    fn cell(&self, input: &serde_json::Value) -> Option<EntryId> {
        let id = input["cell_id"].as_str()?;
        self.cells.iter().find(|c| c.0.0 == id).map(|c| c.1)
    }

    fn question(&mut self, id: EntryId, at: u64, name: &str, input: &serde_json::Value) {
        let asked = self.child.is_none().then(|| ask(name, input)).flatten();
        let Some(input) = asked else { return };
        self.questions.push(Ask {
            id: (self.session.clone(), id),
            text: input["text"].as_str().unwrap_or_default().into(),
            options: input["options"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            at,
        });
    }

    // A start lists the meeting as the newest, recording; an end stops it and a cancel closes it.
    fn meeting(&mut self, call: &str, event: &CallEvent) {
        let state = match event {
            CallEvent::Started { folder, device } => {
                self.meetings.retain(|m| m.id != call);
                self.meetings.push(Meeting {
                    id: call.into(),
                    folder: folder.clone().unwrap_or_default(),
                    device: device.clone(),
                    state: MeetingState::Recording,
                });
                self.meetings
                    .drain(..self.meetings.len().saturating_sub(16));
                return;
            }
            CallEvent::Ended {
                reason: CallReason::Canceled,
            } => MeetingState::Closed,
            CallEvent::Ended { .. } => MeetingState::Stopped,
            _ => return,
        };
        self.advance_meeting(call, state);
    }

    fn advance_meeting(&mut self, id: &str, state: MeetingState) {
        if let Some(meeting) = self.meetings.iter_mut().find(|m| m.id == id) {
            meeting.state = meeting.state.max(state);
        }
    }

    fn input(&mut self, id: EntryId, input: &Input) {
        if let Some(task) = input.child_task {
            self.child_task = Some(task);
            self.reported = None;
        }
        if !matches!(input.from, Origin::Schedule { .. } | Origin::Heartbeat) {
            if let Some(model) = &input.model {
                let read = self.native.is_some() || !self.responses.is_empty();
                (self.model, self.handover) = (model.clone(), input.handover == Some(true) && read);
            }
            if let Some(speed) = input.speed {
                self.settings.speed = speed;
            }
            if let Some(effort) = input.effort {
                self.settings.effort = effort;
            }
        }
        self.unanswered.push(id);
        if let Some(call) = &input.voice_call {
            self.voice_calls.retain(|(id, _)| !call.lines.contains(id));
        }
        if let Some(id) = &input.meeting {
            self.advance_meeting(id, MeetingState::Closed);
        }
        if input.video.as_ref().is_some_and(|v| v.watcher.is_none()) {
            self.videos
                .push((id, matches!(input.from, Origin::Mail { .. })));
        }
        (self.closed, self.interrupted, self.close_reason) = (false, false, None);
        if let Some(request) = &input.request {
            self.requests.push((request.clone(), id));
            self.requests
                .drain(..self.requests.len().saturating_sub(64));
        }
        if let Origin::Mail {
            kind: MailKind::Answer { question },
            ..
        } = &input.from
        {
            self.mail_questions.retain(|id| id != question);
        }
        match &input.from {
            Origin::Person => {
                self.person_input_at = Some(self.at);
                if self.title_input == EntryId(0) {
                    self.title = input.text.lines().next().unwrap_or_default().into();
                    self.title_input = id;
                }
                self.asks.push(id);
            }
            Origin::Mail {
                id: (sender, _), ..
            } if self.child.as_ref().is_some_and(|c| &c.parent == sender) => {
                self.parent_inputs.push(id)
            }
            Origin::Answer { ask } => self.questions.retain(|q| q.id.1 != *ask),
            Origin::Schedule { name, due } => self.schedule(name, *due),
            Origin::Report { child, status } => {
                if let Some(c) = self.children.iter_mut().find(|c| &c.session == child) {
                    c.report = Some(Report {
                        status: *status,
                        no_changes: input.child_no_changes,
                        text: preview(&input.text),
                        entry: id,
                    });
                }
            }
            _ => {}
        }
    }

    fn child_event(&mut self, at: EntryId, session: &SessionId, event: &ChildEvent) {
        let child = self.children.iter_mut().find(|c| &c.session == session);
        match (child, event) {
            (None, ChildEvent::Started(child)) => self.children.push(ChildSummary {
                session: session.clone(),
                child: child.clone(),
                task: at,
                report: None,
                stopped: false,
                settled: None,
            }),
            (Some(child), ChildEvent::Continued) => {
                child.task = at;
                child.report = None;
                child.stopped = false;
            }
            (Some(child), ChildEvent::Stopped) => child.stopped = true,
            (Some(child), ChildEvent::Settled(decision)) => child.settled = Some(*decision),
            _ => {}
        }
    }
}

/// The input of a call that asks the person a question: a `message` of type question to the
/// parent or to no one. A child's calls ask its parent, not the person; callers check that.
pub(crate) fn ask(name: &str, input: &serde_json::Value) -> Option<serde_json::Value> {
    said(name, input).filter(|input| input["type"] == "question")
}

/// The input of Razor's `message` note or question to the parent or to no one, under each
/// route's name for it; another tool whose name ends in `message` is not it.
pub(super) fn said(name: &str, input: &serde_json::Value) -> Option<serde_json::Value> {
    if !["message", "mcp__razor__message", "functions.message"].contains(&name) {
        return None;
    }
    let kind = matches!(input["type"].as_str(), Some("note" | "question"));
    (kind && input["to"].as_str().is_none_or(|to| to == "parent")).then(|| input.clone())
}

/// A text's first line, at most 200 characters: a report's preview and a run's line.
pub(crate) fn preview(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect()
}
