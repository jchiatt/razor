//! Turn partitioning, model changes and the inputs each turn answers.

use super::*;

impl Session {
    /// Starts a turn; a failure ends the turn with its sentence instead of reaching the caller,
    /// whose input is already admitted.
    pub(super) async fn begin(&mut self) {
        if let Err(e) = self.start_turn().await {
            self.fail(&e).await;
        }
    }

    /// Opens the next turn on the held inputs: a heartbeat or schedule alone, on its own model
    /// and effort, else every input up to the next one, as many as fit one message.
    pub(super) async fn start_turn(&mut self) -> Result<(), Error> {
        if !self.blocked && !self.record.summary().closed {
            self.flush_notices(false).await?;
        }
        // Inputs sent and not yet answered keep the path working on them (an interrupted turn's).
        let Some((_, front)) = self.pending.front().filter(|_| !self.blocked) else {
            let state = [State::NeedsYou, State::Working][usize::from(!self.delivered.is_empty())];
            self.set_state(state);
            return Ok(());
        };
        let alone = |i: &Input| matches!(i.from, Origin::Heartbeat | Origin::Schedule { .. });
        let s = self.record.summary();
        let speed = s.settings.speed;
        let (count, model, effort) = if alone(front) {
            (
                1,
                front.model.clone().unwrap_or_else(|| s.model.clone()),
                front.effort.unwrap_or(s.settings.effort),
            )
        } else {
            let count = self.pending.iter().take_while(|(_, i)| !alone(i)).count();
            (count, s.model.clone(), s.settings.effort)
        };
        // A handover the person chose waits for a turn on the model they chose: a heartbeat or
        // schedule on its own model neither takes it nor rebuilds for it, live or restarted.
        let chosen = s.handover && model == s.model;
        let mut batch: Vec<_> = self.pending.drain(..count).collect();
        self.opening = batch.iter().map(|(id, _)| *id).collect();
        self.turn.start();
        self.reconcile();
        let changed = self.model != model;
        self.reported.take_if(|_| changed);
        self.model = model;
        self.model_settings = ModelSettings { effort, speed };
        self.refresh().await?;
        // The one switch decision: the conversation goes on, on the new model, when it can and
        // the person did not choose a handover; otherwise a rebuild.
        let keep = !chosen && self.unkept(&self.model, &self.profile).is_none();
        let same = |c: &Conversation| c.profile.borrow().version == self.profile.version;
        let trigger = match &mut self.conversation {
            Some(c) if c.model == self.model && same(c) => None,
            Some(c) if keep && same(c) => {
                c.model.clone_from(&self.model);
                self.refresh().await?;
                None
            }
            Some(_) => Some(Trigger::Change),
            None => {
                // Loading took the summary's model, so a switch made before a restart reads
                // unchanged; its recorded handover still hands over.
                let changed = changed || chosen;
                let reopened = self.reopen(keep || !changed).await?;
                (!reopened).then_some([Trigger::Restart, Trigger::Change][usize::from(changed)])
            }
        };
        // The switch waits at the front of `pending` for its rebuild. While the cache is warm,
        // the open conversation's model writes a fresh note first, once.
        if trigger == Some(Trigger::Change) {
            let floor = Some(self.settings.config.sessions.handover_tokens);
            let spoke = now().saturating_sub(self.record.summary().spoke_at.unwrap_or(0));
            let warm = self.conversation.is_some() && spoke < 3_600_000 && self.context >= floor;
            self.pending = batch.into_iter().chain(self.pending.drain(..)).collect();
            self.set_state(State::Working);
            // Boxed: an idle session's request would start a turn, though this one is working.
            if warm && Box::pin(self.handover(Trigger::Change)).await?.is_some() {
                return Ok(());
            }
            return self.rebuild(Trigger::Change, None, Vec::new()).await;
        }
        if let Some(trigger) = trigger {
            return self.rebuild(trigger, None, batch).await;
        }
        let fit = self.fit(batch.iter());
        for held in batch.drain(fit..).rev() {
            self.pending.push_front(held);
        }
        self.deliver(batch).await?;
        self.set_state(State::Working);
        Ok(())
    }

    /// Ends the turn on the inputs the path took, which the model has read, and opens the next;
    /// a finished turn that took the threshold input rebuilds first, answering what the previous
    /// rebuild carried, and one that did not leaves its inputs for the turn that will.
    pub(super) async fn turn_ended(
        &mut self,
        success: bool,
        reason: Option<String>,
        message: String,
        usage: Usage,
        took: Option<EntryId>,
    ) -> Result<(), Error> {
        self.actions.clear();
        self.drain().await?;
        let (rebuilt, threshold) = match &mut self.conversation {
            Some(c) => (c.rebuilt.take(), c.threshold),
            None => (None, None),
        };
        if !success && reason.as_deref() == Some("prompt_too_long") && rebuilt.is_none() {
            self.end_turn(Vec::new(), End::Error(sentence(&message)), usage, None)
                .await?;
            return self.rebuild(Trigger::Overflow, None, Vec::new()).await;
        }
        let end = match (reason.as_deref(), rebuilt) {
            _ if success => End::Done,
            (Some("aborted_tools" | "aborted_streaming" | "stopped"), _) => End::Stopped,
            (Some("prompt_too_long"), Some(size)) => {
                let error = format!(
                    "The rebuilt context is {size} bytes and does not fit the model's context window."
                );
                self.conversation = None;
                return self.abandon(error).await;
            }
            _ => End::Error(sentence(&message)),
        };
        self.read(took);
        let taken = self.delivered.iter().filter(|id| !self.sent.contains(id));
        let inputs: Vec<_> = taken.copied().collect();
        // A stopped turn that answered the threshold input leaves the next reading to ask again;
        // a switch asks for its note once, and then rebuilds without one.
        if let Some(c) = self.conversation.as_mut().filter(|_| end != End::Done) {
            c.threshold
                .take_if(|(t, why)| inputs.contains(t) && *why != Trigger::Change);
        }
        if let Some((threshold, trigger)) = threshold
            && end == End::Done
        {
            self.end_turn(self.carried(), end, usage, None).await?;
            return match inputs.contains(&threshold) {
                true => self.rebuild(trigger, None, Vec::new()).await,
                false => Ok(()),
            };
        }
        if self.ended(inputs, end, usage).await? {
            return Ok(());
        }
        if let Some(trigger) = self.planned.take() {
            return self.rebuild(trigger, None, Vec::new()).await;
        }
        self.start_turn().await
    }

    /// Records the turn's end; a child open when it ended reports to its parent if the rule
    /// allows (docs/design/children.md), its report written with the turn's end, so no restart
    /// separates them, and the answer says whether it did. Its calls settle first, so the
    /// notice of a job that ended holds the report back.
    async fn ended(&mut self, inputs: Vec<EntryId>, end: End, usage: Usage) -> Result<bool, Error> {
        self.unknown_results().await?;
        let held = self.blocked || self.pending(&inputs);
        let ended = self.turn.end(&self.record, &inputs, &end, held);
        let Some((parent, report, files)) = ended.await.map_err(other("finishing a child"))? else {
            self.end_turn(inputs, end, usage, None).await?;
            return Ok(false);
        };
        if let Some(files) = files {
            self.append(Body::ChangedFiles { files }, None).await?;
        }
        self.end_turn(inputs, end, usage, Some(report)).await?;
        self.finish(parent);
        Ok(true)
    }

    /// Records the turn's end, answering those of `inputs` still unanswered, and a finished
    /// child's `report` in the same write; calls without a mirrored result get `Unknown` first.
    pub(super) async fn end_turn(
        &mut self,
        mut inputs: Vec<EntryId>,
        end: End,
        usage: Usage,
        report: Option<Input>,
    ) -> Result<(), Error> {
        self.unknown_results().await?;
        self.delivered.retain(|id| !inputs.contains(id));
        self.sent.retain(|id| !inputs.contains(id));
        let s = self.record.summary();
        let fact = |id: &EntryId| s.voice_calls.iter().any(|(fact, _)| fact == id);
        inputs.retain(|id| s.unanswered.contains(id) || fact(id));
        inputs.sort_unstable();
        inputs.dedup();
        // Entry 0 is `/quit`'s: the turn ending now is the last.
        let closed = self.done.is_some_and(|d| d.0 == 0 || inputs.contains(&d));
        let close_reason = (closed && self.done != s.job()).then_some(CloseReason::Person);
        if closed {
            self.close_out(&inputs).await?;
            close(self.c.clone(), self.id.clone(), None);
        }
        let (window, threshold) = limits(&self.model, &self.settings.config, self.reported);
        let end = Body::TurnEnd {
            inputs,
            end,
            usage,
            context: self.context,
            window,
            threshold: Some(threshold),
            closed,
            close_reason,
            report: report.map(Box::new),
        };
        self.append(end, None).await?;
        self.c.inner.space.notify_waiters();
        Ok(())
    }

    /// The batch marked to settle did: a finish closes the child, and a handover, threshold or
    /// night rebuilds, the turn's usage recorded first.
    pub(super) async fn settled(&mut self, usage: Usage) -> Result<(), Error> {
        let Some(c) = &mut self.conversation else {
            return Ok(());
        };
        let (threshold, handover) = (c.threshold, c.handover.take());
        if let Some((parent, report)) = c.closing.take() {
            // Inputs sent after the model's last request wait, unanswered, for a continuation.
            let unread = std::mem::take(&mut self.sent);
            self.put_back(unread).await?;
            let delivered = self.delivered.clone();
            self.end_turn(delivered, End::Done, usage, Some(report))
                .await?;
            self.finish(parent);
            return Ok(());
        }
        self.end_turn(self.carried(), End::Done, usage, None)
            .await?;
        let planned = || self.planned.take().unwrap_or(Trigger::Handover);
        let trigger = threshold.map_or_else(planned, |(_, trigger)| trigger);
        self.rebuild(trigger, handover, Vec::new()).await
    }

    /// The conversation ended by itself: a resumed one that never spoke is rebuilt; otherwise
    /// the turn ends with the reason.
    pub(super) async fn closed(&mut self, error: Option<String>) -> Result<(), Error> {
        let Some(c) = self.conversation.take() else {
            return Ok(());
        };
        // A switch's note turn opens the new model from the input held in `pending`.
        let switching = matches!(c.threshold, Some((_, Trigger::Change)));
        if switching || c.resuming && !self.record.summary().unanswered.is_empty() {
            let trigger = [Trigger::Restart, Trigger::Change][usize::from(switching)];
            return self.rebuild(trigger, None, Vec::new()).await;
        }
        let error = error.as_deref().unwrap_or("The Claude CLI exited.");
        self.abandon(sentence(error)).await
    }

    pub(super) async fn abandon(&mut self, why: String) -> Result<(), Error> {
        self.actions.clear();
        self.release(true);
        let unanswered = self.record.summary().unanswered.clone();
        let reported = !unanswered.is_empty()
            && self
                .ended(unanswered, End::Error(why), Usage::default())
                .await?;
        self.pending.clear();
        self.delivered.clear();
        self.sent.clear();
        // As after any turn's end, a queued end notice opens the next turn, whose end decides.
        if !reported {
            self.set_state(State::NeedsYou);
            self.start_turn().await?;
        }
        Ok(())
    }

    /// What a handover's turn end answers: the inputs up to the previous rebuild the model has
    /// read, and that rebuild's entry, but not the turn's opening inputs or the asks and parent
    /// mail the record lists. So a turn carries an input across one rebuild, and holds at most
    /// what its last two conversations read.
    fn carried(&self) -> Vec<EntryId> {
        let s = self.record.summary();
        let rebuilt = s.last_rebuild.map_or(EntryId(0), |(id, _)| id);
        let kept = [&self.opening, &s.asks, &s.parent_inputs, &self.sent];
        let carried = |id: &&EntryId| **id <= rebuilt && !kept.iter().any(|k| k.contains(id));
        self.delivered.iter().filter(carried).copied().collect()
    }

    /// A finished child, its report recorded with its turn's end, is held, and its jobs and
    /// descendants close before that report reaches the parent.
    fn finish(&mut self, parent: SessionId) {
        self.actions.clear();
        self.blocked = true;
        self.set_state(State::Done);
        close(self.c.clone(), self.id.clone(), Some(parent));
    }
}
