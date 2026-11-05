//! Each session's stream, read from the API, and its lines folded into the app's bounded caches.
use super::api::{self, Incoming, Line, Page};
use super::{ApiEvent, App, Error, Transcript, audio, bindings, brand, children, commands};
use super::{gauge, mcp, panel, text, transcript};
use crate::protocol::{AgentView, Ask, Body, Delivery, Live, SessionId, State, now};
use std::time::Duration;

impl App {
    /// Reads a session's stream from after `after`, replacing any reader it had.
    pub(super) fn stream(&mut self, id: SessionId, after: u64) {
        if let (Some(api), Some(lines), Some(budget)) = (&self.api, &self.lines, &self.budget) {
            let task = api.stream(Some(id.clone()), after, lines.clone(), budget.clone());
            if let Some(old) = self.streams.insert(id, task) {
                old.abort();
            }
        }
    }

    /// Reads the root's first entry (its folder and model, which a long session's newest page does
    /// not reach), the session's newest page and its saved draft behind earlier writes, then
    /// streams on.
    pub(super) fn open(&mut self, id: SessionId) {
        let (edits, root) = (self.edits, self.root.as_ref() == Some(&id));
        self.write(|api| async move {
            let read = async {
                let start = match root {
                    true => api.page(&id, Some(2), 1).await?.events.pop(),
                    false => None,
                };
                let page = api.page(&id, None, 256).await?;
                Ok::<_, Error>((start, page, api.draft(&id, None).await?))
            };
            let read = read.await;
            move |app: &mut App| match read {
                Ok(read) => app.opened(id, read, edits),
                Err(e) => app.say(&e.to_string()),
            }
        });
    }

    // The first page draws at once, not after the stream's frame pacing.
    fn opened(&mut self, id: SessionId, read: (Option<ApiEvent>, Page, String), edits: u64) {
        let (start, page, draft) = read;
        if self.focus.as_ref() != Some(&id) {
            return;
        }
        if let Some(ApiEvent::Entry(entry)) = start
            && let Body::Start { cwd, model, .. } = entry.body
        {
            (self.cwd, self.model) = (cwd, model);
        }
        for event in page.events {
            self.fold(Some(id.clone()), event, now());
        }
        let t = self.transcripts.get_mut(&id);
        let cursor = t.map_or(0, |t| {
            t.read_past_metadata();
            t.cursor.0
        });
        if self.edits == edits && self.sending.is_none() {
            self.composer.replace(draft);
        }
        super::recap::check(self);
        self.stream(id, cursor);
        self.immediate = true;
    }

    pub(super) fn fold_line(&mut self, incoming: Incoming) {
        match incoming.line {
            // The terminal draws a thought when its entry lands.
            Line::Event(ApiEvent::Live(Live::Thought { .. })) => return,
            Line::Event(event) => self.fold(incoming.session, event, now()),
            Line::Login(login) => panel::login(self, login),
            Line::Newer => {}
            Line::Failed(sentence)
                if incoming.session.is_none() || incoming.session == self.focus =>
            {
                self.say(&sentence)
            }
            Line::Failed(_) => {}
        }
        self.dirty = true;
    }

    pub(super) fn fold(&mut self, session: Option<SessionId>, value: ApiEvent, now: u64) {
        // Any line but the main session's own reply words can change a card: cards are rebuilt at
        // the next draw.
        if !matches!(value, ApiEvent::Live(Live::Delta { .. })) || session != self.root {
            self.revision = self.revision.wrapping_add(1);
        }
        let Some(id) = session else {
            if let ApiEvent::Live(line) = value {
                self.fold_status(line);
            }
            return;
        };
        let shown = self.root.as_ref() == Some(&id) || self.focus.as_ref() == Some(&id);
        if !shown && !self.previews.contains_key(&id) {
            return;
        }
        if self.focus.as_ref() == Some(&id)
            && let ApiEvent::Entry(entry) = &value
        {
            self.stage.select(self.focus.as_ref());
            self.stage.entry(entry, now);
        }
        let t = self.transcripts.entry(id.clone()).or_insert_with(|| {
            let voiced = self
                .delivered
                .iter()
                .filter(|d| d.session == id && d.voice.is_some());
            let mut t = Transcript::default();
            t.deliveries = voiced.map(|d| (d.entry, d.clone())).collect();
            t
        });
        let keep = match shown {
            true => (transcript::ITEMS, api::BYTES),
            false => children::KEPT,
        };
        match value {
            ApiEvent::Entry(entry) => {
                if !t.entry(*entry, keep.0) {
                    self.say(
                        "An entry exceeds the transcript buffer; use recall or --print for it.",
                    );
                }
                self.trim();
            }
            ApiEvent::Live(Live::Attached { .. }) => t.attached(),
            ApiEvent::Live(Live::Delta { text }) => {
                if !t.delta(&text, now, keep.1) {
                    self.say(
                        "The streaming reply exceeds 8 MiB; read it with recall when it completes.",
                    );
                }
            }
            ApiEvent::Live(Live::Taken { input }) => t.take(&[input], Some(now)),
            ApiEvent::Live(Live::Tail { job, text, at, .. }) => t.tail(job, text, at),
            ApiEvent::Live(_) => {}
        }
    }

    fn fold_status(&mut self, line: Live) {
        match line {
            Live::Agents {
                agents,
                config,
                brand,
                ui,
                server,
                mut servers,
                ..
            } => {
                self.agents.clear();
                self.agent_bytes = 0;
                if servers.is_empty() {
                    servers.push(*server);
                }
                let keys_changed = self.ui.keys != ui.keys;
                (self.config, self.brand, self.ui) = (config, brand::read(brand), *ui);
                if keys_changed {
                    bindings::reload(self);
                }
                self.servers = servers;

                agents.into_iter().for_each(|row| self.agent(row));
                self.previews.retain(|id, _| self.agents.contains_key(id));
                let (root, focus, agents) = (&self.root, &self.focus, &self.agents);
                let kept = |id: &SessionId| root.as_ref() == Some(id) || focus.as_ref() == Some(id);
                self.transcripts
                    .retain(|id, _| kept(id) || agents.contains_key(id));
            }
            Live::Views(status) => self.list.received(status),
            Live::Server(server) => {
                if let Some(old) = self.servers.iter_mut().find(|s| s.id == server.id) {
                    *old = *server;
                }
            }
            Live::Agent(row) => self.agent(*row),
            Live::Delivery(delivery) => self.delivery(*delivery),
            _ => {}
        }
    }

    // A delivery replaces its earlier copy, keeping when it first came,
    // or joins the newest 256 in order; a fresh one is said, and heard when it is an unread voice
    // note for this terminal.
    fn delivery(&mut self, mut delivery: Delivery) {
        if let Some(t) = self.transcripts.get_mut(&delivery.session)
            && delivery.voice.is_some()
        {
            t.deliveries.insert(delivery.entry, delivery.clone());
            t.invalidate();
        }
        let existing = self
            .delivered
            .iter()
            .position(|d| (&d.session, d.entry) == (&delivery.session, delivery.entry));
        if let Some(at) = existing {
            delivery.at = self.delivered[at].at;
            self.delivered[at] = delivery;
            return;
        }
        let fresh = !delivery.archived && now().saturating_sub(delivery.at) < 10_000;
        let hear = fresh
            && !delivery.read
            && delivery.voice.is_some()
            && delivery.terminal.as_deref() == Some(&*audio::RUN);
        if fresh {
            self.say(&delivery.line);
        }
        let target = (delivery.session.clone(), delivery.entry);
        let at = self.delivered.partition_point(|d| {
            (d.at, &d.session, d.entry) < (delivery.at, &delivery.session, delivery.entry)
        });
        self.delivered.insert(at, delivery);
        if self.delivered.len() > 256
            && let Some(old) = self.delivered.pop_front()
            && let Some(t) = self.transcripts.get_mut(&old.session)
        {
            t.deliveries.remove(&old.entry);
            t.invalidate();
        }
        if hear {
            audio::play(self, target.0, target.1);
        }
    }

    // All transcripts together keep 2,000 entries and 8 MiB of ordinary entries plus one larger
    // whole entry; the oldest of a transcript neither shown nor addressed goes first.
    fn trim(&mut self) {
        loop {
            let (mut bytes, mut large, mut count) = (0, 0, 0);
            for t in self.transcripts.values() {
                let (n, big) = t.budget();
                (bytes, large, count) = (bytes + n, large + big, count + t.entries.len());
            }
            if bytes <= api::BYTES && large <= 1 && count <= 2000 {
                break;
            }
            let (root, focus) = (&self.root, &self.focus);
            let order = |id: &SessionId| (focus.as_ref() == Some(id), root.as_ref() == Some(id));
            let victim = self
                .transcripts
                .iter_mut()
                .filter(|(_, t)| !t.entries.is_empty());
            match victim.min_by_key(|(id, _)| order(id)) {
                Some((_, t)) => t.evict(),
                None => break,
            }
        }
    }

    fn agent(&mut self, mut row: AgentView) {
        // A thread recorded before threads were named at start shows its id's name.
        if row.name.is_empty() {
            row.name = row.session.name();
        }
        let size = |row: &AgentView| serde_json::to_vec(row).map_or(usize::MAX, |v| v.len());
        let old = self.agents.get(&row.session).map_or(0, size);
        let bytes = self.agent_bytes.saturating_sub(old);
        let bytes = bytes.saturating_add(size(&row));
        let full = self.agents.len() >= 1024 && !self.agents.contains_key(&row.session);
        if full || bytes > api::BYTES {
            self.say(
                "The status view exceeds 1,024 agents or 8 MiB; reopen a smaller session tree.",
            );
            return;
        }
        let old = self.agents.get(&row.session);
        if self.focus.as_ref() == Some(&row.session)
            && row.speed != crate::protocol::Speed::Standard
            && row.error.is_some()
            && old.is_none_or(|o| o.error != row.error)
            && let Some(models) = &mut self.models
            && let Some(choices) = models.speeds.get_mut(&row.model)
            && let Some(choice) = choices.iter_mut().find(|c| c.speed == row.speed)
        {
            choice.reason = row.error.clone();
        }
        // A question that arrives is announced once in the notice row, with how many more wait.
        let asked = |a: &&Ask| old.is_some_and(|o| !o.asks.iter().any(|b| b.id == a.id));
        let fresh = row.asks.iter().find(asked).map(|a| a.text.clone());
        // The place lights when its branch changes (the project read), never on an edit:
        // "every edit moves that, and lit it the whole bar flashed grey" (Trey, 2026-10-15).
        if self.root.as_ref() == Some(&row.session)
            && old.is_some_and(|o| o.headline != row.headline)
        {
            self.changed[1] = Some(now());
        }
        if self.focus.as_ref() == Some(&row.session) {
            let was = old.map(|a| a.state);
            self.busy_at = match row.state {
                State::Working if was != Some(State::Working) => {
                    mcp::read(self, Duration::ZERO);
                    Some(now())
                }
                State::Working => self.busy_at,
                _ => None,
            };
            self.tide = gauge::tide(self, &row, now()).or(self.tide.take());
        }
        self.stage.agent(&row, now());
        self.agent_bytes = bytes;
        self.agents.insert(row.session.clone(), row);
        commands::ended(self);
        if let Some(text) = fresh {
            let more = match self.asks().len() {
                0 | 1 => String::new(),
                n => format!(" +{}", n - 1),
            };
            let words = text.split_whitespace().collect::<Vec<_>>().join(" ");
            self.say(&format!("needs you: {}{more}", text::clip(&words, 60)));
        }
    }
}
