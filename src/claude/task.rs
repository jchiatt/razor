//! Native calls stay with the SDK; only exec/wait and their nested calls run here.

use super::{Bridge, Error, FromBridge, Line, ToBridge, shared::Output};
use crate::{
    code_mode::{self, CodeMode},
    profiles::Profile,
    protocol::*,
};
use futures_util::{FutureExt, StreamExt, future::BoxFuture, stream::FuturesUnordered};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use tokio::sync::{mpsc, watch};

type Local = BoxFuture<'static, (CallId, Result<String, code_mode::Error>)>;
struct Task {
    bridge: Bridge,
    session: SessionId,
    open: u64,
    profile: watch::Receiver<Arc<Profile>>,
    outgoing: mpsc::Sender<Output>,
    code: Arc<CodeMode>,
    stop: watch::Sender<bool>,
    locals: FuturesUnordered<Local>,
    nested: BTreeMap<String, code_mode::Run>,
    /// Images the nested results named, sent with the next exec or wait result; paths only.
    images: Vec<Image>,
    active: bool,
    settle: bool,
}

pub(super) async fn serve(
    bridge: Bridge,
    stamp: (SessionId, u64),
    dir: PathBuf,
    profile: watch::Receiver<Arc<Profile>>,
    mut incoming: mpsc::Receiver<ToBridge>,
    mut lines: mpsc::Receiver<Output>,
    outgoing: mpsc::Sender<Output>,
) {
    let (runs, mut receiver) = mpsc::channel(64);
    let mut task = Task {
        bridge,
        session: stamp.0,
        open: stamp.1,
        profile,
        outgoing,
        code: Arc::new(CodeMode::new(dir, runs)),
        stop: watch::channel(false).0,
        locals: FuturesUnordered::new(),
        nested: BTreeMap::new(),
        images: Vec::new(),
        active: false,
        settle: false,
    };
    let result = async {
        loop {
            tokio::select! {
                message = incoming.recv() => {
                    let Some(message) = message else { break; };
                    if !task.message(message).await? { break; }
                },
                line = lines.recv() => {
                    let Some(line) = line else { break; };
                    if !task.line(line).await? { break; }
                },
                Some((call, result)) = task.locals.next(), if !task.locals.is_empty() => {
                    let (output, error) = match result { Ok(output) => (output, false), Err(error) => (error.to_string(), true) };
                    let images = std::mem::take(&mut task.images);
                    task.bridge.send(&ToBridge::Result { session: task.session.clone(), call, output, error, settle: task.settle, images }).await?;
                },
                Some(run) = receiver.recv() => {
                    if task.active && !*task.stop.borrow() {
                        task.emit(FromBridge::Entry { body: Body::Call { call: run.call.clone(), name: run.name.clone(), input: run.input.clone() }, native: None }).await?;
                        task.nested.insert(run.call.0.clone(), run);
                    } else {
                        let _ = run.reply.send(Err(code_mode::Error::Refused("The turn ended before this call was admitted.".into())));
                    }
                },
            }
        }
        Ok::<_, Error>(())
    }.await;
    task.cancel();
    if let Err(error) = result {
        let _ = task
            .emit(FromBridge::Closed {
                error: Some(error.to_string()),
            })
            .await;
    }
}

impl Task {
    async fn emit(&mut self, says: FromBridge) -> Result<(), Error> {
        self.outgoing
            .send((
                Line {
                    session: self.session.clone(),
                    open: self.open,
                    says,
                },
                None,
            ))
            .await
            .map_err(|_| Error::Exited)
    }

    fn cancel(&mut self) {
        self.stop.send_replace(true);
        self.locals.clear();
        self.nested.clear();
        self.images.clear();
        self.active = false;
    }

    /// Opens a turn: its cells may run and admit calls until it ends.
    fn start(&mut self) {
        self.active = true;
        self.stop = watch::channel(false).0;
        self.settle = false;
    }

    async fn message(&mut self, message: ToBridge) -> Result<bool, Error> {
        match message {
            ToBridge::Admitted {
                ref call,
                ref input,
                ..
            } => {
                if let Some(run) = self.nested.get(&call.0) {
                    self.emit(FromBridge::Run {
                        call: call.clone(),
                        tool: run.name.clone(),
                        input: input.clone().unwrap_or_else(|| run.input.clone()),
                    })
                    .await?;
                    return Ok(true);
                }
            }
            ToBridge::Result {
                call,
                output,
                error,
                settle,
                ..
            } if self.nested.contains_key(&call.0) => {
                if let Some(run) = self.nested.remove(&call.0) {
                    self.settle |= settle;
                    self.images.extend(Image::named(&output));
                    self.emit(FromBridge::Entry {
                        body: Body::Result {
                            call,
                            outcome: if error {
                                Outcome::Error
                            } else {
                                Outcome::Success
                            },
                            output: output.clone(),
                            inputs: Vec::new(),
                        },
                        native: None,
                    })
                    .await?;
                    let _ = run.reply.send(if error {
                        Err(code_mode::Error::Refused(output))
                    } else {
                        // An object or a string, as exec's description says.
                        Ok(serde_json::from_str::<Value>(&output).unwrap_or(Value::String(output)))
                    });
                }
                return Ok(true);
            }
            ToBridge::Input { .. } if !self.active => self.start(),
            // Escape stops the cells: an open turn refuses its later calls until it ends, and an
            // Escape that comes after its end leaves the next turn to open as any other.
            ToBridge::Interrupt { .. } => {
                self.stop.send_replace(true);
            }
            ToBridge::Close { .. } => return Ok(false),
            _ => {}
        }
        self.bridge.send(&message).await?;
        Ok(true)
    }

    async fn line(&mut self, (line, held): Output) -> Result<bool, Error> {
        if let FromBridge::Run { call, tool, input } = &line.says {
            let profile = self.profile.borrow().clone();
            if profile.code_mode
                && matches!(
                    tool.strip_prefix("mcp__razor__").unwrap_or(tool),
                    "exec" | "wait"
                )
            {
                // A turn the bridge opened on its own, for an input held past a stop, starts here.
                if !self.active {
                    self.start();
                }
                let mapped = profile.input(tool, input.clone());
                let (code, call, stop, nested) = (
                    self.code.clone(),
                    call.clone(),
                    self.stop.subscribe(),
                    Arc::from(profile.nested.clone()),
                );
                // Keep the native byte permit through the local result, rather than queue unbudgeted calls.
                self.locals.push(
                    async move {
                        let _held = held;
                        let result = async {
                            let (action, input) =
                                mapped.map_err(|e| code_mode::Error::Refused(e.to_string()))?;
                            if action == "exec" {
                                let source = input.as_str().ok_or_else(|| {
                                    code_mode::Error::Refused("exec needs source text.".into())
                                })?;
                                code.exec(call.clone(), source.into(), nested, stop).await
                            } else {
                                code.wait(input).await
                            }
                        }
                        .await;
                        (call, result)
                    }
                    .boxed(),
                );
                return Ok(true);
            }
        }
        if matches!(
            line.says,
            FromBridge::TurnEnd { .. } | FromBridge::Settled { .. } | FromBridge::Closed { .. }
        ) {
            self.cancel();
        }
        let closed = matches!(line.says, FromBridge::Closed { .. });
        self.outgoing
            .send((line, held))
            .await
            .map_err(|_| Error::Exited)?;
        Ok(!closed)
    }
}
