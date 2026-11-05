# Tool service

Design page, 2026-10-27; second pass, 2026-10-28. Module `src/tools/`. Budget row "Tool service, MCP, skills": 6,000 lines, shared with code mode and web.

`tools` holds one session's actions, the list every profile renders (SPEC section 5, "Tool service"). It implements these actions:
- the file actions the GPT and generic profiles need;
- the job verbs, over the shell module's `Shell`;
- `skill`;
- scripted tools and the tools of MCP servers, which can be added while a session runs.

It also holds two more things from SPEC section 3: the two guards, and the spill. It records nothing, it orders nothing, and it renders no text a model reads. The controller records every call and its result, and decides when each call runs. The profiles turn each action into the declaration their family sees.

## Surface

```rust
// `protocol::SPILL` (30,000): the length, in characters, past which a result's text spills to
// a file (SPEC section 3, "Tools"); the shell sizes the end of a job's output by it.

/// One session's actions. Every method takes `&self`; share it as `Arc<Tools>`.
pub struct Tools { /* dir, cwd, env, since, Arc<Shell>, config, the list, MCP servers */ }

/// One action as a profile sees it. Only `Tools` makes them.
pub struct Action {
    /// The name every declaration binds to. It is one of the built-in names below, a scripted
    /// tool's config name, or `mcp__<server>__<tool>`. It always matches `[A-Za-z0-9_-]{1,52}`,
    /// so that with the Claude route's `mcp__razor__` prefix it still fits the 64-character form
    /// every route accepts; a scripted or MCP tool named otherwise is not added.
    pub name: String,
    /// The action changes nothing, so the controller may run it beside other read-only calls
    /// (SPEC section 3). Set by Razor for the built-ins and by the config for a scripted tool;
    /// never set for an MCP tool. The Claude bridge passes it on as MCP's `readOnlyHint`, so
    /// the SDK's own scheduling agrees.
    pub read_only: bool,
    /// The text a scripted or MCP tool gives about itself. `None` for the built-ins, whose
    /// declarations are profile data.
    pub schema: Option<Schema>,
    kind: Kind, // what `run` does; private
}

/// A scripted or MCP tool's description and the JSON Schema of its input, as its config or its
/// server gave them.
pub struct Schema { pub description: String, pub input: serde_json::Value }

/// What this module needs from the config. The config reader reads, parses and watches
/// the three keys (`guards.delete`, `[tools.<name>]`, `[mcp.<name>]`) and the pack, and fills
/// this; `configure` applies it.
#[derive(Clone, PartialEq)]
pub struct Config {
    /// Refuse deleting a path made before `since` (SPEC "Trust"). Key `guards.delete`:
    /// true by default.
    pub delete_guard: bool,
    /// The directories searched, in order, for `<name>/SKILL.md`: the pack's `skills/`.
    pub skills: Vec<PathBuf>,
    /// Scripted tools by name (`[tools.<name>]`).
    pub scripts: BTreeMap<String, Script>,
    /// MCP servers by name (`[mcp.<name>]`).
    pub mcp: BTreeMap<String, Mcp>,
    /// Ordered providers for search, fetch and answer (`[web]`), owned by `tools::web`.
    pub web: web::Config,
}

/// A script plus a schema; the config reader parses `[tools.<name>]` straight into it. Each call runs
/// `command` with bash, as a job in the session's `cwd`, with the call's input as JSON on its
/// standard input; the script checks that input itself, as an MCP server does. `input`'s root
/// must be `{"type": "object"}`, or `configure` leaves the tool out.
#[derive(Clone, PartialEq, Deserialize)]
pub struct Script { pub command: String, pub description: String, pub input: serde_json::Value,
                    #[serde(default)] pub read_only: bool }

/// An MCP server, spoken to over its standard input and output; the config reader parses `[mcp.<name>]`
/// straight into it. It runs while it is configured and the session's tools are open.
#[derive(Clone, PartialEq, Deserialize)]
pub struct Mcp { pub command: String, #[serde(default)] pub args: Vec<String>,
                 #[serde(default)] pub env: BTreeMap<String, String> }

impl Tools {
    /// Ready a session's actions. Call it inside the runtime: it starts the MCP servers.
    /// - `dir` is the session's directory; spill files go in `dir/spill/`.
    /// - `cwd` and `env` are what every job starts with.
    /// - `since` is the moment from which a file counts as this session's own: the time of
    ///   the session's `Start` entry, kept across every reopening; for a child, the moment
    ///   before its worktree is made.
    ///
    /// Opening waits for nothing and never fails. Each configured MCP server starts in the
    /// background, and its tools join the list through the addition path when its tool list
    /// arrives. A server that does not start is reported on standard error, and its tools are
    /// left out.
    pub fn open(dir: PathBuf, cwd: PathBuf, env: BTreeMap<String, String>, since: Millis,
                shell: Arc<Shell>, config: Config) -> Tools;

    /// Apply a changed config. The controller calls this at the next admitted turn.
    /// - The guard switch and the skill directories change at once.
    /// - A new scripted tool, or a new server's tools once it lists them, is appended. A
    ///   changed one keeps its place with its new schema.
    /// - A removed tool, or every tool of a removed server, leaves the list at once and its
    ///   calls are refused by name. The conversation's earlier offer of it stands until the
    ///   next rebuild.
    /// - A server whose command, arguments or environment changed is removed, then added.
    pub fn configure(&self, config: Config);

    /// The current list: the built-ins in a fixed order, then the scripted and MCP tools in the
    /// order they were first added. It holds at most 256 entries; an addition past that is refused,
    /// and the refusal is reported on standard error.
    pub fn actions(&self) -> Arc<[Action]>;
    /// One coalesced current list, immediately ready and then notified on registry changes.
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<Arc<[Action]>>;

    /// Run one call, at once: this method orders nothing. The controller decides when it runs
    /// (one first-in-first-out lock per session, read for a read-only action, write
    /// for the rest). `at` is the call's `Call` entry in the record; spill files are named
    /// after it. A future dropped early stops the call where it is; a job it started keeps
    /// running.
    ///
    /// The input is the action's fields (table below) as JSON; unknown fields are ignored. The
    /// output is JSON, never rendered text.
    pub async fn run(&self, at: EntryId, name: &str, input: serde_json::Value)
        -> Result<serde_json::Value, Error>;

    /// Bound a text the controller made itself (a recalled entry). A text of at most `SPILL`
    /// characters comes back unchanged, with no path. A longer text is written to
    /// `dir/spill/<at>`, and its last `SPILL` characters come back with that path.
    pub async fn spill(&self, at: EntryId, text: String) -> Result<(String, Option<PathBuf>), Error>;
}

pub enum Error {
    /// Something the model should read and act on, in one sentence: no action by that name,
    /// input that does not fit, a guard's refusal, an edit that matched nothing or more than
    /// once, an MCP tool's own error.
    Refused(String),
    /// A failure of the machine (I/O, a server that exited), with its cause and a sentence of
    /// context.
    Other { source: Box<dyn std::error::Error + Send + Sync>, context: &'static str },
}
impl From<shell::Error> for Error;
```

### The actions

Paths are relative to the session's `cwd` unless they are absolute. Field names follow the generic profile's declarations, so that profile's mapping is one-to-one. The other profiles map names and turn the JSON into text.

| action | read-only | input | output |
| --- | --- | --- | --- |
| `read` | yes | `path`; `offset` (first line, counting from 1; default 1); `limit` (lines, default 2,000, at most 30,000) | `path` (absolute), `first`, `lines` (each cut at 2,000 characters), `next` (the first line not shown, or null). The window stops before it passes `SPILL` characters and always holds at least one line. The file is streamed, never loaded whole. A PNG, JPEG or WebP file is the image instead: `path` and `images: [{path, media_type}]`, the copy of at most 7.5 MB the model is shown (Seeing an image file, below) |
| `write` | no | `path`, `content` | `path`, `created`. Missing parent directories are made |
| `edit` | no | `path`, `old_string`, `new_string`, `replace_all` (default false) | `path`, `replaced`. Zero matches, or several without `replace_all`, is `Refused` naming the lines, and the file is left untouched |
| `apply_patch` | no | `patch`: Codex's grammar (`*** Begin Patch` … `*** End Patch`; Add, Delete and Update File, with `Move to`) | `added`, `modified`, `deleted`: paths as the patch wrote them. The whole patch is parsed first, so a patch that does not parse changes nothing. Then its file entries are applied in order, as Codex does (`apply-patch/src/lib.rs:469`): each file's new text is derived and matched in memory, then that file is written (`:572-582`). A file whose hunks do not match stops the patch with `Refused` naming that file and the files already written. Context lines are matched exactly, then ignoring trailing whitespace, then ignoring surrounding whitespace: the first three of the four passes in Codex's `seek_sequence`. The fourth, which maps typographic punctuation to ASCII (`seek_sequence.rs:68-108`), is a deliberate difference from Codex; it returns if the GPT evaluation shows patches failing on it. So is the second: a line ending in a lone carriage return is not split there |
| `bash` | no | `command`; `cwd`; `wait_ms` (how long to wait before returning with the job still running; default 60,000); `deadline_ms` (when the job is killed); `pty`; `stdin` (text, then end of input); `open` (keep standard input open for `send`); `pattern` and `quiet_ms`, as for `wait` (the Claude route's `terminal` tool returns at a program's first prompt in one call); `description`, optional, a short plain sentence for the person, which a live call's voice says while the job runs; code mode's `exec_command` takes it too | a job result |
| `wait` | yes | `job`; `timeout_ms` (default 0, which makes it a peek); `pattern`; `quiet_ms` | a job result. A pattern is searched for only in output after what this session's last result for that job showed |
| `send` | no | `job`; exactly one of `text`, `key`, `secret_file` or `end: true`; then `timeout_ms`, `pattern` and `quiet_ms`, as for `wait` | a job result. For `secret_file`, `send` reads the file (at most 64 KiB of UTF-8, asynchronously) and types its bytes. The call's input holds only the path, so nothing has to be left out of the record. This is the one implementation of secret input; profiles never read files. An empty `text` sends nothing and only waits (Codex's `write_stdin` poll) |
| `kill` | no | `job` | a job result |
| `skill` | yes | `name` (a bare directory name) | `name`, `dir`, `output` (the text of `SKILL.md`), `path` |
| `recall` | yes, session | `id` | run by the controller: `Record::get`, then `Tools::spill` |
| `handover` | no, session | nothing | run by the controller: the rebuild |
| `agent`, `message`, `status`, `memory`, `deliver` | session (`status` is read-only) | their owning module's fields | run by the controller |
| a scripted tool | from its config | its own schema | a job result |
| `mcp__<server>__<tool>` | no | its own schema | `output` (its text blocks, joined), `path`. A tool error is `Refused` with its text |

An MCP tool is never read-only, whatever its server says: MCP's schema calls `readOnlyHint` a hint, "not guaranteed to provide a faithful description of tool behavior", and SPEC section 3's order must not rest on a third party's word.

**A job result**, the same shape for `bash`, `wait`, `send`, `kill` and scripted tools:

```json
{"job": 3, "end": {"exit": 0}, "output": "…", "path": null, "cursor": null, "wall_time_seconds": 0.41}
```

| field | type | meaning |
| --- | --- | --- |
| `job` | number | Always present: the call's `job-N`. A job that is still running is reached through this number. |
| `end` | `protocol::JobEnd` or null | `{"exit": i32}`, `{"signal": i32}` or `"unknown"`. Null while the job runs. |
| `output` | string | At most `SPILL` characters: the end of the job's output, or for a `pty` job the screen's rows joined by newlines. |
| `path` | string or null | The job's whole output (`<session dir>/job-N`) while the job runs, so Claude's `Read` can follow a background job, and after it ends when `output` is not all of it. |
| `cursor` | `[row, column]` or null | `pty` jobs only. |
| `wall_time_seconds` | number | This call's own time. |

Codex's `exit_code` is `end.exit`, and its `session_id` is `job` when `end` is null. Profiles map these fields and never parse text. `bash`, `wait`, `send` and `kill` also accept `max_output_chars`: a smaller preview cap, clamped to `SPILL`; when it cuts text, `path` still names the same whole job spool.

### The two guards (`src/tools/guard.rs`)

The guards check the text of a `bash` command, and the `*** Delete File:` lines of a patch, before anything runs. They are lexical checks of the text, best effort and not a sandbox (SPEC "Trust"): a program the command runs, a script it calls, or a string it hands to another shell can still do either thing.

**How the text is read.** Not the way bash would run it; only as words and commands.
- **Words** are cut at unquoted blanks. Single quotes, double quotes and backslash escapes are honoured. A substitution (`$( )`, `${ }`, backticks) stays inside its word, and its parentheses cut nothing.
- **Commands** are separated by `;`, `&&`, `||`, `|`, `&`, newlines and bare `(` and `)`.
- **Left out:** a redirection with its target, a heredoc's body, and a comment.
- **Stepped over** at the start of each command:
  - leading `NAME=value` words;
  - the wrappers `sudo`, `env`, `command`, `exec`, `nohup`, `nice`, `timeout` and `time`, each with the words after it that begin with `-` or a digit (`timeout 5`, `nice -n 10`);
  - bash's reserved words `!`, `{`, `if`, `then`, `else`, `elif`, `do`, `while` and `until`, so that `then rm -rf x` is read as an `rm`.
- **The command's name** is the last part of its path, so `/bin/rm` is `rm`.
- **Not read at all:** the inside of a substitution, a `bash -c` or `sh -c` string, an `eval`, and the directory a `cd` would move to.

**Kill by pattern: always refused.** The guard refuses:
- `pkill` and `killall`;
- `kill` with a word `-1`;
- `kill` whose words contain the text `pgrep` or `pidof` (`kill $(pgrep -f x)`);
- `xargs` with a word `kill`, after a `pgrep` or `pidof` command earlier in the same pipeline (`pgrep x | xargs kill`).

This is a check of the text: nothing follows a value from one command to another.

**Deletion: refused while `delete_guard` is on.** The targets are:
- the operands of `rm`, `rmdir`, `unlink` and `shred`;
- the start paths of a `find` that has `-delete` or `-exec rm` (`.` when it names none);
- the paths of a `git clean` without `-n` (the working directory when it names none);
- a patch's deleted files.

Each target is resolved against the call's `cwd` alone. Then:
- A target the text cannot name literally is refused: a word with a substitution, a `$`, an unquoted `*`, `?`, `[` or `{`, or a leading `~`.
- A target that does not exist is allowed.
- A target whose birth time is before `since` is refused, and so is one on a filesystem that keeps no birth time.

The refusal is one sentence that names the path and says what would work: "`rm` would delete src/old.rs, which existed before this session started; ask the person to delete it."

### The spill

`SPILL` bounds every unbounded text a result carries. That text always sits in a field named `output`, and the file holding the whole of it is named in `path`. Four kinds of text are bounded this way:
- a job's output: the shell keeps all of it in its spool and returns the end;
- an MCP tool's text;
- a skill;
- a recalled entry, through `spill`.

A text over the limit is written to `dir/spill/<at>`, and `output` keeps its last `SPILL` characters. These results keep the end, as the shell does. Code-mode cells keep the start and end of each new range with a count of omitted bytes and the full-output path (Trey, 2026-10-31, approved the cell preview change). `read` never spills; it pages. An MCP error's text is cut to its last `SPILL` characters, without a file.

## Data flow

1. The controller opens the session's `Shell` and then `Tools::open`. Each configured MCP server starts in the background, and its tools join the list when it has listed them.
2. At the start of a conversation a profile renders `actions()`. At each later request it renders them again and offers every declaration whose name and bytes are not already in the conversation's prefix or its additions, through its route's append path (the hard decision).
3. A call reaches the controller from the model's message, from the bridge's `Run`, or from a code-mode cell (the code-mode profile numbers nested calls `<exec call id>:<n>`; `tools` never sees call ids). The controller appends the `Call` and takes the call's place in the session's first-in-first-out lock, in arrival order. For an action with `session` set, it runs the action itself. For any other, it calls `run(at, name, input)`.
4. `run` finds the name in the list, reads the input into the action's own struct, and runs it.
5. `bash` checks the guards, then calls `Shell::start`. The job's own task queues `Started` and later `Ended`, and `start` returns once `Started` is queued. Then `bash` waits with `Shell::wait` until the job ends, the pattern matches, or `wait_ms` passes. Other jobs are reached by number, through `wait`, `send` and `kill`.
6. File actions use `tokio::fs`. `write` and `apply_patch` share the parent-directory/write operation. `edit` holds at most 64 MiB of file text. Patch paths and lines borrow the input; one file's new text is derived before writing, with one index of its original lines.
7. Each MCP server is an `rmcp` client over the child's standard input and output. Its tool list is paged to the end. On `notifications/tools/list_changed` it lists again: new or changed tools are appended, and tools that are gone leave the list. A server that exits takes its tools out of the list. Each tool call waits at most `timeout` ms (ten minutes by default), set on `[mcp.<name>]` and copied on import; changing it restarts the server like any other key. At the limit rmcp cancels the outstanding request, and the result says the outcome is unknown and it was not retried.
8. The output is JSON. The caller's profile turns it into what its model reads, and the controller appends the `Result`.
9. A config change reaches `configure` at the next admitted turn.
10. Memory held: at most 256 actions, 64 job positions, one file being edited or patched, and a `read` window of at most 30,000 characters and 30,000 lines. The command check retains one command's words; heredoc bodies are compared without retaining their lines. An MCP message is at most 16 MiB (`rmcp`'s own line limit). Spill files and job spools live on disk.

## The hard decision: the shape of an action and of the list

Three questions decide it: what an action is, what it receives and returns, and how an addition reaches a conversation that is already running. Each answer is weighed by what it costs each profile, an addition, and the second-pass rewrite.

**1. An action is a variant of a private enum, handled by one arm of one `match` in `run`.** The arm calls a plain async function with `&Tools` and the parsed input, returning JSON. Job-handle verbs share one input and result path. A scripted variant carries only its command, so a call never clones its schema; an MCP variant carries its server, tool name and peer. Session actions are variants that `run` refuses.
- **A trait per action, and a map of trait objects.** This is Codex's `ToolHandler` with its `ToolRegistry`; Goose and Crush do the same.
  - The profiles see the same snapshot either way.
  - Scripted and MCP tools are the same two kinds either way, with boxed futures and vtables on top.
  - In the rewrite there are eleven impl blocks, each with a name, a read-only flag and a run method, plus a context struct passed through all of them. RULES 2 allows the trait, since it has many implementations, but the rewrite's first cut would fold it back into a match.
- **A table of function pointers keyed by name.** Every future must be boxed to share one function type. Scripted and MCP tools still need data beside the table, so a call does two lookups.
- **The enum and the match (chosen).**
  - It is exhaustive: a new action that `run` does not handle does not compile.
  - Nothing is boxed.
  - Every action is visible in one screen of `run`.
  - The cost: `src/tools/mod.rs` is where each later action is added, as one variant and one list entry. That is RULES 1's one way in.

**2. An action receives JSON and `&Tools`, and returns JSON.** `&Tools` gives it the session's directory, `cwd`, environment, `since`, shell and config.
- **Why not rendered text out.**
  - Claude's `Bash` reports an exit code as text, Codex's `exec_command` as an `exit_code` field, and code mode needs an object. One text format would have two profiles parsing text, which the profiles will not do.
  - Profiles map fields as data (renames, constants), and code mode hands JSON straight to the isolate.
- **Why not a typed Rust enum out.** Every caller would serialize it at once anyway.
- **Why no channel for job events.** The shell's job task queues both of a job's events, so no action needs that channel.
- **Binding is by name only.** Not by name and version: a version on each action adds nothing that is not already covered:
  - nothing would compare it except a check at load;
  - the evaluation runs before every install (SPEC section 7), so a changed action is measured with every profile in any case;
  - declarations and the actions they bind live in one repository, one compile unit, and change in the same commit, and flows 10 and 28 fail if they drift apart.

**3. Within a conversation the list only grows, and each route offers the new entries through its own append path.**
- **Sending the whole tool list again.**
  - Anthropic's documentation: "The `tools` array sits even earlier in the hashed request prefix than the top-level `system` field, so editing it invalidates the prompt cache for the entire conversation."
  - OpenAI's prompt-caching guide asks for stable tool definitions.
  - So every addition would rewrite the whole cache.
- **Deferred tool search.** SPEC section 4 removes it. Its first use rewrote the whole cache, and the search costs an extra request.
- **Holding additions until the next rebuild.** This keeps the cache, but a tool registered mid-conversation cannot be used until then, and SPEC section 3 has tools "register live".
- **Appending (chosen).** At each request a profile renders the list and offers every declaration whose name and bytes are not already in the native conversation's prefix or its additions. That needs no counter, and it holds across a restart, since the prefix and the additions are in the record:
  - **Claude.** The bridge registers the tool on its in-process MCP server, created with `alwaysLoad: true`, and the Claude route starts the CLI with `CLAUDE_CODE_INLINE_TOOLS=1`. The server sends `notifications/tools/list_changed`, and the CLI appends a `role: "system"` message holding a `tool_addition` block with the full definition; the `tools` array stays as it was. `alwaysLoad` alone is not enough: without the switch, a review's capture saw the `tools` array change and the addition sent by reference (below). The `tools` array must keep at least one tool that is not deferred, which Claude Code's own Read, Edit, Write and Bash always provide (Anthropic's documentation names this as the one exception).
  - **Responses.** The loop appends a developer-role `additional_tools` input item holding the new function definitions (OpenAI API reference; Codex at `core/src/context/world_state/top_level_tools.rs:213`). That item is conversation: the profile records it as native data where it was sent, as it records every item it sends. A request rendered after a restart is then the same request (RULES 10).
  - **Code mode.** The new tool changes `exec`'s description. Codex appends the re-rendered `exec` in an `additional_tools` item (its `schema_changed` case), which costs that description's tokens once and leaves the cache in place. The code-mode profile chooses this, or something lighter.
  - **The generic loop.** OpenAI-compatible chat has no append path, so it pays one cache rewrite at the next request, or holds the addition until a rebuild. The generic path decides (`generic.md`).
  - **A change** is a known name with new bytes, so it is offered the same way. Both APIs accept a new definition under a known name: Anthropic's documentation covers a tool "whose schema changes later", and Codex sends changed definitions the same way.
  - **A removal** is announced by one availability line at the next model boundary. Calls to the removed tool refuse from each loaded session's next tool-call boundary; its old declaration stays until rebuild, so the cached prefix holds (the MCP ruling, `mcp.md`).
  - **The rewrite** inherits one snapshot and one comparison.

### The addition paths, checked against their sources

| route | source | what it says or shows |
| --- | --- | --- |
| Claude | Agent SDK 0.3.312 `sdk.d.ts` (CLI 2.1.312): `createSdkMcpServer` and its `alwaysLoad` (639-659); `McpSdkServerConfigWithInstance.instance`, an `McpServer` (1247-1249); `setMcpServers` (3297); `tool` (10159-10163). MCP SDK 1.32.2: `McpServer.registerTool` (`server/mcp.d.ts:161`) takes a zod schema. | A tool added to a live in-process server reaches the CLI. Zod 4.6.7, the SDK's own dependency, has `z.fromJSONSchema`, so the bridge can register a JSON Schema as it is. |
| Claude, on the wire | A review's two captures: a fresh SDK 0.3.312 install with its bundled CLI against a local stand-in through `ANTHROPIC_BASE_URL`, no model called; three turns and five requests each, both added tools called (one registered with a zod shape, one through `z.fromJSONSchema`). | Without `CLAUDE_CODE_INLINE_TOOLS`: the `tools` array changed between requests, additions went by reference (`tool_reference`), and the beta header lacked `inline-tools-2026-09-15`. With `CLAUDE_CODE_INLINE_TOOLS=1`: the `tools` array and `system` were byte for byte the same over all five requests, each addition came by value as a `role: "system"` message with a `tool_addition` block, and the header carried `inline-tools-2026-09-15`. An earlier capture saw the by-value form without setting the switch, so the default depends on rollout state the bridge does not control. A measurement of the by-value path saw it keep the cache on 2.1.306 (six of six runs). |
| Claude, the API | Anthropic, "Mid-conversation system messages and tool changes" (read 2026-10-27) | `tool_addition` carries a definition by value under `inline-tools-2026-09-15`; "the prompt cache still hits"; keep at least one tool that is not deferred. |
| Responses | OpenAI API reference, `responses.create` input items (read 2026-10-27): `AdditionalTools { role: "developer", tools, type: "additional_tools", id }`; prompt-caching guide: "Use a developer-role `additional_tools` input item to add tools during a thread" | The item is documented on the public API. |
| Responses, in Codex | Codex `aa854dab` (2026-10-27): `core/src/client.rs:1073` puts the first list in the prefix; `core/src/context/world_state/top_level_tools.rs:130-265` appends what was added or changed as an `additional_tools` item and announces removals as developer text | Codex's own live path, on the ChatGPT backend Razor uses. |

## Where this departs from the spec or a neighbour

1. **The shell's job task queues `Started`, before its `Ended`** (a ruling on the cold review). `Shell::start` spawns the job's task, which queues `Started` and then watches the job; `start` waits for that queueing and returns the job's number, no longer the event.
   - **Why in the job's task.** An awaited send inside `start` before the task exists would leave a running child with nobody to reap it, kill it at its deadline or report its end, if the caller were dropped while the queue was full. `start` waits only after the task is running, so a dropped caller leaves the job supervised.
   - **Why `start` waits for it.** Otherwise a `bash` call with a short `wait_ms` could return before its job's task first ran, and the call's `Result` would reach the record before the job's `Started`. Flows 19 and 35 need a job's `Started` recorded before its call's `Result`.
   - **One rule for the controller.** The controller drains the job events before it appends a call's `Result`.
   - **What it removes.** No action needs a job-event sender, and the controller drains one channel. Both changes are in `src/shell/mod.rs`.
2. **`SPILL` moves into `tools`,** and `src/shell/output.rs` imports it (one line).
   - **The same file names the spool while a job runs** (the Claude route's ask): `Output.path` is set when the text spills or the job has not ended. That is one condition in the shell's `read`.
3. **The delete guard reads "a path this session did not create" as "a path born before `since`"** (Trey, 22:55: "sure i'm down with what you think is best, lets see").
   - **Why birth time.** It needs no record of what the session made, it survives a restart, and it covers files made by any program the session ran. A list of the paths Razor's own actions wrote would miss everything a command makes (`cargo build`'s `target/`), and would refuse the most common clean-up there is.
   - **What it costs.**
     - A resident's session lasts weeks, so it may delete what the person made during them.
     - A file another program made during the session counts as the session's.
     - On a filesystem that keeps no birth time, every deletion is refused.
4. **`send` takes `secret_file`.** The record's one omission (`"secret": true` text) is no longer needed, so `Record::append` can drop it.
5. **On the Claude route, Read, Edit and Write are Claude Code's own (the Claude slice).** So the file actions serve the GPT and generic profiles. The delete guard sees Claude's shell commands only, since Claude Code has no tool that deletes. `cwd` does not carry over between `bash` calls, because SPEC section 3 makes each shell call its own process. The `Bash` declaration must therefore not promise Claude Code's persistent working directory.
6. **MCP now also speaks streamable HTTP**, with OAuth, search, person questions and rich results under `mcp.md`; the stdio path still serves local servers. That later ruling replaces this page's original stdio/text-only slice.
7. **Code mode's top-level `wait` (for a cell) is not the `wait` action (for a job).** Neither `exec` nor that `wait` is in the list: they are how the code-mode profile shows the list. The controller must not give `exec` itself a place in the lock, since its nested calls need theirs. The Codex profile names the job verbs the way Codex does (`exec_command`, `write_stdin`).
8. **A child's worktree must be made after the moment its `since` names.** Otherwise its checkout counts as made by someone else, and the child cannot delete a file in its own branch.

## Dependencies (RULES 20)

- **`rmcp`**, with only its `client` and `transport-child-process` features, as pinned Codex uses it. It replaces about 150 lines of hand-written JSON-RPC client (the initialize handshake, paged listing, routing replies by id, answering server pings, list-change notices) and follows the protocol's revisions. Measured in scratch against the same `tokio`, `serde_json` and `futures-util`, it adds 34 crates, 2.4 MB to a stripped release binary and about nine seconds to a clean release build. The line budget and the protocol's churn outweigh that.
- `serde_json` comes from the Claude slice; `tokio` and `memchr` are already in the build. Birth times come from `std::fs::Metadata::created`, which uses `statx` on Linux and `st_birthtime` on macOS. The bridge's `z.fromJSONSchema` comes with the SDK's own zod.

## Flows

Both versions pass flows 02, 06, 07, 09, 10, 19, 29 and 34: sessions, shell output, job handles, read/write order, files, restart, the unchanged web registration, and rebuild admission.

## Seeing an image file (30 October)

Trey, 2026-10-30, to "Let GPT look at image files, the way Codex can": "1. yes." Today GPT sees an image only inside an MCP result. A screenshot a command saves to disk never reaches it: in the browser measurements it passed the image task only because `tesseract` happened to be installed. Those runs were plain mode; signed-in GPT runs code mode (`profiles.codex.code_mode` is on by default), which has no file-reading tool at all. Claude already opens an image with Claude Code's own Read, and that route does not change.

### The hard decision: `read` shows the image, and code mode gets Codex's `view_image` bound to `read`

| | A new built-in `view_image` on every route | `read` shows the image (chosen) |
| --- | --- | --- |
| Declarations added | one top-level on plain GPT, one on the generic route, one nested in code mode | none top-level; one nested in code mode, which is Codex's own |
| Tokens on every request, cached after the first | about 75 for the function on each plain or generic request, and about 75 for the code-mode section (estimated at four characters a token) | about 17 for one sentence on `read`; the same 75 in code mode |
| Actions in `tools` | one more variant, list entry and function | none: `read` gains a branch |
| What the makers' harnesses do | Codex: a separate `view_image` in every mode, because it has no file-reading tool and reads files through the shell. Kimi: a separate `ReadMediaFile` | Claude Code's Read ("Reads images (PNG, JPG, …) and presents them visually"); Gemini CLI's `read_file` ("Handles text, images (PNG, JPG, GIF, WEBP, SVG, BMP)…", its Gemini 3 set); OpenCode's read ("Images and PDFs are presented directly to the model"); Crush's view; Pi's read ("Images are sent as attachments") |

Tokens decide nothing: either way is tens of tokens in a cached prefix. What decides is the number of tools in front of the model, which Trey wants "as small as humanly possible": a second way to open a file on routes that already have `read`, against none added. Every harness that has a file-reading tool shows images through it, except Kimi; Codex is the exception only because it has no reader. So each Razor mode keeps its maker's shape. Plain GPT and the generic route have `read`, so `read` shows the image. Code mode is Codex's shape and has no `read`, so it gets Codex's own `view_image`, bound to the same action. One action, the smaller surface.

### Surface

```rust
// src/protocol/api.rs, beside IMAGE_BASE64. Controller::store uses both instead of its own
// copies (RULES 5), so the person's images and read's are judged by one check.
/// The most bytes one image may hold: what `IMAGE_BASE64` characters of base64 decode to (7.5 MB).
pub const IMAGE_BYTES: usize = IMAGE_BASE64 / 4 * 3;
/// A PNG, JPEG or WebP image's media type and file extension, from its first bytes; `None` for
/// any other bytes, whatever the file is named.
pub fn image_type(head: &[u8]) -> Option<(&'static str, &'static str)>;
```

- **`read`** (this module; the actions table's row changes). A file whose first bytes are a PNG, JPEG or WebP image is not paged: it is copied to `dir/images/read-<at>.<ext>`, and the output is `path` (the file's absolute path) and `images: [{path, media_type}]` naming the copy, the shape an MCP tool's images already take, so every model path already carries it. `offset` and `limit` are ignored for it. An image over `IMAGE_BYTES` is `Refused` with its size and what would work (a smaller copy made with a command). Any other file reads as lines, as now. `read` takes `at` to name the copy.
- **Profile data** (`data/tools.json`, not counted). `read`'s description gains one sentence: "A PNG, JPEG or WebP file comes back as the image itself, not lines." A new entry binds `view_image` to the `read` action in code mode only, with Codex's description at the pin (`core/src/tools/handlers/view_image_spec.rs`), "View a local image file from the filesystem when visual inspection is needed. Use this for images already available on disk.", and Codex's `path` with its description. It omits `detail` and `environment_id`, which Codex itself adds only behind options. `Profile::input` maps `view_image` to `read` in one arm, beside `exec_command`. The profile version goes from `razor-6` to `razor-7`.
- **`image()`** is a no-op global in `code_mode/cell.rs`, and `generatedImage()` writes only the result's hint. Codex's scripts pass `view_image`'s result to `image()` to show it; Razor already shows every image a nested result names, so a script written for Codex runs unchanged instead of failing on an undefined name. Neither can show a data URL a script built itself, so exec's description does not carry Codex's lines for them (a code review found they promised that): it says instead that nested results' images are attached automatically and a script cannot add others, and that `generatedImage()` appends the hint.
- **The Responses task** (both wires; its public surface is unchanged).

```rust
struct Task {
    // ...
    /// Images the results named since the last request, in the order they came, each sent there
    /// after its path. Paths only, bounded by the results naming them: `read` names one, an MCP
    /// result is at most 16 MiB; the bytes count toward the request's 64 MiB when it is built.
    images: Vec<Image>,
}
```

`images(&output)` notes them instead of sending a message. `request()` puts them first in the user message it already builds for held inputs (the results' images, then each input's images and text) and sends that message when either is present.

### Data flow

1. The model calls `read` (plain GPT, generic) or `tools.view_image` (code mode). The profile maps either to `read`.
2. `read` peeks at the first bytes through the reader it already opens; `image_type` decides. For an image it checks the size, copies the file under the call's entry id and returns the JSON. The copy is what the model sees, taken when the call ran, so a command later in the batch that overwrites the screenshot cannot change it.
3. The controller records the Result as any other: the JSON, never the bytes.
4. The Responses task notes the images any result names: `read`'s, an MCP tool's, or in code mode a nested call's.
5. At the next request, which comes after the batch's last output, it reads the copies into the head of the user message. On chat that message follows all of the batch's `tool` messages.
6. Claude plain mode is unchanged. A settling conversation drops what it noted. An interrupted turn's noted images go with the next turn's first request, since the outputs that named them were already sent.

**Why the image moves from right after its output to the next request.** Today a result with images puts a user message straight after that output. On chat, in a batch of several calls, that message lands between two `tool` messages. OpenAI's chat endpoint refuses that: an assistant message's calls must be answered by the `tool` messages that follow it, with nothing between. Google's and RunInfra's behaviour there is unmeasured. Gemini is the one shipped generic model that declares `read`, and its shipped route tells it to batch independent reads, so an image in a batch is the expected case there. In code mode today a nested call's image lands between the `exec` call and its output. After the batch's last output is the one place every wire accepts it, and it is where held inputs already go. Holding the image in a function call's own output, as Codex does on Responses, would need this hold on chat anyway, and again for nested calls. `generic.md`'s data flow 8, which says a result's images follow it as a user message, changes with the build.

**Edge cases handled.**

- A file named `.png` that holds text reads as lines: the bytes decide, not the name.
- GIF, BMP, SVG and HEIC read as lines. Google's compatible endpoint takes no GIF, and SVG is text.
- An image over 7.5 MB is refused, naming its size, so it never reaches the task's own limit on one image in a request (10 MiB), which ends the conversation.
- Several images in one batch go after the last output, each after its path, so parallel nested calls that finish out of order stay attributable.
- A code-mode cell that yields: its images go after the yield's output.
- An image overwritten later in the batch: the model sees the copy taken when `read` ran.

**What it costs.**

- An image file no longer reads as lossy text; its raw bytes stay reachable through `bash` (`xxd`).
- Nothing resizes. Codex and Claude Code resize before sending; OpenAI and Google scale a large image themselves before counting its tokens, so a large file costs upload size and record space (native data keeps the base64, as for the person's images), not tokens. The 64 MiB request fills sooner, and at the cap the conversation rebuilds, as with MCP images today. Resizing would need an image crate.
- No per-model image switch. Every model that declares `read` or `view_image` by default takes images: GPT on Responses, and Gemini. DeepSeek's and GLM's shipped variants declare no `read`. A configured variant that gives a text-only model `read` fails at its first image, and every later request in that conversation carries the image until a rebuild: the same as an MCP image on that model today. Third-party pages say DeepSeek V4.1 Flash takes images; RunInfra's serving of it is unchecked.
- `razor-7` rebuilds each live conversation once, at its next turn after install. Claude's too, since the version string is shared.
- Flow 28's pinned code-mode prefix hash changes with `view_image`. Its real-backend token measurement is one request on the ChatGPT sign-in, which reads `~/.razor`, so it is redone by hand.

### As built, after review

- **Each image goes after its path.** The task notes images as they come, so with parallel nested calls they arrive in completion order, not call order. Each is sent after an `input_text` holding its path, which is the name the model gave `read` or `view_image`, or the one a result showed it. Nested call ids are left out because the model never sees them.
- **The copy is bounded as it is made.** `read` copies through a reader limited to 7.5 MB plus one byte; past the limit it removes the copy and refuses. It never trusts an earlier size.
- **Claude code mode carries nested images.** `ToBridge::Result` gains `images` (serde default, skipped when empty). The Claude task collects the images its cells' nested results name and sends them with the next `exec` or `wait` result. The bridge puts each one after its path in that tool result, as it already does for an MCP result's own images. Escape clears them with the cell.
- **One parser.** `Image::named(output)` in protocol reads a result's `images` for both tasks and parses only an output that names images.

**Flows.** 03 (input images), 10 (files), 37 (MCP images, whose placement moves to the next request), 46 (generic) and 25 run. Flow 28 will fail until the prefix is measured again.

## Glossary lines proposed

- **guard**: one of the two checks of a command's text that refuse it before it runs: kill by pattern, and deleting what the session did not make.
- **spill**: writing a result's text past 30,000 characters to a file, and returning its end with the file's path.
- **scripted tool**: a tool made of a script and a schema in config; each call runs the script as a job.
