//! A resident's runs, folded from its record as entries pass: each heartbeat or schedule input,
//! or night rebuild, followed to the turn end or withdrawal that answers it
//! (docs/design/residents-api.md).

use super::summary::{preview, said};
use crate::protocol::*;
use std::collections::{BTreeMap, VecDeque};

/// The most runs kept, and how long a run is kept after a newer one opens.
const KEPT: usize = 512;
const DAY: Millis = 86_400_000;
/// The names whose newest run outlives `KEPT` and `DAY`: the heartbeat, the night and the 64
/// schedule names the missed-run marker holds.
const NAMES: usize = 66;

/// The runs opened in the day before the newest, at most 512, oldest first, and the newest run
/// of each name that has left them. Empty for any session but a resident's or its job thread's.
#[derive(Debug, Clone, Default)]
pub(crate) struct Runs {
    recent: VecDeque<Run>,
    older: BTreeMap<String, Run>,
    /// A turn has recorded work since the last turn end.
    working: bool,
    /// The first line of the last note or question the running run's turn sent the person,
    /// and of its last reply.
    said: Option<String>,
    reply: String,
}

impl Runs {
    /// The runs opened at or after `since`, newest first.
    pub(crate) fn since(&self, since: Millis) -> impl Iterator<Item = &Run> {
        self.recent.iter().rev().take_while(move |r| r.at >= since)
    }

    /// The newest run named `what`, however old.
    pub(crate) fn latest(&self, what: &str) -> Option<&Run> {
        let recent = self.recent.iter().rev().find(|r| r.what == what);
        recent.or_else(|| self.older.get(what))
    }

    /// The entry of the oldest run not yet closed.
    pub(super) fn open(&self) -> Option<EntryId> {
        self.all().find(|r| r.closed.is_none()).map(|r| r.entry)
    }

    pub(super) fn fold(&mut self, session: &SessionId, entry: &Entry, unanswered: &[EntryId]) {
        let (id, at) = (entry.id, entry.at);
        match &entry.body {
            Body::Input(Input {
                from: Origin::Heartbeat,
                ..
            }) => self.add(session, id, at, "heartbeat", at),
            Body::Input(Input {
                from: Origin::Schedule { name, due },
                ..
            }) => self.add(session, id, at, name, *due),
            Body::Rebuild {
                trigger: Trigger::Night,
                ..
            } => self.add(session, id, at, "night", at),
            // The run itself is the thread's; this row stands for it until a reader finds it.
            Body::Fresh {
                schedule,
                due,
                session: thread,
            } => {
                self.add(thread, EntryId(0), at, schedule, *due);
                self.recent
                    .back_mut()
                    .into_iter()
                    .for_each(|r| r.state = RunState::Starting);
            }
            _ => {}
        }
        self.start(entry, unanswered);
        match &entry.body {
            Body::TurnEnd { inputs, end, .. } => {
                self.working = false;
                let line = self.said.clone().unwrap_or_else(|| self.reply.clone());
                if self.close(inputs, entry, end, &line) {
                    (self.said, self.reply) = (None, String::new());
                }
            }
            // An input is withdrawn only before a turn takes it, so its run never ran.
            Body::Withdrawn { inputs } => {
                self.close(inputs, entry, &End::Stopped, "");
            }
            Body::Text { text } if self.running() => self.reply = preview(text),
            Body::Call { name, input, .. } if self.running() => {
                if let Some(input) = said(name, input) {
                    self.said = Some(preview(input["text"].as_str().unwrap_or_default()));
                }
            }
            _ => {}
        }
    }

    /// A turn's first work entry starts the waiting run at the front of the session's queue,
    /// since the controller takes held inputs in arrival order and runs a heartbeat or schedule
    /// alone; a night starts at its own rebuild, and a restart's rebuild covers every open run.
    fn start(&mut self, entry: &Entry, unanswered: &[EntryId]) {
        let restart = match &entry.body {
            Body::Rebuild { trigger, .. } => *trigger == Trigger::Restart,
            Body::Text { .. }
            | Body::Thinking { .. }
            | Body::Call { .. }
            | Body::Response { .. } => false,
            _ => return,
        };
        if self.working && !restart {
            return;
        }
        self.working = true;
        let front = unanswered.first();
        let waiting = self
            .recent
            .iter_mut()
            .filter(|r| r.state == RunState::Waiting);
        for run in waiting {
            if restart || run.entry == entry.id || front == Some(&run.entry) {
                (run.start, run.state) = (Some(entry.id), RunState::Running);
            }
        }
    }

    /// Closes each open run whose entry `inputs` names; true when one of them had started, so
    /// the line it took belongs to it.
    fn close(&mut self, inputs: &[EntryId], entry: &Entry, end: &End, line: &str) -> bool {
        let (state, error) = match end {
            End::Done => (RunState::Ok, None),
            End::Stopped => (RunState::Stopped, None),
            End::Error(why) => (RunState::Failed, Some(why.clone())),
        };
        let mut started = false;
        let named = |r: &&mut Run| r.closed.is_none() && inputs.contains(&r.entry);
        let runs = self.recent.iter_mut().chain(self.older.values_mut());
        for run in runs.filter(named) {
            if run.start.is_some() {
                (started, run.line) = (true, line.into());
            }
            (run.state, run.error) = (state, error.clone());
            (run.closed, run.ended) = (Some(entry.id), Some(entry.at));
        }
        started
    }

    fn add(&mut self, session: &SessionId, entry: EntryId, at: Millis, what: &str, due: Millis) {
        while self.recent.len() >= KEPT || self.recent.front().is_some_and(|r| r.at + DAY < at) {
            if let Some(old) = self.recent.pop_front() {
                if self.older.len() == NAMES && !self.older.contains_key(&old.what) {
                    let oldest = self.older.iter().min_by_key(|(_, run)| run.at);
                    // A map at its nonzero capacity always has an oldest entry.
                    let name = oldest.expect("full run history").0.clone();
                    self.older.remove(&name);
                }
                self.older.insert(old.what.clone(), old);
            }
        }
        self.recent.push_back(Run {
            session: session.clone(),
            entry,
            start: None,
            closed: None,
            at,
            what: what.into(),
            due,
            state: RunState::Waiting,
            ended: None,
            line: String::new(),
            error: None,
        });
    }

    fn running(&self) -> bool {
        self.all().any(|r| r.state == RunState::Running)
    }

    fn all(&self) -> impl Iterator<Item = &Run> {
        self.recent.iter().chain(self.older.values())
    }
}
