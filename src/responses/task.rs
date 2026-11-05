//! One task per native conversation: its requests, its calls in order, and its code-mode cells.

use super::{CAP, Error, Response, Responses};
use crate::{
    code_mode::{self, CodeMode},
    profiles::{self, Conversation, IMAGE, Profile},
    protocol::*,
};
use futures_util::{StreamExt, future::BoxFuture, stream::FuturesUnordered};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering::Relaxed},
};
use tokio::sync::{mpsc, watch};

/// A call the model made in this turn's latest response.
struct Call {
    id: CallId,
    name: String,
    input: Value,
    custom: bool,
    result: Option<(String, bool)>,
}

/// What lives only while a turn runs. Dropping it drops the request in flight and the cells'
/// futures, and stops the turn's cells.
#[derive(Default)]
struct Turn {
    stop: watch::Sender<bool>,
    request: Option<BoxFuture<'static, Result<Response, Error>>>,
    /// The latest response's calls, answered to the model in call order.
    calls: VecDeque<Call>,
    /// Calls from cells, awaiting admission and then their result.
    nested: HashMap<String, code_mode::Run>,
    /// Running exec, wait and nested `web__run` calls.
    cells: FuturesUnordered<BoxFuture<'static, (CallId, Result<String, code_mode::Error>)>>,
    turn_state: Option<String>,
    usage: Usage,
    /// The latest input a request of this turn carried.
    took: Option<EntryId>,
    settle: bool,
    /// Uploads of the videos the next request shows, which waits for them; each video is tried
    /// at most once a turn (`tried`), so a failure is shown, not retried.
    uploads: FuturesUnordered<BoxFuture<'static, (PathBuf, super::upload::Upload)>>,
    tried: Vec<PathBuf>,
}

impl Drop for Turn {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}

struct Task {
    client: Arc<Responses>,
    profile: watch::Receiver<Arc<Profile>>,
    outgoing: mpsc::Sender<Line>,
    session: SessionId,
    /// The session directory, which image paths in native data are relative to.
    dir: PathBuf,
    open: u64,
    settings: crate::protocol::ModelSettings,
    requested_speed: Speed, // The in-flight request keeps its speed when another input changes it.
    conversation: Conversation,
    code: Arc<CodeMode>,
    /// Admitted inputs not yet sent; at most 64.
    inputs: Vec<BridgeInput>,
    /// Images the results named since the last request, in the order they came, each sent there
    /// after its path. Paths only, bounded by the results naming them: `read` names one, an MCP
    /// result is at most 16 MiB; the bytes count toward the request's 64 MiB when it is built.
    images: Vec<Image>,
    turn: Option<Turn>,
    /// The conversation settled for a rebuild or a finish; it takes no further turn.
    settled: bool,
    /// Thoughts begun in this conversation, numbered as their lines leave.
    thoughts: Arc<AtomicU64>,
}

impl Responses {
    /// One task per native conversation, opened by its first message. The controller owns the
    /// record and runs every action; it updates `profile` before telling of new tools. Each
    /// request goes where the profile's endpoint says at that moment.
    pub(crate) fn start(
        self: &Arc<Self>,
        dir: PathBuf,
        profile: watch::Receiver<Arc<Profile>>,
        mut incoming: mpsc::Receiver<ToBridge>,
        outgoing: mpsc::Sender<Line>,
    ) {
        let client = self.clone();
        tokio::spawn(async move {
            let Some(ToBridge::Open {
                session,
                open,
                effort,
                speed,
                resume,
                tools,
                ..
            }) = incoming.recv().await
            else {
                return;
            };
            // The task, its turn and so its cells are gone before Closed is sent.
            let served = async {
                let current = profile.borrow().clone();
                let conversation = match resume {
                    None => Ok(Conversation::new(&current, tools)),
                    Some(Resume::Responses { items }) => Conversation::resume(&current, items),
                    Some(Resume::Claude { .. }) => Err(profiles::Error::Invalid(
                        "A Claude conversation needs a rebuild before Responses".into(),
                    )),
                };
                let (runs, cells) = mpsc::channel(64);
                let mut task = Task {
                    client,
                    profile,
                    outgoing: outgoing.clone(),
                    session: session.clone(),
                    dir: dir.clone(),
                    open,
                    settings: crate::protocol::ModelSettings { effort, speed },
                    requested_speed: speed,
                    conversation: conversation.map_err(|e| e.to_string())?,
                    code: Arc::new(CodeMode::new(dir, runs)),
                    inputs: Vec::new(),
                    images: Vec::new(),
                    turn: None,
                    settled: false,
                    thoughts: Arc::default(),
                };
                task.serve(incoming, cells).await.map_err(|e| e.to_string())
            };
            let says = FromBridge::Closed {
                error: served.await.err(),
            };
            #[cfg(all(target_os = "linux", target_env = "gnu"))]
            {
                // SAFETY: the native task has dropped its histories; trimming preserves live allocations.
                tokio::task::spawn_blocking(|| unsafe { libc::malloc_trim(0) });
            }
            let _ = outgoing
                .send(Line {
                    session,
                    open,
                    says,
                })
                .await;
        });
    }
}

impl Task {
    async fn serve(
        &mut self,
        mut incoming: mpsc::Receiver<ToBridge>,
        mut runs: mpsc::Receiver<code_mode::Run>,
    ) -> Result<(), Error> {
        loop {
            if self.turn.is_none() && !self.settled && !self.inputs.is_empty() {
                self.turn = Some(Turn::default());
            }
            let ready = self
                .turn
                .as_ref()
                .is_some_and(|t| t.request.is_none() && t.calls.is_empty() && t.uploads.is_empty());
            let (request, cells, uploads) = match &mut self.turn {
                Some(turn) => (
                    turn.request.as_mut(),
                    Some(&mut turn.cells),
                    Some(&mut turn.uploads),
                ),
                None => (None, None, None),
            };
            tokio::select! {
                // Apply queued settings before a ready request can leave.
                biased;
                message = incoming.recv() => match message {
                    None | Some(ToBridge::Close { .. }) => return Ok(()),
                    Some(message) => self.message(message).await?,
                },
                Some(response) = async { Some(request?.await) } => {
                    if let Some(turn) = &mut self.turn {
                        turn.request = None;
                    }
                    match response {
                        Ok(response) => self.response(response).await?,
                        Err(error) => {
                            let reason = matches!(error, Error::ContextOverflow(_)).then_some("prompt_too_long");
                            self.end(Some(End::Error(error.to_string())), reason).await?;
                        }
                    }
                }
                Some((call, result)) = async { cells?.next().await } => {
                    // A cell's `web__run` answers its cell; exec and wait answer the model.
                    match self.turn.as_mut().and_then(|turn| turn.nested.remove(&call.0)) {
                        Some(run) => {
                            let output = result.as_ref().map_or_else(ToString::to_string, Clone::clone);
                            self.result(call, output, result.is_err()).await?;
                            let _ = run.reply.send(result.map(Value::String));
                        }
                        None => {
                            let result = result.map_or_else(|e| (e.to_string(), true), |output| (output, false));
                            self.answer(call, result, false).await?;
                        }
                    }
                }
                Some((path, upload)) = async { uploads?.next().await } => match upload {
                    Ok((uri, expires)) => self.conversation.renew(&path, &uri, expires),
                    Err(error) => self.conversation.unsent(&path, &error.to_string()),
                },
                Some(run) = runs.recv() => match &mut self.turn {
                    Some(turn) => {
                        let body = Body::Call { call: run.call.clone(), name: run.name.clone(), input: run.input.clone() };
                        turn.nested.insert(run.call.0.clone(), run);
                        self.emit(FromBridge::Entry { body, native: None }).await?;
                    }
                    None => {
                        let _ = run.reply.send(Err(code_mode::Error::Refused("The turn ended before this call was admitted.".into())));
                    }
                },
                () = std::future::ready(()), if ready => self.request().await?,
            }
        }
    }

    async fn emit(&mut self, says: FromBridge) -> Result<(), Error> {
        let line = Line {
            session: self.session.clone(),
            open: self.open,
            says,
        };
        self.outgoing.send(line).await.map_err(|_| Error::Stopped)
    }

    /// Offers changed tools, sends the results' images, each after its path, then the waiting
    /// inputs, each its images, its video and its words, as one user message after the batch's
    /// outputs, and starts the request. On an endpoint that takes video, the videos the request
    /// will show that need an upload are uploaded first, and the request starts after them.
    async fn request(&mut self) -> Result<(), Error> {
        self.requested_speed = self.settings.speed;
        let profile = self.profile.borrow().clone();
        self.conversation.offer(&profile);
        let Some(turn) = &mut self.turn else {
            return Ok(());
        };
        let video = profile.endpoint.video();
        if video {
            let shown = self.inputs.iter().filter_map(|i| i.video.clone()).collect();
            let mut stale = self.conversation.stale(shown, now());
            stale.retain(|path| !turn.tried.contains(path));
            for path in stale {
                let (client, endpoint) = (self.client.clone(), profile.endpoint.clone());
                turn.tried.push(path.clone());
                turn.uploads.push(Box::pin(async move {
                    let upload = client.upload(&endpoint, &path).await;
                    (path, upload)
                }));
            }
            if !turn.uploads.is_empty() {
                return Ok(());
            }
        }
        let (mut content, mut size) = (Vec::new(), 0);
        for image in self.images.drain(..) {
            let path = image.path.to_string_lossy().into_owned();
            content.push(json!({"type": "input_text", "text": path}));
            content.push(part(&image, &self.dir, &mut size).await?);
        }
        for input in &self.inputs {
            for image in &input.images {
                content.push(part(image, &self.dir, &mut size).await?);
            }
            let path = input.video.as_ref().filter(|_| video);
            content.extend(
                path.map(|p| json!({"type": "razor_video", "path": p, "keep": input.keep})),
            );
            // A rebuild's video alone has no words of its own.
            if path.is_none() || !input.text.is_empty() {
                content.push(json!({"type": "input_text", "text": input.text}));
            }
        }
        if !content.is_empty() {
            self.conversation
                .send(json!({"type": "message", "role": "user", "content": content}));
        }
        turn.took = (self.inputs.drain(..).next_back())
            .map(|i| i.entry)
            .or(turn.took);
        let endpoint = profile.endpoint.clone();
        let body = (self.conversation)
            .body(&profile, self.settings, &self.session, &self.dir)
            .await;
        if body.len() > CAP {
            return Err(Error::Size);
        }
        let (client, session, state) = (
            self.client.clone(),
            self.session.clone(),
            turn.turn_state.clone(),
        );
        let (outgoing, open, lite) = (self.outgoing.clone(), self.open, profile.code_mode);
        let thoughts = self.thoughts.clone();
        turn.request = Some(Box::pin(async move {
            let say = |mut says| {
                match &mut says {
                    FromBridge::Thought { n, start, .. } => {
                        *n = thoughts.fetch_add(u64::from(*start), Relaxed) + u64::from(*start)
                    }
                    FromBridge::Void { n } => *n = thoughts.load(Relaxed),
                    _ => {}
                }
                // A full display channel drops a live line; it never holds up the response.
                let _ = outgoing.try_send(Line {
                    session: session.clone(),
                    open,
                    says,
                });
            };
            (client.post(&endpoint, body, &session, lite, state, say)).await
        }));
        Ok(())
    }

    /// Records a completed response, then its text, reasoning and calls; the calls wait for
    /// admission. A response without calls ends the turn unless it says `end_turn: false`.
    async fn response(&mut self, mut response: Response) -> Result<(), Error> {
        let served = match response.tier.as_deref() {
            Some("default" | "standard") => Speed::Standard,
            Some("fast" | "priority") => Speed::Fast,
            _ => self.requested_speed,
        };
        let requested = self.requested_speed;
        if served < requested {
            let text =
                format!("{requested:?} unavailable; provider used {served:?} for this request.");
            let body = Body::Notice { text };
            self.emit(FromBridge::Entry { body, native: None }).await?;
        }
        if !response.windows.is_empty() {
            let windows = response.windows;
            self.emit(FromBridge::Usage { windows }).await?;
        }
        let Some(turn) = &mut self.turn else {
            return Ok(());
        };
        let usage = response.usage;
        turn.turn_state = response.turn_state;
        let total = &mut turn.usage;
        total.input = total.input.saturating_add(usage.input);
        total.output = total.output.saturating_add(usage.output);
        total.cache_read = total.cache_read.saturating_add(usage.cache_read);
        total.cache_write = total.cache_write.saturating_add(usage.cache_write);
        total.requests = total.requests.saturating_add(usage.requests);
        total.waited = total.waited.saturating_add(usage.waited);
        let (entries, calls) = read(&response.items);
        turn.calls.extend(calls);
        let done = turn.calls.is_empty() && response.end_turn;
        let tokens = usage
            .input
            .saturating_add(usage.cache_read)
            .saturating_add(usage.cache_write);
        let read = turn.took;
        self.emit(FromBridge::Context {
            tokens,
            window: None,
            read,
        })
        .await?;
        let additions = self.conversation.response(&mut response.items);
        let native = Some(Native::Responses {
            additions,
            items: response.items,
        });
        self.emit(FromBridge::Entry {
            body: Body::Response { usage },
            native,
        })
        .await?;
        for body in entries {
            self.emit(FromBridge::Entry { body, native: None }).await?;
        }
        if done {
            self.end(Some(End::Done), None).await?;
        }
        Ok(())
    }

    async fn message(&mut self, message: ToBridge) -> Result<(), Error> {
        match message {
            ToBridge::Input { inputs, .. } => {
                if self.inputs.len() + inputs.len() > 64 {
                    return Err(Error::Size);
                }
                self.inputs.extend(inputs);
            }
            ToBridge::Effort { effort, speed, .. } => {
                self.settings = crate::protocol::ModelSettings { effort, speed }
            }
            ToBridge::Interrupt { .. } => return self.interrupt().await,
            ToBridge::Admitted { call, input, .. } => return self.admitted(call, input).await,
            ToBridge::Result {
                call,
                output,
                error,
                settle,
                ..
            } => {
                // A script's `finish` or `handover` settles the batch its `exec` is in.
                let nested = self.turn.as_mut().and_then(|turn| {
                    turn.settle |= settle;
                    turn.nested.remove(&call.0)
                });
                let Some(run) = nested else {
                    return self.answer(call, (output, error), settle).await;
                };
                self.images.extend(Image::named(&output));
                self.result(call, output.clone(), error).await?;
                let value = serde_json::from_str::<Value>(&output);
                let ran = value.as_ref().is_ok_and(|v| v["end"].is_object());
                // A nested tool returns an object or a string, as exec's description says.
                let value = if error && !ran {
                    Err(code_mode::Error::Refused(output))
                } else {
                    Ok(value.unwrap_or(Value::String(output)))
                };
                let _ = run.reply.send(value);
            }
            // A task opens once; the next request compares the current profile's tools itself.
            ToBridge::Open { .. } | ToBridge::Add { .. } | ToBridge::Close { .. } => {}
        }
        Ok(())
    }

    /// The call is durable: exec, wait and a cell's `web__run` run here in code mode; every other
    /// call runs in the controller, a cell's other calls included.
    async fn admitted(&mut self, call: CallId, changed: Option<Value>) -> Result<(), Error> {
        let Some(turn) = &mut self.turn else {
            return Ok(());
        };
        let (tool, input, model) = match (
            turn.nested.get(&call.0),
            turn.calls.iter().find(|c| c.id == call),
        ) {
            (Some(run), _) => (run.name.clone(), run.input.clone(), false),
            (None, Some(c)) => (c.name.clone(), c.input.clone(), true),
            (None, None) => return Ok(()),
        };
        let mut input = changed.unwrap_or(input);
        // An edit of the conversation's latest images reads them here, where the conversation is.
        if matches!(tool.as_str(), "imagegen" | "image_gen__imagegen") {
            let parsed = match &input {
                Value::String(raw) => serde_json::from_str::<Value>(raw).ok(),
                value => Some(value.clone()),
            };
            if let Some(mut fields) = parsed
                && let Some(count) = fields["num_last_images_to_include"].as_u64()
                && let Some(object) = fields.as_object_mut()
            {
                let images = (self.conversation.images(count.min(5) as usize, &self.dir)).await;
                object.insert("images".into(), images.into());
                input = fields;
            }
        }
        if !model && tool == "web__run" {
            // The profile names the conversation's model, a kept switch's after it.
            let profiles::Endpoint {
                url: base,
                id: model,
                ..
            } = self.profile.borrow().endpoint.clone();
            let (client, session) = (self.client.clone(), self.session.clone());
            let recent = self.conversation.recent();
            turn.cells.push(Box::pin(async move {
                let found = client.search(&base, &session, &model, recent, input).await;
                (
                    call,
                    found.map_err(|e| code_mode::Error::Refused(e.to_string())),
                )
            }));
            return Ok(());
        }
        let profile = self.profile.borrow().clone();
        let cell = matches!(
            tool.strip_prefix("functions.").unwrap_or(&tool),
            "exec" | "wait"
        );
        if !(model && profile.code_mode && cell) {
            return self.emit(FromBridge::Run { call, tool, input }).await;
        }
        let (code, stop) = (self.code.clone(), turn.stop.subscribe());
        turn.cells.push(Box::pin(async move {
            let result = match profile.input(&tool, input) {
                Ok((action, Value::String(source))) if action == "exec" => {
                    code.exec(call.clone(), source, profile.nested.clone().into(), stop)
                        .await
                }
                Ok((_, input)) => code.wait(input).await,
                Err(error) => Err(code_mode::Error::Refused(error.to_string())),
            };
            (call, result)
        }));
        Ok(())
    }

    /// Takes one model call's result, then answers every leading call that has one, in call
    /// order. With all answered, a settling batch ends the conversation's turns.
    async fn answer(
        &mut self,
        call: CallId,
        result: (String, bool),
        settle: bool,
    ) -> Result<(), Error> {
        let Some(turn) = &mut self.turn else {
            return Ok(());
        };
        let Some(waiting) = turn
            .calls
            .iter_mut()
            .find(|c| c.id == call && c.result.is_none())
        else {
            return Ok(());
        };
        waiting.result = Some(result);
        turn.settle |= settle;
        let mut answered = Vec::new();
        while turn.calls.front().is_some_and(|c| c.result.is_some()) {
            answered.extend(turn.calls.pop_front());
        }
        let settled = turn.calls.is_empty() && turn.settle;
        for mut call in answered {
            let (output, error) = call.result.take().unwrap_or_default();
            self.conversation.send(output_item(&call, &output));
            self.images.extend(Image::named(&output));
            self.result(call.id, output, error).await?;
        }
        if settled {
            self.end(None, None).await?;
        }
        Ok(())
    }

    /// Escape: the request in flight and the cells stop with the turn. Each unanswered call is
    /// answered in the native history, with its own result when it has one and otherwise as
    /// stopped (the controller records it unknown), so the next input continues the conversation.
    async fn interrupt(&mut self) -> Result<(), Error> {
        let calls = self.turn.as_mut().map(|t| std::mem::take(&mut t.calls));
        for mut call in calls.into_iter().flatten() {
            let (known, result) = (call.result.is_some(), call.result.take());
            let (output, error) = result.unwrap_or((crate::record::STOPPED.into(), true));
            self.conversation.send(output_item(&call, &output));
            if known {
                self.result(call.id, output, error).await?;
            }
        }
        self.end(Some(End::Stopped), None).await
    }

    /// Records a call's result as an entry.
    async fn result(&mut self, call: CallId, output: String, error: bool) -> Result<(), Error> {
        let outcome = if error {
            Outcome::Error
        } else {
            Outcome::Success
        };
        let inputs = Vec::new(); // The controller names the news a waiting call took.
        let body = Body::Result {
            call,
            outcome,
            output,
            inputs,
        };
        self.emit(FromBridge::Entry { body, native: None }).await
    }

    /// Ends the turn and stops its cells; `None` settles the conversation instead.
    async fn end(&mut self, end: Option<End>, reason: Option<&str>) -> Result<(), Error> {
        let Some(turn) = self.turn.take() else {
            return Ok(());
        };
        let (usage, took) = (turn.usage, turn.took);
        drop(turn);
        if matches!(end, Some(End::Done)) {
            self.conversation.spend(&self.profile.borrow());
        }
        let (success, reason, message) = match end {
            None => {
                self.settled = true;
                return self.emit(FromBridge::Settled { usage }).await;
            }
            Some(End::Done) => (true, None, String::new()),
            Some(End::Stopped) => (
                false,
                Some("stopped".into()),
                "The turn was stopped.".into(),
            ),
            Some(End::Error(message)) => (false, Some(reason.unwrap_or("error").into()), message),
        };
        self.emit(FromBridge::TurnEnd {
            success,
            reason,
            message,
            usage,
            window: 0,
            took,
        })
        .await
    }
}

/// A call's output as the model reads it back.
fn output_item(call: &Call, output: &str) -> Value {
    let kind = match call.custom {
        true => "custom_tool_call_output",
        false => "function_call_output",
    };
    json!({"type": kind, "call_id": call.id.0, "output": output})
}

/// The record entries a response's items carry, in order, and its calls: a message's texts, a
/// reasoning summary, a chat message's reasoning, text and calls, and each function or custom call.
fn read(items: &[Value]) -> (Vec<Body>, Vec<Call>) {
    let (mut entries, mut calls, mut found) = (Vec::new(), Vec::new(), Vec::new());
    for item in items {
        match item["type"].as_str() {
            Some("message") => {
                let texts = item["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c["text"].as_str());
                entries.extend(
                    texts
                        .filter(|t| !t.is_empty())
                        .map(|text| Body::Text { text: text.into() }),
                );
            }
            Some("reasoning") => {
                let summary = item["summary"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|s| s["text"].as_str());
                let text = summary.collect::<Vec<_>>().join("\n");
                if !text.is_empty() {
                    entries.push(Body::Thinking { text });
                }
            }
            // A chat message: its reasoning, its text, then its calls.
            None if item["role"] == "assistant" => {
                let reasoning = item["reasoning_content"].as_str();
                let reasoning = reasoning.or_else(|| item["reasoning"].as_str());
                let thinking = reasoning.filter(|t| !t.is_empty());
                entries.extend(thinking.map(|text| Body::Thinking { text: text.into() }));
                let text = item["content"].as_str().filter(|t| !t.is_empty());
                entries.extend(text.map(|text| Body::Text { text: text.into() }));
                let tool_calls = item["tool_calls"].as_array().into_iter().flatten();
                found.extend(tool_calls.map(|c| {
                    let function = &c["function"];
                    (&c["id"], &function["name"], &function["arguments"], false)
                }));
            }
            Some(kind @ ("function_call" | "custom_tool_call")) => {
                let custom = kind == "custom_tool_call";
                let input = &item[if custom { "input" } else { "arguments" }];
                found.push((&item["call_id"], &item["name"], input, custom));
            }
            _ => {}
        }
        for (id, name, input, custom) in found.drain(..) {
            let id = CallId(id.as_str().unwrap_or_default().into());
            let name = name.as_str().unwrap_or_default();
            entries.push(Body::Call {
                call: id.clone(),
                name: name.into(),
                input: input
                    .as_str()
                    .filter(|_| !custom)
                    .and_then(|raw| serde_json::from_str(raw).ok())
                    .unwrap_or_else(|| input.clone()),
            });
            calls.push(Call {
                id,
                name: name.into(),
                input: input.clone(),
                custom,
                result: None,
            });
        }
    }
    (entries, calls)
}

/// An image file as a `razor_image` part naming it relative to the session directory, which
/// `Conversation::body` reads; refused past `IMAGE` or the request cap that `size` counts its
/// encoding toward, before the request is built. A file already gone counts nothing.
async fn part(image: &Image, dir: &Path, size: &mut usize) -> Result<Value, Error> {
    let bytes = tokio::fs::metadata(&image.path)
        .await
        .map_or(0, |m| m.len());
    *size += (bytes as usize).div_ceil(3) * 4;
    if bytes > IMAGE || *size > CAP {
        return Err(Error::Size);
    }
    let path = image.path.strip_prefix(dir).unwrap_or(&image.path);
    Ok(json!({"type": "razor_image", "path": path, "media_type": image.media_type}))
}
