//! One cell's thread: a plain QuickJS runtime driven by hand, with no JavaScript on the loop.

use super::{Cell, Error, Run};
use crate::{profiles::Declaration, protocol::CallId};
use rquickjs::{
    CatchResultExt, Context, Ctx, Exception, Function, Module, Object, Promise, Runtime, Value,
    function::{Func, Opt},
};
use std::{
    cell::RefCell,
    fs::File,
    future::Future,
    io::Write,
    path::Path,
    pin::Pin,
    rc::Rc,
    sync::{Arc, atomic::Ordering::Relaxed},
    task::{self, Wake, Waker},
    thread::{self, Thread},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, watch};

type Reply = oneshot::Receiver<Result<serde_json::Value, Error>>;

/// What the script holds. JavaScript owns the functions that fill `pending` and `timers`, so
/// both are emptied before the runtime goes: a value Rust still held would outlive it.
#[derive(Default)]
struct State<'js> {
    pending: Rc<RefCell<Vec<(Reply, Function<'js>, Function<'js>)>>>,
    timers: Rc<RefCell<Vec<(u64, Instant, Function<'js>)>>>,
}

impl Drop for State<'_> {
    fn drop(&mut self) {
        self.pending.borrow_mut().clear();
        self.timers.borrow_mut().clear();
    }
}

/// Wakes the cell's thread when a tool's reply arrives.
struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs the module until it completes, calls `exit()`, or is stopped or terminated; a cell
/// ended from outside is marked terminated. An uncaught error is the cell's error, verbatim.
pub(super) fn run(
    cell: &Arc<Cell>,
    spool: &Path,
    call: CallId,
    source: &str,
    nested: &[Declaration],
    runs: mpsc::Sender<Run>,
    stop: watch::Receiver<bool>,
) -> Result<(), Error> {
    let output = File::options()
        .append(true)
        .open(spool)
        .map_err(super::other("opening the cell output"))?;
    let halted = {
        let cell = cell.clone();
        move || cell.terminate.load(Relaxed) || cell.exited.load(Relaxed) || *stop.borrow()
    };
    let halt = || {
        let halted = halted();
        cell.terminate
            .fetch_or(halted && !cell.exited.load(Relaxed), Relaxed);
        halted
    };
    let js = |error: rquickjs::Error| Error::Refused(error.to_string());
    let runtime = Runtime::new().map_err(js)?;
    runtime.set_memory_limit(64 << 20);
    runtime.set_max_stack_size(1 << 20);
    // Checked during synchronous JavaScript too, so even an endless loop stops.
    runtime.set_interrupt_handler(Some(Box::new(halted.clone())));
    let context = Context::full(&runtime).map_err(js)?;
    context.with(|ctx| {
        let state = State::default();
        install(&ctx, cell, output, call, nested, runs, &state).map_err(js)?;
        let promise = match Module::evaluate(ctx.clone(), "exec", source.as_bytes()).catch(&ctx) {
            Ok(promise) => promise,
            Err(_) if halt() => return Ok(()),
            Err(error) => return Err(Error::Refused(error.to_string())),
        };
        let waker = Waker::from(Arc::new(Unpark(thread::current())));
        let mut task = task::Context::from_waker(&waker);
        loop {
            while !halt() && ctx.execute_pending_job() {}
            if halt() {
                return Ok(());
            }
            if let Some(result) = promise.result::<Value>() {
                return result
                    .catch(&ctx)
                    .map(drop)
                    .map_err(|error| Error::Refused(error.to_string()));
            }
            if !(state.replies(&ctx, &mut task).map_err(js)? | state.timers(&ctx)?) {
                let now = Instant::now();
                let next = state
                    .timers
                    .borrow()
                    .iter()
                    .map(|(_, at, _)| at.saturating_duration_since(now))
                    .min();
                // Wake at least every 50 ms to see a stop.
                thread::park_timeout(next.unwrap_or(Duration::MAX).min(Duration::from_millis(50)));
            }
        }
    })
}

/// The globals Codex's exec description promises: `tools`, `ALL_TOOLS`, `text`, `image`,
/// `generatedImage`, `exit`, `setTimeout`, `clearTimeout` and `yield_control`; nothing else.
fn install<'js>(
    ctx: &Ctx<'js>,
    cell: &Arc<Cell>,
    output: File,
    call: CallId,
    nested: &[Declaration],
    runs: mpsc::Sender<Run>,
    state: &State<'js>,
) -> rquickjs::Result<()> {
    let globals = ctx.globals();
    let tools = Object::new(ctx.clone())?;
    let count = Rc::new(std::cell::Cell::new(0u64));
    for declaration in nested {
        let (runs, pending, count, call, name) = (
            runs.clone(),
            state.pending.clone(),
            count.clone(),
            call.clone(),
            declaration.name.clone(),
        );
        let invoke = move |ctx: Ctx<'js>, input: Value<'js>| -> rquickjs::Result<Promise<'js>> {
            let text = ctx
                .json_stringify(input)?
                .map(|s| s.to_string())
                .transpose()?;
            let input = serde_json::from_str(text.as_deref().unwrap_or("null"))
                .map_err(|e| Exception::throw_type(&ctx, &e.to_string()))?;
            count.set(count.get() + 1);
            let (reply, receiver) = oneshot::channel();
            let call = CallId(format!("{}:{}", call.0, count.get()));
            // A plain thread, never the runtime's: blocking here keeps the calls in script order.
            let sent = runs.blocking_send(Run {
                call,
                name: name.clone(),
                input,
                reply,
            });
            sent.map_err(|_| Exception::throw_message(&ctx, "The session controller stopped."))?;
            let (promise, resolve, reject) = ctx.promise()?;
            pending.borrow_mut().push((receiver, resolve, reject));
            Ok(promise)
        };
        tools.set(
            declaration.global_name(),
            Function::new(ctx.clone(), invoke)?,
        )?;
    }
    globals.set("tools", tools)?;
    let all: Vec<_> = nested
        .iter()
        .map(|d| serde_json::json!({"name": d.global_name(), "description": d.sample()}))
        .collect();
    globals.set(
        "ALL_TOOLS",
        ctx.json_parse(serde_json::Value::from(all).to_string())?,
    )?;
    let text = move |ctx: Ctx<'js>, value: Value<'js>| -> rquickjs::Result<()> {
        let text = match value.as_string() {
            Some(text) => Some(text.to_string()?),
            None => ctx
                .json_stringify(value)?
                .map(|s| s.to_string())
                .transpose()?,
        };
        let line = format!("{}\n", text.as_deref().unwrap_or("undefined"));
        (&output)
            .write_all(line.as_bytes())
            .map_err(|e| Exception::throw_message(&ctx, &e.to_string()))
    };
    globals.set("text", Func::from(text))?;
    // Razor shows every image a nested result names, so these two only keep Codex's scripts
    // running; a generated image's hint, which names where it was saved, is written as text.
    globals.set("image", Func::from(|| ()))?;
    let generated = |ctx: Ctx<'js>, result: Opt<Value<'js>>| -> rquickjs::Result<()> {
        let result = result.0.and_then(Value::into_object);
        match result.and_then(|r| r.get::<_, String>("output_hint").ok()) {
            Some(hint) => ctx.globals().get::<_, Function>("text")?.call((hint,)),
            None => Ok(()),
        }
    };
    globals.set("generatedImage", Func::from(generated))?;
    let exited = cell.clone();
    let exit = move |ctx: Ctx<'js>| -> rquickjs::Result<()> {
        exited.exited.store(true, Relaxed);
        Err(Exception::throw_message(&ctx, "exit"))
    };
    globals.set("exit", Func::from(exit))?;
    let (timers, next) = (state.timers.clone(), Rc::new(std::cell::Cell::new(0u64)));
    let set =
        move |ctx: Ctx<'js>, callback: Function<'js>, ms: Opt<f64>| -> rquickjs::Result<u64> {
            let mut timers = timers.borrow_mut();
            if timers.len() >= 64 {
                return Err(Exception::throw_message(
                    &ctx,
                    "A cell can have at most 64 timers.",
                ));
            }
            next.set(next.get() + 1);
            let at = Instant::now()
                + Duration::from_millis(ms.0.unwrap_or(0.0).clamp(0.0, 2_147_483_647.0) as u64);
            timers.push((next.get(), at, callback));
            Ok(next.get())
        };
    globals.set("setTimeout", Func::from(set))?;
    let timers = state.timers.clone();
    let clear = move |id: Opt<u64>| {
        timers
            .borrow_mut()
            .retain(|(timer, _, _)| Some(*timer) != id.0)
    };
    globals.set("clearTimeout", Func::from(clear))?;
    let cell = cell.clone();
    globals.set("yield_control", Func::from(move || cell.wake.notify_one()))
}

impl<'js> State<'js> {
    /// Settles every promise whose reply has come, in the order the script called.
    fn replies(&self, ctx: &Ctx<'js>, task: &mut task::Context<'_>) -> rquickjs::Result<bool> {
        let mut replies = Vec::new();
        let mut pending = self.pending.borrow_mut();
        let ready: Vec<_> =
            (pending.extract_if(.., |(reply, _, _)| match Pin::new(reply).poll(task) {
                task::Poll::Ready(reply) => {
                    replies.push(reply);
                    true
                }
                task::Poll::Pending => false,
            }))
            .collect();
        drop(pending);
        let settled = !ready.is_empty();
        for ((_, resolve, reject), reply) in ready.into_iter().zip(replies) {
            match reply {
                Ok(Ok(value)) => resolve.call::<_, ()>((ctx.json_parse(value.to_string())?,))?,
                Ok(Err(error)) => reject
                    .call::<_, ()>((Exception::from_message(ctx.clone(), &error.to_string())?,))?,
                Err(_) => {
                    let stopped = "The controller stopped before this tool returned.";
                    reject.call::<_, ()>((Exception::from_message(ctx.clone(), stopped)?,))?;
                }
            }
        }
        Ok(settled)
    }

    /// Runs the timers that are due; a callback's uncaught error ends the cell.
    fn timers(&self, ctx: &Ctx<'js>) -> Result<bool, Error> {
        let now = Instant::now();
        let due: Vec<_> = self
            .timers
            .borrow_mut()
            .extract_if(.., |(_, at, _)| *at <= now)
            .collect();
        for (_, _, callback) in &due {
            callback
                .call::<_, ()>(())
                .catch(ctx)
                .map_err(|error| Error::Refused(error.to_string()))?;
        }
        Ok(!due.is_empty())
    }
}
