# Project settings and MCP

Device sign-in/remote skills: deferred (Trey, 2026-10-28).

## Public surface

```rust
/// Complete typed values and winning files; defaults have no source. Each file keeps its
/// last good table and a stat stamp; clones are independent (the controller's machine files,
/// each session's copy with its project's two).
pub struct Settings { pub config: Config, pub project: Option<PathBuf>,
    pub sources: BTreeMap<String, PathBuf>, pub errors: Vec<String>, /* private files */ }
impl Settings {
    /// Refuses invalid machine files at startup; missing optional files are empty.
    pub fn open(home: PathBuf) -> Result<Self, config::Error>;
    /// Reads linked-worktree settings at main, retaining the local root for execution.
    pub fn project(&self, cwd: &Path) -> Result<Self, config::Error>;
    /// Stats each file and reads only changed ones (a file changed within 100 ms of its read
    /// is read again); an invalid edit keeps that file's last good table and reports its error.
    /// True when a table changed.
    pub fn read(&mut self) -> bool;
}
/// Machine-wide sign-ins, credential rotation and reconnect requests.
impl Auth {
    /// Every loaded session's connection to `name`, or all, starts over.
    pub fn reconnect(&self, name: Option<&str>);
    /// Ties `name`'s pending sign-in to the question showing its address in a session.
    pub async fn asked(&self, name: &str, question: (SessionId, EntryId));
    /// The question a landed address's pending sign-in was asked in; none when no session asked.
    pub async fn question(&self, redirect: &str) -> Option<(SessionId, EntryId)>;
}
/// Enabled defaults true; only disabled entries may omit a connection.
pub struct Mcp { pub enabled: bool, pub search: Option<bool>, pub timeout: Option<u64>,
    pub tools: Option<BTreeSet<String>>, pub connection: Option<Connection> }
/// Stdio keeps flat TOML and literal env; HTTP preserves the complete URL, query included.
pub enum Connection {
    Stdio { command: String, args: Vec<String>, env: BTreeMap<String, String> },
    Http { url: String, headers: BTreeMap<String, Reference>, oauth: Option<OAuth> },
}
/// Resolved at use, never into settings or operation output.
pub enum Reference { Env(String), File(PathBuf) }
/// A secret requires its client ID and expected issuer; discovery chooses scopes.
pub struct OAuth { pub issuer: Option<String>, pub client_id: Option<String>,
    pub client_secret: Option<Reference> }
/// Writable files exclude the pack; these and the list row belong to protocol/api.rs.
pub enum McpScope { Global, Project, Private }
pub enum McpOperation { List, Add { name, scope, config }, Remove { name, scope },
    Enable { name, scope }, Disable { name, scope }, Reconnect { name },
    SignIn { name }, SignOut { name }, Import { source, scope } }
impl Controller {
    /// Never waits on the calling shell's action lock; plain shells need no session.
    pub async fn mcp(&self, cwd: &Path, session: Option<&SessionId>, op: McpOperation)
        -> Result<Value, controller::Error>;
    /// Exposes values, sources and errors, never resolved credentials.
    pub async fn settings(&self, session: &SessionId) -> Result<Settings, controller::Error>;
    /// The callback's landed address: a sign-in a session asked for is answered in its question
    /// there, as a pasted address is; any other completes here. Either consumes the same
    /// state-bound, single-use attempt.
    pub async fn mcp_complete(&self, redirect: &str) -> Result<(), controller::Error>;
}
// `Controller::mcp` records a sign-in's question in the asking session and ties it to the
// attempt before it returns the address, so no callback can arrive first
// (controller/questions.rs records it, and a server's questions).
```

## Ten lines of data flow

1. Find project roots once; children load settings after their checkout exists.
2. Merge pack, person, project and private files; each read is at most 1 MiB.
3. Values override individually, named entries whole; relative references use their defining file.
4. Refuse project machine keys, including plugins; expose winning files and independent errors.
5. Add always writes; disable can hide inheritance; removing that marker enables inheritance.
6. External edits reconcile at turn start; a call loads one MCP revision, and a change reconciles before it runs.
7. Each server's task connects, lists, and starts over on failure or reconnect; task identity rejects a replaced task's publications.
8. Filter allow lists first; failed listing fails the server; reconnect restores it.
9. Keep 256 offered actions, 16 MiB catalogs, 100 pages/server; spill text and file images.
10. Announce availability before the next result after the registry changed; found tools survive rebuild and reconnect.

## Decisions and costs

**Files.** `config/{pack,mcp,manage}.rs` own reading, comment-preserving edits/import previews and dispatch/questions. Imports exclude credentials. `RAZOR_SESSION` supplies state/questions; bash alone records operations.

**Search.** Auto-search above 8 KiB/server; `search: Option<bool>` overrides. Rank case-folded matches by exact name, name words, description words, then names. `run` returns five names/one-line descriptions; declarations append before the next request. No schemas, server parameter or prefix limit. Claude re-lists; exec shares rendering. Wording: `profiles/data/{mcp,tools}.json`; purpose: instructions' first sentence or title/name.

**Protocol.** `tools/mcp.rs` and `mcp/{config,auth,http,elicitation}.rs` use rmcp Auto for 2026-07-28 and older revisions. No modern subscriptions; legacy list changes remain. Peer loops over InputRequired, sharing form/URL elicitation. One validated form, one person question. Structured results/images retain failures. Each tool call waits at most `timeout` ms (ten minutes by default), set on `[mcp.<name>]` and included in listing, settings and import; changing it restarts the server like any other key. At the limit rmcp cancels the request; Razor returns an unknown-outcome error without retrying.

**Sign-in.** Authenticated `POST /v1/mcp`, authenticated session settings, and fixed `GET /v1/mcp/callback`, authorized by state. Register HTTPS when configured, otherwise loopback; paste completes either. Configured client precedes dynamic registration. Check S256/issuer/redirect/state. One pending attempt/server; 0600 credentials keyed by name/full URL. Refresh saves under sign-out's lock despite dropped waiters. Pasted answers exchange before normalized success is recorded.

Dependencies: rmcp's `auth` and `transport-streamable-http-client-reqwest` replace protocol/OAuth code, with one HTTP client type (an `AuthClient` with an empty manager before sign-in); `async-trait` implements rmcp's `#[async_trait]` credential store; `toml_edit` preserves comments. A form's answer must be a JSON object: the 2026-07-28 specification says a client SHOULD validate it and the server validates its own, so `jsonschema` and rmcp's server-only `elicitation` feature are gone.

Commands/stdio don't inherit web keys; Exa's header reads Razor's environment:

```toml
[mcp.exa]
url = "https://mcp.exa.ai/mcp"
[mcp.exa.headers]
x-api-key = { env = "EXA_API_KEY" }
```

## Razor's keys in `_meta`, and MCP Apps groundwork, 29 October

Decided the same day: the `razor/` prefix, Razor's keys on `tools/call` only, and the filter without `_meta.ui`. An MCP server learns from these keys which Razor home (which server's socket) and which session called, so that `wait` with `notify` can post its event into that session later. This section adds no route, no operation and no config key.

```rust
// src/tools/mcp.rs
/// MCP Apps: a listed tool is the model's unless its `_meta.ui.visibility` is a list without
/// "model"; absent (or null), it is `["model", "app"]`.
fn for_model(tool: &rmcp::model::Tool) -> bool;

/// Razor's `_meta` on each `tools/call`: `razor/home`, its home, absolute, and `razor/session`,
/// the calling session's id, both from the session's directory `<home>/sessions/<id>`
/// (`controller/mod.rs:444`, the derivation `Tools::open` already uses for `RAZOR_SESSION`).
fn meta(tools: &Tools) -> RequestMetaObject;
```

`Tools::open`'s signature does not change, so the session controller (2,485 of 2,500) is untouched.

**Data flow.**

1. rmcp's lifecycle writes `server/discover` (2026-07-28) or `initialize` (older revisions) straight to the transport, with its own keys only (`service/client.rs:1045`, `:964`). Razor adds nothing there, and no wrapper.
2. Each `tools/list` page goes out as today, with nothing of Razor's: nothing reads a home on a listing.
3. `catalog()` drops every tool `for_model` refuses, before the allow list, the size choice for search, the byte count and the opening section's count. So an app-only tool is in no route's list, no code-mode `tools` object, no `mcp_search` result and no count, and a call by its name is refused as unknown.
4. `call` sets `params.meta = meta(tools)` once, before its rounds. Each `call_tool_once` round, the first and every input round, sends a clone, so each self-contained request carries both keys, as 2026-07-28's statelessness requires.
5. rmcp adds `io.modelcontextprotocol/protocolVersion`, `clientInfo`, `clientCapabilities` and `progressToken` over Razor's keys. They share no key, so all of them go out.

**What the 2026-07-28 specification allows.** From https://modelcontextprotocol.io/specification/2026-07-28/basic/index, "General fields", `_meta`:

> Prefix: If specified, MUST be a series of labels separated by dots (`.`), followed by a slash (`/`). Labels MUST start with a letter and end with a letter or digit; interior characters can be letters, digits, or hyphens (`-`). Implementations SHOULD use reverse DNS notation (e.g., `com.example/` rather than `example.com/`). Any prefix where the second label is `modelcontextprotocol` or `mcp` is reserved for MCP use.
>
> Name: Unless empty, MUST begin and end with an alphanumeric character (`[a-z0-9A-Z]`). MAY contain hyphens (`-`), underscores (`_`), dots (`.`), and alphanumerics in between.
>
> Official extensions define additional `_meta` keys under the `io.modelcontextprotocol/` prefix, and third-party extensions use their own vendor prefix.

The keys this revision reserves are `progressToken`, `io.modelcontextprotocol/{protocolVersion,clientInfo,clientCapabilities,logLevel,subscriptionId}`, and `traceparent`, `tracestate` and `baggage`. None collides with Razor's.

**rmcp 3.8's way to set `_meta` per request.** There are two ways.

1. The typed params' own field, `meta: Option<RequestMetaObject>`: `CallToolRequestParams` (`model.rs:4411`) and `PaginatedRequestParams` (`model.rs:1761`). Serialization merges it as the base, under the envelope's own `_meta`, which wins a conflict (`model/serde_impl.rs:75–79`).
2. `PeerRequestOptions::with_meta` through `Peer::send_request_with_option` (`service.rs:783`). It is merged after the lifecycle's keys, so it would win (`service.rs:871–879`).

`call_tool_once` takes params only, so Razor uses the first, and today's call keeps its shape. `RequestMetaObject` deserializes from the JSON object Razor builds.

**The hard decision: the prefix.** The keys are written once, in `meta`, so the choice costs a line now and a change in any client that reads it.

| Prefix | Meets the SHOULD | Owned by Razor | Cost |
| --- | --- | --- | --- |
| `razor/` | no: one label | no | The shape vendors use today (`anthropic/maxResultSizeChars`; OpenAI's `openai/outputTemplate`). Shortest. "razor" names other products (ASP.NET Razor). |
| a domain named for Razor, reversed (`dev.<domain>/`, `com.<domain>/`) | yes | only once Trey names one | None on record. |

**Chosen: `razor/`**, keys `razor/home` and `razor/session`. One label meets the specification's MUST; only the reverse-DNS SHOULD is unmet. Razor is white-label, so a deployer's domain does not belong on the wire, and Razor owns no domain to reverse. The reason sits beside `meta` in one sentence.

**The hard decision: which requests carry a session.** Every connection belongs to one session's `Tools` (`controller/session.rs:224` is the only caller), so every request has an owning session. Only a `tools/call` is made by a session's turn.

| Request | Who makes it | Razor's keys |
| --- | --- | --- |
| `server/discover` or `initialize` | rmcp's lifecycle, at connect | none: rmcp writes it to the transport itself |
| `tools/list`, each page | the connection's task, at connect and after a legacy change notice | none: nothing reads a home on a listing (RULES 19) |
| `tools/call` and each of its input rounds | the session's turn: a native call, a code-mode cell's nested call, a child's own call (its own id) | `home` and `session` |
| `notifications/cancelled` and `notifications/initialized`; answers to the server's requests (elicitation, ping) | rmcp | none: these are notifications and results, not requests |

Alternatives were home on listings too, both keys on listings, or a transport wrapper stamping every request. Only `tools/call` and its input rounds carry Razor's keys, both of them; listings, `server/discover` and `initialize` carry nothing of Razor's, so `Link` keeps no `meta` and `start` is unchanged.

**MCP Apps: the fields, from their source.** The current stable revision is 2026-01-26, https://github.com/modelcontextprotocol/ext-apps/blob/main/specification/2026-01-26/apps.mdx, "Resource Discovery". The draft has the same fields.

- `_meta.ui.resourceUri` (string, optional): the `ui://` resource that renders the tool's results.
- `_meta.ui.visibility` (`Array<"model" | "app">`, optional): "`visibility` defaults to `["model", "app"]` if omitted".
- "Host MUST NOT include tools in the agent's tool list when their visibility does not include `"model"`". `"app"` means "callable by the app from the same server connection only".
- The flat `_meta["ui/resourceUri"]` is deprecated and "will be removed before GA". Razor does not read it.
- rmcp's `Tool` carries `_meta` untyped as `meta: Option<MetaObject>` (`model/tool.rs:38–39`), so `for_model` reads `meta["ui"]["visibility"]`.

**Keeping `_meta.ui` itself: not yet.** Keeping each tool's resource and visibility on the catalog would be unread until a client shows the view, and the gate runs `cargo clippy --all-targets -- -D warnings` (`scripts/gate.sh:9`), which refuses a field never read. RULES 19 says the same. So the filter is built alone, and the change that shows views adds `ui: Option<Ui { resource: Option<String>, app: bool }>` to `Tool` with its reader; no `#[allow(dead_code)]` is written. App-only tools are dropped, not kept: a view's calls have no path yet, and keeping them would mean a second list beside the model's.

**Budget, flow.** Flow 37 and its stand-in grow by 5 lines (flows 5,083 of 8,000):
- The stand-in (`flows/mcp.py`) also lists `view`, with `_meta: {ui: {resourceUri: "ui://flow/view", visibility: ["app"]}}`.
- `native_case` adds `view` to its allow list, so only visibility can keep it out. On both routes (Claude native, GPT code mode) it then asserts:
  - the connected row still has 3 tools (an existing assertion);
  - `mcp__remote__view` is in no model request body and no search result;
  - every `tools/call` body, input rounds included, carries `razor/home` equal to `f.home / ".razor"` and `razor/session` equal to the session.

## A sign-in finished at the callback (29 October)

A sign-in started in a session records a question showing its address (`Controller::mcp`, through `controller/questions.rs`), and only an answer closes it. A pasted address answers it through the session's answer path, which completes the sign-in first (`controller/session/inputs.rs:123`) so the code never enters the record. The callback (`GET /v1/mcp/callback`, and `razor mcp complete`, which calls it) completes the sign-in and answers nothing, so the band keeps a dead ask. The question's task also waits on the session's stream for an answer nobody reads, for as long as the question stays open: after a callback, for good.

**The fix.** The callback answers the question with the landed address, through the same answer path a paste takes:

1. `Controller::mcp` with a session records the question, tells residents as of any ask, and calls `Auth::asked` to tie it to the pending attempt, all before the address leaves Razor. It no longer waits for an answer; the wait stays only where an answer is read, a server's own questions (`questions`).
2. `Controller::mcp_complete`, the callback's, asks `Auth::question` for the tie by the address's state. With one, it answers that question with the address (`Controller::answer`): the session completes the sign-in, records `MCP sign-in completed.` as the answer and starts no agent turn, exactly as for a paste. Without one (the `/mcp` panel, `razor mcp sign-in` with no session) it completes as today.
3. The session's answer path calls `Auth::complete` itself, not `mcp_complete`, which would route the same question back to it.

The dispatch and the direct call are the one change in this batch that must enter the controller: the attempts live in the controller's `Auth`, and only the session's answer path may record a sign-in answer.

**Who closes the question**, by what each costs:

- *The callback answers it with the address* (chosen). One path finishes every sign-in a session asked for, and the code is consumed before anything is recorded, as now.
- *The waiting task closes it* (an alternative: `Auth::complete` names the completed server; the question's task closes its question). The task would record a sign-in answer without completing, a second way to answer that the session's answer path must not repeat; a paste would have two writers racing to close one question, settled by "already answered" refusals; and a task per open sign-in would keep listening.
- *The callback searches for an open sign-in question with its state.* No index of questions across sessions exists, and searching loaded sessions misses one that was unloaded.

One case stays as it is: a callback whose asking session has since been deleted fails at the answer, and the person signs in again.

Flow 37 signs in from a session, completes through the callback, and checks that the question's answer is in the record, the code is not, and no agent turn ran.
