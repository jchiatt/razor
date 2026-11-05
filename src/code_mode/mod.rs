//! Code mode's cells: JavaScript in a fresh QuickJS runtime per `exec`, its tool calls sent back
//! through the controller (docs/design/code-mode.md).

mod cell;

use crate::{profiles::Declaration, protocol::CallId};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fmt,
    io::SeekFrom,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
    time::Duration,
};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt},
    sync::{Mutex, Notify, mpsc, oneshot, watch},
};

/// The status line of a cell whose code threw: an error Result that still shows the cell's output.
pub(crate) const FAILED: &str = "Script failed";

/// A cell's tool call, in the order the script made it. The controller records it before
/// running it and sends exactly one reply.
pub(crate) struct Run {
    pub(crate) call: CallId,
    pub(crate) name: String,
    pub(crate) input: Value,
    pub(crate) reply: oneshot::Sender<Result<Value, Error>>,
}

/// A cell or call failure the model reads; `Other` is a machine failure with its cause.
#[derive(Debug)]
pub(crate) enum Error {
    Refused(String),
    Other {
        source: Box<dyn std::error::Error + Send + Sync>,
        context: &'static str,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(text) => f.write_str(text),
            Self::Other { source, context } => write!(f, "Failed {context}: {source}."),
        }
    }
}

impl std::error::Error for Error {}

fn other<E: std::error::Error + Send + Sync + 'static>(
    context: &'static str,
) -> impl FnOnce(E) -> Error {
    move |source| Error::Other {
        source: Box::new(source),
        context,
    }
}

/// One conversation's cells, each named by its exec's call id. At most four run at once; a
/// finished cell is only its files: the output spool `cells/<id>`, its status `<id>.end` and
/// the read position `<id>.cursor`.
pub(crate) struct CodeMode {
    dir: PathBuf,
    runs: mpsc::Sender<Run>,
    running: Mutex<BTreeMap<String, Arc<Cell>>>,
    reading: Mutex<()>,
}

/// What a running cell shares with its thread.
#[derive(Default)]
struct Cell {
    terminate: AtomicBool,
    /// Set by the script's `exit()`.
    exited: AtomicBool,
    done: AtomicBool,
    /// Rung by `yield_control()` and when the cell ends; a ring with no waiter is kept.
    wake: Notify,
}

impl CodeMode {
    /// `runs` has capacity 64 and is drained while cells run: a full channel holds the script.
    pub(crate) fn new(dir: PathBuf, runs: mpsc::Sender<Run>) -> Self {
        let (running, reading) = (Mutex::default(), Mutex::default());
        Self {
            dir: dir.join("cells"),
            runs,
            running,
            reading,
        }
    }

    /// Runs `source` as an async module in its own thread until it finishes, yields or reaches
    /// its pragma's `yield_time_ms`, in a cell named by `call`, once per session. Nothing started
    /// is undone when `stop` ends the cell.
    pub(crate) async fn exec(
        &self,
        call: CallId,
        source: String,
        nested: Arc<[Declaration]>,
        stop: watch::Receiver<bool>,
    ) -> Result<String, Error> {
        if *stop.borrow() {
            return Err(Error::Refused(
                "The turn has ended; no cell was started.".into(),
            ));
        }
        let (yield_ms, cap) = options(&source)?;
        let id = call.0.clone();
        if !named(&id) {
            return Err(Error::Refused(format!(
                "The call id {id} cannot name a cell, so no cell was started."
            )));
        }
        let mut running = self.running.lock().await;
        running.retain(|_, cell| !cell.done.load(Relaxed));
        if running.len() >= 4 {
            return Err(Error::Refused(
                "Four cells are already running; wait for one before starting another.".into(),
            ));
        }
        let spool = self.dir.join(&id);
        fs::create_dir_all(&self.dir)
            .await
            .map_err(other("making the cell directory"))?;
        fs::File::create_new(&spool)
            .await
            .map_err(other("creating the cell output"))?;
        let cell = Arc::new(Cell::default());
        let (shared, runs) = (cell.clone(), self.runs.clone());
        let thread = std::thread::Builder::new().name(id.clone()).spawn(move || {
            let status = match cell::run(&shared, &spool, call, &source, &nested, runs, stop) {
                Ok(()) if shared.terminate.load(Relaxed) => "Script terminated",
                Ok(()) => "Script completed",
                Err(error) => {
                    use std::io::Write;
                    let spooled = std::fs::OpenOptions::new().append(true).open(&spool);
                    let _ = spooled.and_then(|mut file| writeln!(file, "Script error:\n{error}"));
                    FAILED
                }
            };
            // The status is on disk before the ring, so whoever wakes reads the whole output.
            let _ = std::fs::write(spool.with_extension("end"), status);
            shared.done.store(true, Relaxed);
            shared.wake.notify_one();
        });
        thread.map_err(other("starting the cell thread"))?;
        running.insert(id.clone(), cell.clone());
        drop(running);
        self.output(&id, Some(cell), yield_ms, cap).await
    }

    /// Codex's wait: `cell_id`, `yield_time_ms` (10,000), `max_tokens` (10,000) and `terminate`.
    /// Returns only the output since the cell's last result.
    pub(crate) async fn wait(&self, input: Value) -> Result<String, Error> {
        let id = input["cell_id"].as_str().filter(|id| named(id));
        let id =
            id.ok_or_else(|| Error::Refused("wait needs a cell_id returned by exec.".into()))?;
        let terminate = match input.get("terminate") {
            None => false,
            Some(value) => value
                .as_bool()
                .ok_or_else(|| Error::Refused("terminate must be a boolean.".into()))?,
        };
        let (yield_ms, cap) = (
            number(&input, "yield_time_ms", 10_000)?,
            budget(&input, "max_tokens")?,
        );
        let cell = self.running.lock().await.get(id).cloned();
        if let Some(cell) = &cell {
            cell.terminate.fetch_or(terminate, Relaxed);
        }
        self.output(id, cell, yield_ms, cap).await
    }

    /// Codex's status line, then the new output: at most `cap` characters, the start and end kept, with the
    /// spool's path when any was left out. A failed script is an error result.
    async fn output(
        &self,
        id: &str,
        cell: Option<Arc<Cell>>,
        yield_ms: u64,
        cap: usize,
    ) -> Result<String, Error> {
        if let Some(cell) = cell.filter(|cell| !cell.done.load(Relaxed)) {
            let _ =
                tokio::time::timeout(Duration::from_millis(yield_ms), cell.wake.notified()).await;
        }
        let _reading = self.reading.lock().await;
        let spool = self.dir.join(id);
        let missing = |_| Error::Refused(format!("No cell named {id} exists in this session."));
        let mut file = fs::File::open(&spool).await.map_err(missing)?;
        // Read the status first: once it is final, the length read after it is too.
        let status = fs::read_to_string(spool.with_extension("end")).await;
        let status = status.unwrap_or_else(|_| format!("Script running with cell ID {id}"));
        let end = file
            .metadata()
            .await
            .map_err(other("reading cell output"))?
            .len();
        let cursor = fs::read_to_string(spool.with_extension("cursor"))
            .await
            .ok();
        let cursor = cursor
            .and_then(|text| text.parse().ok())
            .unwrap_or(0u64)
            .min(end);
        // Four bytes per character plus a split character; only two bounded ranges are read.
        let range = cap as u64 * 4 + 4;
        let head = range_text(&mut file, cursor, end.min(cursor + range)).await?;
        let output = if end - cursor > range || head.chars().count() > cap {
            let tail = range_text(&mut file, cursor.max(end.saturating_sub(range)), end).await?;
            let kept = cap.saturating_sub(
                format!("\n… {} bytes omitted …\n", end - cursor)
                    .chars()
                    .count(),
            );
            let head = &head[..head
                .char_indices()
                .nth(kept / 2)
                .map_or(head.len(), |(i, _)| i)];
            let tail = &tail[tail
                .char_indices()
                .rev()
                .nth(kept - kept / 2)
                .map_or(0, |(i, c)| i + c.len_utf8())..];
            let omitted = end - cursor - head.len() as u64 - tail.len() as u64;
            format!(
                "{head}\n… {omitted} bytes omitted …\n{tail}\nFull output: {}",
                spool.display()
            )
        } else {
            head
        };
        fs::write(spool.with_extension("cursor"), end.to_string())
            .await
            .map_err(other("saving the cell output position"))?;
        let result = format!("{status}\n{output}");
        if status == FAILED {
            Err(Error::Refused(result))
        } else {
            Ok(result)
        }
    }
}

/// One bounded UTF-8 range, dropping only characters split by its byte boundaries.
async fn range_text(file: &mut fs::File, start: u64, end: u64) -> Result<String, Error> {
    file.seek(SeekFrom::Start(start))
        .await
        .map_err(other("reading cell output"))?;
    let mut bytes = Vec::new();
    file.take(end - start)
        .read_to_end(&mut bytes)
        .await
        .map_err(other("reading cell output"))?;
    let leading = bytes.iter().take_while(|b| **b & 0xc0 == 0x80).count();
    let bytes = &bytes[leading..];
    let text = std::str::from_utf8(bytes).unwrap_or_else(|e| {
        // The spool contains UTF-8; only the last character can be incomplete.
        std::str::from_utf8(&bytes[..e.valid_up_to()]).unwrap_or("")
    });
    Ok(text.into())
}

/// Whether `id` can name a cell's files, as every call id on the routes with code mode can.
fn named(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && (id.bytes()).all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The optional first-line pragma `// @exec: {"yield_time_ms": …, "max_output_tokens": …}`.
fn options(source: &str) -> Result<(u64, usize), Error> {
    let refuse = |sentence: &str| Err(Error::Refused(sentence.into()));
    if source.trim().is_empty() {
        return refuse("exec needs nonempty JavaScript source.");
    }
    let (first, rest) = source.split_once('\n').unwrap_or((source, ""));
    let pragma = match first.trim_start().strip_prefix("// @exec:") {
        None => Value::Null,
        Some(_) if rest.trim().is_empty() => {
            return refuse("The @exec pragma must be followed by JavaScript source.");
        }
        Some(pragma) => match serde_json::from_str::<Value>(pragma) {
            Ok(Value::Object(fields))
                if fields
                    .keys()
                    .all(|k| k == "yield_time_ms" || k == "max_output_tokens") =>
            {
                Value::Object(fields)
            }
            Ok(Value::Object(_)) => {
                return refuse(
                    "The @exec pragma supports only yield_time_ms and max_output_tokens.",
                );
            }
            _ => return refuse("The @exec pragma must be a JSON object."),
        },
    };
    Ok((
        number(&pragma, "yield_time_ms", 30_000)?,
        budget(&pragma, "max_output_tokens")?,
    ))
}

/// A non-negative integer JavaScript can hold exactly, or the default when absent.
fn number(value: &Value, field: &str, default: u64) -> Result<u64, Error> {
    match value.get(field).filter(|v| !v.is_null()) {
        None => Ok(default),
        Some(n) => n
            .as_u64()
            .filter(|n| *n < 1 << 53)
            .ok_or_else(|| Error::Refused(format!("{field} must be a non-negative safe integer."))),
    }
}

/// A token budget as characters: four per token, at most the spill size.
fn budget(value: &Value, field: &str) -> Result<usize, Error> {
    Ok(number(value, field, 10_000)?
        .saturating_mul(4)
        .min(crate::protocol::SPILL as u64) as usize)
}
