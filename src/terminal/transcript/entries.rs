//! Retained entries, streamed words and job tails within the transcript’s memory limits.
use super::*;
use crate::protocol::{JobEnd, JobEvent};
use crate::terminal::api;

impl Transcript {
    /// Folds one entry, keeping at most `keep`; false when it is past the record's 64 MiB line cap.
    pub(in crate::terminal) fn entry(&mut self, entry: Entry, keep: usize) -> bool {
        let consumed = self.cursor;
        self.cursor = self.cursor.max(entry.id);
        if self.entries.contains_key(&entry.id) {
            return true;
        }
        let mut words = String::new();
        match &entry.body {
            Body::TurnEnd { inputs, end, .. } => {
                self.take(inputs, None);
                if self.end.is_none_or(|(id, _)| entry.id > id) {
                    self.end = Some((entry.id, matches!(end, End::Error(_))));
                }
                if self.live && entry.id > consumed {
                    self.ended = Some(end.clone());
                    words = std::mem::take(&mut self.partial);
                }
            }
            Body::Withdrawn { inputs } => self.withdraw(inputs),
            _ => {}
        }
        if matches!(entry.body, Body::Text { .. }) {
            self.delta_at = None;
            self.partial.clear();
        }
        let said = match &entry.body {
            Body::Text { .. } | Body::Thinking { .. } => Some(None),
            Body::Input(input) if input.from.is_person() => {
                let words = input.text.split_whitespace().collect::<Vec<_>>().join(" ");
                Some(Some(words.chars().take(512).collect()))
            }
            _ => None,
        };
        if let Some(said) = said.filter(|_| self.heard.as_ref().is_none_or(|h| entry.id > h.0)) {
            self.heard = Some((entry.id, said));
        }
        if let Some(marks) = &mut self.marks {
            marks.saw(&entry);
        }
        // Live entries keep the cursor current without splicing a gap into an older window.
        let last = self.entries.last_key_value().map(|(id, _)| *id);
        let gap = last.is_some_and(|last| last < consumed && entry.id.0 > last.0.saturating_add(1));
        if gap || self.retain_older && last.is_some_and(|last| entry.id > last) {
            self.dirty = true;
            return true;
        }
        let n = serde_json::to_vec(&entry).map_or(api::BYTES + 1, |s| s.len()) + words.len();
        if n > 64 << 20 {
            return false;
        }
        match &entry.body {
            Body::Job(JobEvent::Started { job, call, .. }) => {
                self.jobs.insert(
                    *job,
                    tools::Job {
                        call: *call,
                        start: entry.at,
                        ..Default::default()
                    },
                );
            }
            Body::Job(JobEvent::Ended { job, end, tail }) => {
                self.tail.remove(job);
                if self.output.as_ref().is_some_and(|v| v.job == *job) {
                    self.output = None;
                }
                if let Some(state) = self.jobs.get_mut(job) {
                    state.end = Some(*end);
                    if *end != JobEnd::Exit(0) && !tail.is_empty() {
                        let tail = tail.iter().take(2).filter(|l| l.len() <= 1024);
                        state.last = tail.cloned().collect();
                    }
                }
            }
            Body::Result { call, output, .. } => {
                self.results.insert(call.clone(), entry.id);
                if let Some(decoded) = tools::Decoded::read(output) {
                    // A skill that loaded takes the next lamp number, kept for this reading.
                    let name = decoded.skill.as_ref().map(|s| &s.name);
                    if let Some(name) =
                        name.filter(|n| !self.skills.contains(n) && self.skills.len() < 64)
                    {
                        self.skills.push(name.clone());
                    }
                    self.decoded.insert(entry.id, decoded);
                }
            }
            _ => {}
        }
        while self.jobs.len() > ITEMS {
            self.jobs.pop_first();
        }
        // A held view keeps its mounted first entry as entries append below it.
        if self.held.is_some() && last.is_none_or(|last| entry.id > last) {
            self.mounted = (self.mounted() + 1).min(ITEMS);
        }
        if !words.is_empty() {
            self.unsaved.insert(entry.id, words);
        }
        if let Body::Call { name, input, .. } = &entry.body
            && let Some(command) = tools::command(tools::kind(name), input)
        {
            let lines = command.trim_end().lines().count();
            self.commands.insert(entry.id, lines);
        }
        self.sizes.insert(entry.id, n);
        self.entries.insert(entry.id, entry);
        self.dirty = true;
        while self.budget().0 > api::BYTES || self.budget().1 > 1 || self.entries.len() > keep {
            self.evict();
        }
        true
    }

    /// Ordinary bytes and the count of entries over 8 MiB: the app keeps 8 MiB of the first
    /// and one of the second, so a TurnEnd never evicts the large reply it completes.
    pub(in crate::terminal) fn budget(&self) -> (usize, usize) {
        self.sizes.values().fold((0, 0), |(bytes, large), &n| {
            if n > api::BYTES {
                (bytes, large + 1)
            } else {
                (bytes + n, large)
            }
        })
    }

    /// Drops the entry farthest from the window being read.
    pub(in crate::terminal) fn evict(&mut self) {
        let old = match self.retain_older {
            true => self.entries.pop_last(),
            false => self.entries.pop_first(),
        };
        let Some((id, old)) = old else { return };
        self.sizes.remove(&id);
        self.unsaved.remove(&id);
        self.commands.remove(&id);
        self.opened = self.opened.filter(|opened| *opened != id);
        self.queued.remove(&id);
        self.expanded.remove(&id);
        self.decoded.remove(&id);
        if let Body::Result { call, .. } = &old.body {
            self.results.remove(call);
        }
        self.jobs.retain(|job, state| {
            let kept = state.call != id;
            if !kept {
                self.tail.remove(job);
            }
            kept
        });
        self.dirty = true;
    }

    pub(in crate::terminal) fn attached(&mut self) {
        self.live = true;
        self.partial.clear();
        self.dirty = true;
    }
    /// Folds a piece of the reply being written, keeping its last `keep` bytes; false past 8 MiB.
    pub(in crate::terminal) fn delta(&mut self, text: &str, now: u64, keep: usize) -> bool {
        self.delta_at = Some(now);
        let fits = self.partial.len() + text.len() <= api::BYTES;
        if fits {
            self.partial.push_str(text);
            let cut = (self.partial).ceil_char_boundary(self.partial.len().saturating_sub(keep));
            self.partial.drain(..cut);
            self.recount = true;
        }
        fits
    }
    /// The model took `ids`; one it took from the queue at `at` (live) lights as it leaves.
    pub(in crate::terminal) fn take(&mut self, ids: &[EntryId], at: Option<u64>) {
        for id in ids {
            if self.queued.remove(id) && !self.taking.contains(id) {
                self.left = at.map(|at| (*id, at, false)).or(self.left);
            }
            self.dirty = true;
            self.taken.insert(*id);
        }
        while self.taken.len() > ITEMS {
            self.taken.pop_first();
        }
    }
    fn withdraw(&mut self, ids: &[EntryId]) {
        for id in ids {
            self.dirty |= self.queued.remove(id) | self.withdrawn.insert(*id);
            self.taking.remove(id);
        }
        while self.withdrawn.len() > ITEMS {
            self.withdrawn.pop_first();
        }
    }
    pub(in crate::terminal) fn tail(&mut self, job: JobId, text: String, at: u64) {
        if text.len() > 1024 || self.tail.len() >= 128 && !self.tail.contains_key(&job) {
            return;
        }
        if self
            .output
            .as_ref()
            .is_some_and(|v| v.job == job && v.total.is_some() && !v.loading)
        {
            self.output = None;
        }
        self.tail.insert(job, (text, at));
        if let Some(state) = self.jobs.get(&job) {
            self.counts.remove(&state.call);
        }
        self.recount = true;
    }
}
