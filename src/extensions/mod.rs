//! Configured plugins: four before-events, appended entries, and JavaScript tools.

mod hook;
mod javascript;

use crate::config::{Event, Plugin};
use crate::protocol::{CallId, EntryId, Live, Origin, SessionId, Trigger};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};
use std::{error, fmt};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{Instant, timeout_at};

const DEADLINE: Duration = Duration::from_secs(10);
const MOST: usize = 32;
const QUEUE: usize = 256;
// Bits 0..4 select before-events; bit 4 selects appended entries.
const OBSERVED: u8 = 1 << 4;
type Stream = broadcast::Sender<(Option<EntryId>, Arc<str>)>;

/// A borrowed payload: constructing it allocates nothing. Only its last field may change.
#[derive(Serialize)]
#[serde(tag = "before", rename_all = "snake_case")]
pub(crate) enum Before<'a> {
    Prompt {
        model: &'a str,
        prompt: &'a mut String,
    },
    Input {
        from: &'a Origin,
        text: &'a mut String,
    },
    /// Input has the record's shape: decoded arguments or verbatim custom source or malformed JSON.
    Call {
        call: &'a CallId,
        name: &'a str,
        input: &'a mut Value,
    },
    Rebuild {
        trigger: Trigger,
        context: &'a mut String,
    },
}

/// Cheap to clone; each event uses the plugins current when it starts.
#[derive(Clone)]
pub(crate) struct Plugins(Arc<Shared>);

struct Shared {
    home: PathBuf,
    mask: AtomicU8,
    current: Mutex<Arc<[Arc<Loaded>]>>,
    // Holding this lock serializes file loading without blocking event dispatch.
    slots: tokio::sync::Mutex<Vec<Slot>>,
}

struct Slot {
    config: Plugin,
    stamp: Option<(SystemTime, u64)>,
    good: Option<Arc<Loaded>>,
    failure: Option<String>,
}

struct Loaded {
    name: String,
    path: PathBuf,
    javascript: bool,
    registered: Registered,
    // JavaScript's whole queue, or a command hook's ordered observer queue.
    queue: Option<mpsc::Sender<Job>>,
}

#[derive(Default, Serialize, Deserialize)]
struct Registered {
    before: Vec<Event>,
    after: Vec<String>,
    tools: Vec<Tool>,
    commands: Vec<Command>,
    prompts: usize,
}

#[derive(Serialize, Deserialize)]
struct Command {
    name: String,
    description: String,
}

struct Job {
    payload: String,
    reply: Reply,
}

enum Reply {
    Wait {
        answer: oneshot::Sender<Result<Vec<u8>, Error>>,
        deadline: Instant,
    },
    Report {
        stream: Stream,
        cwd: PathBuf,
    },
}

impl Plugins {
    /// Starts empty; `load` resolves plugin paths under `home`, including `~/`.
    pub(crate) fn new(home: PathBuf) -> Plugins {
        Plugins(Arc::new(Shared {
            home,
            mask: AtomicU8::new(0),
            current: Mutex::new(Arc::new([])),
            slots: Default::default(),
        }))
    }

    /// Loads at most 32 plugins, retaining the last good version after a failed edit.
    /// Unchanged files are not evaluated again; failures are returned as one notice.
    pub(crate) async fn load(&self, config: &[Plugin]) -> Option<String> {
        let mut slots = self.0.slots.lock().await;
        let mut old = std::mem::take(&mut *slots);
        let mut notices = Vec::new();
        if config.len() > MOST {
            notices.push(format!(
                "Razor loads at most {MOST} plugins; the last {} are left out.",
                config.len() - MOST
            ));
        }
        for config in config.iter().take(MOST) {
            let old = old
                .iter()
                .position(|slot| slot.config.file == config.file)
                .map(|at| old.swap_remove(at));
            let slot = self.slot(config, old).await;
            notices.extend(slot.failure.clone());
            slots.push(slot);
        }
        let next: Vec<_> = slots.iter().filter_map(|slot| slot.good.clone()).collect();
        let mut names = BTreeSet::new();
        let mut mask = 0;
        for plugin in &next {
            for tool in &plugin.registered.tools {
                if !names.insert(&tool.name) {
                    notices.push(refused(&plugin.name, format!(
                        "its tool `{}` is left out: an earlier plugin has one by that name.", tool.name
                    )).to_string());
                }
            }
            for event in &plugin.registered.before {
                mask |= 1 << *event as u8;
            }
            if !plugin.registered.after.is_empty() {
                mask |= OBSERVED;
            }
        }
        let mut current = lock(&self.0.current);
        if !next
            .iter()
            .map(Arc::as_ptr)
            .eq(current.iter().map(Arc::as_ptr))
        {
            *current = next.into();
            self.0.mask.store(mask, Relaxed);
        }
        (!notices.is_empty()).then(|| notices.join(" "))
    }

    async fn slot(&self, config: &Plugin, old: Option<Slot>) -> Slot {
        let path = match (config.file.strip_prefix("~"), std::env::var_os("HOME")) {
            (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
            _ => self.0.home.join(&config.file),
        };
        let javascript = path.extension().is_some_and(|e| e == "js");
        let stamp = if javascript {
            tokio::fs::metadata(&path)
                .await
                .ok()
                .and_then(|m| Some((m.modified().ok()?, m.len())))
        } else {
            None
        };
        let good = match old {
            Some(old) if old.config == *config && old.stamp == stamp => return old,
            old => old.and_then(|old| old.good),
        };
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned();
        let loaded = if javascript {
            javascript::load(&name, &path).await
        } else {
            Ok(hook::load(&name, &path, config))
        };
        let (good, failure) = match loaded {
            Ok((registered, queue)) => {
                let loaded = Loaded {
                    name,
                    path,
                    javascript,
                    registered,
                    queue,
                };
                (Some(Arc::new(loaded)), None)
            }
            Err(e) => {
                let end = if good.is_some() {
                    "Its last version that loaded stays."
                } else {
                    "It is left out."
                };
                (good, Some(format!("{e} {end}")))
            }
        };
        Slot {
            config: config.clone(),
            stamp,
            good,
            failure,
        }
    }

    /// Runs handlers in order under one ten-second deadline, including queueing.
    /// A refusal leaves the borrowed field unchanged. With no handler: one atomic read,
    /// no allocation and no lock. `true` means at least one handler changed the field.
    pub(crate) async fn before(
        &self,
        session: &SessionId,
        cwd: &Path,
        event: Before<'_>,
    ) -> Result<bool, Error> {
        let (kind, field) = match &event {
            Before::Prompt { .. } => (Event::Prompt, "prompt"),
            Before::Input { .. } => (Event::Input, "text"),
            Before::Call { .. } => (Event::Call, "input"),
            Before::Rebuild { .. } => (Event::Rebuild, "context"),
        };
        if self.0.mask.load(Relaxed) & 1 << kind as u8 == 0 {
            return Ok(false);
        }
        let deadline = Instant::now() + DEADLINE;
        let mut payload = json!(&event);
        payload["session"] = json!(session);
        payload["cwd"] = json!(cwd.to_string_lossy());
        let mut changed = false;
        for plugin in self.current().iter() {
            if !plugin.registered.before.contains(&kind) {
                continue;
            }
            let answer = plugin.ask(payload.to_string(), cwd, deadline).await?;
            if answer.trim_ascii().is_empty() {
                continue;
            }
            let Ok(Value::Object(mut answer)) = serde_json::from_slice(&answer) else {
                return Err(refused(&plugin.name, "its answer is not a JSON object."));
            };
            if let Some(new) = answer.remove(field) {
                let old = &payload[field];
                if std::mem::discriminant(old) != std::mem::discriminant(&new) {
                    return Err(refused(
                        &plugin.name,
                        format!("it changed `{field}` to another kind of value."),
                    ));
                }
                changed |= *old != new;
                payload[field] = new;
            }
        }
        match (event, payload[field].take()) {
            _ if !changed => {}
            (Before::Call { input, .. }, new) => *input = new,
            (
                Before::Prompt { prompt: text, .. }
                | Before::Input { text, .. }
                | Before::Rebuild { context: text, .. },
                Value::String(new),
            ) => *text = new,
            // The field starts as a string and every answer keeps its JSON type.
            _ => {}
        }
        Ok(changed)
    }

    /// Queues an entry before returning, preserving its order with later events and calls.
    /// Observer errors appear on `stream` and stderr, never as another observed entry.
    /// With no observer this reads one atomic word and allocates nothing.
    pub(crate) fn after(&self, session: &SessionId, cwd: &Path, line: &Arc<str>, stream: &Stream) {
        if self.0.mask.load(Relaxed) & OBSERVED == 0 {
            return;
        }
        // A stored line starts with `id` and `at`; the first `kind` is the entry's.
        let kind = line
            .split_once(r#""kind":""#)
            .and_then(|(_, rest)| rest.split_once('"'));
        let Some((kind, _)) = kind else { return };
        let mut payload = None;
        for plugin in self.current().iter() {
            let observes = plugin.registered.after.iter().any(|k| k == kind);
            let Some(queue) = plugin.queue.as_ref().filter(|_| observes) else {
                continue;
            };
            let payload = payload.get_or_insert_with(|| {
                let (session, cwd) = (json!(session), json!(cwd.to_string_lossy()));
                format!(r#"{{"session":{session},"cwd":{cwd},"entry":{line}}}"#)
            });
            let reply = Reply::Report {
                stream: stream.clone(),
                cwd: cwd.to_path_buf(),
            };
            let job = Job {
                payload: payload.clone(),
                reply,
            };
            if queue.try_send(job).is_err() {
                let why = format!("it is {QUEUE} entries behind, so it missed one.");
                report(stream, &refused(&plugin.name, why).to_string());
            }
        }
    }

    /// Returns tools for the registry; the first plugin in config order owns a duplicate name.
    pub(crate) fn tools(&self) -> BTreeMap<String, Tool> {
        let mut tools = BTreeMap::new();
        for plugin in self.current().iter() {
            for tool in &plugin.registered.tools {
                tools.entry(tool.name.clone()).or_insert_with(|| {
                    let mut tool = tool.clone();
                    tool.source = Some((self.clone(), plugin.path.clone()));
                    tool
                });
            }
        }
        tools
    }

    fn current(&self) -> Arc<[Arc<Loaded>]> {
        lock(&self.0.current).clone()
    }
}

impl PartialEq for Plugins {
    fn eq(&self, other: &Plugins) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Loaded {
    async fn ask(&self, payload: String, cwd: &Path, deadline: Instant) -> Result<Vec<u8>, Error> {
        if Instant::now() >= deadline {
            return Err(late(&self.name));
        }
        let asked = async {
            let Some(queue) = self.queue.as_ref().filter(|_| self.javascript) else {
                return hook::run(&self.name, &self.path, cwd, &payload).await;
            };
            let (answer, answered) = oneshot::channel();
            let stopped = || refused(&self.name, "its thread has stopped.");
            let reply = Reply::Wait { answer, deadline };
            queue
                .send(Job { payload, reply })
                .await
                .map_err(|_| stopped())?;
            answered.await.map_err(|_| stopped())?
        };
        timeout_at(deadline, asked)
            .await
            .unwrap_or_else(|_| Err(late(&self.name)))
    }
}

/// A registered tool resolves its current plugin version at each call, not when cloned.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Tool {
    pub(crate) description: String,
    /// A JSON Schema whose root is an object.
    pub(crate) input: Value,
    name: String,
    /// Registration carries the declaration; `tools` binds it to its owner.
    #[serde(skip)]
    source: Option<(Plugins, PathBuf)>,
}

impl Tool {
    /// Returns the handler's JSON result, or a sentence on refusal, failure or deadline.
    pub(crate) async fn run(&self, input: Value) -> Result<Value, Error> {
        let deadline = Instant::now() + DEADLINE;
        let missing =
            || Error::Refused(format!("No plugin has the tool `{}` any more.", self.name));
        let (plugins, path) = self.source.as_ref().ok_or_else(missing)?;
        let plugins = plugins.current();
        let plugin = plugins
            .iter()
            .find(|p| p.path == *path && p.registered.tools.iter().any(|t| t.name == self.name))
            .ok_or_else(missing)?;
        let payload = json!({"tool": self.name, "input": input}).to_string();
        let answer = plugin.ask(payload, Path::new(""), deadline).await?;
        serde_json::from_slice(&answer)
            .map_err(|e| refused(&plugin.name, format!("its answer is not JSON: {e}.")))
    }
}

/// Loads plugins in this process and returns their registrations as JSON lines, then failures.
pub async fn list(home: &Path, config: &[Plugin]) -> String {
    let plugins = Plugins::new(home.to_path_buf());
    let failures = plugins.load(config).await;
    let mut lines = String::new();
    for p in plugins.current().iter() {
        let mut line = json!(p.registered);
        line["file"] = json!(p.path.to_string_lossy());
        line["hook"] = json!(!p.javascript);
        lines.push_str(&format!("{line}\n"));
    }
    if let Some(failure) = failures {
        lines.push_str(&format!("{failure}\n"));
    }
    if lines.is_empty() {
        lines.push_str("No plugins configured; add a [[plugins]] entry with file = \"path/to/plugin\" to your config.toml.\n");
    }
    lines
}

/// A refusal's sentence or the underlying cause of a file/process/thread failure.
#[derive(Debug)]
pub(crate) enum Error {
    Refused(String),
    Other {
        plugin: String,
        source: Box<dyn error::Error + Send + Sync>,
    },
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Refused(sentence) => f.write_str(sentence),
            Error::Other { plugin, source } => write!(f, "{plugin}: {source}."),
        }
    }
}
impl error::Error for Error {}

fn refused(plugin: &str, why: impl fmt::Display) -> Error {
    Error::Refused(format!("{plugin}: {why}"))
}
fn late(plugin: &str) -> Error {
    refused(plugin, "it did not finish within 10 seconds.")
}

fn report(stream: &Stream, sentence: &str) {
    eprintln!("razor: {sentence}");
    let live = Live::Plugin {
        text: sentence.into(),
    };
    let _ = stream.send((None, json!(live).to_string().into()));
}

// A poisoned lock still contains a whole snapshot: updates never span an unlock.
fn lock(mutex: &Mutex<Arc<[Arc<Loaded>]>>) -> MutexGuard<'_, Arc<[Arc<Loaded>]>> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn other(plugin: &str, source: std::io::Error) -> Error {
    Error::Other {
        plugin: plugin.into(),
        source: Box::new(source),
    }
}
