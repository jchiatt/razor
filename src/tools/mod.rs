//! One session's actions, the two guards and the spill (docs/design/tools.md).

mod files;
mod guard;
mod jobs;
mod mcp;
pub(crate) use files::{find_skill, list_skills};
pub(crate) use mcp::{Auth, Questions, callback, idle};
mod patch;
pub mod web;

use crate::config::Script;
use crate::protocol::{EntryId, Mcp, Millis, SPILL};
use crate::shell::{self, Shell};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};
use tokio::{io::AsyncReadExt, sync::watch};

/// The most bytes of one file an action holds in memory (`edit`, `apply_patch`, a skill).
const FILE: u64 = 64 << 20;

/// One session's actions. Every method takes `&self`; share it as `Arc<Tools>`.
pub(crate) struct Tools {
    dir: PathBuf,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    auth: Auth,
    questions: Questions,
    since: Millis,
    shell: Arc<Shell>,
    post: crate::children::Post,
    state: Arc<Mutex<State>>,
}

struct State {
    config: Config,
    /// At most 256 entries.
    list: watch::Sender<Arc<[Action]>>,
    /// The MCP servers that are configured; dropping one stops it.
    servers: BTreeMap<String, mcp::Server>,
    /// Canonical schemas across servers retain at most 16 MiB.
    catalogs: BTreeMap<String, mcp::Catalog>,
    /// At most 256 found action names, retained across connection replacement.
    selected: BTreeSet<String>,
}

/// One action as a profile sees it. Only `Tools` makes them.
#[derive(Clone)]
pub(crate) struct Action {
    /// The name every declaration binds to. It is one of the built-in names below, a scripted
    /// tool's config name, or `mcp__<server>__<tool>`. It always matches `[A-Za-z0-9_-]{1,52}`,
    /// so that with the Claude route's `mcp__razor__` prefix it still fits the 64-character form
    /// every route accepts; a scripted or MCP tool named otherwise is not added.
    pub(crate) name: String,
    /// The action changes nothing, so the controller may run it beside other read-only calls
    /// (SPEC section 3). Set by Razor for the built-ins and by the config for a scripted tool;
    /// never set for an MCP tool.
    pub(crate) read_only: bool,
    /// The text a scripted or MCP tool gives about itself. `None` for the built-ins, whose
    /// declarations are profile data.
    pub(crate) schema: Option<Schema>,
    kind: Kind,
}

/// A scripted or MCP tool's description and the JSON Schema of its input, as its config or its
/// server gave them.
#[derive(Clone, PartialEq)]
pub(crate) struct Schema {
    pub(crate) description: String,
    pub(crate) input: Value,
}

#[derive(Clone)]
enum Kind {
    Read,
    Write,
    Edit,
    Patch,
    Bash,
    Wait,
    Send,
    Kill,
    Skill,
    McpSearch,
    Session,
    Script(String),
    Mcp(mcp::Tool),
    Web,
    Plugin(crate::extensions::Tool),
}

/// What this module needs from the config. The config reader reads, parses and watches
/// the three keys (`guards.delete`, `[tools.<name>]`, `[mcp.<name>]`) and the pack, and fills
/// this; `configure` applies it.
#[derive(Clone, PartialEq)]
pub(crate) struct Config {
    /// Refuse deleting a path made before `since` (SPEC "Trust"). Key `guards.delete`:
    /// true by default, false in a pack that turns it off.
    pub(crate) delete_guard: bool,
    /// The directories searched, in order, for `<name>/SKILL.md`.
    pub(crate) skills: Vec<PathBuf>,
    /// How many of `skills`, from the first, list each skill with its description at a
    /// conversation's start; the rest list names only.
    pub(crate) described: usize,
    /// Scripted tools by name (`[tools.<name>]`).
    pub(crate) scripts: BTreeMap<String, Script>,
    /// MCP servers by name (`[mcp.<name>]`).
    pub(crate) mcp: BTreeMap<String, Mcp>,
    /// Stdio starts at the session's local project root, not its settings checkout.
    pub(crate) mcp_cwd: Option<PathBuf>,
    /// Ordered providers for search, fetch and answer (`[web]`).
    pub(crate) web: web::Config,
    /// The tools JavaScript plugins registered, by name; never read-only, as MCP tools are not.
    pub(crate) plugins: BTreeMap<String, crate::extensions::Tool>,
}

/// Why a call failed.
#[derive(Debug)]
pub(crate) enum Error {
    /// Something the model should read and act on, in one sentence: no action by that name,
    /// input that does not fit, a guard's refusal, an edit that matched nothing or more than
    /// once, an MCP tool's own error.
    Refused(String),
    /// A failure of the machine (I/O, a server that exited), with its cause and a sentence of
    /// context.
    Other {
        source: Box<dyn std::error::Error + Send + Sync>,
        context: &'static str,
    },
}

impl Tools {
    /// Ready a session's actions. Call it inside the runtime: it starts the MCP servers.
    /// - `dir` is the session's directory; spill files go in `dir/spill/`.
    /// - `cwd` and `env` are what every job starts with.
    /// - `since` is the moment from which a file counts as this session's own: the time of
    ///   the session's `Start` entry, kept across every reopening; for a child, the moment
    ///   before its worktree is made.
    /// - `post` gives a `bash` job that runs `post` the session's Post participant.
    ///
    /// Opening waits for nothing and never fails. Each configured MCP server starts in the
    /// background, and its tools join the list through the addition path when its tool list
    /// arrives. A server that does not start is reported on standard error, and its tools are
    /// left out.
    #[allow(clippy::too_many_arguments)] // Each is one thing a session's actions start from.
    pub(crate) fn open(
        dir: PathBuf,
        cwd: PathBuf,
        mut env: BTreeMap<String, String>,
        since: Millis,
        shell: Arc<Shell>,
        config: Config,
        mcp: (Auth, Questions),
        post: crate::children::Post,
    ) -> Tools {
        if let Some(id) = dir.file_name().and_then(|s| s.to_str()) {
            env.insert("RAZOR_SESSION".into(), id.into());
        }
        let list = [
            ("read", true, Kind::Read),
            ("write", false, Kind::Write),
            ("edit", false, Kind::Edit),
            ("apply_patch", false, Kind::Patch),
            ("bash", false, Kind::Bash),
            ("wait", true, Kind::Wait),
            ("send", false, Kind::Send),
            ("kill", false, Kind::Kill),
            ("skill", true, Kind::Skill),
            ("recall", true, Kind::Session),
            ("handover", false, Kind::Session),
            ("agent", false, Kind::Session),
            ("message", false, Kind::Session),
            ("status", true, Kind::Session),
            ("memory", false, Kind::Session),
            ("task", false, Kind::Session),
            ("deliver", false, Kind::Session),
        ]
        .into_iter()
        .map(|(name, read_only, kind)| Action {
            name: name.into(),
            read_only,
            schema: None,
            kind,
        })
        .collect();
        let empty = Config {
            scripts: BTreeMap::new(),
            mcp: BTreeMap::new(),
            plugins: BTreeMap::new(),
            ..config.clone()
        };
        let tools = Tools {
            dir,
            cwd,
            env,
            auth: mcp.0,
            questions: mcp.1,
            since,
            shell,
            post,
            state: Arc::new(Mutex::new(State {
                config: empty,
                list: watch::channel(list).0,
                servers: BTreeMap::new(),
                catalogs: BTreeMap::new(),
                selected: BTreeSet::new(),
            })),
        };
        tools.apply(config);
        tools
    }

    /// Apply a changed config. The controller calls this at the next admitted turn.
    /// - The guard switch and the skill directories change at once.
    /// - A new scripted tool, or a new server's tools once it lists them, is appended. A
    ///   changed one keeps its place with its new schema.
    /// - A removed tool, or every tool of a removed server, leaves the list at once and its
    ///   calls are refused by name. The conversation's earlier offer of it stands until the
    ///   next rebuild.
    /// - A server whose command, arguments or environment changed is removed, then added.
    /// - An unchanged config changes nothing.
    pub(crate) fn configure(&self, config: Config) {
        if self.state().config != config {
            self.apply(config);
        }
    }

    fn apply(&self, mut config: Config) {
        // A model's arguments are always an object, and the Claude CLI drops any other root.
        config.scripts.retain(|name, script| {
            if ["search", "fetch", "answer", "mcp_search"].contains(&name.as_str()) {
                eprintln!("The tool `{name}` has a built-in's name; it is left out.");
                return false;
            }
            let object = script.input["type"] == "object";
            if !object {
                eprintln!("The tool `{name}` is left out: its input schema's root must be an object (`\"type\": \"object\"`).");
            }
            object
        });
        let mut state = self.state();
        state.selected.retain(|selected| {
            config
                .mcp
                .keys()
                .any(|name| selected.starts_with(&format!("mcp__{name}__")))
        });
        let State {
            config: old,
            list,
            servers,
            catalogs,
            ..
        } = &mut *state;
        catalogs.retain(|name, _| config.mcp.get(name) == old.mcp.get(name));
        let mut entries = list.borrow().to_vec();
        entries.retain(|action| match &action.kind {
            Kind::Script(_) => config.scripts.contains_key(&action.name),
            Kind::Mcp(tool) => config.mcp.get(&tool.server) == old.mcp.get(&tool.server),
            Kind::Plugin(_) => config.plugins.contains_key(&action.name),
            Kind::Web => false,
            _ => true,
        });
        // The web verbs follow the other built-ins.
        let at = entries.iter().position(|a| a.schema.is_some());
        let at = at.unwrap_or(entries.len());
        let room = 256usize.saturating_sub(entries.len());
        entries.splice(at..at, config.web.actions().take(room));
        for (name, script) in &config.scripts {
            if old.scripts.get(name) != Some(script) {
                let schema = Schema {
                    description: script.description.clone(),
                    input: script.input.clone(),
                };
                let kind = Kind::Script(script.command.clone());
                put(&mut entries, name.clone(), script.read_only, schema, kind);
            }
        }
        for (name, tool) in &config.plugins {
            if name == "mcp_search" {
                eprintln!("The tool `{name}` has a built-in's name; it is left out.");
                continue;
            }
            if old.plugins.get(name) != Some(tool) {
                let (description, input) = (tool.description.clone(), tool.input.clone());
                let kind = Kind::Plugin(tool.clone());
                put(
                    &mut entries,
                    name.clone(),
                    false,
                    Schema { description, input },
                    kind,
                );
            }
        }
        for (name, server) in &config.mcp {
            if server.enabled && old.mcp.get(name) != Some(server) {
                // Replacing the entry drops the old server, which stops it.
                let cwd = config.mcp_cwd.as_deref().unwrap_or(&self.cwd);
                servers.insert(name.clone(), mcp::start(self, name, server, cwd));
            }
        }
        servers.retain(|name, _| config.mcp.get(name).is_some_and(|c| c.enabled));
        list.send_replace(entries.into());
        *old = config;
    }

    /// The current list: the built-ins in a fixed order, then the scripted and MCP tools in the
    /// order they were first added. It holds at most 256 entries; an addition past that is
    /// refused, and the refusal is reported on standard error.
    pub(crate) fn actions(&self) -> Arc<[Action]> {
        self.state().list.borrow().clone()
    }

    /// The current actions without searched MCP tools; discoveries remain callable and are
    /// offered after the opening's declarations, preserving its prompt-cache prefix.
    pub(crate) fn initial(&self) -> Arc<[Action]> {
        let state = self.state();
        state
            .list
            .borrow()
            .iter()
            .filter(|a| !state.selected.contains(&a.name))
            .cloned()
            .collect()
    }

    /// One coalesced current list, immediately ready and then notified on registry changes.
    pub(crate) fn subscribe(&self) -> watch::Receiver<Arc<[Action]>> {
        let mut list = self.state().list.subscribe();
        list.mark_changed();
        list
    }

    /// Run one call, at once: this method orders nothing. The controller decides when it runs
    /// (one first-in-first-out lock per session, read for a read-only action, write
    /// for the rest). `at` is the call's `Call` entry in the record; spill files are named
    /// after it. A future dropped early stops the call where it is; a job it started keeps
    /// running.
    ///
    /// The input is the action's fields (docs/design/tools.md) as JSON; unknown fields are
    /// ignored. The output is JSON, never rendered text.
    pub(crate) async fn run(&self, at: EntryId, name: &str, input: Value) -> Result<Value, Error> {
        let kind = self
            .state()
            .list
            .borrow()
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.kind.clone());
        let Some(kind) = kind else {
            return Err(Error::Refused(format!(
                "There is no tool named `{name}`; use one from the tool list."
            )));
        };
        match kind {
            Kind::Read => files::read(self, at, parse(input)?).await,
            Kind::Write => files::write(self, parse(input)?).await,
            Kind::Edit => files::edit(self, parse(input)?).await,
            Kind::Patch => patch::apply(self, parse(input)?).await,
            Kind::Skill => files::skill(self, at, parse(input)?).await,
            Kind::McpSearch => self.find(input),
            Kind::Bash => jobs::bash(self, at, parse(input)?).await,
            Kind::Wait | Kind::Send | Kind::Kill => jobs::handle(self, name, parse(input)?).await,
            Kind::Script(script) => jobs::script(self, at, script, input).await,
            Kind::Mcp(tool) => mcp::call(self, at, tool, input).await,
            Kind::Web => web::run(self, at, name, input).await,
            Kind::Plugin(tool) => {
                (tool.run(input).await).map_err(|e| Error::Refused(e.to_string()))
            }
            Kind::Session => Err(Error::Refused(format!(
                "`{name}` is run by the session, not the tool service."
            ))),
        }
    }

    /// Bound a text the controller made itself (a recalled entry). A text of at most `SPILL`
    /// characters comes back unchanged, with no path. A longer text is written to
    /// `dir/spill/<at>`, and its last `SPILL` characters come back with that path.
    async fn spill(&self, at: EntryId, text: String) -> Result<(String, Option<PathBuf>), Error> {
        let count = text.chars().count();
        if count <= SPILL {
            return Ok((text, None));
        }
        let dir = self.dir.join("spill");
        let path = dir.join(at.0.to_string());
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| other(e, "Could not make the spill directory."))?;
        tokio::fs::write(&path, &text)
            .await
            .map_err(|e| other(e, "Could not write the spill file."))?;
        Ok((text.chars().skip(count - SPILL).collect(), Some(path)))
    }

    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    /// A path from a call, relative to the session's `cwd` unless it is absolute.
    fn path(&self, path: &str) -> PathBuf {
        self.cwd.join(path)
    }

    async fn save(&self, shown: &str, text: &str) -> Result<(), Error> {
        let path = self.path(shown);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| io(e, shown))?;
        }
        tokio::fs::write(path, text).await.map_err(|e| io(e, shown))
    }
}

/// The state is plain data and every holder leaves it whole, so a panic elsewhere spoils nothing.
fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Add a scripted or MCP tool, or give a known one its new schema in place.
fn put(list: &mut Vec<Action>, name: String, read_only: bool, schema: Schema, kind: Kind) {
    let valid = (1..=52).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    let action = Action {
        name,
        read_only,
        schema: Some(schema),
        kind,
    };
    match list.iter().position(|a| a.name == action.name) {
        Some(i) if list[i].schema.is_some() => list[i] = action,
        Some(_) => eprintln!(
            "The tool `{}` has a built-in's name; it is left out.",
            action.name
        ),
        None if !valid => eprintln!(
            "The tool `{}` is left out: a tool's name is 1 to 52 letters, digits, `_` or `-`.",
            action.name
        ),
        None if list.len() >= 256 => eprintln!(
            "The tool `{}` is left out: a session holds at most 256 tools.",
            action.name
        ),
        None => list.push(action),
    }
}

fn parse<T: DeserializeOwned>(input: Value) -> Result<T, Error> {
    serde_json::from_value(input)
        .map_err(|e| Error::Refused(format!("The input does not fit this tool: {e}.")))
}

fn other(source: impl std::error::Error + Send + Sync + 'static, context: &'static str) -> Error {
    Error::Other {
        source: Box::new(source),
        context,
    }
}

/// A file action's I/O failure; `shown` is the path as the call gave it.
fn io(error: std::io::Error, shown: &str) -> Error {
    match error.kind() {
        ErrorKind::NotFound => Error::Refused(format!("{shown} does not exist; check the path.")),
        ErrorKind::IsADirectory => Error::Refused(format!("{shown} is a directory, not a file.")),
        ErrorKind::PermissionDenied => Error::Refused(format!(
            "{shown} is not permitted to this session: {error}."
        )),
        ErrorKind::InvalidData => {
            Error::Refused(format!("{shown} is not UTF-8 text; use a command for it."))
        }
        _ => other(error, "A file action failed."),
    }
}

/// A whole UTF-8 file of at most `FILE` bytes.
async fn load(path: &Path, shown: &str) -> Result<String, Error> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| io(e, shown))?;
    let size = file.metadata().await.map_err(|e| io(e, shown))?.len();
    if size > FILE {
        return Err(Error::Refused(format!(
            "{shown} is over 64 MiB; change it with a command instead."
        )));
    }
    let mut text = String::with_capacity(size as usize);
    file.take(FILE)
        .read_to_string(&mut text)
        .await
        .map_err(|e| io(e, shown))?;
    Ok(text)
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(text) => f.write_str(text),
            Self::Other { source, context } => write!(f, "{context} {source}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Other { source, .. } => Some(source.as_ref()),
            Self::Refused(_) => None,
        }
    }
}

impl From<shell::Error> for Error {
    fn from(error: shell::Error) -> Error {
        match error {
            shell::Error::Refused(text) => Error::Refused(text.into()),
            shell::Error::Other { source, context } => Error::Other { source, context },
        }
    }
}
