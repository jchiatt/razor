# Profiles

`src/profiles/` shares the context/prompt/profile row; target 1,200 lines including data. A profile binds declarations to tool-service actions; it owns neither actions nor transport I/O.

## Public surface

```rust
/// Wire choice, not model family; GPT code mode is always Lite.
pub enum Transport { Claude, Responses }
/// The controller's one source for route and context window; Claude's window is the SDK's.
pub struct Model { pub transport: Transport, pub window: Option<u64> }
/// Refuses a model not listed; GPT names and windows are Codex's catalog at the pin.
pub fn model(name: &str) -> Result<Model, Error>;
/// Versioned declarations; version names the shipped data, the route and the code-mode choice,
/// and on Claude code mode the web verbs (web.md).
/// `prompt` is empty from `load`; the controller sets it from `startup::opening` (below).
pub struct Profile {
    pub version: String, pub prompt: String, pub tools: Vec<serde_json::Value>,
    pub nested: Vec<Declaration>, pub code_mode: bool, /* claude, wording, route */
}
/// A JavaScript binding's displayed name, description and TypeScript input declaration.
pub struct Declaration { pub name: String, pub description: String, pub input: String }
/// A refused model, configuration or call; Other retains source and context.
pub enum Error { Invalid(String),
    Other { source: Box<dyn std::error::Error + Send + Sync>, context: &'static str } }
/// Bind built-ins to pinned data and scripts/MCP to their supplied declarations, beside the
/// maker's own web tools.
pub fn load(endpoint: &str, model: &str, transport: Transport, code_mode: bool,
    actions: Arc<[tools::Action]>) -> Result<Profile, Error>;
impl Profile {
    /// Validate and normalize one call; failure becomes its Error Result, not a lost response.
    pub fn input(&self, name: &str, input: serde_json::Value)
        -> Result<(String, serde_json::Value), Error>;
    /// Nested/GPT JSON is unchanged; Claude direct shell gets trained text. The flag means error.
    pub fn result(&self, name: &str, input: &serde_json::Value,
        output: serde_json::Value, nested: bool) -> (serde_json::Value, bool);
    /// Job notices are input, not another Result; output includes absolute path and call_id.
    pub fn notice(&self, name: &str, input: &serde_json::Value,
        output: serde_json::Value) -> String;
}
/// One native conversation's wire input; resumes from indexed native additions/output only.
pub struct Conversation { /* prefix descriptor, items, pending additions, lite */ }
impl Conversation {
    /// Fresh prefix, or exact recorded prefix plus wire history; neither reads the record.
    pub fn new(profile: &Profile, tools: Vec<serde_json::Value>) -> Self;
    pub fn resume(profile: &Profile, items: Vec<serde_json::Value>) -> Result<Self, Error>;
    /// Descriptor first, wire items after it: the payload of Resume::Responses.
    pub fn snapshot(&self) -> Vec<serde_json::Value>;
    /// Appends the declarations whose bytes differ from the latest ones sent.
    pub fn offer(&mut self, profile: &Profile);
    /// Appends a request-side item: an input or a call's output.
    pub fn send(&mut self, item: serde_json::Value);
    /// Appends a completed response's items; returns the additions it was the first to carry.
    pub fn response(&mut self, items: &[serde_json::Value]) -> Vec<serde_json::Value>;
    /// The whole request body; the caller refuses one over 64 MiB.
    pub fn body(&self, model: &str, effort: Effort, session: &SessionId) -> Vec<u8>;
    /// What a `web.run` call sends as its recent input: the last two user messages and the
    /// assistant text between them, text parts only, each at most 4,000 characters (its end).
    pub fn recent(&self) -> Vec<serde_json::Value>;
}
```

## Data flow

1. `model` lists the GPT names and windows of Codex's pinned `models.json`, GPT-5.6 and newer; Claude's window is None, using the SDK's reported one. Versioned data holds prompts, declarations and result wording. GPT and nested calls retain tool JSON; Claude direct Bash/terminal/TaskStop and job notices use the trained shapes in claude.md. The controller supplies a job notice's absolute job-spool path and call_id; a result's `path` is the shell's own, set when the output spilled or was cut or the job still runs.
2. `Tools::actions()` supplies `Action { name, read_only, session, schema }`; built-ins use profile data, scripts/MCP use `Schema {description, input}`. There is no `added` counter.
3. Compare declaration name and bytes with the conversation's recorded prefix/additions; offer only absent/changed declarations. Code mode appends the updated exec definition. Restarts use the same comparison; removal is refused by name until rebuild.
4. Persist the initial prompt/tools as a native request-prefix descriptor, and the exact rendered user inputs and direct results sent with each completed request before its output. Resume reads only the record's indexed native entries; it never re-renders Body::Input or includes merely admitted mail. Function/custom outputs keep their call kind. Nested calls have no wire outputs.
5. Codex retains exec_command, write_stdin, raw apply_patch, exec and wait at **aa854dabf54e1dd6c2bda80653944b4f2c0672d7**. TypeScript input declarations use the pinned schema renderer for built-ins, scripts and MCP. Shared MCP Types are included even with no MCP tools, as Codex's code-mode-only builder requires; MCP output schemas are not supplied to the profile.
6. Remove sandbox/approval, alternate-shell/login fields; the only added shell control is `secret_file`. `write_stdin` maps session_id to job and chars to text; no route exposes `secret:true`. `max_output_tokens * 4`, capped at SPILL, feeds the single end-only spill.
7. In code mode `handover` is nested in exec like every other Razor tool, and only exec and wait are direct (5 November; before, `handover` was direct-only: see code-mode.md). The controller appends Call, validates input, runs session actions itself, and otherwise calls lock-free `Tools::run(at,name,input)` under its own FIFO read/write lock. Exec/wait never hold that lock.
8. GPT/nested shell results stay `{job,end,output,path,cursor,wall_time_seconds}` unchanged; declarations name that shape instead of Codex's different wrapper. Nonzero/signal/unknown job ends set the Result's error outcome.
9. GPT code mode off uses four plain Responses functions read/write/edit/bash plus Razor controls. Both evaluation arms have the same model, endpoint/auth and actions; code mode uses Lite, plain standard Responses. API-key routes refuse code mode.
10. Claude code mode uses the same exec/wait descriptions and schemas through `Open.tools`, as ordinary MCP declarations rather than Lite custom tools. Plain mode supplies the bridge's built-in aliases and served declarations. The SDK injects cwd/model/date/bypass-permission text and an image-source block; Razor's profile owns only what Razor sends.

## Decision: pinned declarations, not another executor

Keep family declarations separate from actions and wire choice. A universal declaration falsely equates raw custom input and JSON arguments; per-family executors duplicate file/process behaviour. GPT-off reuses the four plain declarations rather than Codex's direct tools: Codex itself is the third evaluation arm. Declaration wording changes keep the version so native conversations resume; changes to behaviour rerun the fifteen evaluation tasks. Version includes mode; Start/Rebuild record it, and native resume requires equality. The controller reads mode at turn admission and rebuilds on change.

GPT's system prompt is the pinned GPT-6.1 template, cutting the opening identity paragraph, the 'As Codex' personality paragraph, auto-review guidance, request_user_input_async guidance, the two compaction paragraphs, the orchestrator skill-loading paragraph, and Apps/Plugins sections (the template contains no apply_patch or sandbox section); keep the remaining text verbatim. Code mode keeps the exact batching line: "Batch independent searches and reads in one functions.exec using await Promise.allSettled([...]); inspect every result. Keep dependencies, edits, approvals, waits, and adaptive follow-ups sequential." Flow 28 estimates the rendered prefix and any request instructions at four characters per token and asserts the estimate under 12,000; no runtime prefix limit or silent shortening. Exec emits Codex's status lines verbatim; its description says QuickJS instead of V8.

Source at the pin: `core/src/tools/code_mode/{execute_spec,wait_spec}.rs`, `handlers/shell_spec.rs`, `code-mode-protocol/src/description.rs`, `models-manager/models.json`. Flow 28 checks declarations and native continuation. The GPT fifteen compare Razor code mode, Razor plain and Codex on the same tasks/model, recording effort/version/transport, finished, tokens/cache reads, turns and wall time. Count recorded Error Results naming uncaught omitted-helper ReferenceErrors; caught attempts are not observable.

## Second pass (2026-10-28)

The surface the controller, the configuration and the Claude task call is unchanged: `model`, `load`, `Profile`'s public fields and `input`, `result`, `notice`, `Declaration`, `Error::Other`, and `Conversation::{resume, snapshot}`. What only the Responses task called was reshaped for it: `Profile::decode` moved into the task, and `Conversation` lost its record folding for `send` and `response`.

Dropped or changed:

- gpt-5.5, the one Responses model without Lite, is gone as SPEC section 3 asks, and with it `Profile::lite` and every branch for code mode without Lite: GPT code mode is Lite. The Conversation still keeps a `lite` flag, because plain GPT is not Lite. `codex-auto-review` is gone too: at the pin it is Codex's hidden approval-review model, and Razor has no approval review. Bare `gpt-6.1` was never in Codex's catalog and is gone.
- The model list is a `match` in `model` instead of `data/models.json`, which the first version parsed on every call; the controller calls `model` for every action.
- `Error::Unsupported` folds into `Invalid`; nothing matched on it.
- Two actions declaring one name are no longer refused. Action names are unique in the tool service; the only collision left is a script named `exec_command` or `write_stdin` in code mode, which now shows twice in `exec`'s description, the script's binding shadowed by the built-in's mapping, instead of failing the whole profile.
- Claude code mode's nested results stay JSON. The controller always passes `nested: false`, so the first version gave a cell's `tools.kill` Claude Code's trained text, which the Claude task then failed to parse: the script saw `expected value at line 1 column 1`. A scratch scenario on the Claude route showed that error on the first version and the kill's JSON on this one.

## What a model is given at opening (built 2026-10-29)

A conversation's system prompt is the startup list; everything else Razor supplies at opening rides in the first input. Before this, the prompt was `data/claude.md` or `data/gpt.md` alone, a resident's persona replaced it, and none of the person's or the repository's instruction files reached either model.

### Surface

```rust
// ---- src/profiles/startup.rs (`pub use startup::{Opening, opening, save}`) -------------------

/// What a conversation opens with besides its tools.
pub struct Opening {
    /// Each entry's text in the list's order, joined by one blank line, with nothing of Razor's
    /// between entries; it holds the profile's route part exactly once.
    pub prompt: String,
    /// The head of the first input: the session's facts, a resident's identity record, and one
    /// line per entry left out.
    pub facts: String,
}

/// Reads the startup list for one opening. An entry is `razor` (the editable part, then the
/// profile's route part), `project` (in each folder from `root` down to `cwd`, its `AGENTS.md`
/// then its `CLAUDE.md`), a file, or a folder (its first 64 `.md` files by name). A line that is
/// only `@<path>` (relative to its file, or `~/`) is replaced by that file's text; one naming no
/// file stays as written. Each file and word is used once; a file is at most 64 KiB and the
/// whole at most 1 MiB; what cannot be used is left out and named in `facts`, never fatal. A
/// list without `razor` gets the route part after its last entry. Reads run on a thread of their
/// own; git has two seconds to name the branch. The facts name the session by the name it
/// answers to (`Summary::name`; docs/design/mail.md).
pub async fn opening(profile: &Profile, list: &[String], root: Option<&Path>, s: &Summary,
    identity: Option<String>) -> Opening;

impl Opening {
    /// The first input's prefix: the facts, the memory page, the skills, then the MCP servers,
    /// each part present at most once and separated by a blank line.
    pub fn page(&self, memory: String, skills: Option<String>, mcp: &str) -> String;
}

/// Replaces `dir/startup.md` with what an opening sent ahead of the first input: the system
/// prompt, the tool declarations as sent, and the first input's prefix, each under a heading
/// with its size. Written to a sibling and renamed, so a reader never sees half a file; a
/// failure is logged and never stops the opening.
pub async fn save(dir: &Path, prompt: &str, tools: &[serde_json::Value], page: &str);
```

`load` lost its `preset` parameter with `claude_prompt`. `Profile.prompt` is left empty, and a private `route` holds `razor`'s route part for this family and mode. Claude's `WebSearch` and `WebFetch` are always declared, in both modes; plain GPT always carries the hosted `{"type": "web_search"}`; GPT code mode always nests `web__run` (docs/design/responses.md). The `[web]` verbs are declared when `[web]` names a provider whose key is set (docs/design/web.md), and each verb's own description in `data/tools.json` tells the model to use its native web tool first and for routine work, and the verb only when that did not find what it needs, a page refused it, or a claim needs a second, independent check (Trey, 2026-10-29). The descriptions are the one place that is there exactly when a verb is, on every route and mode, and they name no family's tool. `Profile::result` spills a Claude shell result only when the shell gave it a `path` (spilled past `SPILL`, cut, or still running), as every other route does; the controller no longer stamps every Claude job result with its file. The version is `aa854dab-razor-6-…`.

### Data flow

1. Config: `startup = [...]`, a person's or pack's key (refused in a project's file), default `["razor", "project"]`; a resident's `[residents.<name>] startup` replaces it for that resident (`Config::startup_for`). Every child instead uses `[children] startup`, default `["project"]`, also a person's or pack's key. Paths are made absolute when the file is parsed: `~/` against `HOME`, others against the file's folder.
2. Every open (fresh, resumed or rebuilt) renders the profile, then calls `opening` with the session's list, the local root from its settings, its summary and a resident's identity record; the controller sets `profile.prompt`, then `Before::Prompt` plugins may change it, then the conversation opens.
3. `razor` expands to `data/razor.md`, then the route part. `razor.md` is family-neutral: the first lines and "How you work" of the old `claude.md` without "You are Claude", plus six things Codex's prompt says, in Razor's own words (Trey, 2026-10-29): finish the authorized work first so approval is the last step; "can you…" means do it; a message sent mid-work steers the task; no unrequested warnings or disclaimers; no wait over 60 seconds in one call; `rg` first. The last two went on 2026-11-02 (below).
4. The route part. GPT: `data/gpt.md` (exec lines filtered on plain GPT; Codex's visualizations part cut, Trey 2026-10-29, since Razor's terminal draws neither Mermaid nor interactive visuals; lines false in Razor corrected, 2026-11-02, below; the rest verbatim), then `data/record.md`. Claude: `data/tools-claude.md` (plain) or `data/claude-code.md` (code mode), then `data/record.md` with the route's name for `handover`. The design's `tools-gpt.md` and `tools-gpt-code.md` were not needed: Codex's text already explains its tools.
5. `project` walks from the local root to the session's folder (a worktree's own root, so a child reads its branch's files); outside git it is the session's folder alone. Every child gets the destination machine's child list, including a resident's child; resident threads keep their resident list.
6. A fresh open or a rebuild builds the first input's prefix with `Opening::page`: facts, then the memory page, the skills and the MCP servers, then the person's words or a child's role text and brief.
7. The same opening calls `save`, so `startup.md` always shows the latest fresh opening or rebuild; the API serves it at `files/startup.md`, and the terminal's `/startup` opens it in the detail view, through the reader that pages a job's saved output, which now takes a session file's name.
8. A resumed conversation keeps the prompt it opened with, whatever the files now say: Responses resumes its recorded prefix, and on Claude the pinned SDK's request after a resume carried the original prompt though the controller read the list again (a probe, 2026-10-29). A changed file reaches the model in a fresh conversation or at a rebuild, and `startup.md` shows it then.
9. The facts are data (`data/startup.json`): the session's name as Razor knows it and its session id (Trey, 2026-10-29, so an agent can sign its mail and be addressed), the date and time from `jiff`, the machine from `gethostname`, the folder, and the branch from `git symbolic-ref --quiet --short HEAD`.
10. A resident's identity record (`residents::identity`, docs/design/residents.md) follows the session's facts; its persona is an entry in its list.

### Decision: the shape for editing and replacing Razor's prompt

**One key, `startup`. Leaving `razor` out of the list is how its editable part is replaced, and then the route part ends the system prompt.** To edit, the person copies `src/profiles/data/razor.md` (or the text `startup.md` shows), edits it, and names the copy where `razor` was; to replace, names their own file or nothing. Codex's text and the tools-and-record part are never the person's to remove: SPEC section 3 requires Codex's batching instruction verbatim on GPT, and a model whose tools are not explained works worse.

| Option | Keys | Cost |
| --- | ---: | --- |
| **A. `startup` only; no `razor` puts the route part after the last entry** (chosen) | 1 | Replacing the editable part also moves the route part to the end, so on GPT Codex's 2,500 words follow the person's files, the reverse of Codex's own order (its instructions, then `AGENTS.md`). |
| B. `startup` plus `prompt = "<file>"`, which replaces the editable part inside `razor` | 2 | The route part keeps its place; but there are two ways to drop Razor's text (an empty `prompt` file, or no `razor`), and a list without `razor` still needs A's rule. |
| C. The route part outside `razor`, always first in the system prompt | 1 | Contradicts the expansion Trey agreed (editable, family, tools), and puts Razor's text ahead of the person's first entry; his own example puts his persona first. |
| D. Refuse a list without `razor` | 1 | Replacing then needs B's key anyway. |

The end, not the start, when `razor` is missing: the person's first entry stays first and nothing is inserted between entries, the two rules Trey gave about order. The start would match the order the makers' own harnesses use, which is the case against; a person who wants that writes `razor` first.

### Smaller decisions

- **The identity record goes in the first input, not the system prompt.** Then the system prompt is exactly the list plus, at most, the route part, and everything else Razor supplies is in the first input. Against: an identity is persona-like, and models weigh the system prompt more.
- **No `version` bit for the list.** A changed file takes effect at the next fresh open or rebuild; folding the text into `Profile.version` would make `refresh` read every listed file at each turn's admission.
- **No headers between files**, except the project's: each `AGENTS.md` or `CLAUDE.md` follows a line naming its path (2026-11-02, below), so an agent knows it already has the file.
- **`claude_prompt` is gone.** The prompt A/B ran on 2026-10-29 (the evaluation design): Razor's prompt matched the preset's quality at fewer tokens, so, as SPEC section 3 says, the question died with its key, `load`'s `preset` parameter and the bridge's empty-prompt branch.

### Budget and flows

Data files are not counted: `razor.md`, `tools-claude.md` and `record.md` replace `claude.md`; `startup.json`, `web-run.md` (Codex's `web_run_description.md` verbatim) and `web-run.json` (the pinned schema less `literature_search_query`, which no listed model supports) are new.

Flows extended, each passing, and each failing against the code before this change at a new assertion or at its refusal of the `startup` key, with a mutation for the resident's name and for the verbs' wording: 02 (the default list's order, an include in place and a missing one left as written, the facts with the root's name ahead of the memory page, `startup.md`; a list without `razor` with a folder and a missing file), 06 (a 2,560-character Claude shell result inline), 16 (a child's facts name it), 17 (a rebuild reads the list again and replaces `startup.md`), 21 (the resident's persona through its own list, its name and identity record in the first input), 24 (the memory page after the facts), 26 (`/startup`), 28 (`web__run` in exec's description and run as one search request; the prefix re-measured against the real backend at 8,785 tokens, and at 8,843 after later changes), 29 (`WebSearch`, `WebFetch` and the hosted tool always present; each verb says native first). Flows 22, 23, 30 and 37 name `startup` where they named `persona`.

SPEC section 5's 12,000-token bound on a child's prefix applies to Razor's shipped default list, which flow 28 holds. A person's list is theirs, and `startup.md` shows its size: an example list of a persona and a notes folder made a 52,607-byte system prompt.

### Rulings (2026-10-29)

1. The `[web]` verbs stay beside the vendors' own tools when configured, and the model is told to use the native tools first (Trey).
2. `claude_prompt` goes (after the A/B).
3. The 12,000-token bound holds for Razor's default list only.
4. `startup.md` and `/startup` are built (Trey called the command "a great idea").

## A variant names only what it changes (2026-10-30)

Until now an entry in a chat variant's `tools` replaced its action's declaration whole, so a variant that changed one sentence carried a copy of the rest: `bash-coord.json`'s `agent` and `message` were 4 KB copied from `tools.json`, and the next change to the shared wording would have reached every model but DeepSeek and GLM, which ship that file.

### Surface

No signature changes. In `load`, before any declaration is rendered, each entry of the variant's `tools` is set over the shared declaration of its `action` in this mode, the one `tools.json` gives: every key of the entry is a path of field names joined by `/`, a missing step is made, and the value replaces whole what is at its path. `omit`, `keys`, `name`, `description` and `schema` are one-step paths, and an entry's `description` is also its chat wording (`generic`, below) unless the entry names `generic`, so every variant written before this keeps its meaning. An entry whose action has no shared declaration in this mode starts from an empty one, as before.

```json
{"action": "read", "description": "Read a file by lines, at most 200 at a time. …",
 "schema/properties/limit/description": "Lines to read, at most 200.",
 "schema/required": ["path", "limit"]}
```

### Decision: field paths, not a JSON merge patch

| | Field paths (chosen) | JSON merge patch (RFC 7386) |
| --- | --- | --- |
| Lines | about 10: one loop over each entry's keys | about 14: a recursive function |
| Reads as | one flat line per change, naming where it lands | a sparse copy of the declaration, nested as it is; a published standard |
| Sets or adds | a value at any depth, an object whole by naming it | a value at any depth; never an object whole, since objects always merge |
| Removes | nothing; a variant replaces the parent instead | any field, with `null` |
| Variants written before | unchanged, and render the same bytes | the three maker aliases (`variants/*-maker.json`) must each list as `null` every shared property they lack (`path`, `additionalProperties`, the shell's six), and then take on any property `tools.json` gains |

The maker alias decides it. An alias is the maker's declaration, not ours with changes, so its schema must stay the maker's when `tools.json` changes. Paths say so by naming `schema`, which replaces it whole. A merge patch cannot say it: each alias would list our properties to delete and then silently take on every one we add, the drift this change removes, the other way round. What a merge patch has over paths is deletion, and no variant removes part of a shared declaration. Paths are longer for a deep change (`schema/properties/limit/description`), which costs little at a handful of changes per variant.

A corner both shapes share: on chat a web verb shows its `generic` text where it has one, so an entry that replaced a verb whole, with its own `description` and no `generic`, would show the shared chat wording instead of its own. An entry's `description` therefore sets `generic` too, unless the same entry names `generic`.

### Data and version

`bash-coord.json` loses `agent` and `message`, which now equal `tools.json`'s, and keeps its route and its three omits; `gemini-lean.json` holds only a route and does not change. Two evaluation variants carried the same kind of copy and are cut the same way: `coord.json`'s two copies equal the shared ones and go, and `gemini-window.json` keeps only what it changes (two descriptions, two property descriptions and `read`'s `required`). The maker aliases do not change. Every model's opening request, and every evaluation variant's, must render the same bytes before and after, so no profile version changes: a version names what a model sees, and a native conversation recorded before the change resumes after it.

## Fable and the short GPT names (2026-10-30)

Trey, 2026-10-30: "astra and fable need to be supported as Razor native subagent model choices." `fable` joins opus, sonnet and haiku as a Claude name the bundled SDK resolves; it is listed by `models()` and recorded as written, as those three are. The short GPT names (sol for gpt-6.1-sol, terra for gpt-6.1-terra, luna for gpt-6-luna, astra for gpt-6-astra) are one table beside `MODELS`. Terra joined the same night (Trey: "dont forget abt our other GPT friend terra!!"), and a ChatGPT sign-in served gpt-6.1-terra a short turn before it was listed. One function reads the table:

```rust
/// The id a model name stands for: a short GPT name's full id, any other name unchanged.
pub fn id(name: String) -> String;
```

Each place a name enters Razor calls it once: the API's session create and input (the terminal's `--model`, `/model` and picker reach Razor only through these two), the `agent` tool's start and continue, the config's `model`, in `Config::parse`, which every machine and project file passes through, and a resident's heartbeat or schedule `model`, as its input is made (src/residents). The controller, the record, status and requests therefore only ever see full ids. `model` still refuses a short name, so a new entry that forgot `id` is refused at once instead of recording a name nothing else knows; its refusal names the Claude and short GPT names. `models()` keeps listing full ids, so the picker shows each GPT model once. The `agent` tool's `model` description, in `tools.json` and `claude.json`, gains one sentence naming the choices. That changes the declaration GPT code mode carries inside exec's description, so flow 28's pinned prefix digest moves and needs the real token measurement again; no profile version changes, as with an earlier revision's change to the same declaration.

## What agents are told, corrected (2026-11-02)

No profile version changes: each change is wording or a declaration offered through the addition path.

- **One source for Razor's tool declarations.** `data/tools.json` holds every Razor tool's declaration; `data/claude.json` keeps only Claude Code's built-ins and the shell's aliases (`bash`, `kill`, `monitor`, `terminal`). `claude::tools` declares every other action whose shared declaration covers both GPT modes, so a wording change reaches every route at once. Five of the eight copies had drifted; each took the better wording.
- **Claude's `Grep` and `Glob` are gone**: 38 calls and none in a week of 592 Claude sessions, against about 70,000 `Bash` calls, for about 4,000 bytes in every request. No other declaration mentions either.
- **A child is offered what it may do.** `load` takes `child`; `data/child-tools.json` limits `memory` to reads and `task` to reads, claims and comments on existing tasks, through the same field paths as a variant, after the variant's own. The refusals stay for a call made anyway. The task declaration change keeps profile version `razor-7`: live conversations take changed tools in place, while a version bump rebuilds every open conversation at its next turn and loses its prompt cache.
- **GPT's image generation waits for `[profiles.codex] image_gen`.** Razor's MCP search finds only MCP servers' tools and is declared only beside a searchable server, so it cannot carry a built-in without a second kind of catalog; a live key costs three lines, and turning it on mid-session offers the tool at the next turn.
- **Project files carry their path** (`startup.json`'s `project` line), because 114 sessions re-read an `AGENTS.md` or `CLAUDE.md` they already had.
- **`gpt.md`'s lines false in Razor**: Codex's skill locations, `r0` aliases, `skills.read` and `skill://` became three sentences on the `skill` tool; the 60-second wait became a long `write_stdin` poll (up to 300,000 ms, returning when the command ends); `$CODEX_HOME` and the warnings line, which `razor.md` repeats, went. Its `rg` line stays: with `razor.md`'s gone it is GPT's only one, and GPT ran `rg` for 99% of its searches.
