# The Claude path

Design page. Modules: `src/claude/` (the bridge's Rust half) and `bridge/` (the TypeScript bridge), grown from the first slice into SPEC.md section 3's Claude path. The SDK is `@anthropic-ai/claude-agent-sdk` 0.3.312 with its bundled CLI 2.1.312; line numbers are in its `sdk.d.ts` and `sdk-tools.d.ts`. Every claim about the CLI's behaviour comes from a run of the real SDK and CLI against a local capture server, with no model.

## Surface

From the slice, unchanged: one bridge process for every session; every call held in `PreToolUse` until its `Call` is in the record; the SDK's messages mirrored into entries, the mirrored `tool_result` being the only way a `Result` enters the record on this route; the `PostToolBatch` stop for a rebuild; resume or rebuild at open.

### The messages (`src/protocol/conversation.rs`)

The bridge reads and writes these; the fields below are what the Claude path needs beyond the slice, as ruled.

```rust
pub enum ToBridge {
    /// `prompt` and `tools` are the Claude profile's, copied as they are, and kept until the
    /// native conversation closes. On a resume `tools` is the session's whole current list:
    /// the CLI keeps the tools array it started with, and the additions stay in its history
    /// (runs resume-all, resume-first).
    Open { session, open, cwd, model, effort, resume: Option<Resume>,
           prompt: String, tools: Vec<serde_json::Value> },
    /// More of Razor's tools for the open conversation, each replacing one of the same name.
    /// The model's next request carries them inline; the tools array and the system prompt
    /// do not change.
    Add { session, tools: Vec<serde_json::Value> },
    /// Each `protocol::Image { media_type, path }` has an absolute path on this boundary (the
    /// record keeps it relative to the session's directory). Images go before the text.
    Input { session, inputs: Vec<BridgeInput> }, // BridgeInput { entry, text, images }
    /// Applied in place, before the next input (`Query.applyFlagSettings({effortLevel})`,
    /// sdk.d.ts 3018-3035).
    Effort { session, effort },
    /// With `settle`, no further request starts after this batch (handover or finish).
    Result { session, call, output, error, settle: bool },
    // Admitted, Stop and Close: as in the slice.
}
pub enum FromBridge {
    /// `tool` is the served name (`terminal`, or `bash` reached through the model's `Bash`);
    /// the profile maps it to an action.
    Run { call, tool: String, input: serde_json::Value },
    /// This native conversation's sign-in readings: at most 32 named windows, percent 0–100.
    /// Never combined with another session's account.
    Usage { windows: Vec<UsageWindow> },
    // Entry (with `native` now optional), Delta, Context, TurnEnd, Settled, Closed: as in the slice.
}
```

`Resume` is serde's default external tagging: `{"claude": {"conversation", "at"}}`.

### `src/claude/mod.rs`

`Bridge`, `Bridge::send`, `Lines` and `Error` stay as the slice has them.

```rust
impl Bridge {
    /// Start the bridge found beside the running binary (`bridge/` in its directory), or else
    /// the source tree's `bridge/` this binary was built from. There is no config key: nobody
    /// tunes it, and swapping the one bridge would stop every Claude session.
    pub async fn start() -> Result<(Bridge, Lines), Error>;
}
/// Until the profiles land: the slice's system prompt and tool list (`prompt.md`,
/// `tools.json` beside this file), which the controller copies into `Open`.
pub fn profile() -> (&'static str, Vec<serde_json::Value>);
```

### `bridge/main.ts`

One streaming `query` per open conversation, as in the slice. Everything the model sees now comes in `Open`:

```ts
// Open.tools holds three shapes, split here and nowhere else:
//   {"builtin": "Read"}                            Claude Code's own tool and declaration
//   {"builtin": "Bash", "to": "bash"}              Claude Code's declaration; the call runs Razor's `bash`
//   {name, description, inputSchema, annotations}  one of Razor's tools, served as written
const razor = createSdkMcpServer({ name: 'razor' });
razor.instance.server.registerCapabilities({ tools: { listChanged: true } });
razor.instance.server.setRequestHandler(ListToolsRequestSchema, () => ({ tools: served })); // each with _meta['anthropic/alwaysLoad']
razor.instance.server.setRequestHandler(CallToolRequestSchema, (request) => run(request.params));

query({ prompt: inbox, options: {
  cwd, model, effort, systemPrompt: prompt,
  tools: builtins, toolAliases: aliases, mcpServers: { razor },
  hooks: { PreToolUse: [{ hooks: [admit], onFailure: 'block' }], PostToolBatch: [{ hooks: [settle], onFailure: 'block' }] },
  settingSources: [],
  settings: { autoCompactEnabled: false, precomputeCompactionEnabled: false, idleCompaction: false,
              attribution: false, disableClaudeAiConnectors: true },
  permissionMode: 'bypassPermissions', allowDangerouslySkipPermissions: true,
  includePartialMessages: true, verbatimPrompts: true, title: `razor ${session}`,
  ...(resume ? { resume: resume.claude.conversation, resumeSessionAt: resume.claude.at } : {}),
  env: { ...process.env, ENABLE_TOOL_SEARCH: 'true', CLAUDE_CODE_INLINE_TOOLS: '1',
         CLAUDE_BASH_MAINTAIN_PROJECT_WORKING_DIR: '1', CLAUDE_AGENT_SDK_CLIENT_APP: app },
}});
```

- **`run`** reads the call's id from `_meta['claudecode/toolUseId']`, sends `Run { call, tool, input }`, and returns the `Result`'s text. A `Result` with `settle` marks the conversation, and from then on `run` marks each result `_meta['claude/endTurn']` (the SDK's end-turn tool convention, `sdk.d.ts` on `resumeDropsTurn`), which ends the CLI's turn on that result with no further request. `PostToolBatch` still answers `continue: false` for a marked conversation's batch, for a settling result the CLI does not end on (an error); on a result that ended the turn the CLI discards that stop. The hook's stop alone writes a note after the result, `PostToolBatch hook stopped continuation: Razor settled the batch.`, which the resume just after cuts (it resumes at the result) and every later resume puts back mid-history, so a twice-continued child wrote its whole cache again from its first finish (flow 16).
- **Razor's tools are served raw**, so each declaration reaches the request byte for byte (run raw-meta), with no zod. A scripted tool receives the model's arguments as JSON and checks them itself, as an MCP server does; Razor's own actions are checked by their input types in `Tools::run`. The CLI declares only a schema whose root is `{"type": "object"}` (a probe; MCP SDK `ToolSchema`), so `Tools::configure` leaves out a scripted tool with any other root, with a message.
- **Names.** The registry leaves out any scripted or MCP tool whose name is over 52 characters, so `mcp__razor__<name>` always fits the API's 64.
- **`Add`** puts each tool into `served`, replacing one of the same name, and calls `sendToolListChanged()`. The notice reaches the CLI ahead of any later user message: in run order1 the addition was in the next request 5 times out of 5.
- **`Input`** becomes one SDK user message for the whole admitted batch, preserving each input's image blocks then its text, in entry order. Images are read from their files and sent as base64 blocks. The message's UUID is made from the batch's last entry. The CLI's `command_lifecycle` frame `started` for it is a candidate; once the reply to the request carrying it begins (a top-level `message_start` or assistant frame), it is the turn's latest taken input, which `Context` names as `read` and `TurnEnd` as `took`; at the turn's end it is `took` even if no reply began, as after Escape (controller.md, "Admission counts what the model has not read").
- **`Effort`** calls `applyFlagSettings({ effortLevel })` on the open query.
- **A failed hook blocks.** Both matchers set `onFailure: 'block'` (SDK 0.3.317): a callback that throws or answers badly refuses the call, or stops the batch, instead of the CLI treating its answer as empty. In a scratch copy of the bridge whose `admit` threw, a `Bash` call ran without its admission; with the setting it got the CLI's error result and nothing ran (3 November 2026). No flow can make the callback fail, so none checks it.
- **Notices.** A `system/informational` line becomes a `Notice` when its level is `warning` or it stops continuation; `info`, `notice` and `suggestion` lines are the CLI's status and tips, written for its own screen. A `system/session_writes_lost` notice is not acted on (Trey, 2026-11-03: "Delete it"). When lines at the transcript's end are lost, the CLI cannot find the message it is to resume at and exits before init; the controller then rebuilds from the record. A gap in the middle, with the last message saved, resumes without the lost lines. Flow 41 checks the missing-conversation rebuild by deleting the CLI files and checks that a fresh CLI that cannot start ends its turn.
- **Tool search is on, with inline tools.** With it off, an addition joins the tools array and rewrites the cached prefix (runs a, raw-tsoff). With it on and `CLAUDE_CODE_INLINE_TOOLS=1`, the addition travels inline and the prefix stays byte-identical (runs i, iii, iv, raw-meta). The array gains one inert `DeferredToolPlaceholder` (204 bytes). This is not the deferred search SPEC section 4 removes: no search tool is offered and nothing is deferred.
- **Attribution off**, since `settingSources: []` loses the person's own setting, and without it the CLI adds an attribution reminder to every conversation's first message (run b).
- **No claude.ai connectors.** With a claude.ai sign-in the CLI connects the account's claude.ai connectors in the background, and their tools join from the second request: tools Razor never declared, run outside its tool service, and a rewrite of the cached prefix (live run init). `disableClaudeAiConnectors` keeps them out (live run init-off).
- **Full descriptions.** The CLI cuts every MCP tool description at 4,096 characters unless `CLAUDE_CODE_MAX_MCP_DESCRIPTION_LENGTH` says otherwise; code mode's `exec` carries every tool's declaration, so the bridge sets it to a million (flow 39).
- **Child reminders.** The controller passes `child` to `SharedBridge::start`, which sets `CLAUDE_CODE_SILENT_TURN_REMINDER=false` in that conversation’s `Open.env`. Top-level conversations inherit the CLI’s reminder setting.
- **A sign-in per conversation.** With `[claude] sign_in` naming a command (a machine key, read live like the rest), `SharedBridge` runs it for each `Open`, outside its lock, and adds `env` to that one message: the first line of the command's output, trimmed (at most 64 KiB is read), as `CLAUDE_CODE_OAUTH_TOKEN`, the other sign-in variables (`claude::KEYS`) blank, and `CLAUDE_CONFIG_DIR` set to `<home>/claude`. An optional second line names the account the token signs in to; it travels on the same message as `account`, beside `[claude] usage_file` as `usage_file`, and nothing else holds it (Account usage, below). Every session, child and fresh conversation so signs in with what the command gives at that moment. A resume within an hour of its conversation's close keeps that conversation's sign-in instead: the prompt cache, an hour by the bridge's setting, belongs to the account, and a resume on another account writes the whole conversation to the cache again. The kept sign-ins live in the server's memory only, one per session that signed in by command and at most 64 (about three times the most sessions a server opened in one hour); when a 65th conversation closes, the oldest is dropped. A resume reuses one only if its conversation closed less than an hour before, checked at that resume, and nothing else removes an older one: it stays held, unused, until it is pushed out or the server restarts. After the hour or a restart a resume signs in afresh, and since the CLI's files stay in one folder, a resume on another account finds its conversation. The token travels only on the bridge's stdin: it is in no protocol type, record or log. A command that fails, takes over 30 s or prints nothing fails the open with a sentence naming it; its stderr goes to the server's. Without the key the CLI signs in from the server's own environment, as before (flow 41).
- **What goes:** `bridge/tools.ts`, `bridge/prompt.md` and `zod`. `@modelcontextprotocol/sdk` 1.32.2 joins `package.json` for the two request schemas; it is already in the lock as the Agent SDK's own dependency (RULES.md 20).

### Account usage

Each conversation asks `usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET({ skipBehaviors: true })` at init and after turn ends, at most once a minute and with only one request in flight. Valid readings become percent values with millisecond reset times. `model_scoped` is additive and filtered by the SDK's overage allowlist (`sdk.d.ts:4475`), so it cannot replace unrelated legacy Opus/Sonnet readings. A scoped row with the same model name replaces that legacy reading; equal utilization and reset times alone do not establish that differently named models share one limit. The all-model 5-hour and 7-day readings remain separate; OAuth-app readings are shown too.

A thrown call, unavailable result or changed shape leaves `rate_limit_event` in charge. Those events update only the named windows, preserving the rest; a status without a measurement retains the window's utilization and reset time and sets only `rejected`. The current CLI also emits `unifiedWindows` on that event although the pinned SDK's type omits it; validated known windows there provide the 5-hour and 7-day fallback. Per-event utilization is fractional; the usage answer is percent. A rejected window carries `reason`, why extra usage did not cover it, from the event's `limitScope` (`group_pool`), `overageDisabledReason` and `errorCode`, and `overageEnabled`; the status pane's alert shows it. The usage answer's `extra_usage.is_enabled` is not used: it carries no rejection to explain (flow 41). At most 32 names of 64 characters are retained per conversation, not a history.

Live probes on 29 October returned `rate_limits_available: false, rate_limits: null`, including after a real successful Opus turn. That turn's event did report `unifiedWindows.five_hour` and `.seven_day`; no model-scoped window was available to verify or invent. The GPT probe returned a 10,080-minute primary reading and a zero-length secondary window; the latter is not a measured 0%.

The reason is the sign-in: a setup token carries only `user:inference`, and the usage answer needs `user:profile`. So the model rows have a second source. `[claude] usage_file` (a machine key, read live, made absolute like the other path keys) names a JSON file, kept by a program outside Razor, holding each account's plan usage. At each usage check, before the CLI's answer so that a live answer replaces the file's rows, the bridge reads at most 1 MiB of it and finds `accounts.<realm>.<account>` for the conversation's `account`. It measures only that row's `weekly_scoped` limits with a string `model` (the per-model weekly row, `Fable` among them; `percent` 0-100, `resets_at` in epoch seconds), and only when the file's `observed_at` is within 20 minutes of now and the row's `status` is `ok`. A missing file or account, bad JSON, a stale file or a missing row measures nothing and changes nothing. It never supplies the 5-hour or 7-day rows: each reply's rate-limit event gives those, fresher. The `usage` line goes out when either source measured. Razor only reads the file and holds no new secret (flow 41).

### What the Claude profile gives the bridge

`Profile::tools` for `Transport::Claude`. Bytes are each declaration's compact JSON in a captured request (run i).

| entry | the model sees | bytes | runs |
| --- | --- | ---: | --- |
| `{"builtin":"Read"}`, `Edit`, `Write` (`Grep` and `Glob`, 3,239 and 731, left on 2026-11-02 after 38 calls and none in a week of 592 sessions) | Claude Code's own | 1,929; 969; 639 | Claude Code |
| `{"builtin":"Bash","to":"bash"}` | Claude Code's `Bash` | 3,441 | Razor's `bash` |
| `{"builtin":"TaskStop","to":"kill"}` | Claude Code's `TaskStop` | 789 | Razor's `kill` |
| `WebSearch` and `WebFetch`, always (2026-10-29; before, only without a `search` or `fetch` provider) | Claude Code's own | 866; 1,085 | Claude Code |
| served `bash`, `kill` | one line each, sending the model to `Bash` and `TaskStop`; the SDK declares alias targets too (run hide) | about 120 each | the aliases' targets |
| served `terminal` | the hard decision, below | about 1,130 | `bash` with `pty`, `send`, `wait` |
| served `recall`, `handover`, `agent`, `message`, `status`, `memory`, `task`, `deliver`, `skill` and the web verbs | the declarations both GPT modes share, from `data/tools.json` (2026-11-02; before, `claude.json` held copies, five of which had drifted); a child's `memory` and `task` offer only their reads | about 10,000 | the controller or the tool service |
| a scripted or MCP tool | `mcp__razor__<name>`; `readOnlyHint` only on Razor's own read-only actions | its own | the tool service |

The first request's declarations total about 16,000 bytes, roughly 4,000 tokens. On 2026-11-02 they measured 29,403 bytes for a top-level session and 27,458 for a child, Claude Code's Monitor's 7,623 among them, since dropped.

**`Profile::input` maps the Claude shapes onto the tool service's actions:**
- `bash` from `Bash`: with `run_in_background`, `{command, wait_ms: 0, deadline_ms: timeout or 1,800,000, at most 7,200,000}`; without, `{command, wait_ms: timeout or 120,000, at most 600,000, deadline_ms: wait_ms + 1,800,000}` (the numbers Claude Code's declaration states, sdk-tools.d.ts 816-849). `description` stays in the `Call` for the notice; `dangerouslyDisableSandbox` is ignored.
- `kill` from `TaskStop`: `task_id`, or the deprecated `shell_id`, as `job-N` or `N` (sdk-tools.d.ts 988-997).
- `terminal`: with `command`, `bash {command, pty: true, wait_ms: timeout_ms or 10,000, pattern, quiet_ms}`; with `job` and one of `text`, `key` or `secret_file`, `send`; with `job` alone, `wait`.

**`Profile::result` shapes each result as Claude Code's own `Bash` does, in Razor's own words** (runs b, c, d; reworded 2026-11-02 for the public release): an ended command's output after "Exit code N", `error` when N is not 0; a running one as "Running in the background as job-N, writing its output to <path>…", or after a foreground timeout "Still running after Ns, so it moved to the background…"; a spill as `<persisted-output>` with the end of the output, only when the job ended and its output spilled past `SPILL` (2026-10-29; before, any output over 2,000 characters); `kill` as "Stopped job-N."; `terminal` as a status line, then the screen; the job notice as Claude Code's `<task-notification>` with Razor's job id, the call's id, the output file, the status and the call's `description`.

## Data flow

1. The controller sends `Open`: the session, its working directory, model and effort, a resume point or none, and the profile's prompt and tools.
2. The bridge splits `tools`: Claude Code's own into `tools`, the aliases into `toolAliases`, Razor's onto the conversation's MCP server, raw and always loaded.
3. An `Input` batch becomes one SDK user message; each input's images precede its text, without moving later images ahead of earlier text.
4. Every call waits in `PreToolUse` until its `Call` is appended (`Admitted`).
5. A call to a served tool, by name or through an alias, arrives as `Run`. The controller maps it (`Profile::input`), runs it, words it (`Profile::result`) and answers `Result`. The SDK's `tool_result` comes back as the `Result` entry, except the CLI's own for a call Escape stopped before Razor's answer reached the SDK, which carries no answer and is not mirrored; the turn's end records that call `Unknown` (controller.md).
6. The order is the SDK's: read-only calls (Claude Code's read-only tools, and Razor's tools marked `readOnlyHint`) run together and their results come back as they finish; every other call runs alone, in call order (a batch-order probe). SPEC's Tools paragraph says call order on every route.
7. No call steps aside on this route. The CLI lets a WebFetch or WebSearch call step aside for a person's message only while no `PostToolBatch` hook is registered (its detach gate, CLI 2.1.312), and the bridge registers one in every conversation; run detach saw no step-aside, hook or no hook. So every call's one `Result` is its real result.
8. A job whose call returned while it ran ends; if no result showed its end, the controller admits a Razor input in Claude Code's notice shape, and the model wakes.
9. `Add` serves a live addition and signals it; `Effort` changes effort in place.
10. `handover`'s `Result` carries `settle`; its batch settles, the turn ends (`Settled{usage}` from the SDK result), and the controller records the usage before rebuilding. A rebuilt context names each carried ask's image files by path, since the SDK's history holds images only until a rebuild.

## The hard decision: how Razor's job verbs reach Claude

**Decision: Claude Code's own shapes for everything it has a shape for, made true over Razor's shell, and one Razor tool, `terminal`, for what it has none for.**

What Claude Code's shapes already cover (CLI 2.1.312):
- **Start:** `Bash` in the foreground with a `timeout`, or with `run_in_background`. A foreground command that reaches its timeout is moved to the background, not killed (run c).
- **Look:** "To check interim output, use Read on that file path" (run b).
- **Wait:** a background command's end "re-invokes you", as a `<task-notification>` (run b).
- **Stop:** `TaskStop`, by id.

So the model already knows the shell contract's handle, peek, end notice and kill. It has no shape for a terminal, keys, a screen or secret input.

**The options:**
- **A. Razor's own tools for every verb** (`wait`, `peek`, `send`, `kill`, and a way to start under a terminal), beside the aliased `Bash`. The model gets four or five names it was not trained on, and two accounts of the same job: Claude Code's `Bash` declaration, which the model sees whatever Razor declares, tells it to Read the output file and wait to be notified, so Razor has to make that account true anyway. About 2,000 bytes.
- **B. Claude Code's nearest tools, made true, plus one Razor tool (chosen).** The model knows `Bash` with its timeout and background, `TaskStop` aliased to `kill` (run a), `Read` on the output file, and the notice: the loop Claude models run in Claude Code every day. Making it true takes no bridge code: the mapping and wording are the profile's data, the notice is the controller's one admission arm, and `CLAUDE_BASH_MAINTAIN_PROJECT_WORKING_DIR=1` makes the declaration say a `cd` does not carry over (run b2). About 2,040 bytes. Razor's `monitor` tool starts an ordinary background job (Trey, 2026-11-02): command, description and timeout_ms, a 300,000 ms default capped at 1,800,000 ms. A command exits to produce the usual end notice, pointing to its output file; per-line events are not forwarded, so Claude Code's own Monitor, whose declaration promises them and cost 7,623 bytes, is not offered (2026-11-03). The until-loop waits recommended by Bash print their event and exit, so the end notice wakes the agent to read the event without adding a notification mechanism.
- **C. Fold the verbs into `Bash`'s input fields.** Rejected: the model sees Claude Code's `Bash` schema, which Razor cannot change, so the fields would be taught only by the prompt, and they would give `Bash` a second job model beside `run_in_background`.

**The prompt-cache cost** is the same for all three: every declaration is in the first request and read from the cache after it. What differs is turns. For the evaluation's task 7 (a long job), B's backgrounded command costs no polling turn: the notice wakes the model and one `Read` gives the output. For task 8 (an interactive program), B's `terminal` makes each prompt and its answer one call, Codex's `write_stdin` shape, so an installer with N prompts takes N + 1 calls; A needs a `send` and a `wait` per prompt.

**What `terminal` is:**

```
mcp__razor__terminal {command?, job?, text?, key?, secret_file?, pattern?, quiet_ms?, timeout_ms?}
```

It runs a program under a real terminal, 40 rows by 120 columns. `command` starts it as `job-N`. `job` first types one of `text`, `key` (the shell's key names) or `secret_file` (a file whose contents are typed and never recorded), then waits until `pattern` shows, the screen has been still for `quiet_ms`, the program exits, or `timeout_ms` passes (default 10 seconds), and returns the screen. `job` alone returns the screen at once; on a job started with `Bash`, it waits on that job's output. About 130 words of description.

## Where this departs from the spec or a neighbour

1. **Claude Code's `Bash` declaration is not entirely true of Razor's shell.** It says the shell "is initialized from the user's profile" and that foreground `sleep` is blocked; Razor's bash reads no startup files and blocks no `sleep`. Neither steers the model into harm, and only the CLI could change the text.
2. **No Claude Code hooks run** (`settingSources: []`). SPEC section 3 says existing hooks "run once, through the SDK"; none is worth running here, and whether that sentence goes is undecided.

## Flows and the proof

Flows on this route: 6 (exit 7 and the 40 KB spill, in Claude Code's shapes), 7 (`Bash` with `run_in_background`; `terminal` with `job` and `pattern`; `terminal` with `job` alone; `TaskStop`; a second job's notice), 8 (`terminal` with `text`, `key` and `secret_file`), 9 (three `Read`s together, two `Edit`s in order), 3 (the image block before the text), 27's tool half (an added scripted tool offered inline and run), 29's second half (`WebSearch` and `WebFetch` in the tools with no provider). Flows 6 and 7 are rewritten for these names and the revised messages here.

The Claude proof, against a real sign-in, has the runs that need no controller: the cache reads around a live addition (and the control with tool search off), an image reaching the model, the rate-limit line, and the claude.ai connectors finding. Still owed there: a background job waited on and killed, a notice waking an idle session, and an installer driven by `terminal` with a secret typed from a file.

## Second pass (2026-10-28)

The Rust task has one path for emitting session-stamped entries, runs and failures, and one lookup per nested admission instead of checking membership before looking up the same call. A handled result is consumed by value, so its call id and error output need no extra copies. The process and routing files stay: their byte permits, restart handling and open-number routing earned their lines.

The TypeScript bridge owns each conversation's state in one class rather than passing a mutable object through free functions. The query is initialized without a cast from `undefined`; admission uses Node's standard promise resolvers instead of a second hand-written promise type.

## The startup list and native web (built 2026-10-29)

The bridge's part of the startup list. Everything the model is given still comes in `Open`; what fills `Open.prompt` is now the startup list (docs/design/profiles.md), so the bridge itself changes by at most one line.

- **`settingSources: []` stays.** The person's and the repository's instruction files reach the model through the startup list, in the person's order, on every route; the CLI's own loading would add them a second time, in its order, and its hooks with them.
- **`WebSearch` and `WebFetch` are always in `Open.tools`**, as `{"builtin": …}` entries; the bridge passes them to `tools` as it does today. They run inside the CLI on the subscription, are admitted through `PreToolUse` like every call, and their `tool_result` is mirrored as their `Result`.
- **The prompt.** `systemPrompt` is `Open.prompt`, the startup list. The empty-prompt branch that handed the SDK Claude Code's preset went with `claude_prompt` once the prompt A/B was decided (the evaluation design): one line changed.
- **A resumed conversation** keeps the system prompt it opened with. The controller reads the list again at the open, but in a probe (2026-10-29, `sessions.idle = 0`, `AGENTS.md` edited between two turns) the pinned SDK's request after the resume carried the original prompt. A changed file reaches the model in a fresh conversation or at a rebuild.

Flows: 06 (a 2,560-character `Bash` output comes back inline), 29 (`WebSearch` and `WebFetch` in the first request with `[web]` naming both verbs). The live check, 2026-10-29: a Claude turn (sonnet, a scratch server with every `[web]` key unset) called `WebSearch` and answered from its results.

## A resume the CLI refuses (29 October)

claude-slice.md says a resume that failed is rebuilt (`Restart`) and the turn goes on; it ended the turn in the CLI's error instead. Reproduced on main with flow 41's setup and the CLI's conversation files deleted before `--resume`, the same refusal a probe got from a token with a `CLAUDE_CONFIG_DIR` of its own. The resumed conversation's SDK messages, logged in a scratch copy of the bridge: `command_lifecycle`, then `result` (`error_during_execution`, errors `No conversation found with session ID: …`), then the SDK's `Query closed before response received`; no `system init`. The bridge sends that result as `turn_end` with `success: false`, so the controller ends the turn with the error, and its rule for a resumed conversation that never spoke (`closed`, `session.rs:620`) never runs: it acts on `closed` only.

The fix is in the bridge: a `result` before the conversation's `init` is the CLI refusing to start, not the end of a turn, so the bridge closes the conversation and sends `closed` with the CLI's words. The controller's existing rule then rebuilds from the record and the turn goes on. A fresh conversation the CLI refuses still ends its turn with those words, now through `closed`.

The choice, by what each costs:

- *The bridge translates* (chosen). The SDK's message order is the bridge's knowledge, and `closed` means "the conversation ended by itself", which this is. An API failure on the first request after a resume that loaded comes after `init`, so it still ends the turn as a failure.
- *The controller treats a failed `turn_end` while still resuming as `closed`.* About three lines in a controller at 2,485 of 2,500, and it would rebuild on any model failure in the first request after a resume.
- *Match the CLI's sentence* (`No conversation found`). A vendor's wording as a contract.

Flow 41 deletes `.razor/claude/projects` and resumes once more: the `--print` answers, exit 0, and the record holds a `Restart` rebuild. Then, with the session's folder moved away so no CLI can start, the resume rebuilds once and the fresh conversation's failure ends the turn: no second rebuild.
