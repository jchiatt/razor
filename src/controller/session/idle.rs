//! Idle handover and close, owned by the session task and its ordinary tick.

use super::*;

impl Session {
    /// Checks child grace, configured idle closes and the existing idle handover boundary.
    pub(in crate::controller) async fn tick(&mut self) -> Result<(), Error> {
        self.settings.read();
        self.record.mark(self.due()).await?;
        if self.state == State::Working || self.blocked {
            return Ok(());
        }
        if let Some(input) = self.turn.tick(&mut self.settings) {
            self.admit(input).await?;
            self.turn.nudge();
        }
        if self.state == State::Working || self.record.summary().closed {
            return Ok(());
        }
        if self.close_at().is_some_and(|due| now() >= due) && !self.pending(&[]) {
            return self.idle_close().await;
        }
        // A working child does not hold the parent's own handover.
        if self.handover_at().is_some_and(|due| now() >= due) && !self.unfinished(&[]) {
            self.handover(Trigger::Idle).await?;
        }
        Ok(())
    }

    /// Idle maintenance remains due: the task stays loaded, and its marker across a restart.
    pub(super) fn due(&self) -> bool {
        !self.blocked
            && !self.record.summary().closed
            && (self.close_at().is_some() || self.handover_at().is_some())
    }

    /// The remaining handover deadline; small and already rebuilt contexts have none, nor does
    /// one whose idle request is already admitted, until external input.
    pub(super) fn handover_at(&self) -> Option<Millis> {
        let s = self.record.summary();
        let at = s.idle_since?;
        let sessions = &self.settings.config.sessions;
        (sessions.handover_minutes > 0
            && !s.idle_handover
            && self.context.is_some_and(|n| n >= sessions.handover_tokens)
            && s.last_rebuild.is_none_or(|(_, rebuilt)| rebuilt < at))
        .then_some(at.saturating_add(sessions.handover_minutes.saturating_mul(60_000)))
    }

    /// The remaining close deadline; children and resident homes never have one.
    pub(super) fn close_at(&self) -> Option<Millis> {
        let s = self.record.summary();
        let at = s.idle_since?;
        let config = &self.settings.config;
        let minutes = config.sessions.close_minutes;
        (minutes > 0 && s.child.is_none() && !config.residents.contains_key(&self.id))
            .then_some(at.saturating_add(minutes.saturating_mul(60_000)))
    }

    /// Closes only an idle conversation: its handover note reports to its resident without a turn.
    async fn idle_close(&mut self) -> Result<(), Error> {
        let s = self.record.summary();
        let resident = s.thread.as_ref().map(|t| t.resident.clone());
        let mut note = s.last_reply.clone();
        if let Some((id, at)) = s.last_rebuild
            && s.person_input_at.is_none_or(|input| input <= at)
            && let Body::Rebuild { note: saved, .. } = self.record.entry(id).await?.body
            && !saved.is_empty()
        {
            note = saved;
        }
        self.close_out(&[]).await?;
        self.conversation = None;
        let (window, threshold) = limits(&self.model, &self.settings.config, self.reported);
        self.append(
            Body::TurnEnd {
                inputs: Vec::new(),
                end: End::Done,
                usage: Usage::default(),
                context: self.context,
                window,
                threshold: Some(threshold),
                closed: true,
                close_reason: Some(CloseReason::Idle),
                report: None,
            },
            None,
        )
        .await?;
        self.set_state(State::Done);
        if let Some(resident) = resident {
            let c = self.c.clone();
            let name = self.id.to_string();
            let input = Input::new(Origin::Program { name }, note);
            tokio::spawn(async move {
                if let Err(e) = c.admit(&resident, input).await {
                    eprintln!("razor: idle conversation report: {e}");
                }
            });
        }
        Ok(())
    }
}
