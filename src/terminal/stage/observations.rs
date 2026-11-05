//! Record folds for the stage: calls are bounded separately from the transcript.
use crate::protocol::{Body, CallId, Entry, EntryId, JobEnd, JobEvent, JobId, Outcome};
use crate::terminal::tools::{command, job_outcome, test::runner};
use std::collections::VecDeque;

#[derive(Clone, Copy)]
pub(super) enum Prop {
    Read,
    Write,
    Edit,
    Shell,
    Tests,
}

pub(super) struct Call {
    pub(super) id: EntryId,
    call: CallId,
    pub(super) prop: Prop,
    pub(super) at: u64,
    job: Option<JobId>,
}

#[derive(Default)]
pub(super) struct Records {
    cursor: EntryId,
    // At most 64 running calls, and three completed test outcomes; no output is retained.
    pub(super) calls: VecDeque<Call>,
    pub(super) tests: VecDeque<(bool, u64)>,
    pub(super) rebuild: Option<u64>,
    pub(super) thanks: Option<(String, u64)>,
    pub(super) failed: Option<u64>,
    pub(super) input: Option<u64>,
    // The duck's arrival, last failed run and departure.
    pub(super) duck: Option<(u64, u64, Option<u64>)>,
    failures: u8,
    pub(super) firework: Option<u64>,
}

impl Records {
    pub(super) fn entry(&mut self, entry: &Entry, now: u64) {
        if entry.id <= self.cursor {
            return;
        }
        self.cursor = entry.id;
        let at = entry.at;
        match &entry.body {
            Body::Call { call, name, input } => {
                let prop = match super::super::tools::kind(name) {
                    "read" => Prop::Read,
                    "write" => Prop::Write,
                    "edit" | "apply_patch" => Prop::Edit,
                    "shell" if runner(command("shell", input).unwrap_or("")).is_some() => {
                        Prop::Tests
                    }
                    "shell" => Prop::Shell,
                    _ => return,
                };
                if self.calls.len() == 64 {
                    self.calls.pop_front();
                }
                let call = CallId(call.0.chars().take(128).collect());
                self.calls.push_back(Call {
                    id: entry.id,
                    call,
                    prop,
                    at,
                    job: None,
                });
            }
            Body::Result {
                call,
                outcome,
                output,
                ..
            } => self.result(call, *outcome, output, at),
            Body::Job(JobEvent::Started { job, call, .. }) => {
                if let Some(c) = self.calls.iter_mut().find(|c| c.id == *call) {
                    c.job = Some(*job);
                }
            }
            Body::Job(JobEvent::Ended { job, end, .. }) => {
                if let Some(i) = self.calls.iter().position(|c| c.job == Some(*job)) {
                    self.complete(i, job_outcome(*end), at);
                }
            }
            Body::Rebuild { .. } => {
                self.rebuild = Some(if at + 5000 >= now { at } else { 0 });
                self.tested(true, at);
            }
            Body::Input(input) if input.from.is_person() => self.input(input, at),
            _ => {}
        }
    }

    fn input(&mut self, input: &crate::protocol::Input, at: u64) {
        self.input = Some(at);
        let text = input.text.chars().take(256).collect::<String>();
        // `/\b(?:thanks|thank you|good job)\b/i`, in a short turn or opening a long one.
        let lower = text.to_ascii_lowercase().into_bytes();
        let word = |i: usize| {
            lower
                .get(i)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        };
        let phrase = |i: usize| {
            (i == 0 || !word(i - 1))
                && ["thanks", "thank you", "good job"]
                    .iter()
                    .any(|p| lower[i..].starts_with(p.as_bytes()) && !word(i + p.len()))
        };
        let short = input.text.encode_utf16().nth(80).is_none();
        if (phrase(0) || short && (1..lower.len()).any(phrase))
            && self
                .thanks
                .as_ref()
                .is_none_or(|(_, last)| at.saturating_sub(*last) >= 600_000)
        {
            self.thanks = Some((text, at));
        }
    }

    fn result(&mut self, call: &CallId, result: Outcome, output: &str, at: u64) {
        let Some(i) = self.calls.iter().position(|c| c.call == *call) else {
            return;
        };
        #[derive(serde::Deserialize)]
        struct Shell {
            job: Option<JobId>,
            end: Option<JobEnd>,
        }
        let shell = serde_json::from_str::<Shell>(output).ok();
        if let Some(end) = shell.as_ref().and_then(|s| s.end) {
            self.complete(i, job_outcome(end), at);
        } else if let Some(job) = shell.and_then(|s| s.job) {
            if let Some(c) = self.calls.get_mut(i) {
                c.job = Some(job);
            }
        } else if self.calls.get(i).is_some_and(|c| c.job.is_none()) || result == Outcome::Error {
            self.complete(i, result, at);
        }
    }

    fn complete(&mut self, i: usize, outcome: Outcome, at: u64) {
        let Some(call) = self.calls.remove(i) else {
            return;
        };
        if outcome == Outcome::Error {
            self.failed = Some(at);
        }
        if matches!(call.prop, Prop::Tests) && outcome != Outcome::Unknown {
            if self.tests.len() == 3 {
                self.tests.pop_front();
            }
            let pass = outcome == Outcome::Success;
            self.tests.push_back((pass, at));
            self.tested(pass, at);
            if pass
                && self
                    .firework
                    .is_none_or(|last| at.saturating_sub(last) >= 3_600_000)
            {
                self.firework = Some(at);
            }
        }
    }

    // The duck comes after three failed runs in a row and leaves at a pass or a rebuild; two
    // hours with no failed run send it away unseen, with the count.
    fn tested(&mut self, pass: bool, at: u64) {
        if self
            .duck
            .is_some_and(|(_, last, _)| at.saturating_sub(last) >= 7_200_000)
        {
            (self.duck, self.failures) = (None, 0);
        }
        if pass {
            self.failures = 0;
            if let Some((_, _, leaving @ None)) = &mut self.duck {
                *leaving = Some(at);
            }
            return;
        }
        self.failures = self.failures.saturating_add(1);
        if self.failures >= 3 && self.duck.is_none_or(|d| d.2.is_some()) {
            self.duck = Some((at, at, None));
        }
        if let Some(duck) = &mut self.duck {
            duck.1 = at;
        }
    }
}
