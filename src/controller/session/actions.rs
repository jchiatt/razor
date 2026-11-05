//! The session's actions and jobs: each admitted call runs under the FIFO lock, and every job
//! event is recorded before a Result can overtake it.

use super::*;
use serde_json::{Value, json};
use std::task::Poll;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// The handover action's result and recall's for a thinking entry, which the model reads
/// (RULES.md 16). Neither names the model's thinking: Claude refuses a request that asks for it
/// back as reasoning extraction.
const HANDOVER: &str = include_str!("handover.md");

impl Session {
    /// Mirrors a native message into the record. A Call is admitted once the held person inputs
    /// join the turn; a Result is recorded only for an open call, after the job events it raced,
    /// and one that showed its job's end drops that job's notice.
    pub(super) async fn entry(&mut self, body: Body, native: Option<Native>) -> Result<(), Error> {
        let (mut admitted, mut shown, mut body) = (None, None, body);
        let error = matches!(&body, Body::Result { outcome, .. } if *outcome == Outcome::Error);
        // A failed cell's Result is an error that still showed the cell, so it answers its news.
        let error =
            error && !matches!(&body, Body::Result { output, .. } if output.starts_with(FAILED));
        match &mut body {
            Body::Call { call, name, input } => {
                let cwd = &self.record.summary().cwd;
                let event = Before::Call { call, name, input };
                let ran = self.plugins.before(&self.id, cwd, event).await;
                let changed = ran.map(|changed| changed.then(|| input.clone()));
                admitted = Some((call.clone(), changed));
            }
            Body::Result { call, inputs, .. } => {
                let Some(took) = self.took(call, error) else {
                    return Ok(());
                };
                *inputs = took;
                self.drain().await?;
                let at = self.shown.iter().position(|(shown, _)| shown == call);
                shown = at.map(|at| self.shown.swap_remove(at).1);
            }
            _ => {}
        }
        self.append(body, native).await?;
        if let Some(job) = shown {
            self.notices.retain(|(queued, _)| *queued != job);
            self.stopped.remove(&job);
            self.shell.ack(job).await;
        }
        // A refused call is answered with its sentence, which the model path records as its
        // Result; its action never runs.
        if let Some((call, admitted)) = admitted {
            self.boundary().await?;
            self.say(|session| match admitted {
                Ok(input) => ToBridge::Admitted {
                    session,
                    call,
                    input,
                },
                Err(e) => ToBridge::Result {
                    session,
                    call,
                    output: e.to_string(),
                    error: true,
                    settle: false,
                    images: Vec::new(),
                },
            })
            .await?;
        }
        Ok(())
    }

    /// Runs an admitted call's action. Its first poll is here, in call order, so the FIFO lock
    /// queues actions in the order the model made the calls, never in spawned tasks.
    pub(super) async fn run(
        &mut self,
        call: CallId,
        name: String,
        raw: Value,
    ) -> Result<(), Error> {
        let s = self.record.summary();
        // After Escape, nothing more runs: the turn's end records its open calls unknown.
        let stopped = self.conversation.as_ref().is_some_and(|c| c.interrupted);
        let Some(&(_, at)) = s.unresolved.iter().find(|(id, _)| *id == call && !stopped) else {
            return Ok(());
        };
        // A refused input is answered in call order, like any other result.
        let (action, input, mut known) = match self.profile.input(&name, raw.clone()) {
            Ok((action, input)) => (action, input, None),
            Err(e) => (String::new(), Value::Null, Some((e.to_string(), true))),
        };
        // Another session's entry and a search run as children's actions.
        if action == "recall" && input["session"].is_null() && input["query"].is_null() {
            let entry = EntryId(input["id"].as_u64().unwrap_or(0));
            known = Some(match self.record.entry(entry).await.map(|e| e.body) {
                Ok(Body::Thinking { .. }) => (
                    record::REASONING.replace("{id}", &entry.0.to_string()),
                    true,
                ),
                _ => said(self.record.get(entry).await),
            });
        } else if action == "handover" {
            known = Some((HANDOVER.into(), false));
        }
        let cited = match input["ref"]
            .as_u64()
            .map(EntryId)
            .or(s.asks.last().copied())
        {
            Some(id) if action == "memory" => crate::memory::cited(&self.record, id).await,
            _ => None,
        };
        if action == "handover"
            && let Some(c) = &mut self.conversation
        {
            c.handover = Some(input["note"].as_str().unwrap_or_default().to_owned());
        }
        let actions = self.tools.actions();
        let read_only = actions.iter().any(|a| a.name == action && a.read_only);
        let run = Run {
            c: self.c.clone(),
            session: self.id.clone(),
            at,
            call,
            name,
            raw,
            action,
            input,
            known,
            cited,
            cwd: s.cwd.clone(),
            child: s.child.is_some(),
            tools: self.tools.clone(),
            profile: self.profile.clone(),
            revision: self.mcp_revision,
        };
        let order = self.order.clone();
        let mut future: BoxFuture<'static, Ran> = Box::pin(async move {
            let _held = match read_only {
                true => Ok(order.read_owned().await),
                false => Err(order.write_owned().await),
            };
            run.act().await
        });
        match futures_util::poll!(&mut future) {
            Poll::Ready(ran) => self.ran(ran).await,
            Poll::Pending => {
                self.actions.push(future);
                Ok(())
            }
        }
    }

    /// An action finished: the job it leaves running leaves the foreground, then held inputs
    /// join before its Result, after any job events it raced.
    pub(super) async fn ran(&mut self, ran: Ran) -> Result<(), Error> {
        let Ran {
            call,
            output,
            error,
            settle,
            close,
            job,
            refresh,
        } = ran;
        self.drain().await?;
        if refresh {
            self.refresh().await?;
        }
        if let (Some(c), Some(close)) = (&mut self.conversation, close) {
            c.closing = Some(close);
        }
        // Marked shown first, so the end this result shows never joins as a notice beside it.
        if let Some((job, shown)) = job {
            if let Some(Some(notice)) = self.foreground.remove(&job) {
                self.notices.push_back((job, notice));
            }
            if shown {
                self.shown.push((call.clone(), job));
            }
        }
        self.boundary().await?;
        let settle = settle || matches!(&self.conversation, Some(c) if c.handover.is_some());
        // The model hears of changed MCP availability before this result, when there is room.
        if !self.full()
            && std::mem::take(&mut self.news)
            && let Some(text) = self.tools.mcp_news(&mut self.mcp_key)
        {
            let input = Input::new(Origin::Razor, text);
            let entry = self.append(Body::Input(input.clone()), None).await?;
            self.deliver(vec![(entry, input)]).await?;
        }
        let result = |session| ToBridge::Result {
            session,
            call,
            output,
            error,
            settle,
            images: Vec::new(),
        };
        self.say(result).await
    }

    /// Records every job event already sent, so a Result never overtakes its job's end.
    pub(super) async fn drain(&mut self) -> Result<(), Error> {
        while let Ok(event) = self.events.try_recv() {
            self.job_event(event, true).await?;
        }
        Ok(())
    }

    /// Records a job event. A foreground job's end notice waits for its call to return; any
    /// other is queued, and with `wake` it starts a turn when the session waits for one. A
    /// notice that wakes the session or a waiting call is admitted before the end is recorded,
    /// so no restart between them leaves an idle session's last job end unheard.
    pub(super) async fn job_event(&mut self, event: JobEvent, wake: bool) -> Result<(), Error> {
        let job = match event {
            JobEvent::Started { job, .. } => {
                self.append(Body::Job(event), None).await?;
                self.foreground.insert(job, None);
                return Ok(());
            }
            JobEvent::Ended { job, .. } => job,
        };
        self.tails.remove(&job);
        let mut opens = false;
        if let Some(notice) = self.notice(job, &event).await? {
            match self.foreground.get_mut(&job) {
                Some(slot) => *slot = Some(notice),
                None => {
                    self.notices.push_back((job, notice));
                    opens = wake && self.state != State::Working;
                    if opens && !self.blocked && !self.record.summary().closed {
                        self.flush_notices(false).await?;
                    } else if !self.waiters.is_empty() {
                        self.flush_notices(true).await?;
                    }
                }
            }
        }
        self.append(Body::Job(event), None).await?;
        if opens {
            Box::pin(self.start_turn()).await?;
        }
        Ok(())
    }

    /// The end notice of `job`, rendered from its recorded call before its end is recorded; for
    /// a job the person's stop killed, with how long its call had run. It names its job as its
    /// request, so a notice recorded before a restart that lost the end is not made again.
    async fn notice(&mut self, job: JobId, ended: &JobEvent) -> Result<Option<Input>, Error> {
        let (summary, spool) = (self.record.summary(), format!("job-{}", job.0));
        let started = summary.jobs.iter().find_map(|event| match event {
            JobEvent::Started { job: id, call, .. } if *id == job => Some(*call),
            _ => None,
        });
        let Some(at) = started.filter(|_| summary.repeat(Some(&spool)).is_none()) else {
            return Ok(None);
        };
        let entry = self.record.entry(at).await?;
        let Body::Call { call, name, input } = entry.body else {
            return Ok(None);
        };
        let path = self.record.dir().join(&spool);
        let ran = (self.stopped.contains(&job)).then(|| now().saturating_sub(entry.at));
        let output = json!({"ended": ended, "path": path, "call_id": call.0, "stopped_ms": ran});
        let text = self.profile.notice(&name, &input, output);
        Ok(Some(Input {
            request: Some(spool),
            ..Input::new(Origin::Razor, text)
        }))
    }

    /// Admits queued end notices while there is room, each releasing its job's shell handle,
    /// except one whose end an unmirrored Result shows: mirroring it drops the notice, and one
    /// never mirrored stays queued for the next turn. A job the person's stop killed starts no
    /// turn: unless a turn is `running`, its notice waits for another input to admit with.
    pub(super) async fn flush_notices(&mut self, running: bool) -> Result<(), Error> {
        while !self.full()
            && let Some(at) = self.notices.iter().position(|(job, _)| {
                let held = !running && self.pending.is_empty() && self.stopped.contains(job);
                !held && !self.shown.iter().any(|(_, shown)| shown == job)
            })
            && let Some((job, notice)) = self.notices.remove(at)
        {
            let entry = self.append(Body::Input(notice.clone()), None).await?;
            self.shell.ack(job).await;
            self.stopped.remove(&job);
            if !self.wake(entry, &notice) {
                self.pending.push_back((entry, notice));
            }
        }
        Ok(())
    }

    /// Shows each running job's latest output, skipping a job whose size has not changed.
    pub(super) async fn tails(&mut self) {
        for event in self.record.summary().jobs.clone() {
            let JobEvent::Started { job, call, .. } = event else {
                continue;
            };
            let path = self.record.dir().join(format!("job-{}", job.0));
            if let Ok(Some((size, text))) = tail(path, self.tails.get(&job).copied()).await {
                self.tails.insert(job, size);
                self.publish(&Live::Tail {
                    job,
                    call,
                    text,
                    at: crate::protocol::now(),
                });
            }
        }
    }
}

/// An admitted call's action with everything it runs with, apart from the session's task.
struct Run {
    c: Controller,
    session: SessionId,
    at: EntryId,
    call: CallId,
    /// The call as the model made it, for its profile's rendering of the result.
    name: String,
    raw: Value,
    action: String,
    input: Value,
    /// A result known before the action runs: a recall, a handover, or a refused input.
    known: Option<(String, bool)>,
    /// The entry a memory write cites.
    cited: Option<crate::memory::Cited>,
    cwd: std::path::PathBuf,
    /// Whether the session is a child, which writes no memory or tasks.
    child: bool,
    tools: Arc<Tools>,
    profile: Arc<Profile>,
    revision: u64,
}

impl Run {
    /// Runs the action.
    async fn act(mut self) -> Ran {
        // An MCP operation since this call was admitted applies before it runs.
        if self.revision != self.c.inner.mcp_revision.load(Ordering::Acquire) {
            let reconciled = self.c.ask(&self.session, |s| {
                Box::pin(async move {
                    s.reconcile();
                    s.refresh().await
                })
            });
            if let Err(e) = reconciled.await {
                self.known = Some((e.to_string(), true));
            }
        }
        let (c, session, at, cwd) = (&self.c, &self.session, self.at, &self.cwd);
        let dir = c.dir(session);
        let (mut close, mut job) = (None, None);
        let (output, error) = match (self.known, self.action.as_str()) {
            (Some(known), _) => known,
            (None, tool @ ("memory" | "task")) => {
                let (child, cited) = (self.child, self.cited);
                let caller = crate::memory::Caller {
                    session,
                    at,
                    cwd,
                    child,
                    cited,
                };
                said(c.inner.memory.run(c, tool, caller, self.input).await)
            }
            (None, "deliver") => said(
                c.inner
                    .deliveries
                    .deliver(c, session, at, &dir, cwd, self.input)
                    .await,
            ),
            // Codex's image generation on the sign-in, in its place in the session's call order.
            (None, "imagegen") => {
                let saved = dir.join(format!("images/gen-{}.png", at.0));
                let (responses, base) = (&c.inner.responses, &self.profile.endpoint.url);
                let turn = format!("{session}:{}", at.0);
                let made = responses
                    .image(base, session, &turn, self.input, &saved)
                    .await;
                said(made.map(|size| self.profile.generated(&saved, size)))
            }
            (None, action @ ("agent" | "message" | "status" | "recall")) => {
                let children = &c.inner.children;
                let ran = children.run(c, session, at, action, self.input).await;
                said(ran.map(|(value, finish)| {
                    close = finish;
                    value
                }))
            }
            (None, action) => match self.tools.run(at, action, self.input).await {
                Ok(output) => {
                    let ended = !output["end"].is_null();
                    job = output["job"].as_u64().map(|n| (JobId(n as u32), ended));
                    match self.profile.result(&self.name, &self.raw, output, false) {
                        (Value::String(text), error) => (text, error),
                        (value, error) => (value.to_string(), error),
                    }
                }
                Err(e) => (e.to_string(), true),
            },
        };
        Ran {
            call: self.call,
            output,
            error,
            settle: self.action == "handover" || close.is_some(),
            close,
            job,
            refresh: self.action == "mcp_search",
        }
    }
}

/// The last five lines of a file, at most 1 KiB, with its size; `None` while its size is `seen`.
async fn tail(
    path: std::path::PathBuf,
    seen: Option<u64>,
) -> std::io::Result<Option<(u64, String)>> {
    let mut file = tokio::fs::File::open(path).await?;
    let size = file.metadata().await?.len();
    if seen == Some(size) {
        return Ok(None);
    }
    let from = size.saturating_sub(1024);
    file.seek(std::io::SeekFrom::Start(from)).await?;
    let mut bytes = Vec::new();
    file.take(size - from).read_to_end(&mut bytes).await?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<_> = text.lines().rev().take(5).collect();
    lines.reverse();
    let text = lines.join("\n");
    let cut = text.ceil_char_boundary(text.len().saturating_sub(1024));
    Ok(Some((size, text[cut..].to_string())))
}

/// An action's output as the model reads it, and whether it failed.
fn said(result: Result<impl ToString, impl ToString>) -> (String, bool) {
    match result {
        Ok(output) => (output.to_string(), false),
        Err(error) => (error.to_string(), true),
    }
}
