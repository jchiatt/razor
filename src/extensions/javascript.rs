//! One bounded QuickJS runtime per plugin, kept on its thread and fed in queue order.

use super::{DEADLINE, Error, Job, QUEUE, Registered, Reply, late, other, refused, report};
use rquickjs::{CaughtError, Context, Ctx, Function, Module, Promise, Runtime};
use std::{cell::Cell, path::Path, rc::Rc, time::Instant};
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, oneshot};

const RAZOR: &str = include_str!("razor.js");
const SOURCE: u64 = 1 << 20;

/// The same deadline covers reading the capped file, starting its thread and evaluating it.
pub(super) async fn load(
    name: &str,
    path: &Path,
) -> Result<(Registered, Option<mpsc::Sender<Job>>), Error> {
    let deadline = tokio::time::Instant::now() + DEADLINE;
    let loading = async {
        let mut source = String::new();
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|e| other(name, e))?;
        file.take(SOURCE + 1)
            .read_to_string(&mut source)
            .await
            .map_err(|e| other(name, e))?;
        if source.len() as u64 > SOURCE {
            return Err(refused(name, "it is larger than 1 MiB."));
        }
        let (queue, jobs) = mpsc::channel(QUEUE);
        let (loaded, registered) = oneshot::channel();
        let thread = name.to_owned();
        std::thread::Builder::new()
            .name(format!("plugin {name}"))
            .spawn(move || serve(&thread, &source, deadline.into_std(), loaded, jobs))
            .map_err(|e| other(name, e))?;
        let registered = registered
            .await
            .map_err(|_| refused(name, "its thread stopped while it loaded."))??;
        Ok((registered, Some(queue)))
    };
    tokio::time::timeout_at(deadline, loading)
        .await
        .unwrap_or_else(|_| Err(late(name)))
}

fn serve(
    name: &str,
    source: &str,
    at: Instant,
    loaded: oneshot::Sender<Result<Registered, Error>>,
    mut jobs: mpsc::Receiver<Job>,
) {
    let deadline = Rc::new(Cell::new(at));
    let interrupt = deadline.clone();
    let made = Runtime::new().and_then(|runtime| {
        runtime.set_memory_limit(64 << 20);
        runtime.set_max_stack_size(1 << 20);
        runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= interrupt.get())));
        Ok((Context::full(&runtime)?, runtime))
    });
    let (context, _runtime) = match made {
        Ok(made) => made,
        Err(e) => return drop(loaded.send(Err(refused(name, format!("{e}."))))),
    };
    context.with(|ctx| {
        let started = (|| -> Result<(Registered, Function), Error> {
            let dispatch: Function = ctx.eval(RAZOR).map_err(|e| thrown(&ctx, name, e))?;
            settle(&ctx, name, at, Module::evaluate(ctx.clone(), name, source))?;
            let text = settle(&ctx, name, at, dispatch.call(()))?;
            let registered =
                serde_json::from_str(&text).map_err(|e| refused(name, format!("{e}.")))?;
            Ok((registered, dispatch))
        })();
        let dispatch = match started {
            Ok((registered, dispatch)) => {
                let _ = loaded.send(Ok(registered));
                dispatch
            }
            Err(e) => return drop(loaded.send(Err(e))),
        };
        while let Some(Job { payload, reply }) = jobs.blocking_recv() {
            let at = match &reply {
                Reply::Wait { answer, .. } if answer.is_closed() => continue,
                Reply::Wait { deadline, .. } => deadline.into_std(),
                Reply::Report { .. } => Instant::now() + DEADLINE,
            };
            deadline.set(at);
            let ran = if Instant::now() >= at {
                Err(late(name))
            } else {
                settle(&ctx, name, at, dispatch.call((payload,)))
            };
            match reply {
                Reply::Wait { answer, .. } => drop(answer.send(ran.map(String::into_bytes))),
                Reply::Report { stream, .. } => {
                    if let Err(e) = ran {
                        report(&stream, &e.to_string());
                    }
                }
            }
        }
    });
}

/// Drain jobs even when the promise has settled: a handler's queued work belongs to this event.
fn settle<'js>(
    ctx: &Ctx<'js>,
    name: &str,
    deadline: Instant,
    promise: rquickjs::Result<Promise<'js>>,
) -> Result<String, Error> {
    let result = promise.and_then(|p| {
        while ctx.execute_pending_job() {}
        p.finish::<Option<String>>()
    });
    match result {
        _ if Instant::now() >= deadline => Err(late(name)),
        Ok(text) => Ok(text.unwrap_or_default()),
        Err(rquickjs::Error::WouldBlock) => Err(refused(
            name,
            "it waits on a promise nothing is left to settle.",
        )),
        Err(e) => Err(thrown(ctx, name, e)),
    }
}

fn thrown(ctx: &Ctx<'_>, name: &str, error: rquickjs::Error) -> Error {
    match CaughtError::from_error(ctx, error) {
        CaughtError::Exception(e) => refused(
            name,
            e.message()
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "it threw an error.".into()),
        ),
        e => refused(name, e),
    }
}
