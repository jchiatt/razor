//! What one model family sees: its declarations, prompt and result wording, and on Responses
//! the native conversation (docs/design/profiles.md).

mod claude;
mod conversation;
mod models;
mod startup;
mod typescript;

pub(crate) use conversation::{Conversation, IMAGE};
pub(crate) use models::{Transport, id, limits, model, models, speed, speed_choice, video};
pub(crate) use startup::{machine, opening, save};

use crate::{config::Replay, protocol::SPILL, tools::Action};
use serde_json::{Map, Value, json};
use std::{fmt, sync::Arc};

const GPT: &str = include_str!("data/gpt.md");
/// The part of the prompt about the record, on every route.
const RECORD: &str = include_str!("data/record.md");
/// The one nudge for a child whose mail grace ends without new input.
pub(crate) const CHILD_NUDGE: &str = include_str!("data/child-nudge.md");
/// A bounded implicit report names the entry that holds its full closing text.
pub(crate) const CHILD_REPORT: &str = include_str!("data/child-report.md");
/// A multi-block report instead names the range containing its full closing text.
pub(crate) const CHILD_REPORT_RANGE: &str = include_str!("data/child-report-range.md");
/// The last line of a child's report when its turn ended in an error.
pub(crate) const CHILD_ERROR: &str = include_str!("data/child-error.md");

/// The tokens an opening of `bytes` (a `startup.md` and a rebuilt context) takes on `transport`,
/// fitted to the first requests of 1,267 recorded openings.
pub(crate) fn tokens(transport: Transport, bytes: u64) -> u64 {
    match transport {
        Transport::Claude => 6_600 + bytes * 5 / 14,
        _ => (bytes * 5 / 19).saturating_sub(3_400),
    }
}

/// Fills MCP's versioned model-facing text without resolving credential references.
pub(crate) fn mcp_text(key: &str, fields: &[(&str, &str)]) -> String {
    static TEXT: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    let data = TEXT
        .get_or_init(|| serde_json::from_str(include_str!("data/mcp.json")).unwrap_or_default());
    fields.iter().fold(
        data[key].as_str().unwrap_or_default().to_owned(),
        |text, (key, value)| text.replace(&format!("{{{key}}}"), value),
    )
}

/// Where one profile's requests go and what rides on each, as config resolved it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Endpoint {
    /// The Responses base the server fixed at startup, or the chat host's address.
    pub(crate) url: String,
    /// The chat host's key variable (`Some("")`: no auth); `None` on Responses, which uses
    /// Razor's sign-in or public API key file.
    pub(crate) key: Option<String>,
    /// The model id the request names: the host's id, or Razor's name on Responses.
    pub(crate) id: String,
    pub(crate) speed_ids: std::collections::BTreeMap<crate::protocol::Speed, String>,
    pub(crate) replay: Replay,
    /// Top-level fields added to every chat request: a host's switch for keeping history.
    pub(crate) request: Map<String, Value>,
}

impl Endpoint {
    /// Whether requests use the public OpenAI host rather than the signed-in backend.
    pub(crate) fn public(&self) -> bool {
        reqwest::Url::parse(&self.url).is_ok_and(|u| u.host_str() == Some("api.openai.com"))
    }

    /// A chat endpoint at Google's OpenAI-compatibility path, `…/v1beta/openai`, beside which the
    /// host keeps its Files API: its model takes video (docs/design/video.md).
    pub(crate) fn video(&self) -> bool {
        self.key.is_some() && self.url.trim_end_matches('/').ends_with("/v1beta/openai")
    }
}

/// One immutable rendering for a family, transport and code-mode choice.
pub(crate) struct Profile {
    /// Names the shipped data, the route and the code-mode choice, and on Claude code mode the
    /// web verbs; a native resume needs the recorded version to equal it. Tools that come and go
    /// (the web verbs elsewhere, MCP and scripted tools) take the addition path instead.
    pub(crate) version: String,
    /// The system prompt: empty from `load`, then the startup list as `opening` read it.
    pub(crate) prompt: String,
    /// The direct declarations, in the transport's own shape.
    pub(crate) tools: Vec<Value>,
    /// Code mode's bindings, shown inside `exec`: every Razor tool there.
    pub(crate) nested: Vec<Declaration>,
    pub(crate) code_mode: bool,
    pub(crate) endpoint: Endpoint,
    transport: Transport,
    wording: Value,
    /// What `razor` gives after its editable part: the family's own text, then how the route's
    /// tools and the record work.
    route: String,
    /// A variant's declarations named apart from their actions: (name, action, renames).
    renames: Vec<(String, String, Map<String, Value>)>,
}

/// A code-mode binding: its name, description and TypeScript input type.
#[derive(Clone)]
pub(crate) struct Declaration {
    pub(crate) name: String,
    description: String,
    input: String,
}

impl Declaration {
    /// The callable JavaScript identifier, normalized exactly as at the Codex pin.
    pub(crate) fn global_name(&self) -> String {
        let name: String = self
            .name
            .chars()
            .enumerate()
            .map(|(i, c)| {
                if c == '_' || c == '$' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
                {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if name.is_empty() { "_".into() } else { name }
    }

    /// The same description and TypeScript signature in exec and ALL_TOOLS.
    pub(crate) fn sample(&self) -> String {
        let argument = if self.name == "apply_patch" {
            "input"
        } else {
            "args"
        };
        format!(
            "{}\n\nexec tool declaration:\n```ts\ndeclare const tools: {{ {}({argument}: {}): Promise<unknown>; }};\n```",
            self.description,
            self.global_name(),
            self.input
        )
    }

    /// A complete Codex code-mode-only tool section, also used for live additions.
    fn section(&self) -> String {
        let name = self.global_name();
        let heading = if name == self.name {
            format!("### `{name}`")
        } else {
            format!("### `{name}` (`{}`)", self.name)
        };
        format!("{heading}\n{}", self.sample().trim())
    }
}

/// A model or call this profile refuses, or shipped data it cannot read.
#[derive(Debug)]
pub(crate) enum Error {
    Invalid(String),
    Other {
        source: Box<dyn std::error::Error + Send + Sync>,
        context: &'static str,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(sentence) => write!(f, "{sentence}."),
            Self::Other { source, context } => write!(f, "Failed {context}: {source}."),
        }
    }
}

impl std::error::Error for Error {}

fn shipped(json: &str) -> Result<Value, Error> {
    serde_json::from_str(json).map_err(|e| Error::Other {
        source: Box::new(e),
        context: "reading shipped profile data",
    })
}

/// Codex's image generation wording, run on the sign-in only and when `image_gen` asks for it
/// (docs/design/responses.md): `tool` is its name and `view` the tool that shows its image.
fn imagegen(tool: &str, view: &str) -> String {
    let text = include_str!("data/imagegen.md").replace("{tool}", tool);
    text.replace("{view}", view)
}

/// Whether `b` is the shipped declaration of `action` that `mode` shows.
fn shared(b: &Value, action: &str, mode: &str) -> bool {
    b["action"] == action && (b["mode"] == "both" || b["mode"] == mode)
}

/// The shipped built-in declarations, with a variant's entries and then, for a child, the
/// child's set over them: an entry sets each of its fields over its action's shared declaration,
/// a field being a path of names joined by `/` whose value replaces what is there whole.
fn builtins(variant: &Value, child: bool, mode: &str) -> Result<Vec<Value>, Error> {
    let Value::Array(mut builtins) = shipped(include_str!("data/tools.json"))? else {
        return Err(Error::Invalid("Shipped tools.json is not a list".into()));
    };
    let reads = shipped(include_str!("data/child-tools.json"))?;
    let (tools, reads) = (variant["tools"].as_array(), reads.as_array());
    for entry in tools.into_iter().chain(reads.filter(|_| child)).flatten() {
        let action = entry["action"].as_str().unwrap_or_default();
        let at = builtins.iter().position(|b| shared(b, action, mode));
        let at = at.unwrap_or_else(|| {
            builtins.push(json!({"mode": mode}));
            builtins.len() - 1
        });
        for (path, value) in entry.as_object().into_iter().flatten() {
            let mut slot = &mut builtins[at];
            for field in path.split('/') {
                if !slot.is_object() {
                    *slot = json!({});
                }
                slot = &mut slot[field];
            }
            *slot = value.clone();
        }
        // An entry's description is its chat wording too unless it names `generic`, as when
        // an entry replaced its declaration whole.
        if entry.get("generic").is_none()
            && let Some(description) = entry.get("description")
        {
            builtins[at]["generic"] = description.clone();
        }
    }
    Ok(builtins)
}

/// Binds at most 256 actions: built-ins to the shipped declarations, scripted and MCP tools to
/// the schema they supplied, beside the maker's own web tools. Code mode is refused on the
/// public API route and on chat. On chat a variant (docs/design/generic.md), config's or else
/// the row's, changes or omits built-in declarations field by field and replaces the route
/// text, and the version names it and the host. A child reads memory and may claim or comment
/// on existing tasks; Codex's image generation is offered only with `image_gen`.
#[allow(clippy::too_many_arguments)] // Each is one choice the declarations depend on.
pub(crate) fn load(
    endpoint: Endpoint,
    name: &str,
    transport: Transport,
    code_mode: bool,
    variant: Option<&Value>,
    actions: Arc<[Action]>,
    child: bool,
    image_gen: bool,
) -> Result<Profile, Error> {
    let (claude, chat) = (
        transport == Transport::Claude,
        matches!(transport, Transport::Chat(_)),
    );
    if model(name).transport != transport || actions.len() > 256 {
        let refusal = "The model route or tool list does not fit this profile";
        return Err(Error::Invalid(refusal.into()));
    }
    let public = endpoint.public();
    if code_mode && (chat || !claude && public) {
        return Err(Error::Invalid(
            "API-key and chat routes support plain tools only".into(),
        ));
    }
    let row = match (variant, transport) {
        (None, Transport::Chat(row)) => shipped(row.variant)?,
        _ => Value::Null,
    };
    let variant = variant.unwrap_or(&row);
    let family = match (claude, code_mode) {
        _ if chat => variant["route"]
            .as_str()
            .unwrap_or(include_str!("data/tools-generic.md"))
            .into(),
        (true, false) => include_str!("data/tools-claude.md").into(),
        (true, true) => include_str!("data/claude-code.md").into(),
        (false, true) => GPT.into(),
        // Plain GPT has no exec, so Codex's lines about it go.
        (false, false) => GPT
            .lines()
            .filter(|l| !l.contains("functions.exec"))
            .collect::<Vec<_>>()
            .join("\n"),
    };
    let handover = if claude && !code_mode {
        "mcp__razor__handover"
    } else {
        "handover"
    };
    let family_name = match (claude, chat) {
        (true, _) => "claude".to_owned(),
        // The host, id, replay and request fields, so a score certifies them.
        (_, true) => format!(
            "generic-{}-{}-{}-{:?}-{}",
            variant["version"].as_str().unwrap_or("plain"),
            endpoint.url,
            endpoint.id,
            endpoint.replay,
            Value::Object(endpoint.request.clone()),
        ),
        _ => "gpt".to_owned(),
    };
    let mode = if code_mode { "code" } else { "plain" };
    // Claude Code keeps the exec declaration it opened with, so there the web verbs nested in
    // it name the version, and turning one on or off rebuilds.
    let on = |verb: &str| u8::from(actions.iter().any(|a| a.name == verb));
    let verbs = format!("-web-{}{}{}", on("search"), on("fetch"), on("answer"));
    let web: &str = if claude && code_mode { &verbs } else { "" };
    let mut profile = Profile {
        version: format!("aa854dab-razor-7-{family_name}-{mode}{web}"),
        prompt: String::new(),
        tools: Vec::new(),
        nested: Vec::new(),
        code_mode,
        endpoint,
        transport,
        wording: shipped(include_str!("data/results.json"))?,
        route: format!(
            "{}\n\n{}",
            family.trim_end(),
            RECORD.replace("{handover}", handover).trim_end()
        ),
        renames: Vec::new(),
    };
    let builtins = builtins(variant, child, mode)?;
    if claude && !code_mode {
        profile.tools = claude::tools(&actions, &builtins)?;
        return Ok(profile);
    }
    for action in actions.iter() {
        let builtin = builtins.iter().find(|b| shared(b, &action.name, mode));
        if builtin.is_some_and(|b| b["omit"] == true) {
            continue;
        }
        // On chat the web verbs say nothing of a native web tool: this route has none.
        let wording = if chat { "generic" } else { "description" };
        let text = |field: &str| builtin.and_then(|b| b[field].as_str()).unwrap_or_default();
        let (name, description, schema) = match (&action.schema, builtin) {
            (Some(own), _) => (&*action.name, &*own.description, &own.input),
            (None, Some(b)) => (
                text("name"),
                b[wording].as_str().unwrap_or(text("description")),
                &b["schema"],
            ),
            (None, None) => continue,
        };
        if chat && (name != action.name || builtin.is_some_and(|b| b["keys"].is_object())) {
            let keys = builtin.and_then(|b| b["keys"].as_object()).cloned();
            let entry = (
                name.into(),
                action.name.to_string(),
                keys.unwrap_or_default(),
            );
            profile.renames.push(entry);
        }
        if code_mode {
            let (name, description) = (name.into(), description.into());
            profile.nested.push(Declaration {
                name,
                description,
                input: typescript::render(schema),
            });
        } else {
            profile
                .tools
                .push(profile.declare(name, description, schema.clone()));
        }
    }
    if chat {
        return Ok(profile);
    }
    if !code_mode {
        // OpenAI's hosted search, on the subscription; Lite carries no hosted tools.
        profile.tools.push(json!({"type": "web_search"}));
        if image_gen && !public {
            let input = shipped(include_str!("data/image-gen.json"))?["input"].take();
            let tool = profile.declare("imagegen", &imagegen("imagegen", "read"), input);
            profile.tools.push(tool);
        }
        return Ok(profile);
    }
    profile.exec(image_gen)
}

impl Profile {
    /// A direct declaration in the transport's own shape.
    fn declare(&self, name: &str, description: &str, schema: Value) -> Value {
        match self.transport {
            Transport::Claude => json!({"name": name, "description": description,
                "inputSchema": schema}),
            Transport::Chat(_) => json!({"type": "function", "function":
                {"name": name, "description": description, "parameters": schema}}),
            Transport::Responses => json!({"type": "function", "name": name,
                "description": description, "parameters": schema, "strict": false}),
        }
    }

    /// Code mode's direct tools, ahead of the rest: `exec`, whose description holds each nested
    /// declaration's section, and `wait`. On GPT, Codex's image generation (only with
    /// `image_gen`) and Lite's search join the nested ones first, each under its namespace's
    /// heading; on Claude, Claude Code's own web tools follow, on the subscription.
    fn exec(mut self, image_gen: bool) -> Result<Self, Error> {
        let claude = self.transport == Transport::Claude;
        let mut description = include_str!("data/exec.md").trim_end().to_owned();
        for nested in &self.nested {
            description += "\n\n";
            description += &nested.section();
        }
        if !claude {
            // Codex's own image generation and search for Lite, run on the sign-in, each under its
            // namespace's heading.
            let namespaced = [
                (
                    "image_gen__imagegen",
                    include_str!("data/image-gen.json"),
                    imagegen("image_gen.imagegen", "view_image"),
                ),
                (
                    "web__run",
                    include_str!("data/web-run.json"),
                    include_str!("data/web-run.md").into(),
                ),
            ];
            // Image generation, the first, only with `image_gen`.
            for (name, data, text) in namespaced.into_iter().skip(usize::from(!image_gen)) {
                let data = shipped(data)?;
                let declaration = Declaration {
                    name: name.into(),
                    description: text,
                    input: typescript::render(&data["input"]),
                };
                let heading = data["heading"].as_str().unwrap_or_default();
                description += &format!("\n\n{heading}\n\n{}", declaration.section());
                self.nested.push(declaration);
            }
        }
        let exec = match claude {
            true => self.declare(
                "exec",
                &description,
                json!({"type": "object",
                "properties": {"code": {"type": "string"}}, "required": ["code"]}),
            ),
            false => json!({"type": "custom", "name": "exec", "description": description, "format":
                {"type": "grammar", "syntax": "lark", "definition": include_str!("data/exec.lark")}}),
        };
        let wait = self.declare(
            "wait",
            include_str!("data/wait.md"),
            json!({"type": "object", "properties": {
            "cell_id": {"type": "string"}, "yield_time_ms": {"type": "number"},
            "max_tokens": {"type": "number"}, "terminate": {"type": "boolean"}},
            "required": ["cell_id"], "additionalProperties": false}),
        );
        self.tools.splice(0..0, [exec, wait]);
        if claude {
            // Claude Code's own web tools, on the subscription.
            self.tools.extend([
                json!({"builtin": "WebSearch"}),
                json!({"builtin": "WebFetch"}),
            ]);
        }
        Ok(self)
    }

    /// Normalizes one call to its action and input without running it. A malformed call is
    /// that call's own error; exec's input is its raw source.
    pub(crate) fn input(&self, name: &str, input: Value) -> Result<(String, Value), Error> {
        let name = (name
            .strip_prefix("functions.")
            .or_else(|| name.strip_prefix("mcp__razor__")))
        .unwrap_or(name);
        if self.code_mode && name == "exec" {
            let source = input.as_str().or_else(|| input["code"].as_str());
            let source =
                source.ok_or_else(|| Error::Invalid("exec needs JavaScript source".into()))?;
            return Ok((name.into(), source.into()));
        }
        let value = match input {
            Value::String(raw) if name == "apply_patch" => json!({"patch": raw}),
            Value::String(raw) => serde_json::from_str(&raw)
                .map_err(|_| Error::Invalid(format!("{name} arguments are not valid JSON")))?,
            value => value,
        };
        if self.transport == Transport::Claude
            && !self.code_mode
            && let Some(mapped) = claude::input(name, &value)
        {
            return mapped;
        }
        if !self
            .tools
            .iter()
            .any(|t| t["name"] == name || t["function"]["name"] == name)
            && !self.nested.iter().any(|d| d.name == name)
        {
            return Err(Error::Invalid(format!("No tool named {name} is available")));
        }
        let Value::Object(mut fields) = value else {
            return Err(Error::Invalid(
                "Function arguments must be a JSON object".into(),
            ));
        };
        if let Some((_, action, keys)) = self.renames.iter().find(|(n, ..)| n == name) {
            for (from, to) in keys {
                if let (Some(v), Some(to)) = (fields.remove(from), to.as_str()) {
                    fields.insert(to.into(), v);
                }
            }
            return Ok((action.clone(), Value::Object(fields)));
        }
        let value = Value::Object(fields);
        let budget = || {
            number(&value, "max_output_tokens", 10_000)
                .map(|n| n.saturating_mul(4).min(SPILL as u64))
        };
        Ok(match name {
            "exec_command" => (
                "bash".into(),
                json!({"command": value["cmd"], "cwd": value.get("workdir"),
                "pty": value.get("tty").unwrap_or(&json!(false)), "open": true, "max_output_chars": budget()?,
                "wait_ms": number(&value, "yield_time_ms", 10_000)?.clamp(250, 30_000)}),
            ),
            "write_stdin" => {
                if value.get("secret_file").is_some() && value.get("chars").is_some() {
                    return Err(Error::Invalid("Use chars or secret_file, not both".into()));
                }
                let poll = value["chars"].as_str().is_none_or(str::is_empty)
                    && value.get("secret_file").is_none();
                let (least, most) = if poll {
                    (5_000, 300_000)
                } else {
                    (250, 30_000)
                };
                let timeout = number(&value, "yield_time_ms", least)?.clamp(least, most);
                let mut mapped = json!({"job": value["session_id"], "timeout_ms": timeout, "max_output_chars": budget()?});
                match value.get("secret_file") {
                    Some(path) => mapped["secret_file"] = path.clone(),
                    None => mapped["text"] = value.get("chars").cloned().unwrap_or(json!("")),
                }
                ("send".into(), mapped)
            }
            "view_image" => ("read".into(), value),
            "image_gen__imagegen" => ("imagegen".into(), value),
            _ => (name.into(), value),
        })
    }

    /// The model-facing result and whether the call failed. Results stay the action's JSON except
    /// Claude's direct shell calls in plain mode, which get Claude Code's result shapes: an ended
    /// job's output inline, or its end in the spill wording when the output spilled past `SPILL`.
    pub(crate) fn result(
        &self,
        name: &str,
        input: &Value,
        output: Value,
        nested: bool,
    ) -> (Value, bool) {
        let name = name.strip_prefix("mcp__razor__").unwrap_or(name);
        let (end, kill) = (&output["end"], matches!(name, "TaskStop" | "kill"));
        let failed = match kill {
            true => end.is_null(),
            false => end == "unknown" || (end.is_object() && end["exit"] != 0),
        };
        let error = output["is_error"] == true || failed;
        let shell = kill || matches!(name, "Bash" | "bash" | "terminal" | "monitor");
        if nested || self.transport != Transport::Claude || self.code_mode || !shell {
            return (output, error);
        }
        let job = output["job"].to_string();
        let path = output["path"].as_str().unwrap_or("");
        let raw = output["output"].as_str().unwrap_or("");
        let text = match path.is_empty() || end.is_null() {
            true => raw.to_owned(),
            false => {
                let skip = raw.chars().count().saturating_sub(2000);
                let tail: String = raw.chars().skip(skip).collect();
                self.say("spill", &[("path", path), ("text", &tail)])
            }
        };
        let status = self.status(&output);
        let rendered = match name {
            "TaskStop" | "kill" => self.say("kill", &[("job", &job)]),
            "terminal" => {
                let at = output["cursor"].as_array().filter(|c| c.len() == 2);
                let cursor = at.map(|c| {
                    self.say(
                        "cursor",
                        &[("row", &c[0].to_string()), ("column", &c[1].to_string())],
                    )
                });
                let cursor = cursor.unwrap_or_default();
                self.say(
                    "terminal",
                    &[
                        ("job", &job),
                        ("status", &status),
                        ("cursor", &cursor),
                        ("text", &text),
                    ],
                )
            }
            _ if end.is_null() => {
                let template = if input["run_in_background"] == true || name == "monitor" {
                    "background"
                } else {
                    "timeout"
                };
                let seconds =
                    (input["timeout"].as_u64().unwrap_or(120_000).min(600_000) / 1000).to_string();
                let status = self.say(
                    template,
                    &[("job", &job), ("path", path), ("seconds", &seconds)],
                );
                format!("{status}\n{text}")
            }
            _ => format!("{status}\n{text}"),
        };
        (Value::String(rendered), error)
    }

    /// An image generation's result: the PNG's path, Codex's hint naming it, which code mode's
    /// `generatedImage()` writes, and the image to show the model when a request can hold it.
    pub(crate) fn generated(&self, path: &std::path::Path, size: u64) -> Value {
        let small = size <= crate::protocol::IMAGE_BYTES as u64;
        let dir = path.parent().unwrap_or(path).to_string_lossy();
        let image = json!({"path": path, "media_type": "image/png"});
        let (key, images) = match small {
            true => ("generated", json!([image])),
            false => ("generated_large", json!([])),
        };
        let hint = self.say(key, &[("dir", &dir), ("path", &path.to_string_lossy())]);
        json!({"path": path, "images": images, "output_hint": hint})
    }

    /// A job's end notice, delivered as input, from its call's input and
    /// the controller's `ended` event, `path` and `call_id`; when the person's stop ended the job,
    /// `stopped_ms` after its call, it says so, with the job's last lines.
    pub(crate) fn notice(&self, _name: &str, input: &Value, output: Value) -> String {
        let escape = |s: &str| {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        };
        let description = input["description"]
            .as_str()
            .or_else(|| input["command"].as_str());
        let ended = &output["ended"];
        let finished = if ended["end"]["exit"] == 0 {
            "completed"
        } else {
            "failed"
        };
        let notice = self.say(
            if self.transport == Transport::Claude {
                "notice"
            } else {
                "plain_notice"
            },
            &[
                ("job", &ended["job"].to_string()),
                ("path", &escape(output["path"].as_str().unwrap_or(""))),
                ("call", &escape(output["call_id"].as_str().unwrap_or(""))),
                ("description", &escape(description.unwrap_or(""))),
                ("status", &self.say(finished, &[])),
                ("end", &self.status(ended)),
            ],
        );
        let Some(ms) = output["stopped_ms"].as_u64() else {
            return notice;
        };
        let seconds = ms.div_ceil(1000).to_string();
        let tail = ended["tail"].as_array().into_iter().flatten();
        let tail: Vec<_> = tail.filter_map(Value::as_str).collect();
        let last = match tail.is_empty() {
            true => self.say("stopped_silent", &[]),
            false => self.say("stopped_tail", &[("tail", &tail.join("\n"))]),
        };
        let stopped = self.say("stopped", &[("seconds", &seconds)]);
        format!("{notice}\n{stopped} {last}")
    }

    fn status(&self, output: &Value) -> String {
        let end = &output["end"];
        match (end["exit"].as_i64(), end["signal"].as_i64()) {
            _ if end.is_null() => self.say("running", &[]),
            (Some(code), _) => self.say("exit", &[("code", &code.to_string())]),
            (None, Some(code)) => self.say("signal", &[("code", &code.to_string())]),
            (None, None) => self.say("unknown", &[]),
        }
    }

    /// Fills one shipped wording template, field by field in the order given.
    fn say(&self, key: &str, fields: &[(&str, &str)]) -> String {
        let template = self.wording[key].as_str().unwrap_or_default().to_owned();
        fields.iter().fold(template, |text, (field, value)| {
            text.replace(&format!("{{{field}}}"), value)
        })
    }
}

fn number(input: &Value, key: &str, default: u64) -> Result<u64, Error> {
    match input.get(key).filter(|v| !v.is_null()) {
        None => Ok(default),
        Some(v) => v
            .as_u64()
            .ok_or_else(|| Error::Invalid(format!("{key} must be a non-negative integer"))),
    }
}
