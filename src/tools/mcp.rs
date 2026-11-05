//! Session-owned MCP connections, their catalogs and the search that adds found tools; the
//! protocol, sign-in and person questions are rmcp's.

mod auth;
mod elicitation;
mod http;
pub(crate) use auth::{Auth, callback};
pub(crate) use elicitation::Questions;

use super::{Action, Error, Kind, Schema, State, Tools, lock, other, parse, put};
use crate::profiles::mcp_text;
use crate::protocol::{Connection, EntryId, Mcp, Reference};
use base64::Engine;
use elicitation::{Handler, elicit};
use rmcp::{
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, DEFAULT_MRTR_MAX_ROUNDS,
        InputRequest, PaginatedRequestParams, ProtocolVersion, RequestMetaObject,
    },
    service::{ClientLifecycleMode, ClientServiceExt, Peer, RoleClient},
    transport::TokioChildProcess,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex, Weak},
};
use tokio::task::JoinHandle;

/// The most bytes of tool listings a session holds, across its servers.
const CATALOGS: usize = 16 << 20;

/// A native action stays bound to the peer that published it.
#[derive(Clone)]
pub(super) struct Tool {
    pub(super) server: String,
    name: String,
    peer: Peer<RoleClient>,
    questions: Questions,
}

/// A configured server's task; dropping it stops the task and its transport.
pub(super) struct Server {
    task: JoinHandle<()>,
    state: &'static str,
    error: Option<String>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A server's tools after its allow list, with what search and the opening section need.
pub(super) struct Catalog {
    purpose: String,
    entries: Vec<(String, Schema, Tool)>,
    search: bool,
    bytes: usize,
}

/// What one server's task holds.
struct Link {
    name: String,
    config: Mcp,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    state: Weak<Mutex<State>>,
    auth: Auth,
    questions: Questions,
}

/// Starts one server's task. It connects, lists, and lists again on a legacy change notice;
/// after a failure, and on a reconnect request, it starts over.
pub(super) fn start(tools: &Tools, name: &str, config: &Mcp, cwd: &Path) -> Server {
    let link = Link {
        name: name.into(),
        config: config.clone(),
        cwd: cwd.into(),
        env: tools.env.clone(),
        state: Arc::downgrade(&tools.state),
        auth: tools.auth.clone(),
        questions: tools.questions.clone(),
    };
    let (task, state, error) = (tokio::spawn(link.run()), "connecting", None);
    Server { task, state, error }
}

fn current(server: &Server) -> bool {
    tokio::task::try_id() == Some(server.task.id())
}

/// Transport diagnostics can hold credential-bearing URLs or bodies, so only a sentence remains.
fn connection_error(error: impl std::fmt::Debug) -> Error {
    let debug = format!("{error:?}");
    let signing = [
        "AuthRequired",
        "AuthorizationRequired",
        "InsufficientScope",
        "401",
    ];
    Error::Refused(match signing.iter().any(|word| debug.contains(word)) {
        true => "The MCP server requires sign-in; sign in and reconnect.".into(),
        false => "The MCP server could not connect; check its configuration and reconnect.".into(),
    })
}

impl Link {
    async fn run(self) {
        let mut reconnects = self.auth.reconnects();
        loop {
            self.status("connecting", None);
            let error = tokio::select! {
                served = self.serve() => served.err().map_or_else(
                    || "The MCP server disconnected; reconnect to restore its tools.".into(),
                    |e| e.to_string()),
                () = auth::requested(&mut reconnects, &self.name) => String::new(),
            };
            self.listed(String::new(), Vec::new());
            if !error.is_empty() {
                let signing = error.contains("sign in") || error.contains("sign-in");
                let shown = if signing { "needs_sign_in" } else { "failed" };
                self.status(shown, Some(error));
                auth::requested(&mut reconnects, &self.name).await;
            }
        }
    }

    /// The connection's state, published to the registry's subscribers by this task only.
    fn status(&self, shown: &'static str, error: Option<String>) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let mut state = lock(&state);
        if let Some(server) = state.servers.get_mut(&self.name).filter(|s| current(s)) {
            (server.state, server.error) = (shown, error);
            state.list.send_modify(|_| {});
        }
    }

    async fn serve(&self) -> Result<(), Error> {
        let (name, questions, changed) =
            (self.name.clone(), self.questions.clone(), Arc::default());
        let handler = Handler {
            name,
            changed: Arc::clone(&changed),
            questions,
        };
        let lifecycle = ClientLifecycleMode::Auto {
            preferred_versions: vec![ProtocolVersion::LATEST],
            legacy_version: Some(ProtocolVersion::LATEST_WITH_INITIALIZE),
        };
        let client = match &self.config.connection {
            Some(Connection::Stdio { command, args, env }) => {
                let mut command = tokio::process::Command::new(command);
                command.args(args).current_dir(&self.cwd).env_clear();
                command.envs(&self.env).envs(env);
                let spawned = TokioChildProcess::builder(command)
                    .stderr(Stdio::null())
                    .spawn();
                let (transport, _) =
                    spawned.map_err(|e| other(e, "Could not start the MCP server."))?;
                let served = handler.serve_with_lifecycle(transport, lifecycle).await;
                served.map_err(connection_error)?
            }
            _ => http::connect(&self.name, &self.config, handler, lifecycle, &self.auth).await?,
        };
        let peer = client.peer().clone();
        let info = peer.peer_info();
        let info = info.as_ref();
        let modern = info.is_some_and(|i| i.protocol_version >= ProtocolVersion::V_2026_07_28);
        // The server's purpose: its instructions' first sentence, else its title or name.
        let sentence = info
            .and_then(|i| i.instructions.as_deref())
            .and_then(|text| {
                let first = text.split_inclusive(['.', '!', '?', '\n']).next()?.trim();
                (!first.is_empty()).then(|| first.to_owned())
            });
        let title = info.and_then(|i| i.server_info.as_ref());
        let title = title.map(|i| i.title.clone().unwrap_or_else(|| i.name.clone()));
        let purpose = sentence.or(title).unwrap_or_else(|| self.name.clone());
        let mut stopped = std::pin::pin!(client.waiting());
        loop {
            self.listed(purpose.clone(), self.catalog(&peer).await?);
            tokio::select! {
                _ = changed.notified(), if !modern => {},
                _ = &mut stopped => return Ok(()),
            }
        }
    }

    async fn catalog(&self, peer: &Peer<RoleClient>) -> Result<Vec<(String, Schema, Tool)>, Error> {
        let (mut tools, mut cursor, mut bytes) = (Vec::new(), None, 0);
        for _ in 0..100 {
            let request = PaginatedRequestParams::default().with_cursor(cursor);
            let page = peer
                .list_tools(Some(request))
                .await
                .map_err(connection_error)?;
            bytes += serde_json::to_vec(&page.tools).map_or(0, |listed| listed.len());
            if bytes > CATALOGS {
                return Err(Error::Refused(
                    "The MCP catalog exceeds 16 MiB; reduce its tools.".into(),
                ));
            }
            tools.extend(page.tools.into_iter().filter(for_model).map(|tool| {
                let description = tool.description.map(String::from).unwrap_or_default();
                let input = Value::Object((*tool.input_schema).clone());
                let (server, peer, questions) =
                    (self.name.clone(), peer.clone(), self.questions.clone());
                let tool = Tool {
                    server,
                    name: tool.name.into(),
                    peer,
                    questions,
                };
                (
                    format!("mcp__{}__{}", self.name, tool.name),
                    Schema { description, input },
                    tool,
                )
            }));
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(tools);
            }
        }
        Err(Error::Refused(
            "The MCP catalog exceeds 100 pages; reduce its tools.".into(),
        ))
    }

    /// Publishes a current connection's catalog: its allow list first, then search when the
    /// server says so or its tools pass 8 KiB, all within the session's catalog bytes.
    fn listed(&self, purpose: String, mut tools: Vec<(String, Schema, Tool)>) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let mut guard = lock(&state);
        let (state, server) = (&mut *guard, self.name.as_str());
        let running = state.servers.get_mut(server).filter(|s| current(s));
        let (Some(config), Some(running)) = (state.config.mcp.get(server), running) else {
            return;
        };
        let prefix = format!("mcp__{server}__").len();
        tools.retain(|(name, ..)| {
            config
                .tools
                .as_ref()
                .is_none_or(|set| set.contains(&name[prefix..]))
        });
        let size = |(name, schema, _): &(String, Schema, Tool)| {
            name.len() + schema.description.len() + schema.input.to_string().len()
        };
        let mut bytes = tools.iter().map(size).sum();
        let others = state.catalogs.iter().filter(|(name, _)| *name != server);
        let others: usize = others.map(|(_, catalog)| catalog.bytes).sum();
        (running.state, running.error) = ("connected", None);
        if others + bytes > CATALOGS {
            (tools, bytes, running.state) = (Vec::new(), 0, "failed");
            let why = "The server exceeds this session's 16 MiB of MCP catalogs; reduce its tools.";
            running.error = Some(why.into());
        }
        let search = config.search.unwrap_or(bytes > 8 << 10);
        let mut entries = state.list.borrow().to_vec();
        entries.retain(|action| match &action.kind {
            Kind::Mcp(tool) if tool.server == server => tools.iter().any(|t| t.0 == action.name),
            _ => true,
        });
        for (name, schema, tool) in &tools {
            if !search || state.selected.contains(name) {
                put(
                    &mut entries,
                    name.clone(),
                    false,
                    schema.clone(),
                    Kind::Mcp(tool.clone()),
                );
            }
        }
        let searchable = entries.iter().any(|a| matches!(a.kind, Kind::McpSearch));
        if search && entries.len() < 256 && !searchable {
            entries.push(Action {
                name: "mcp_search".into(),
                read_only: false,
                schema: None,
                kind: Kind::McpSearch,
            });
        }
        let catalog = Catalog {
            purpose,
            entries: tools,
            search,
            bytes,
        };
        state.catalogs.insert(server.into(), catalog);
        state.list.send_replace(entries.into());
    }
}

impl Tools {
    /// `mcp_search`: up to five tools by exact name, name words, then description words; each
    /// found one joins the list now and stays found across reconnection.
    pub(super) fn find(&self, input: Value) -> Result<Value, Error> {
        #[derive(serde::Deserialize)]
        struct Query {
            query: String,
        }
        let Query { query } = parse(input)?;
        let words = |text: &str| -> Vec<String> {
            let words = text
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty());
            words.map(str::to_lowercase).collect()
        };
        let wanted = words(&query);
        let matched = |text: &str| {
            let have = words(text);
            wanted.iter().filter(|word| have.contains(word)).count()
        };
        let mut state = self.state();
        let entries = state
            .catalogs
            .values()
            .filter(|c| c.search)
            .flat_map(|c| &c.entries);
        let mut found: Vec<_> = entries
            .filter_map(|(name, schema, tool)| {
                let exact =
                    name.eq_ignore_ascii_case(&query) || tool.name.eq_ignore_ascii_case(&query);
                let rank = (exact, matched(name), matched(&schema.description));
                (rank != (false, 0, 0)).then(|| (rank, name.clone(), schema.clone(), tool.clone()))
            })
            .collect();
        found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut list = state.list.borrow().to_vec();
        let mut result = Vec::new();
        for (_, name, schema, tool) in found.into_iter().take(5) {
            let line = schema
                .description
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            put(&mut list, name.clone(), false, schema, Kind::Mcp(tool));
            if list.iter().any(|a| a.name == name) {
                result.push(json!({"name": name, "description": line}));
                state.selected.insert(name);
            }
        }
        state.list.send_replace(list.into());
        Ok(Value::Array(result))
    }

    /// The servers for the opening and availability texts: each one's purpose, tool count and
    /// state, with the shell commands that manage them; empty with no server configured.
    pub(crate) fn mcp_section(&self) -> String {
        let state = self.state();
        let servers = state.config.mcp.keys().map(|name| {
            let catalog = state.catalogs.get(name).filter(|c| !c.purpose.is_empty());
            let purpose = catalog.map_or(name.as_str(), |c| c.purpose.as_str());
            let (shown, count, _) = shown(&state, name);
            let fields = [
                ("name", &**name),
                ("purpose", purpose),
                ("count", &count.to_string()),
                ("state", shown),
            ];
            format!("\n{}", mcp_text("server", &fields))
        });
        let servers: String = servers.collect();
        match servers.is_empty() {
            true => servers,
            false => mcp_text("opening", &[]) + &servers,
        }
    }

    /// The configured servers and their availability, ignoring connection progress and counts.
    pub(crate) fn mcp_key(&self) -> String {
        let state = self.state();
        state
            .config
            .mcp
            .keys()
            .map(|name| {
                let (shown, _, _) = shown(&state, name);
                let shown = match shown {
                    "configured" | "connecting" | "connected" => "available",
                    other => other,
                };
                format!("{name:?}:{shown:?};")
            })
            .collect()
    }

    /// The availability notice when the server key differs from `seen`, which takes the new key.
    pub(crate) fn mcp_news(&self, seen: &mut String) -> Option<String> {
        let current = self.mcp_key();
        if current == *seen {
            return None;
        }
        let servers = self.mcp_section();
        let key = match servers.is_empty() {
            true => "availability_empty",
            false => "availability",
        };
        let text = mcp_text(key, &[("servers", &servers)]);
        *seen = current;
        Some(text)
    }

    /// A server's state, its allowed tools (each one's own name and its description's first
    /// line, cut to 120 characters) and its last error, never credential bytes.
    pub(crate) fn mcp_state(&self, name: &str) -> (String, Vec<(String, String)>, Option<String>) {
        let state = self.state();
        let (shown, count, error) = shown(&state, name);
        let entries = state.catalogs.get(name).map_or(&[][..], |c| &c.entries);
        let first = |s: &str| s.lines().next().unwrap_or("").chars().take(120).collect();
        let tools = entries.iter().take(count);
        let tools = tools.map(|(_, s, tool)| (tool.name.clone(), first(&s.description)));
        (shown.into(), tools.collect(), error)
    }
}

fn shown(state: &State, name: &str) -> (&'static str, usize, Option<String>) {
    let count = state.catalogs.get(name).map_or(0, |c| c.entries.len());
    match state.servers.get(name) {
        Some(server) => (server.state, count, server.error.clone()),
        None => (
            idle(state.config.mcp.get(name).is_none_or(|c| c.enabled)),
            0,
            None,
        ),
    }
}

/// MCP Apps: a listed tool is the model's unless its `_meta.ui.visibility` is a list without
/// "model"; absent, it is `["model", "app"]`.
fn for_model(tool: &rmcp::model::Tool) -> bool {
    let ui = tool.meta.as_ref().and_then(|meta| meta.get("ui"));
    let seen = ui.and_then(|ui| ui.get("visibility")?.as_array());
    seen.is_none_or(|seen| seen.iter().any(|v| v == "model"))
}

/// Razor's `_meta` on each `tools/call`: its home, absolute, and the calling session's id, from
/// the session's directory `<home>/sessions/<id>`. The prefix is one label, since Razor ships
/// under other names and owns no domain to reverse.
fn meta(tools: &Tools) -> RequestMetaObject {
    let home = tools.dir.ancestors().nth(2).unwrap_or(&tools.dir);
    let home = std::path::absolute(home).unwrap_or_else(|_| home.into());
    let session = tools.dir.file_name().unwrap_or_default().to_string_lossy();
    let meta = json!({"razor/home": home.to_string_lossy(), "razor/session": session});
    serde_json::from_value(meta).unwrap_or_default()
}

/// A server with no connection in this session: configured, or disabled.
pub(crate) fn idle(enabled: bool) -> &'static str {
    if enabled { "configured" } else { "disabled" }
}

/// One call. Input requests go to the person within rmcp's round cap; a continuation is not a
/// replay of an ambiguous call.
pub(super) async fn call(
    tools: &Tools,
    at: EntryId,
    tool: Tool,
    input: Value,
) -> Result<Value, Error> {
    let mut request = CallToolRequestParams::new(tool.name.clone());
    request.meta = Some(meta(tools));
    request.arguments = match input {
        Value::Object(arguments) => Some(arguments),
        Value::Null => None,
        _ => return Err(Error::Refused("An MCP tool takes a JSON object.".into())),
    };
    let timeout = tools
        .state()
        .config
        .mcp
        .get(&tool.server)
        .and_then(|s| s.timeout)
        .map_or(600_000, |timeout| timeout.get());
    let refused = |why: &str| Err(Error::Refused(why.into()));
    for round in 0..DEFAULT_MRTR_MAX_ROUNDS {
        // rmcp's call cancels its request on drop, including when this timeout expires.
        let response = tokio::time::timeout(
            std::time::Duration::from_millis(timeout),
            tool.peer.call_tool_once(request.clone()),
        )
        .await
        .map_err(|_| {
            Error::Refused(format!(
                "The MCP server {} did not answer within {timeout} ms; \
             the call's outcome is unknown and it was not retried.",
                tool.server,
            ))
        })?
        .map_err(|e| match e {
            rmcp::ServiceError::McpError(error) => Error::Refused(error.message.into_owned()),
            error => connection_error(error),
        })?;
        let result = match response {
            CallToolResponse::Complete(result) => return rich(tools, at, result).await,
            CallToolResponse::InputRequired(result) => result,
            _ => {
                return refused(
                    "The MCP server returned an unsupported task; use a synchronous tool.",
                );
            }
        };
        let requests = result.input_requests.unwrap_or_default();
        if requests.is_empty() && result.request_state.is_none() {
            return refused("The MCP server returned an empty continuation; reconnect and retry.");
        }
        let mut responses = BTreeMap::new();
        for (key, request) in requests {
            let InputRequest::Elicitation(request) = request else {
                return refused(
                    "The MCP server asked for an unsupported capability; only questions are.",
                );
            };
            let answer = elicit(&tool.server, &tool.questions, request.params).await?;
            let answer = serde_json::to_value(answer)
                .map_err(|e| other(e, "Could not encode the answer."))?;
            responses.insert(key, answer);
        }
        if responses.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(100 * (round as u64 + 1))).await;
        }
        request.input_responses = (!responses.is_empty()).then_some(responses);
        request.request_state = result.request_state;
    }
    refused("The MCP server did not finish within its input rounds; retry when it is ready.")
}

/// Text through the spill, images saved under the session's `images/`, structured content and
/// the server's error flag.
async fn rich(tools: &Tools, at: EntryId, result: CallToolResult) -> Result<Value, Error> {
    let text: Vec<_> = result
        .content
        .iter()
        .filter_map(|b| b.as_text())
        .map(|b| b.text.as_str())
        .collect();
    let mut images = Vec::new();
    for image in result.content.iter().filter_map(|b| b.as_image()) {
        let bytes = base64::engine::general_purpose::STANDARD.decode(&image.data);
        let bytes = bytes.map_err(|e| other(e, "The MCP image was not valid base64."))?;
        let extension = match image.mime_type.as_str() {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            _ => "img",
        };
        let dir = tools.dir.join("images");
        let path = dir.join(format!("mcp-{}-{}.{extension}", at.0, images.len()));
        let saved = async {
            tokio::fs::create_dir_all(&dir).await?;
            tokio::fs::write(&path, bytes).await
        };
        saved
            .await
            .map_err(|e| other(e, "Could not save the MCP image."))?;
        images.push(json!({"path": path, "media_type": image.mime_type}));
    }
    let (output, path) = tools.spill(at, text.join("\n")).await?;
    let (structured, error) = (result.structured_content, result.is_error.unwrap_or(false));
    Ok(
        json!({"output": output, "path": path, "images": images, "structured": structured, "is_error": error}),
    )
}

/// A credential's value: the named variable's, or the file's without its trailing newline.
async fn resolve(reference: &Reference) -> Result<String, Error> {
    match reference {
        Reference::Env(name) => std::env::var(name).map_err(|_| {
            let why = "An MCP credential environment variable is missing; set it and retry.";
            Error::Refused(why.into())
        }),
        Reference::File(path) => super::load(path, "The MCP credential file")
            .await
            .map(|s| s.trim_end_matches(['\r', '\n']).to_owned()),
    }
}
