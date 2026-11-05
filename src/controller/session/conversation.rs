//! The session's model conversation: opening, resuming and rebuilding it, what is sent to it,
//! and the lines it sends back.

use super::*;
use crate::protocol::{Effort, Switch};
use crate::tools::Action;

impl Session {
    /// Sends this session's `message` to the open conversation, the opening's page (facts,
    /// memory, skills) ahead of its first input.
    pub(super) async fn say(
        &mut self,
        message: impl FnOnce(SessionId) -> ToBridge,
    ) -> Result<(), Error> {
        let Some(c) = &mut self.conversation else {
            return Ok(());
        };
        let mut message = message(self.id.clone());
        if let ToBridge::Input { inputs, .. } = &mut message {
            inputs.splice(..0, c.ahead.drain(..));
            if let Some(first) = inputs.first_mut()
                && let Some(page) = c.page.take()
            {
                first.text = format!("{page}\n\n{}", first.text);
            }
        }
        c.send
            .send(message)
            .await
            .map_err(other("speaking to the model path"))
    }

    /// Re-renders the profile from the live registry; a conversation on the same model and
    /// version takes changed tools in place (`Add`).
    pub(super) async fn refresh(&mut self) -> Result<(), Error> {
        self.profile = Arc::new(self.render(&self.model, self.tools.actions())?);
        let Some(c) = self.conversation.as_ref().filter(|c| {
            c.model == self.model && c.profile.borrow().version == self.profile.version
        }) else {
            return Ok(());
        };
        let added = c.profile.send_replace(self.profile.clone()).tools != self.profile.tools;
        if !added {
            return Ok(());
        }
        let named = |tool: &&serde_json::Value| tool.get("name").is_some();
        let tools: Vec<_> = self.profile.tools.iter().filter(named).cloned().collect();
        self.say(|session| ToBridge::Add { session, tools }).await
    }

    /// Applies changed settings files to the actions; the MCP revision is taken first, so an
    /// operation after it is seen again.
    pub(super) fn reconcile(&mut self) {
        self.mcp_revision = self.c.inner.mcp_revision.load(Ordering::Acquire);
        self.settings.read();
        self.tools.configure(crate::tools::Config {
            plugins: self.plugins.tools(),
            ..self.settings.tools()
        });
    }

    /// The profile for `model` and `actions`, from the turn's config.
    pub(super) fn render(&self, model: &str, actions: Arc<[Action]>) -> Result<Profile, Error> {
        let child = self.record.summary().child.is_some();
        let config = &self.settings.config;
        Ok(config.profile(&self.c.inner.endpoint, model, actions, child)?)
    }

    /// Why the conversation cannot go on with `model`, rendered as `profile`: another profile,
    /// or a latest reading at that model's rebuild threshold where its window is known.
    pub(super) fn unkept(&self, model: &str, profile: &Profile) -> Option<String> {
        if self.record.summary().profile_version != profile.version {
            return Some(format!("this conversation does not carry over to {model}"));
        }
        let (window, threshold) = limits(model, &self.settings.config, None);
        let full = |w: u64| self.context >= Some((threshold * w as f64) as u64);
        let over = window.is_some_and(full);
        over.then(|| format!("the conversation is past {model}'s rebuild threshold"))
    }

    /// What a switch to each of `models` but the session's own reads without the prompt cache:
    /// the latest reading kept, or about an opening of `startup.md`, whose rebuilt context (a
    /// median 11% of an opening's bytes) is left out, so pricing reads neither the record nor the
    /// working tree.
    pub(in crate::controller) async fn switch(
        &mut self,
        models: Vec<String>,
    ) -> Result<BTreeMap<String, Switch>, Error> {
        let startup = tokio::fs::metadata(self.record.dir().join("startup.md")).await;
        let (s, bytes) = (self.record.summary(), startup.map_or(0, |m| m.len()));
        // Closed, a conversation resumes only after a finished turn, and an unread rebuild, a
        // handover already made, opens whatever the model.
        let closed = self.conversation.is_none();
        let unread =
            closed && s.native.is_none() && s.responses.is_empty() && s.last_rebuild.is_some();
        let lost = closed && s.last_end != Some(End::Done);
        let floor = self.settings.config.sessions.handover_tokens;
        let mut switches = BTreeMap::new();
        for model in models.into_iter().filter(|m| *m != self.model) {
            let Ok(profile) = self.render(&model, self.tools.actions()) else {
                continue;
            };
            let reason = lost.then(|| "the last turn did not finish".to_owned());
            let reason = reason.or_else(|| self.unkept(&model, &profile));
            let keep = self.context.filter(|_| !unread && reason.is_none());
            let tokens = profiles::tokens(profiles::model(&model).transport, bytes);
            let handover =
                (self.context.is_some() && keep.is_none_or(|n| n >= floor)).then_some(tokens);
            let switch = Switch {
                keep,
                handover,
                reason,
            };
            switches.insert(model, switch);
        }
        Ok(switches)
    }

    /// Opens a conversation fresh, or with `keep` resumes the native one; false when only a
    /// rebuild can carry the record on.
    pub(super) async fn reopen(&mut self, keep: bool) -> Result<bool, Error> {
        let s = self.record.summary();
        let unread = s.native.is_none() && s.responses.is_empty();
        // A finished rebuild no model has read, an idle one's, goes ahead of the first input.
        let mut rebuilt = None;
        if unread
            && s.last_end == Some(End::Done)
            && let Some((id, _)) = s.last_rebuild
            && let Body::Rebuild { context, .. } = self.record.entry(id).await?.body
        {
            rebuilt = Some(context);
        }
        let resume = if unread && (s.last_end.is_none() || rebuilt.is_some()) {
            None
        } else if keep
            && s.last_end == Some(End::Done)
            && s.profile_version == self.profile.version
            && let Some(resume) = self.resume().await?
        {
            Some(resume)
        } else {
            return Ok(false);
        };
        self.open(resume).await?;
        if let Some(c) = &mut self.conversation
            && let (Some(page), Some(context)) = (&mut c.page, rebuilt)
        {
            *page = format!("{page}\n\n{context}");
        }
        Ok(true)
    }

    /// The native history a clean reopening continues, when the model path keeps one.
    pub(super) async fn resume(&mut self) -> Result<Option<Resume>, Error> {
        let s = self.record.summary();
        Ok(match profiles::model(&self.model).transport {
            Transport::Claude => match &s.native {
                Some(Native::Claude { conversation, uuid }) => Some(Resume::Claude {
                    conversation: conversation.clone(),
                    at: uuid.clone(),
                }),
                _ => None,
            },
            Transport::Responses | Transport::Chat(_) if !s.responses.is_empty() => {
                let mut history = Vec::new();
                for id in &s.responses {
                    if let Some(Native::Responses { additions, items }) =
                        self.record.entry(*id).await?.native
                    {
                        // This response's calls have no native outputs until another response.
                        if Some(id) == s.responses.last()
                            && items.iter().any(|i| {
                                i.get("call_id").is_some() || i["tool_calls"][0].is_object()
                            })
                        {
                            return Ok(None);
                        }
                        history.extend(additions);
                        history.extend(items);
                    }
                }
                let items = profiles::Conversation::resume(&self.profile, history)?.snapshot();
                Some(Resume::Responses { items })
            }
            Transport::Responses | Transport::Chat(_) => None,
        })
    }

    /// Opens a conversation on the current model, its system prompt read from the startup list,
    /// fresh with the facts, memory page and skills (shown in `startup.md`) or resumed. Its lines
    /// arrive on a channel of its own, so a dropped conversation's are never seen, and dropping
    /// it ends the native conversation.
    pub(super) async fn open(&mut self, resume: Option<Resume>) -> Result<(), Error> {
        let mut profile = self.render(&self.model, self.tools.actions())?;
        let tools = self.render(&self.model, self.tools.initial())?.tools;
        let (dir, s) = (self.record.dir().to_path_buf(), self.record.summary());
        let (config, root) = (&self.settings.config, self.settings.project.as_deref());
        let resident = s.thread.as_ref().map_or(&self.id, |t| &t.resident);
        let (home, thread) = (self.c.dir(resident), s.thread.is_some());
        let identity = crate::residents::identity(config, resident, &home, thread).await;
        let list = if s.child.is_some() {
            &config.children.startup
        } else {
            config.startup_for(resident)
        };
        let razor_home = &self.c.inner.home;
        let opening = profiles::opening(&profile, list, root, s, identity, razor_home).await;
        profile.prompt.clone_from(&opening.prompt);
        let (model, prompt) = (&self.model, &mut profile.prompt);
        let event = Before::Prompt { model, prompt };
        self.plugins.before(&self.id, &s.cwd, event).await?;
        self.profile = Arc::new(profile);
        // A resumed conversation already holds this list; only later changes are news.
        self.mcp_key = self.tools.mcp_key();
        let page = match resume {
            Some(_) => None,
            None => {
                let memory = self.c.inner.memory.page(&self.c, &s.cwd, &self.id);
                let (memory, skills) = tokio::join!(memory, self.tools.skills());
                let page = opening.page(memory, skills, &self.tools.mcp_section());
                profiles::save(&dir, &self.profile.prompt, &tools, &page).await;
                Some(page)
            }
        };
        let resuming = resume.is_some();
        let (send, incoming) = mpsc::channel(64);
        let (outgoing, lines) = mpsc::channel(64);
        let (profile, watching) = watch::channel(self.profile.clone());
        let open = ToBridge::Open {
            session: self.id.clone(),
            // Every open follows an admitted input or rebuild, even after unloading.
            open: s.last.0,
            cwd: s.cwd.clone(),
            model: self.model.clone(),
            effort: self.model_settings.effort,
            speed: self.model_settings.speed,
            resume,
            prompt: self.profile.prompt.clone(),
            tools,
            additions: self.profile.tools.clone(),
        };
        send.send(open)
            .await
            .map_err(other("speaking to the model path"))?;
        let (inner, claude) = (&self.c.inner, &self.settings.config.claude);
        match profiles::model(&self.model).transport {
            Transport::Claude => inner
                .bridge
                .start(dir, claude, s.child.is_some(), watching, incoming, outgoing)
                .await
                .map_err(other("using the model path"))?,
            Transport::Responses | Transport::Chat(_) => {
                let (native, mut replies) = mpsc::channel(64);
                (inner.responses).start(dir, watching, incoming, native);
                tokio::spawn(async move {
                    while let Some(line) = replies.recv().await {
                        if outgoing.send((line, None)).await.is_err() {
                            break;
                        }
                    }
                });
            }
        }
        lock(&self.c.inner.status).usage(&self.id, Vec::new());
        self.conversation = Some(Conversation {
            send,
            lines,
            profile,
            model: self.model.clone(),
            page,
            ahead: Vec::new(),
            resuming,
            rebuilt: None,
            threshold: None,
            handover: None,
            closing: None,
            interrupted: false,
            thoughts: self.thoughts,
        });
        Ok(())
    }

    /// Records and applies effort now, retaining speed and the current conversation.
    pub(in crate::controller) async fn effort(
        &mut self,
        effort: Effort,
    ) -> Result<Option<EntryId>, Error> {
        if effort == self.model_settings.effort && effort == self.record.summary().settings.effort {
            return Ok(None);
        }
        let entry = self.append(Body::Effort { effort }, None).await?;
        self.model_settings.effort = effort;
        let speed = self.model_settings.speed;
        let model = self.conversation.as_ref().map(|c| c.model.clone());
        self.say(|session| ToBridge::Effort {
            session,
            model: model.unwrap_or_default(),
            effort,
            speed,
            notice: Some(entry),
        })
        .await?;
        Ok(Some(entry))
    }

    /// Sends `batch` as one native user message, image paths made absolute, and tells clients
    /// each input is taken. The person's handover in it is this conversation's threshold input.
    pub(super) async fn deliver(&mut self, batch: Vec<(EntryId, Input)>) -> Result<(), Error> {
        for (_, input) in &batch {
            if let Some(effort) = input.effort {
                self.model_settings.effort = match input.from {
                    Origin::Heartbeat | Origin::Schedule { .. } => effort,
                    _ => self.record.summary().settings.effort,
                };
            }
            self.model_settings.speed = input.speed.unwrap_or(self.model_settings.speed);
        }
        let settings = self.model_settings;
        let model = self.conversation.as_ref().map(|c| c.model.clone());
        self.say(|session| ToBridge::Effort {
            session,
            model: model.unwrap_or_default(),
            effort: settings.effort,
            speed: settings.speed,
            notice: None,
        })
        .await?;
        let asked = batch
            .iter()
            .find(|(_, i)| i.command == Some(Command::Handover));
        if let (Some(c), Some((entry, _))) = (&mut self.conversation, asked) {
            c.threshold = Some((*entry, Trigger::Handover));
        }
        let (taken, mut inputs): (Vec<_>, Vec<_>) = batch
            .into_iter()
            .map(|(id, i)| (id, self.rendered(id, i)))
            .unzip();
        self.delivered.extend(&taken);
        self.sent.extend(&taken);
        self.calls(&mut inputs).await?;
        self.say(|session| ToBridge::Input { session, inputs })
            .await?;
        for input in taken {
            self.publish(&Live::Taken { input });
        }
        Ok(())
    }

    /// Puts the unread call and meeting facts not yet sent to this conversation ahead of `inputs`,
    /// as one input that its turn's end answers: every start, naming and end, the newest lines
    /// within 64 KiB, and how many earlier lines were left out. A call is running only if this
    /// server started it; a meeting until its end. A rebuild renders none of them, so it sends
    /// them all again.
    async fn calls(&mut self, inputs: &mut Vec<BridgeInput>) -> Result<(), Error> {
        let s = self.record.summary();
        let facts = s.voice_calls.iter().copied();
        let unsent: Vec<_> = facts
            .filter(|(id, _)| !self.delivered.contains(id))
            .collect();
        let Some(&(entry, _)) = unsent.first() else {
            return Ok(());
        };
        let (mut left_out, mut room, mut said) = (s.voice_calls_left_out, 64 << 10, Vec::new());
        // The oldest unsent line stands for those left out, which are older still.
        let mut oldest = None;
        for &(id, line) in unsent.iter().rev() {
            let fact = self.record.entry(id).await?;
            let Body::VoiceCall {
                call,
                source,
                event,
            } = fact.body
            else {
                continue;
            };
            let text = record::call_line(source, &event, fact.at >= self.c.inner.started);
            oldest = if line { Some(call) } else { oldest };
            if line && text.len() > room {
                left_out += 1;
                continue;
            }
            room = room.saturating_sub(text.len());
            said.push(text);
        }
        let mut meetings = s.meetings.iter();
        let folder = oldest.and_then(|call| meetings.find(|m| m.id == call));
        let folder = folder.map(|m| m.folder.as_str());
        said.extend((left_out > 0).then(|| record::calls_left_out(left_out, folder)));
        said.reverse();
        self.delivered.extend(unsent.iter().map(|(id, _)| id));
        let calls = BridgeInput {
            entry,
            text: said.concat(),
            ..Default::default()
        };
        inputs.insert(0, calls);
        Ok(())
    }

    pub(super) fn rendered(&self, entry: EntryId, input: Input) -> BridgeInput {
        record::heard(self.record.summary(), self.record.dir(), entry, input)
    }

    /// How many of `inputs` fit one encoded message within the bridge's line budget, the memory
    /// page and a rebuild's context counted, and within UNREAD less what the model path holds
    /// unread and what goes ahead of them (a rebuild's videos, any unsent call facts): so news
    /// put back past UNREAD waits in `pending`.
    pub(super) fn fit<'a>(&self, inputs: impl Iterator<Item = &'a (EntryId, Input)>) -> usize {
        let empty = ToBridge::Input {
            session: self.id.clone(),
            inputs: Vec::new(),
        };
        let mut bytes = encoded(&empty) + 1;
        let mut calls = self.record.summary().voice_calls.iter();
        let mut ahead = usize::from(calls.any(|(id, _)| !self.delivered.contains(id)));
        if let Some(c) = &self.conversation {
            // The rebuilt context's slot is in `sent`, its videos' are not. The page: two quotes
            // off, four bytes of escaped newlines on.
            ahead += c.ahead.len().saturating_sub(1);
            let context = c.ahead.iter().map(|input| encoded(input) + 1);
            bytes += context.sum::<usize>() + c.page.as_ref().map_or(0, |page| encoded(page) + 2);
        }
        let fits = |(entry, input): &&(EntryId, Input)| {
            bytes += encoded(&self.rendered(*entry, input.clone())) + 1;
            bytes <= crate::claude::BUDGET
        };
        let room = UNREAD.saturating_sub(self.sent.len() + ahead);
        inputs.take(room).take_while(fits).count()
    }

    /// At a tool boundary, queued end notices are admitted first; then held inputs join the turn
    /// when they are the person's, an answer to the person's ask, direct mail from this
    /// session's parent, an answer to a question this session asked (Trey, 2026-10-29), or a
    /// job's end notice (Trey, 2026-10-30): any Razor input but a command's, since `/done` waits
    /// for the turn, handover requests and MCP news are sent at once, and the nudge only starts a
    /// waiting session. The rest, channel mentions included, wait for the turn's end, and those
    /// that join pass them (Trey, 2026-10-30). The person's words join though they name another
    /// model, which takes over at the next turn (Trey, 2026-11-04). Not while the conversation
    /// settles, nor in a switch's note turn, whose inputs wait at the front for the new model.
    pub(super) async fn boundary(&mut self) -> Result<(), Error> {
        if matches!(&self.conversation, Some(c) if c.handover.is_some() || c.closing.is_some()) {
            return Ok(());
        }
        self.flush_notices(true).await?;
        if matches!(&self.conversation, Some(c) if matches!(c.threshold, Some((_, Trigger::Change))))
        {
            return Ok(());
        }
        let batch = self.joining(Vec::new());
        if batch.is_empty() {
            return Ok(());
        }
        self.deliver(batch).await
    }

    /// `batch`, then the held inputs that join a turn at a tool boundary, as many as fit one
    /// message; the rest stay held, in their order.
    fn joining(&mut self, mut batch: Vec<(EntryId, Input)>) -> Vec<(EntryId, Input)> {
        let parent = self.record.summary().child.as_ref().map(|c| &c.parent);
        let joins = |(_, input): &(EntryId, Input)| match &input.from {
            Origin::Person | Origin::Answer { .. } => true,
            Origin::Razor => input.command.is_none(),
            Origin::Mail { id, kind, .. } => match kind {
                MailKind::Mention => false,
                MailKind::Answer { question } => question.0 == self.id || Some(&id.0) == parent,
                _ => Some(&id.0) == parent,
            },
            _ => false,
        };
        let (joined, held): (Vec<_>, Vec<_>) = self.pending.drain(..).partition(joins);
        batch.extend(joined);
        let fit = self.fit(batch.iter());
        self.pending = batch.drain(fit..).chain(held).collect();
        batch
    }

    /// A thought line under the session's number for the conversation's `n`th thought.
    fn thought(&mut self, n: u64, text: String, start: bool, void: bool) {
        let n = self.conversation.as_ref().map_or(0, |c| c.thoughts) + n;
        self.thoughts = n;
        self.publish(&Live::Thought {
            n,
            text,
            start,
            void,
        })
    }

    /// One line of the open conversation; `None` when its model path is gone.
    pub(super) async fn line(&mut self, line: Option<Output>) -> Result<(), Error> {
        let Some((line, _held)) = line else {
            return self.closed(None).await;
        };
        if let Some(c) = &mut self.conversation {
            c.resuming &= matches!(line.says, FromBridge::Closed { .. });
            c.interrupted &= !matches!(line.says, FromBridge::TurnEnd { .. });
        }
        match line.says {
            FromBridge::Entry { body, native } => self.entry(body, native).await?,
            FromBridge::Delta { text } => self.publish(&Live::Delta { text }),
            FromBridge::Thought { n, text, start } => self.thought(n, text, start, false),
            FromBridge::Void { n } => self.thought(n, String::new(), false, true),
            FromBridge::Run { call, tool, input } => self.run(call, tool, input).await?,
            FromBridge::Context {
                tokens,
                window,
                read,
            } => {
                self.read(read);
                self.context(tokens, window).await?
            }
            FromBridge::TurnEnd {
                success,
                reason,
                message,
                usage,
                window,
                took,
            } => {
                self.reported = Some(window).filter(|w| *w > 0).or(self.reported);
                self.turn_ended(success, reason, message, usage, took)
                    .await?
            }
            FromBridge::Settled { usage } => self.settled(usage).await?,
            FromBridge::Closed { error } => self.closed(error).await?,
            FromBridge::Usage { windows } => lock(&self.c.inner.status).usage(&self.id, windows),
        }
        Ok(())
    }

    /// Replaces the conversation with one rebuilt from the record up to the cut, the record's
    /// end (so news a wait took, which only a Result carried, is in it); the rebuilt turn answers
    /// every unanswered input at or before the cut, and `batch`, the turn's own inputs, which
    /// follow the context in its message, so the new model's request ends with them: after a
    /// switch's note, the switch's turn's opening inputs held in `pending`. Held inputs that
    /// would join at a tool boundary, wherever they sit, come next in the same message, so the
    /// person's words held at a rebuild end its request; the rest follow in later turns.
    pub(super) async fn rebuild(
        &mut self,
        trigger: Trigger,
        note: Option<String>,
        mut batch: Vec<(EntryId, Input)>,
    ) -> Result<(), Error> {
        self.actions.clear();
        // Any rebuild gives a planned night the fresh context it was waiting for.
        self.planned = None;
        self.unknown_results().await?;
        if trigger == Trigger::Change && batch.is_empty() {
            let held = std::mem::take(&mut self.pending).into_iter();
            let (opening, rest): (Vec<_>, Vec<_>) = held.partition(|i| self.opening.contains(&i.0));
            (batch, self.pending) = (opening, rest.into());
        }
        self.conversation = None;
        let cut = self.record.summary().last;
        let held: Vec<_> = (self.pending.iter().chain(&batch))
            .map(|(id, _)| *id)
            .collect();
        let (note, mut context, videos) = self.record.rebuild(trigger, note, cut, &held).await?;
        let cwd = &self.record.summary().cwd;
        let event = Before::Rebuild {
            trigger,
            context: &mut context,
        };
        self.plugins.before(&self.id, cwd, event).await?;
        let rebuild = Body::Rebuild {
            trigger,
            note,
            context: context.clone(),
            profile_version: self.profile.version.clone(),
            cut,
        };
        let entry = self.append(rebuild, None).await?;
        let covered = self
            .record
            .summary()
            .unanswered
            .iter()
            .filter(|id| **id <= cut && !held.contains(id));
        self.delivered = std::iter::once(entry).chain(covered.copied()).collect();
        self.sent = vec![entry];
        // Nobody waits on an idle handover with no input since: its rebuild ends the turn, however
        // the note turn ended (an overflow, a closed conversation), and the next input opens the
        // fresh conversation with it (`reopen`). A resident's night rebuild still opens its turn.
        if trigger != Trigger::Night && self.record.summary().idle_handover {
            self.end_turn(self.delivered.clone(), End::Done, Usage::default(), None)
                .await?;
            self.pending = batch.into_iter().chain(self.pending.drain(..)).collect();
            self.set_state(State::NeedsYou);
            return Ok(());
        }
        self.open(None).await?;
        // A video still unanswered, or a watcher's own, goes on to a model path that takes it.
        let video = self.profile.endpoint.video();
        if let Some(c) = &mut self.conversation {
            c.rebuilt = Some(context.len());
            c.ahead = videos.into_iter().filter(|_| video).collect();
            c.ahead.push(BridgeInput {
                entry,
                text: context,
                ..Default::default()
            });
        }
        let batch = self.joining(batch);
        self.deliver(batch).await?;
        self.set_state(State::Working);
        Ok(())
    }

    /// Notes the context's size; past the threshold share of the window, the one threshold
    /// input asks the model to hand over.
    pub(super) async fn context(&mut self, tokens: u64, window: Option<u64>) -> Result<(), Error> {
        self.context = Some(tokens);
        self.reported = window.filter(|w| *w > 0).or(self.reported);
        self.publish_row();
        let (window, threshold) = limits(&self.model, &self.settings.config, self.reported);
        let over = window.is_some_and(|w| tokens as f64 >= threshold * w as f64);
        if over && !self.blocked && self.conversation.is_some() {
            self.handover(Trigger::Threshold).await?;
        }
        // The rebuilt request fitted: later readings may ask for the next handover. One that did
        // not keeps the guard, so a rebuild too large for the threshold cannot rebuild again.
        if let Some(c) = self.conversation.as_mut().filter(|_| !over) {
            c.rebuilt = None;
        }
        Ok(())
    }
}

/// The open conversation's next line, `None` once its model path is gone; never, without one.
pub(super) async fn next(conversation: &mut Option<Conversation>) -> Option<Output> {
    match conversation {
        Some(c) => c.lines.recv().await,
        None => std::future::pending().await,
    }
}

/// The bytes `value` takes as JSON; protocol values hold only strings, numbers and plain enums.
fn encoded(value: &impl serde::Serialize) -> usize {
    serde_json::to_vec(value).map_or(0, |json| json.len())
}
