# The Claude slice

The slice is one thin path through the session controller (`src/controller/`), the Claude path (the TypeScript bridge `bridge/` and its Rust half `src/claude/`), the API server (`src/api/`) and `--print` (`src/main.rs`). It is proven against the real Claude Agent SDK and a signed-in account, over the record and the types of `protocol.md`.

The SDK was read at **0.3.312** (`claudeCodeVersion` 2.1.312), installed fresh on 2026-10-27. Line numbers below are in that package's `sdk.d.ts`. Probes A to F ran the real SDK and its bundled CLI against a scripted local Messages endpoint (no model, no sign-in); their scripts are scratch files outside the repository, and what each showed is in the verification table.

## Surface

### The session controller: `src/controller/`

```rust
/// Every session this server has loaded. Cheap to clone; any task may call any method.
#[derive(Clone)]
pub struct Controller { /* Arc: home, config, the bridge once started, each loaded session's task */ }

impl Controller {
    /// Start on the state directory `home`. Reads the config and loads every session
    /// `record::live` names. In each one: a call with no result gets a `Result` with outcome
    /// `Unknown` and is never run again; a job the shell cannot reattach gets `Ended { Unknown }`;
    /// and the session, live with a turn open or a job running, is rebuilt (`Trigger::Restart`),
    /// its open turn going on in it. An input admitted meanwhile is sent after that rebuild.
    pub async fn start(home: PathBuf) -> Result<Controller, Error>;

    /// Start a session in `cwd` and admit its first input from the person. Returns the
    /// session and the input's entry.
    pub async fn create(&self, cwd: PathBuf, model: Option<String>, effort: Option<Effort>, text: String)
        -> Result<(SessionId, EntryId), Error>;

    /// The admission path: every input to every session enters here, loading the session if it
    /// is not loaded. The input is appended, then sent to the model at once: an idle session
    /// starts a turn with it, and a working one takes it at its next tool boundary, or as its
    /// next turn if the current one ends first. Inputs keep their arrival order. One admitted
    /// during a rebuild is sent after the rebuilt context. At most 64 inputs wait unanswered;
    /// past that, a refusal with a sentence.
    pub async fn admit(&self, session: &SessionId, input: Input) -> Result<EntryId, Error>;

    /// Escape: kill the current turn's foreground job and interrupt the model. The turn ends
    /// `Stopped`, and inputs waiting for it open the next turn. Children, and jobs that
    /// outlived their call, keep running. A session that is not working is left as it is.
    pub async fn stop(&self, session: &SessionId) -> Result<(), Error>;

    /// Attach to a session's stream: `Live::Attached`, then every entry after `after`, then
    /// entries and live events as they happen, each once and in record order, as lines ready to
    /// send. A client that falls 1,024 lines behind is attached again from its cursor, so it
    /// gets `Attached`, with the state now, and then what it missed. Attaching loads the
    /// session if needed; loading never starts a CLI process.
    pub async fn attach(&self, session: &SessionId, after: EntryId) -> Result<mpsc::Receiver<Arc<str>>, Error>;

    /// Sessions newest first (`record::list`).
    pub async fn list(&self, cwd: Option<PathBuf>, limit: usize) -> Result<Vec<Listed>, Error>;
}

pub struct Listed { pub session: SessionId, pub cwd: PathBuf, pub model: String }

pub enum Error {
    /// No session with this id.
    NoSession(SessionId),
    /// The session already holds 64 inputs it has not answered.
    Full(SessionId),
    /// The config file does not parse or cannot be read; the sentence names it.
    Config(String),
    Record(record::Error),
    Bridge(claude::Error),
}
```

**Inside.**
- Each loaded session has one task (`session.rs`). It owns the session's `Record`, its open conversation, its turn's foreground job, and a broadcast of its lines (1,024). The methods above reach that task through a channel of 256 commands. A loaded session stays loaded; what `sessions.idle` limits is CLI processes.
- A command that fails closes the conversation, so no call waits on an admission or an answer that will not come, and ends the open turn with the failure. A full session's refusal is not a failure: it changes nothing.
- The bridge's reader task routes each line to its session's task.
- The slice's actions:
  - `bash` runs a foreground command in its own process group, with output spooled under the session's directory, and kills the group on Escape. It is about 50 lines, and the shell module replaces it.
  - `recall` is `Record::get`.
  - `handover` marks the rebuild.
- At most `sessions.idle` idle sessions keep their CLI process. Past that, the one idle longest has its conversation closed; its next input resumes it (the hard decision).

**A turn on the Claude path.**
- **Opening a conversation.** When an input is to be sent and the session has none open: a new session opens a fresh one; a session whose last turn ended `done` resumes its own SDK conversation, cut at the record's latest native marker; any other session rebuilds (`Restart`) first.
- **Inputs.** Each input goes to the SDK with a uuid made from its entry id (`00000000-0000-4000-8000-`, then the id in 12 hex digits). The SDK names the uuids a turn has taken on its frames and on its result (`user_message_uuids`), and the bridge turns them back into entry ids.
- **Calls.** The bridge mirrors each `tool_use` block as a `Call` entry. The SDK's `PreToolUse` callback hook holds that call until Razor has appended the entry and answered `Admitted`; only then does any tool run, Claude Code's own or Razor's. A Razor action (`Run`) is answered with its output, and the controller does not append it: its `Result`, like every other, is mirrored from the SDK's `tool_result`, which is what the model saw. Each kind of entry thus has one way into the record.
- **Ending.** Every SDK result message is one `TurnEnd`. It answers every input sent up to the latest one the turn took, by the order they were sent; the SDK takes inputs in that order. When the turn names none, it answers none. (Since 30 October `took` comes from the CLI's `command_lifecycle` frames, not `user_message_uuids`, which stop at a turn's first 64 inputs: controller.md, "Admission counts what the model has not read".) A call of the turn still without a result gets `Unknown` first. The end is `done` for a success, `stopped` when `terminal_reason` is `aborted_tools` or `aborted_streaming`, and otherwise an error sentence. The session is needs-you once nothing is unanswered; while anything is, the SDK runs it as the next turn by itself.
- **Stop.** The controller kills the foreground job's process group and calls `interrupt()`. The SDK ends the turn `aborted_tools` or `aborted_streaming`, writes its own `tool_result` for the interrupted call, and runs any input already sent to it as the next turn (probe B).
- **The threshold.** It is crossed when a request's context (input plus cache reads and writes) reaches `rebuild.threshold` times the model's window; the window is known from the first turn's end on. The controller then admits `record::THRESHOLD` once per conversation, as a `Razor` input.
- **The rebuild.**
  - A `handover` call only marks it. The bridge's `PostToolBatch` hook runs once every call of the batch has its result and before the next request (lines 2697-2702). When the batch held `handover`, the hook answers `continue: false`: the SDK ends that turn with `terminal_reason` `hook_stopped` and makes no further request (probe D). The bridge reports `Settled` in place of that result and closes the conversation.
  - A turn that took the threshold's request and ended without calling `handover` is rebuilt after its `TurnEnd`; the controller closes the conversation.
  - Either way the controller then appends `Rebuild` from `Record::rebuild`, opens a fresh conversation, and sends the context as its first input, under the `Rebuild` entry's uuid. The turn goes on in it. The context holds the person's latest asks verbatim, so the turn that takes it answers every input before it, and an input the old conversation never took is not sent again.
  - Inputs admitted while this runs wait in the session task's channel and go after the context.
  - If the context does not fit, the fresh conversation's first request ends `prompt_too_long`, and the turn ends with the error "The rebuilt context is N bytes and does not fit the model's context window." The session then waits for the person; nothing is dropped and nothing is rebuilt again.
- **A conversation that ends by itself** (its CLI exited, or the bridge did): a resume that failed is rebuilt (`Restart`) and the turn goes on; any other ends its open turn with the error, after `Unknown` results for its calls.

**Config.** The slice reads `$RAZOR_HOME/config.toml` (`RAZOR_HOME` defaults to `~/.razor`). It re-reads the file when its modification time changes, checked as each turn opens. The reader lives in `src/controller/config.rs` until configuration gets its own module.

| key | default | read by |
| --- | --- | --- |
| `model` | `"opus"` | `create` |
| `effort` | `"high"` | `create` |
| `rebuild.threshold` | `0.8` | the session task, on each context report |
| `sessions.idle` | `4`: a closed CLI costs the next message about half a second, a warm idle one about 167 MB (the proof, step 6) | the controller, after each turn ends |
| `claude.bridge` | the source tree's `bridge/` (until there is an install layout) | the bridge's start; a change applies when the bridge next starts |
| `server.address` | `"127.0.0.1:0"` | `razor serve` at start; a change says it needs a restart |

### The Claude path: `src/claude/` and `bridge/`

```rust
/// The bridge: one `node <claude.bridge>/main.ts` for the whole server, with every Claude
/// session multiplexed over its standard input and output, one JSON line per message. It is
/// started on first use. If it exits, every open conversation is reported `Closed`, and the
/// next use starts another. Each direction holds at most 64 MiB of lines: a sender waits for
/// room, and a line over 64 MiB stops the bridge with an error.
#[derive(Clone)]
pub struct Bridge { /* the child, the writer task's queue, the two byte budgets */ }

impl Bridge {
    /// Start the bridge from `dir`. Its lines arrive on the returned receiver, each with the
    /// share of the budget it holds; dropping that returns the line's bytes.
    pub async fn start(dir: &Path) -> Result<(Bridge, Lines), Error>;
    /// Queue one message for the bridge, waiting while 64 MiB are already queued.
    pub async fn send(&self, message: &ToBridge) -> Result<(), Error>;
}
pub type Lines = mpsc::UnboundedReceiver<(Line, OwnedSemaphorePermit)>;

#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToBridge {
    /// Open the session's native conversation, numbered `open`: fresh, or resumed from the
    /// SDK's session file up to and including the message `resume.at`. It replaces any open one.
    Open { session: SessionId, open: u64, cwd: PathBuf, model: String, effort: Effort, resume: Option<Resume> },
    /// An input for the open conversation, under the uuid made from `entry`.
    Input { session: SessionId, entry: EntryId, text: String },
    /// The `Call` for this call is in the record; its tool may run.
    Admitted { session: SessionId, call: CallId },
    /// The answer to a `Run`.
    Result { session: SessionId, call: CallId, output: String, error: bool },
    /// Interrupt the running turn.
    Stop { session: SessionId },
    /// End the conversation and its CLI process. Any of its lines that follow are dropped.
    Close { session: SessionId },
}
pub struct Resume { pub conversation: String, pub at: String }

/// One line from the bridge: the session, the `Open` whose conversation it is about, and what
/// it says. The controller drops the lines of a conversation it has closed.
pub struct Line { pub session: SessionId, pub open: u64, #[serde(flatten)] pub says: FromBridge }

#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromBridge {
    /// An entry mirrored from an SDK message: `Text`, `Thinking`, `Call`, or `Result`.
    Entry { body: Body, native: Native },
    /// Text as it streams. Never recorded.
    Delta { text: String },
    /// One of Razor's actions was called (`bash`, `recall`, `handover`). Answer with `Result`.
    Run { call: CallId, action: String, input: serde_json::Value },
    /// The context size of the request just made: input plus cache reads and writes.
    Context { tokens: u64 },
    /// An SDK turn ended (its result message). `success`: subtype `success` and not `is_error`;
    /// `reason`: its `terminal_reason`; `message`: the SDK's own words when it failed;
    /// `window`: the model's context window; `took`: the inputs it took, as entries.
    TurnEnd { success: bool, reason: Option<String>, message: String, usage: Usage, window: u64,
              took: Vec<EntryId> },
    /// A batch holding `handover` settled, and the conversation is closed.
    Settled,
    /// The conversation ended by itself: its CLI exited. `error` says why when it failed.
    Closed { error: Option<String> },
}

pub enum Error {
    /// Node, or the bridge directory, is missing. The sentence says what to install or set.
    Missing(String),
    /// The bridge exited.
    Exited,
    /// A message longer than the bridge's 64 MiB budget.
    TooLong(usize),
    Io { doing: &'static str, source: std::io::Error },
}
```

The bridge (`bridge/main.ts`; `bridge/prompt.md`; `bridge/tools.ts` with the three declarations and the bridge's own refusals; `bridge/package.json` and its lock pinning the SDK at exactly 0.3.312) makes one streaming `query` per open session:

```ts
query({ prompt: inbox, options: {           // inbox: the session's queue of user messages
  cwd, model, effort,
  systemPrompt: prompt,                      // bridge/prompt.md, under 2,000 tokens
  tools: ['Read', 'Edit', 'Write', 'Grep', 'Glob', 'Bash'],
  toolAliases: { Bash: 'mcp__razor__bash' }, // the model's Bash runs Razor's action
  mcpServers: { razor: createSdkMcpServer({ name: 'razor', alwaysLoad: true,
                                            tools: [bash, recall, handover] }) },
  hooks: { PreToolUse: [{ hooks: [admit] }],  // hold each call until Razor has appended it
           PostToolBatch: [{ hooks: [settle] }] }, // stop after a batch that held handover
  settingSources: [],
  settings: { autoCompactEnabled: false, precomputeCompactionEnabled: false, idleCompaction: false },
  permissionMode: 'bypassPermissions', allowDangerouslySkipPermissions: true, // two guards only, both Razor's
  includePartialMessages: true,              // deltas
  verbatimPrompts: true,                     // a person's "/compact" or "@file" is text
  title: `razor ${session}`,                 // no title request per conversation
  resume, resumeSessionAt,                   // only when Open carries `resume`
  env: { ...process.env, ENABLE_TOOL_SEARCH: 'false', CLAUDE_AGENT_SDK_CLIENT_APP: 'razor/<version>' },
}});
```

**The environment.** The bridge passes on the server's own environment unchanged, so `ANTHROPIC_BASE_URL`, `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN` and `CLAUDE_CONFIG_DIR` reach the CLI as the client that started the server had them. With none set, the CLI uses the person's own sign-in. It adds two variables. `ENABLE_TOOL_SEARCH=false` turns off deferred tool loading, which the spec excludes: the cold review's probe of this version found a `DeferredToolPlaceholder` with `defer_loading: true` in the request under the page's other options, and found it gone only with this variable (`disallowedTools: ['ToolSearch']` did not remove it); probe A shows the request's tools with it set: `Bash`, `Edit`, `Glob`, `Grep`, `Read`, `Write` and Razor's own, nothing deferred. `CLAUDE_AGENT_SDK_CLIENT_APP` names Razor to Anthropic.

**Closing.** The SDK's `close()` ends the CLI's input and signals it only two seconds later (`sdk.mjs`, the process transport's `close`). In that time a CLI whose tool call was cut off made one more model request, against the stand-in. The bridge therefore interrupts a conversation before it closes it. Razor's exit, by SIGTERM or a crash, closes the bridge's input, and the bridge then closes every conversation this way and exits.

**What the bridge maps.**
- Each main-thread `assistant` message's `text`, `thinking` (when it has text) and `tool_use` block becomes an `Entry`, marked with the message's `session_id` and `uuid`. An assistant message that carries `error` is the SDK's own (such as "Prompt is too long") and is not mirrored; the turn's end says it. The first frame of each `message.id` also gives `Context`.
- A `user` message's `tool_result` blocks become `Result` entries: `is_error` gives outcome `error`, and the text blocks joined give `output`.
- `stream_event` text deltas become `Delta`.
- `result` becomes `TurnEnd`, or `Settled` after the bridge's own `continue: false`. `took` is the turn's latest input whose request's reply began (controller.md, "Admission counts what the model has not read").
- `system` `init` gives one line on stderr per conversation, with the CLI's version, the model and `apiKeySource` (`none` is the person's own sign-in). The CLI sends `init` at every turn's start; the bridge logs the first.
- Every other message is ignored, as the SDK asks (line 5592). The one exception is `compact_boundary`, which the bridge reports on stderr as a defect, since compaction is off.
- Every line the bridge writes carries its session and its `Open`'s number, and the controller drops a line whose conversation it has closed: what a closed conversation still had in flight never reaches the record.

**Razor's three tools.**
- `bash` takes Claude Code's `BashInput` fields. The slice honours `command` and accepts the rest, until the full Claude route brings the full shell contract under `Bash`.
- `recall` takes `{id}`.
- `handover` takes `{note}` (no arguments in the slice; the note field since 2026-10-29).
- The declarations live in `bridge/tools.ts` until the profiles take them (RULES.md 16). Each handler reads the call's id from the MCP request's `_meta["claudecode/toolUseId"]` (probe A: it arrives), sends `Run`, and returns the `Result`'s text. A run without that id refuses with a sentence, and nothing runs.
- The SDK declares the MCP tool to the model as well as Claude Code's `Bash`, as `mcp__razor__bash`. Neither `disallowedTools` (which also blocks the alias) nor `alwaysLoad: false` hides it (probe E). Its description is one line telling the model to call `Bash`, and both names run the same action.

### The API server: `src/api/`

```rust
/// Serve the API on `listener` until the process ends. Every request must carry
/// `Authorization: Bearer <device key>`. The local device's key is `$RAZOR_HOME/device.key`,
/// which `razor serve` makes on its first start (mode 0600). Enrolling other devices is flow 25's.
pub async fn serve(controller: Controller, listener: TcpListener, key: String) -> Result<(), Error>;
```

| request | body | answer |
| --- | --- | --- |
| `GET /v1/sessions?cwd=&limit=` | | `[{session, cwd, model}]`, newest first |
| `POST /v1/sessions` | `{cwd, model?, effort?, text}` | `{session, entry}` |
| `POST /v1/sessions/{id}/input` | `{text}` | `{entry}` |
| `GET /v1/sessions/{id}/events?after=N` | | `application/x-ndjson`: one `Event` per line, open until the client leaves |
| `POST /v1/sessions/{id}/stop` | | `{}` |

An error is a status and `{"error": "<one sentence>"}`: 401 without the device key, 400 for a malformed session id, 404 for no such session, 409 when 64 inputs wait unanswered, 503 when the bridge or the config fails, 500 when the record does, and axum's own 4xx for a body it refuses. A request body is at most 2 MiB.

**`razor serve`.**
- It holds an exclusive lock on `$RAZOR_HOME/server.lock`, so a second server exits at once, saying one is running.
- It binds `server.address`, then writes `$RAZOR_HOME/server.json` (`{pid, address}`).
- It serves until SIGTERM, then exits. Its exit closes the bridge's input, and the bridge closes every conversation, as after a crash.

**How a client finds it.**
- A client reads `server.json` and connects.
- If nothing answers, it starts `razor serve` detached: its own process group, no standard input, output appended to `$RAZOR_HOME/server.log`.
- It then waits up to 10 seconds for `server.json` to name a server that answers.

The server outlives the client that started it, so a person's other clients, and a session's running turn, never die with a `--print` that ends.

### `--print`: what it reads and writes (the flows' driver is built to this)

```
razor [--model M] [--effort E] [--resume ID | --continue] --print[=text|json|stream] [MESSAGE]
razor serve
razor --version
```

**Reads.**
- `MESSAGE`, or else standard input to its end (UTF-8). An empty message is a usage error.
- The mode: `--print=MODE`, or the word after `--print` when it is exactly `text`, `json` or `stream`; otherwise that word is the message, and the mode is `text`.
- `$RAZOR_HOME` (default `~/.razor`): `config.toml`, `server.json`, `device.key`.
- The current directory: a new session's `cwd`, and the directory in which `--continue` looks for the newest session.

**Does.**
- With neither `--resume` nor `--continue`, it creates a session with the message. `--resume ID` admits the message to that session. `--continue` lists sessions with this `cwd`, limit 1, then admits the message to that one. `--model` or `--effort` with either is refused in the slice.
- It attaches with `after` set to its input's id minus one, so the stream starts with `attached` and then its own `input`.
- It reads until the first `turn_end` whose `inputs` holds its input's id. That turn is the one that answered it, whether the input opened it, was taken at a tool boundary of a turn already running, or was carried by a rebuild.

**Writes.**
- `text`: the answer and a newline. The answer is the `text` entries of that turn after its last `call` (all of them if it made none), joined by blank lines. The turn's entries are those after the previous `turn_end` on the stream.
- `json`: one line, `{"session": "<id>", "message": "<answer>"}`, or `{"session": "<id>", "error": "<sentence>"}` when the turn did not end `done`.
- `stream`: every API line as received, from `attached` through that `turn_end`, inclusive.
- stderr: one sentence when the turn did not end `done`, or no server could be reached or started.

**Exit codes.** 0 when the turn ended `done`; 1 when it ended `stopped` or with an error, or no server could be reached; 2 for a usage error; 130 for Ctrl-C. Ctrl-C quits the client only; the turn goes on in the server, because Escape is what stops a turn (spec section 3).

**For the flows.**
- `RAZOR_HOME` names the scratch home.
- `ANTHROPIC_BASE_URL` and `ANTHROPIC_API_KEY`, as the driver sets them for `razor`, reach the SDK's own bundled CLI unchanged, through the server it starts.
- The driver ends the server with SIGTERM to the pid in `server.json`.
- The threshold is computed from the `usage` in the stand-in's `message_start` and the window the CLI knows for the model; flow 17 sets `rebuild.threshold` low or reports a large usage.

## Data flow

1. `razor --print "…"` finds or starts `razor serve` and posts the message.
2. The API calls `create` or `admit`. The session's task appends the `Input`, whose line is in the file before anything else happens, and sends it to the bridge, opening a conversation first if none is open.
3. The bridge feeds the SDK's streaming `query`, and maps each SDK message to a line. The task appends text, thinking, calls and results as entries with their native marker and publishes each; deltas are published, never appended.
4. Before any tool runs, the `PreToolUse` hook waits for `Admitted`, which the task sends once the `Call` is appended.
5. The model's `Bash` reaches the bridge's MCP tool through the alias and arrives as `Run`. The task runs it and answers with the output; the SDK's `tool_result` comes back as the `Result` entry.
6. Each SDK result becomes `TurnEnd`, naming the inputs it answered. The record is synced, and when nothing is unanswered the state returns to needs-you.
7. `--print`'s stream, attached at its input, prints and exits after the `TurnEnd` that names its input.
8. `POST …/stop` kills the turn's foreground job and interrupts the SDK; the turn ends `stopped`.
9. `handover` marks a rebuild that runs once its batch settles; the threshold's runs at its turn's end; a restart's runs at load. Each appends `Rebuild` and opens a fresh conversation, which continues the same session.
10. Reopening a session whose last turn ended `done` resumes its SDK conversation instead.

## The hard decision: how the record mirrors the SDK, and what the SDK's session files are

**Decision: the record mirrors the SDK's stream, and the SDK's session file is a cache that Razor never opens.**

**What the record keeps.**
- Each mirrored entry keeps the common form plus `native: {conversation, uuid}`.
- No raw frames, no thinking signatures, nothing else of the SDK's.
- On the Claude path Razor never renders a request (the SDK does), so it needs no native data beyond where to resume.

**When the CLI reads the SDK's file.** The CLI, not Razor, reads it in one case only: a session's conversation must continue in a new CLI process (after a server restart, a bridge exit, or closing an idle session's CLI) and the record's last turn ended `done`. Then `Open` carries `resume: conversation` and `resumeSessionAt` set to the last uuid the record holds, so the native history is cut to exactly what the record has. The conversation keeps its id (probe C).

**When Razor rebuilds instead.** Every other case rebuilds from the record: an open turn, a call without a result, a stopped or failed last turn, or a resume the CLI refuses.

**The options compared:**
- **The SDK's files as the record** (`SessionStore`, or parsing the file).
  - The entry format is "CLI-internal and not part of the SDK API surface" (lines 6957-6969), so every CLI upgrade can break the reader.
  - The spec rules it out.
- **Never use the files** (`persistSession: false`).
  - Simplest to write.
  - But every restart, and every idle session whose CLI is closed, becomes a rebuild. The model loses the tool results and file contents it was working from, and the prompt cache is written again.
  - The only way to avoid that is to keep every idle session's CLI alive, at a measured 167 MB each.
- **The choice: mirror, plus a cache read in one place.**
  - Writer: one marker per entry, and two options on `Open`.
  - Reader: nothing reads the cache but the CLI.
  - Crash: the record decides. Anything not cleanly ended rebuilds, so an unknown outcome is shown once and nothing is run twice.
  - Next route: nothing Claude-specific sits inside the common entry; the Responses route's items take the same `native` slot.
  - Cost: the files live where the CLI keeps them (`~/.claude/projects/`, swept by the CLI's own cleanup), so they also appear in the person's `claude --resume` list.

## Instruction files, persona and Claude Code hooks

`settingSources: []` keeps the CLI from loading CLAUDE.md, AGENTS.md, rules files and the person's settings files, and so also the person's Claude Code hooks. Trey kept the first two, and the spec runs existing Claude Code hooks once, through the SDK. The slice loads none of them, and neither choice below changes its surface.

- **Instruction files and persona: Razor's prompt module** (the "Context, prompts, profiles" row), which the GPT route needs anyway, over the SDK's `project` source. One loader gives every route the same text from the same files; the text sits in our system prompt, under our prompt-cache discipline; and the `project` source would also bring in the folder's permissions, environment, model, MCP servers and plugins, which Razor must not take from a folder. `verbatimPrompts` already skips the CLI's per-turn pass that attaches nested CLAUDE.md and rules files (line 6617), so the SDK's loading would be partial in any case.
- **Claude Code hooks: through the SDK**, with `settingSources` still empty. Razor reads the `hooks` key of the person's settings files and passes it in `Options.settings`, the flag layer (lines 2261-2278; `Settings.hooks`, line 7338). The CLI then runs them once, as it does today, and takes nothing else from those files.

## Verified against the SDK: 0.3.312, CLI 2.1.312

The cold review's notes read this same version; each of their findings below holds. Probes A to F are this page's; "review" is the cold review's of this page.

| option or type | `sdk.d.ts` lines | what it says, or what the probe showed | the slice's use |
| --- | --- | --- | --- |
| `Options.systemPrompt` | 2380-2491 | string, string array, custom or preset; a changed prompt applies at compaction or in a new conversation | `bridge/prompt.md` as a string; a prompt change reaches a session at its next native conversation |
| `Options.toolAliases` | 1656-1681 | a single-hop rename of a model-emitted tool name before lookup. Probe E: the MCP target is declared to the model too | `{ Bash: 'mcp__razor__bash' }` with Claude Code's own `Bash` in `tools` |
| `Options.tools` | 1683-1695 | the base set of built-in tools | Read, Edit, Write, Grep, Glob, Bash |
| `Options.hooks`, `PreToolUse` | 1775-1782, 2818-2833 | a callback gets `tool_name`, `tool_input`, `tool_use_id`. Probe A: a held hook keeps Claude Code's `Write` from creating its file and holds the aliased `Bash` (`tool_name` `mcp__razor__bash`); the `tool_use` frame reaches the iterator first | admission: allow a call once its `Call` is appended |
| `PostToolBatch` | 2697-2714 | once per batch, after every call resolved, before the next request. Probe C: holding it and then closing still lets the CLI send the next request; probe D: `continue: false` ends the turn `hook_stopped` with no further request | the settled-batch boundary for `handover` |
| `Settings.autoCompactEnabled`, `precomputeCompactionEnabled`, `idleCompaction` | 9611-9622 | compaction switches. Probe F: without them a `prompt_too_long` made the CLI summarize the conversation (`compact_boundary`); with them the turn ends `prompt_too_long` and nothing is summarized | all three off, through `Options.settings` (2261-2278, the flag layer). `PreCompact` (2765) can block as a backstop |
| `ENABLE_TOOL_SEARCH` | (environment) | review: absent, the request held a `DeferredToolPlaceholder` with `defer_loading: true`; `false` removed it, `disallowedTools: ['ToolSearch']` did not. Probe A: with it, nothing deferred | set to `false` in the CLI's environment |
| `Options.resume`, `resumeSessionAt`, `forkSession`, `persistSession` | 2136-2153, 1757-1761, 1862-1869 | `resume` loads a conversation; the cut takes any chain uuid. Probe C: the resumed conversation keeps its id and its history ends at the cut; 354 ms from `query` to `init` against the local endpoint | as in the hard decision; `persistSession` stays true |
| `SessionStore` | 6864-6955, 1870-1881 | mirrors local writes and needs persistence; its entries are opaque | not used |
| `SDKMessage` | 5591-5594 | the union; consumers ignore what they do not recognize | assistant, user, result and stream_event are mapped |
| `SDKUserMessage.uuid` | 6562-6656 | a client uuid on a sent message. Probe B: `00000000-0000-4000-8000-…` is taken and echoed | each input's uuid, made from its entry id |
| `SDKAssistantMessage` | 3702-3730 | one message per completed content block, sharing `message.id`; `user_message_uuid(s)`; `error` on SDK-made messages (probe F) | blocks become entries; the first frame's usage gives the context size |
| `SDKResultMessage` | 5948-6110 | one per turn, after its other messages; `user_message_uuids` (5985-5989) lists every input the turn took. Probe B: an input sent mid-tool is folded into the running turn and listed; `queued_turn_count` was 0 with an input queued, so it is not used | `TurnEnd` and its `inputs` |
| `Query.interrupt`, `streamInput`, `close` | 2935-2943, 3298-3304, 3325-3333 | probe B: during a tool, `error_during_execution` with `terminal_reason` `aborted_tools`, the SDK's own `tool_result`, and a queued input run next; probe F: while streaming, `aborted_streaming` after the partial text. `close` ends the CLI's input and sends SIGTERM 2,000 ms later (`sdk.mjs`) | `Stop`, inputs, and `Close`, which interrupts first |
| `SDKPartialAssistantMessage`, `includePartialMessages` | 5754, 1910-1914 | raw stream events | `Delta` |
| `verbatimPrompts` | 1922-1945 | text delivered as written; skips the attachment pass at a turn's start. Probes B, C: the CLI still sends its environment section (working directory, platform, model, date) and a commit-attribution reminder | on; `prompt.md` needs no environment section |
| `title` | 2492-2500 | a given title replaces the generated one | set, so a conversation makes no title request |
| `_meta["claudecode/toolUseId"]` | absent | undocumented. Probe A: it arrives on the MCP call, equal to the hook's `tool_use_id` | ties a `Run` to its call |

**Measured, once** (Node 26.12.1, SDK 0.3.312, one query open, no input yet):
- The Node process with the SDK loaded used 73,044 kB of memory.
- Its CLI child used 166,632 kB.
- A closed idle session's next message waited 1,657 ms for its first delta (median of five, a CLI start and a resume), against 1,188 ms on a warm conversation (median of ten), against a signed-in account at effort `low` on a short conversation: about half a second (the proof, step 6). `sessions.idle` defaults to 4 from it.

Hence one bridge for all sessions, and warm CLIs kept for a bounded number of idle sessions.

## Dependencies (RULES.md 20)

- **tokio**: the runtime the spec lists, for every task, pipe and socket.
- **serde, serde_json**: every boundary is JSON (record, bridge pipe, API); deriving replaces hand-written parsers.
- **axum**: the HTTP server the spec lists; routing and streamed bodies over hyper, in less of our code than hyper alone.
- **futures-util**: turns the attach receiver into the stream axum's body takes; axum already depends on it.
- **reqwest**, without default features: the HTTP client the spec lists, for `--print`. The Responses route needs it, with TLS, anyway.
- **toml**: the config file.
- **libc**: killing a process group and locking `server.lock`. The shell needs it too.
- **The bridge**: `@anthropic-ai/claude-agent-sdk` pinned exactly, with its peer dependencies. TypeScript and Node's types are only development dependencies, for `tsc --noEmit` against `sdk.d.ts`; Node 22.18 and later runs `.ts` directly, so the bridge has no build step.
- **Not taken**:
  - clap: seven flags and two commands are a 40-line `match`.
  - chrono: times are Unix milliseconds, and the session id's date is 15 lines.
  - uuid: randomness comes from `/dev/urandom`, and input uuids are made from entry ids.

## The proof (run on 2026-10-27)

Each step runs against the real SDK and a signed-in account. Every `init` said `apiKeySource: "none"`, and every step below held.

1. **A `Bash` call runs once, through Razor, and its result is durable.** The record holds `Call`, then `Result`, once each. A counter file shows the command ran once. A Claude Code `Write` in the same session leaves its `Call` in the record before the file exists.
2. **Escape mid-turn.** `sleep 300`, then `POST …/stop`. The turn ends `stopped` within a second, the process group is gone, and the call has its result.
3. **A rebuild by `handover`.** Tell the model a fact, ask for a handover, then ask about the fact. The record holds `Rebuild`, later entries carry a new conversation id, the model makes a `recall` of the fact's entry, and the answer is right.
4. **A crash.** Kill -9 the server during a long command, then `--resume`. The call's outcome is `Unknown`, a `Rebuild` with trigger `restart` follows, the command is not run again, and the turn goes on.
5. **A clean restart.** Kill -9 while idle, then `--resume`. `Open` resumes, and the model answers about earlier turns without `recall`.
6. **What an idle session's closed CLI costs.** The time from admitting a message to its first delta, on a warm conversation and on a resumed one, five times each; `sessions.idle`'s default is set from the difference.
7. **`--print` answers its own input** when a second `--print` sends to the same working session: each prints the answer of the turn that named its input.

Flows 2, 4, 17, 18 and 19 follow on the flows' driver.

## Where this departs from the spec

1. **Admission of Claude Code's own tools.** Every tool call on the Claude route waits in the SDK's `PreToolUse` hook for its `Call` to be appended (a ruling on the cold review's finding; spec section 3 requires the intent before the action). It adds one pipe round trip per call.
2. **A second declaration of the shell.** The model sees `mcp__razor__bash` beside `Bash` (probe E); its description is one line.
