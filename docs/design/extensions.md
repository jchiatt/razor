# Extension surface

Plugins handle four before-events and observe the record's entries. A command hook receives JSON on stdin; a JavaScript file runs in the QuickJS engine already shipped for code mode. Both are listed in config and hot-reloaded. The second pass keeps the controller's public surface unchanged.

## Public surface

```rust
/// In config (`[[plugins]]`), read here: one configured file; array order is handler order.
/// Relative paths start at Razor's home.
/// `.js` selects JavaScript; other files run as programs. `~/` resolves under HOME.
#[derive(Clone, PartialEq, Deserialize)]
pub struct Plugin { pub file: PathBuf, pub before: Vec<Event>, pub after: Vec<String> }
/// Only these events may change or refuse pending work.
pub enum Event { Prompt, Input, Call, Rebuild }
/// Borrowed fields allocate nothing; only the last field of each variant may change.
pub enum Before<'a> {
    Prompt { model: &'a str, prompt: &'a mut String },
    Input { from: &'a Origin, text: &'a mut String },
    Call { call: &'a CallId, name: &'a str, input: &'a mut Value },
    Rebuild { trigger: Trigger, context: &'a mut String },
}
/// Cheap to clone; each event uses the current plugins.
#[derive(Clone)]
pub struct Plugins(/* shared configuration, current plugins and atomic event mask */);
impl Plugins {
    /// Starts empty; relative plugin paths resolve under home.
    pub fn new(home: PathBuf) -> Plugins;
    /// Loads at most 32 plugins, retaining the last good version after a failed edit.
    /// Returns a notice about failures or tool-name collisions, or None.
    pub async fn load(&self, config: &[Plugin]) -> Option<String>;
    /// Runs handlers in order under one ten-second deadline, including queueing.
    /// Returns whether a handler changed the field; refusal leaves the borrowed field unchanged.
    /// With no handler: one atomic read, no allocation and no lock.
    pub async fn before(&self, session: &SessionId, cwd: &Path, event: Before<'_>) -> Result<bool, Error>;
    /// Queues an entry before returning. Observer errors reach stream and stderr, not the record.
    /// With no observer: one atomic read and no allocation.
    pub fn after(&self, session: &SessionId, cwd: &Path, line: &Arc<str>, stream: &Stream);
    /// The first plugin in config order owns a duplicate tool name.
    pub fn tools(&self) -> BTreeMap<String, Tool>;
}
/// A declaration bound to its owner; each call resolves that plugin's current version.
#[derive(Clone, PartialEq)]
pub struct Tool { pub description: String, pub input: Value, /* private name and owner */ }
impl Tool {
    /// Returns JSON or a sentence on failure, refusal, deadline or removal of the tool.
    pub async fn run(&self, input: Value) -> Result<Value, Error>;
}
/// Loads in this process: one registration per JSON line, followed by any failure notice.
pub async fn list(home: &Path, config: &[Plugin]) -> String;
/// A refusal's sentence or the underlying cause of a file/process/thread failure.
pub enum Error { Refused(String), Other { plugin: String, source: Box<dyn std::error::Error + Send + Sync> } }
```

## Event contract and call sites

A configured hook's `before` and `after` arrays default to empty; a JavaScript file registers its own events. Before payloads contain `before`, `session`, `cwd` and the variant's fields. Each handler sees its predecessor's permitted change. A returned field must keep its JSON type; omitted fields leave it unchanged. A call's input has the record's shape: decoded function arguments, or verbatim custom source or malformed arguments. No change reaches the borrowed field until every handler succeeds.

| Event | Controller site | Field and destination | On refusal |
| --- | --- | --- | --- |
| `input` | `Session::admit`, before the size check and append | `text`, to the record and every delivery path | sender gets the sentence and the stream gets `Live::Plugin`; no entry or state change |
| `prompt` | `Session::open`, after the resident persona | `prompt`, to the model only | opening fails and pending inputs end with its error |
| `call` | `Session::entry`, before the call append | `input`, to the record and action through `Admitted.input` | the original call is recorded and answered with an error result; the action never runs |
| `rebuild` | `Session::rebuild`, before its append | `context`, to `Rebuild.context` and the model | the closed conversation stays closed; the turn fails and the next input rebuilds again |
| appended entries | `Session::append`; `Session::run` also offers a new record's `start` | observe only, exactly the stored line | stream and server.log get the sentence; the record does not |

After payloads are `{"session": …, "cwd": …, "entry": {…}}`; `entry.kind` selects handlers. Entries cover session start; admitted inputs, mail and resident wakes; responses, text and thinking; calls and results; rebuilds; children; jobs; headlines, changed files and sent mail. A child's report is its parent's input; memory writes and deliveries are their calls and results. The record has no session-end entry: a session can always receive another input. Razor's continuation request and job notices are appended directly and raise only after-events.

`Admitted.input` carries changes through both model paths, including nested code-mode calls. The Claude bridge's gate answers `updatedInput` or `deny`. Plugin tools use `Kind::Plugin` in the registry, whose one accepted owner controls execution; a tool rejected for colliding with a built-in or another tool cannot run. No controller or bridge call-site signature changes in the second pass.

## Handler contract

A hook runs in the session's cwd with the server environment and its own process group. Exit 0 allows; stdout is empty or a JSON object containing the changeable field. A nonzero exit refuses with trimmed stderr, or its exit status when stderr is empty. Stdout is capped at 4 MiB and stderr at 4 KiB; reaching a cap closes that pipe. Observer stdout is ignored. Dropping a timed-out run kills its whole process group, not just its leader.

JavaScript has no imports, timers, files or network. Registration is available only while its module loads:

```js
razor.before(event, handler)                       // prompt | input | call | rebuild
razor.after(kind, handler)                        // an entry kind
razor.tool({ name, description, input }, handler)  // input schema root: object
razor.command(name, description, handler)          // /name rest -> input text
razor.prompt(text)                                // append a prompt section at open
```

A handler returns the payload, changed or not, or nothing; a throw refuses with its message. Tool handlers return JSON. Promises run to completion; a promise with no remaining work to settle it refuses. Pending jobs are drained within the same event even when its returned promise already settled. Slash commands are input handlers for person inputs; prompt sections are prompt handlers. Each plugin has one thread, a 64 MiB heap and a 1 MiB stack. Its source file is capped at 1 MiB.

One ten-second deadline covers a before-event's entire chain, including time waiting in a JavaScript queue. Tools have the same limit. Loading has one limit covering the file read, thread start and evaluation; the second pass extends the first pass's evaluation-only timer to cover that whole load. An observer's deadline starts when dequeued, not when offered. Each queue holds at most 256 jobs; a full observer queue drops the entry and reports it. Before-events and tools wait for room within their deadline. JavaScript runs everything FIFO; hooks run observed entries FIFO but before-events directly in their callers, so different sessions' hook calls may run concurrently. A removed runtime finishes already-queued jobs and exits. Tools retain a plugin file and name, not an old runtime.

## Data flow

1. The controller reads the live config and calls `load`; its notice joins the config notice.
2. `load` serializes reloads; a JavaScript file is reread when its mtime or length changes.
3. The last good version survives a failed edit; removed plugins leave the current snapshot.
4. The snapshot and one atomic mask select the current handlers without loading files at dispatch.
5. `before` serializes only past the mask check, then runs plugins in config order.
6. A hook receives one process; JavaScript receives one queued job and replies through a channel.
7. Only successful before-chains write the permitted field back to the pending work.
8. Every append queues its line before a later event or tool from that session can arrive.
9. At turn start the registry receives bound tool declarations; a call resolves their current owner.
10. `razor plugins` loads locally and prints registrations without needing a server.

## The hard decision and remaining departures

The four before-events and record-based observation remain the chosen surface. Pi-shaped per-request mutation would need seams inside the SDK's own request rendering, which Razor does not own, and would let requests drift from the record. Mutating every append instead would change results after the action already ran. The split keeps changes at the four handoffs and observation at one append site. With no plugin, a call and its result add three atomic reads in total, no locks or allocations.

Four provisional readings of SPEC still need Trey's ruling: prompt plus inputs stand for request hooks, not every follow-up request; plugin errors may be unrecorded transcript lines; JavaScript gets events and tool registration, not API operations; a hook calling its own session's API is unsupported and stalls until its deadline. These are not new second-pass decisions.

Other existing costs remain: input changes replace the person's words in the record, including rebuild's verbatim asks; the record does not separately mark plugin edits; observers cannot change results or request continuation except by submitting input through the API; plugin tools are never read-only; the ten-second limit is a constant until a real plugin needs a live setting. Claude Code's own hooks depend on the bridge's setting sources, outside this module. Commands use Razor's JSON/exit-code contract, not Claude's hook field names or non-blocking exit codes.

## Second-pass accounting and proof

No dependency was added: QuickJS, Tokio, libc and serde_json were already shipped. The rewrite removes the separate tool-declaration type, constructs loaded plugins in one place, shares I/O error conversion, and makes the JavaScript bootstrap return one callable for registration and dispatch. Public signatures and controller call sites are unchanged. Runtime registration and event behavior are retained. Deadline handling is tightened: the load timer covers source I/O, and an expired job is refused before a hook starts or JavaScript dispatch runs, even if the caller has not yet closed its reply channel.

Flow 27 checks admission expansion, call refusal, ordered observation, plugin tool execution and listing. Additional scratch scripts outside the repository exercise changed calls through the real Claude bridge and Responses loop, prompt sections, rebuilt context, observer failure and whole-group deadline cleanup. The kill assertion first proves that both the hook leader and its child exist in the hook's own group; disabling only `killpg` makes that assertion fail on the surviving child. A counting allocator measured 0 allocations over 1,000 no-handler rounds in each version; a handled observer event counted 7 allocations in each. Serializing before the empty check red-proved the assertion with 18,000 allocations. A blocked-source probe found the first pass's notice arrived only when released at 12 seconds; the rewrite emitted its timeout at 10.01 seconds. A source-level scratch probe offered 64 already-expired JavaScript jobs with their reply channels still open: the first pass ran all 64 handlers, the rewrite ran none. An expired hook likewise gets the deadline sentence without trying to start its program. No tests or stand-ins for Razor code are committed.

Glossary lines still proposed by the first pass: **before-event**, one of the four mutable handoffs; **after-event**, an appended entry offered for observation; **handler**, a hook run or JavaScript function for one event.
