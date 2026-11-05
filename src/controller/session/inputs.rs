//! Admission into the session: inputs, answers to its questions, `/done` and `/quit`, a
//! handover request, withdrawals, and the news a waiting call takes.

use super::*;

/// `/done`'s input, which the model reads (RULES.md 16).
const DONE: &str = include_str!("done.md");

impl Session {
    /// Records an input and starts a turn on it when the session waits for one; news goes to a
    /// waiting call first.
    pub(in crate::controller) async fn admit(
        &mut self,
        mut input: Input,
    ) -> Result<EntryId, Error> {
        if let Some(entry) = self.record.summary().repeat(input.request.as_deref()) {
            return Ok(entry);
        }
        if input.from == Origin::Heartbeat && self.state == State::Working {
            return Err(Error::Busy(self.id.clone()));
        }
        // A delayed request never runs a task its parent has since replaced, or one already
        // reported, again: a child's tasks only move forward.
        let s = self.record.summary();
        if let (Some(task), Some(now)) = (input.child_task, s.child_task)
            && (task < now || task == now && s.reported.is_some())
        {
            return invalid("The child finished or moved past this task; its report reaches you.");
        }
        // A closed session that is no child reopens for the person and their commands, and for
        // its resident's schedules and heartbeats; mail and programs are told so (Trey, 2026-11-01).
        let others = matches!(input.from, Origin::Program { .. } | Origin::Mail { .. });
        let closed = self.blocked && self.record.summary().child.is_none();
        if closed && (others || matches!(input.from, Origin::Report { .. })) {
            return invalid("This session is closed; only the person can reopen it.");
        }
        if self.full() {
            return Err(Error::Full(self.id.clone()));
        }
        // A call's hand-off answers the agent's own newest open ask; a server's question stays open.
        for ask in self.record.summary().questions.iter().rev() {
            if input.voice_call.is_some()
                && let Body::Call { .. } = self.record.entry(ask.id.1).await?.body
            {
                input.from = Origin::Answer { ask: ask.id.1 };
                break;
            }
        }
        let s = self.record.summary();
        // A plugin's change is what is recorded, so the size check below applies to it.
        let (from, text) = (&input.from, &mut input.text);
        let event = Before::Input { from, text };
        if let Err(e) = self.plugins.before(&self.id, &s.cwd, event).await {
            let text = e.to_string();
            self.publish(&Live::Plugin { text });
            return Err(e.into());
        }
        let max = match input.from {
            Origin::Person | Origin::Program { .. } => 1 << 20,
            _ => 64 << 10,
        };
        if input.text.len() > max {
            return invalid(&format!(
                "This input is larger than {max} bytes; shorten it."
            ));
        }
        let outside = |c: Component| !matches!(c, Component::Normal(_));
        let mut paths = input.images.iter().map(|i| &i.path).chain(&input.files);
        if paths.any(|path| path.components().any(outside)) {
            return invalid("Image and file paths must stay inside the session directory.");
        }
        self.choosable(input.model.as_deref(), input.speed)?;
        let person = input.from.is_person() && input.voice.is_none() && input.voice_call.is_none();
        let entry = self.append(Body::Input(input.clone()), None).await?;
        // A fresh run's thread closes with the turn that answers its schedule.
        self.done = self.done.or(self.record.summary().job());
        if !self.wake(entry, &input) {
            self.pending.push_back((entry, input));
            // A closed session that is no child reopens on its next input.
            self.blocked &= self.record.summary().child.is_some();
            if self.state != State::Working && !self.blocked {
                self.begin().await;
            }
        }
        if person {
            clear_draft(self.record.dir().to_path_buf()).await?;
        }
        Ok(entry)
    }

    /// Admits `input`. A closed child reopens on the person's input or a `continuing` one as a
    /// new task: its parent records `Continued`, which the input then names. A running turn
    /// refuses a `continuing` one. Reopenings run in this queue one at a time, so a second
    /// finds the child open.
    pub(in crate::controller) async fn take(
        &mut self,
        mut input: Input,
        continuing: bool,
    ) -> Result<EntryId, Error> {
        let closed = self.blocked && self.record.summary().child.is_some();
        if continuing && !closed && self.state == State::Working {
            return invalid("Still working; message it: notes reach it at its next tool call.");
        }
        let reopen = closed && (continuing || input.from.is_person());
        if reopen && input.child_task.is_none() {
            let task = children::continuation(&self.c, self.record.summary()).await;
            input.child_task = Some(task.map_err(|e| Error::Invalid(e.to_string()))?);
        }
        let entry = self.admit(input).await?;
        if reopen {
            self.continued().await?;
        }
        Ok(entry)
    }

    /// Refuses a model or speed this session cannot run: an input's, or a live call's choice.
    pub(in crate::controller) fn choosable(
        &self,
        model: Option<&str>,
        speed: Option<crate::protocol::Speed>,
    ) -> Result<(), Error> {
        let s = self.record.summary();
        if model.is_some() || speed.is_some() {
            let speed = speed.unwrap_or(s.settings.speed);
            profiles::speed(model.unwrap_or(&s.model), speed, &self.settings.config)?;
        }
        model.map_or(Ok(()), |model| {
            self.render(model, self.tools.actions()).map(drop)
        })
    }

    /// Admits `/done`'s input and clears the draft; the turn that answers it is the last. With no
    /// closeout (`/quit`), entry 0 marks the turn ending now as the last: a running turn stops
    /// there, an idle session ends one that answers nothing, and 0 is returned.
    pub(in crate::controller) async fn done(&mut self, closeout: bool) -> Result<EntryId, Error> {
        if !closeout {
            (self.done, self.conversation) = (Some(EntryId(0)), None);
            return self.stop().await.map(|()| EntryId(0));
        }
        let entry = self.admit(Command::Done.input(DONE.into())).await?;
        clear_draft(self.record.dir().to_path_buf()).await?;
        self.done = Some(entry);
        self.publish_row();
        Ok(entry)
    }

    /// The person's request and the context threshold share one continuation-note input, the
    /// person's recorded as `/handover`. While the session works it is sent at once and ends
    /// every waiting call; an idle session admits it as a turn.
    pub(in crate::controller) async fn handover(
        &mut self,
        trigger: Trigger,
    ) -> Result<Option<EntryId>, Error> {
        if self.blocked && self.record.summary().child.is_some() {
            return invalid("This child is closed; continue it before requesting a handover.");
        }
        let busy =
            |c: &Conversation| c.threshold.is_some() || c.handover.is_some() || c.rebuilt.is_some();
        if self.conversation.as_ref().is_some_and(busy) {
            return Ok(None);
        }
        // With no room a reading asks again at the next, and the person is refused as for input.
        if self.full() {
            return match trigger {
                Trigger::Threshold | Trigger::Change => Ok(None),
                _ => Err(Error::Full(self.id.clone())),
            };
        }
        let text = record::handover(trigger).into();
        let asked = match trigger {
            Trigger::Handover => Command::Handover.input(text),
            _ => Input {
                trigger: Some(trigger),
                ..Input::new(Origin::Razor, text)
            },
        };
        let entry = if self.state == State::Working {
            let entry = self.append(Body::Input(asked.clone()), None).await?;
            self.waiters.clear();
            self.deliver(vec![(entry, asked)]).await?;
            entry
        } else {
            self.admit(asked).await?
        };
        if let Some(c) = &mut self.conversation {
            c.threshold = Some((entry, trigger));
        }
        Ok(Some(entry))
    }

    pub(in crate::controller) async fn answer(
        &mut self,
        ask: EntryId,
        answer: Answer,
        device: Option<String>,
    ) -> Result<EntryId, Error> {
        let answered = |text| Input {
            device,
            ..Input::new(Origin::Answer { ask }, text)
        };
        let questions = &self.record.summary().questions;
        let Some(question) = questions.iter().find(|q| q.id.1 == ask) else {
            return invalid("This ask is not open; it may already have an answer.");
        };
        let mut text = match answer {
            Answer::Text(text) => text,
            Answer::Option(index) => match question.options.get(index) {
                Some(text) => text.clone(),
                None => return invalid("That option is not in this ask."),
            },
        };
        if let Body::Question { redirect, .. } = self.record.entry(ask).await?.body {
            if redirect {
                let completed = self.c.inner.mcp_auth.complete(&text).await;
                completed.map_err(|e| Error::Invalid(e.to_string()))?;
                text = profiles::mcp_text("signed_in", &[]);
            }
            let entry = self.append(Body::Input(answered(text)), None).await?;
            if self.state == State::Working {
                self.delivered.push(entry);
            } else {
                self.end_turn(vec![entry], End::Done, Usage::default(), None)
                    .await?;
            }
            return Ok(entry);
        }
        self.admit(answered(text)).await
    }

    pub(in crate::controller) async fn withdraw(&mut self, entry: EntryId) -> Result<(), Error> {
        let Some(at) = self.pending.iter().position(|(id, _)| *id == entry) else {
            return invalid("This input was already taken, answered or withdrawn.");
        };
        self.pending.remove(at);
        self.done.take_if(|done| *done == entry);
        let inputs = vec![entry];
        self.append(Body::Withdrawn { inputs }, None).await?;
        self.c.inner.space.notify_waiters();
        Ok(())
    }

    /// The waiting call recorded at `at` takes the first held news at once, else the next news
    /// admitted; the Result `took` names answers it.
    pub(in crate::controller) fn wait_input(
        &mut self,
        at: EntryId,
    ) -> Result<oneshot::Receiver<EntryId>, Error> {
        self.waiters.retain(|(_, waiter)| !waiter.is_closed());
        if self.taken.len() >= TAKEN {
            return invalid(
                "This turn's waits hold 64 news the model has not read yet; return from the cell, \
                 or wait on it by cell_id, so the model reads them, then wait again.",
            );
        }
        let (reply, answer) = oneshot::channel();
        match self.pending.iter().position(|(_, input)| news(input)) {
            Some(held) => {
                if let Some((entry, _)) = self.pending.remove(held) {
                    self.consume(entry, at);
                    let _ = reply.send(entry);
                    // Held news leaving `pending` frees room for an input waiting for it.
                    self.c.inner.space.notify_waiters();
                }
            }
            // An input sent but unread, such as the person's that joined at this call's own
            // admission or a handover request, reaches the model only after this batch's results,
            // so it ends the call, as does the person's input admitted since the call (see
            // `wake`); `reply` drops unsent.
            None if !self.sent.is_empty()
                || self.pending.iter().any(|(id, i)| *id > at && person(i)) => {}
            None if self.waiters.len() >= 256 => {
                return invalid("This session already has 256 waiting calls.");
            }
            None => self.waiters.push_back((at, reply)),
        }
        Ok(answer)
    }

    /// Whether an `agent wait` call is still waiting, before its deadline.
    pub(in crate::controller) fn waiting(&mut self) -> BoxFuture<'_, Result<bool, Error>> {
        let waiting = self.waiters.iter().any(|(_, waiter)| !waiter.is_closed());
        Box::pin(std::future::ready(Ok(waiting)))
    }

    /// Gives admitted news to a waiting call, if one waits. The person's input ends every wait
    /// with no news, so it joins the turn at the boundary after the call's Result.
    pub(super) fn wake(&mut self, entry: EntryId, input: &Input) -> bool {
        if person(input) {
            self.waiters.clear();
        }
        // At TAKEN the news waits in `pending`, as during any other long call.
        if !news(input) || self.taken.len() >= TAKEN {
            return false;
        }
        while let Some((at, waiter)) = self.waiters.pop_front() {
            if waiter.send(entry).is_ok() {
                self.consume(entry, at);
                return true;
            }
        }
        false
    }

    fn consume(&mut self, entry: EntryId, at: EntryId) {
        self.taken.push((at, entry));
        self.publish(&Live::Taken { input: entry });
    }

    /// A continued child takes its held inputs.
    pub(in crate::controller) async fn continued(&mut self) -> Result<(), Error> {
        self.blocked = false;
        self.turn = Default::default();
        self.begin().await;
        Ok(())
    }
}

/// Mail, reports and Razor notices, not the person's commands: news a waiting call can take.
fn news(input: &Input) -> bool {
    let razor = matches!(input.from, Origin::Razor) && input.command.is_none();
    razor || matches!(input.from, Origin::Mail { .. } | Origin::Report { .. })
}

/// The person's words: a message, or an answer to the person's ask.
fn person(input: &Input) -> bool {
    matches!(input.from, Origin::Person | Origin::Answer { .. })
}

/// Empties the person's saved draft.
async fn clear_draft(dir: std::path::PathBuf) -> Result<(), Error> {
    super::super::blocking(move || super::super::files::write(&dir, "draft", b"")).await
}
