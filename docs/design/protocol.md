# Protocol and types

Design page, 2026-10-27; second pass, 2026-10-28. Module `src/protocol/`, budget row "Protocol and types" (2,500 lines; first pass 605). The declarations below describe the original slice; `src/protocol/` also holds the landed children, API and model-path declarations.

`protocol` is the vocabulary the modules share: ids, the record's entries, inputs, states, job events, and the lines the API server streams. It is plain data with derived serialization. The only behaviour is making and parsing a session id, and reading the clock. Every module depends on it, and it depends on nothing else in the crate. Each JSON shape below is the wire form, used in the record file, in the API stream, in `--print stream` and on the bridge's pipe.

## Surface

```rust
// ---- ids ---------------------------------------------------------------------------------

/// A session's id: `YYYYMMDD-HHMMSS-xxxx`, the UTC second it started plus four random hex
/// digits. Sorts by start time as text, and holds only digits, `a`-`f` and `-`, so it is safe
/// as a directory name and in a URL path.
#[serde(try_from = "String", into = "String")]
pub struct SessionId(String);

impl SessionId {
    /// A fresh id for a session starting now. Two sessions started in the same second share
    /// an id once in 65,536; `Record::create` refuses the second and the caller draws again.
    pub fn new() -> SessionId;
}
/// Refuses any string not in the format above.
impl TryFrom<String> for SessionId { type Error = Error; }

/// A value `protocol` refused to parse.
pub enum Error {
    /// The text is not a session id.
    NotSessionId(String),
}

/// An entry's position in its session's record, counting from 1: dense, never reused. It is
/// the API's cursor and the argument to `recall`.
pub struct EntryId(pub u64);

/// A tool call's id as the model gave it (`toolu_…` on Claude, `call_…` on Responses).
pub struct CallId(pub String);

/// A job's number within its session, shown to the model as `job-N`. The shell gives it out;
/// at open the controller seeds the shell's counter from the record (highest seen, plus one).
pub struct JobId(pub u32);

/// Milliseconds since the Unix epoch.
pub type Millis = u64;

// ---- the record ----------------------------------------------------------------------------

/// One line of a session's record, and of the API stream.
pub struct Entry {
    pub id: EntryId,
    pub at: Millis,
    #[serde(flatten)]
    pub body: Body,
    /// What a model path needs to continue its own conversation, kept beside the common
    /// form and read by nothing else. Absent on entries Razor makes itself.
    pub native: Option<Native>,
}

/// What an entry says. On the wire it is tagged `"kind"`: `start`, `input`, `text`, `thinking`,
/// `call`, `result`, `job`, `turn_end`, `rebuild`, `response`.
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Body {
    /// The first entry of every session, and only there.
    Start { cwd: PathBuf, model: String, effort: Effort },
    /// An admitted input.
    Input(Input),
    /// Text the model showed.
    Text { text: String },
    /// The model's reasoning as it showed it; never empty. Reasoning a model path keeps only
    /// encrypted lives in its native data (on Responses, the `Response` entry).
    Thinking { text: String },
    /// A tool call's intent, in the record before the call's action starts. `name` is the name
    /// the model used (`Bash`, `Read`, `mcp__razor__recall`).
    Call { call: CallId, name: String, input: serde_json::Value },
    /// A tool call's result: exactly one per call, after its `Call`. `output` is what the
    /// model saw.
    Result { call: CallId, outcome: Outcome, output: String },
    /// A job's start or end, as the shell reports it.
    Job(JobEvent),
    /// A turn's end. `inputs` are the entries the turn answered: the `Input` and `Rebuild`
    /// entries not answered by an earlier turn, up to and including the latest one the model
    /// took. Each is answered exactly once. `closed`, omitted when false: the turn answered
    /// `/done`'s input, so the session is closed until its next input.
    TurnEnd { inputs: Vec<EntryId>, end: End, usage: Usage, context: Option<u64>, closed: bool },
    /// A rebuild: what set it off, the continuation note it carried, and the context it sent
    /// to the fresh conversation, verbatim. The context counts as an input: the turn that
    /// takes it answers it, and with it every input before it, all of which the context holds.
    Rebuild { trigger: Trigger, note: String, context: String },
    /// One completed request on a model path that renders its own requests (Responses), with
    /// `Native::Responses`. It is appended before any of its actions runs, and its `Text`,
    /// `Thinking` and `Call` entries follow it with no native data of their own.
    Response { usage: Usage },
    /// Sent mail keeps its id and kind separate from the containing entry's fields.
    MailSent {
        #[serde(rename = "mail_id")]
        id: MailId,
        to: String,
        #[serde(rename = "mail_kind")]
        kind: MailKind,
        /// First line, at most 200 Unicode characters.
        text: String,
    },
}

/// Anything that enters a session; every input goes through `Controller::admit`.
pub struct Input { pub from: Origin, pub text: String }

/// Who an input is from. Mail and schedules add variants.
pub enum Origin {
    /// A person, through any client.
    Person,
    /// Razor itself: the threshold's request for a continuation note.
    Razor,
}

/// How a tool call came out. `Unknown`: the call has no recorded result because Razor
/// stopped first, or the model path never reported one; it is never run again.
pub enum Outcome { Success, Error, Unknown }

/// How a turn ended. `Error` carries one sentence for the person.
pub enum End { Done, Stopped, Error(String) }

/// What set off a rebuild.
pub enum Trigger { Threshold, Handover, Restart }

/// A turn's token counts, summed over its model requests.
pub struct Usage { pub input: u64, pub output: u64, pub cache_read: u64, pub cache_write: u64 }

/// The effort a model is asked for.
pub enum Effort { Low, Medium, High, Xhigh, Max }

/// Native continuation data, one variant per model path that needs it.
pub enum Native {
    /// The SDK conversation an entry was mirrored from, and that message's uuid (the point a
    /// resume cuts at).
    Claude { conversation: String, uuid: String },
    /// On a `Response`: the request-side items first sent with that request (a live tool
    /// addition), then its whole output array (`encrypted_content`, item ids, `phase`,
    /// `call_id`, unknown fields), each byte for byte. The next request replays additions
    /// before output, so the cached prefix is unchanged.
    Responses { additions: Vec<serde_json::Value>, items: Vec<serde_json::Value> },
}

// ---- jobs: the seam with the shell ------------------

#[serde(tag = "event", rename_all = "snake_case")]
pub enum JobEvent {
    /// A job's session of processes started; `pid` leads it. `process_start` is the boot
    /// identity plus the OS start time of `pid`, so a reused pid is never mistaken for the job.
    /// The output file is the session's directory plus `job-N`; the call that started the job
    /// is the `Call` before it in the record, and the controller holds the link while the
    /// turn runs.
    Started { job: JobId, pid: u32, pty: bool, process_start: String },
    /// A job ended: every process of its session is gone.
    Ended { job: JobId, end: JobEnd },
}

/// How a job ended, as the OS reported it. `Unknown`: its exit could not be collected.
pub enum JobEnd { Exit(i32), Signal(i32), Unknown }

// ---- the API stream -------------------------------------------------------------------------

/// One agent's one state. The slice has two of the four; `Done` and `Stuck` arrive with
/// children and residents, which are the first to need them.
pub enum State { NeedsYou, Working }

/// One line of the API's event stream, and of `--print stream`: a record entry exactly as
/// stored, or a live line that is never stored.
#[serde(untagged)]
pub enum Event { Entry(Entry), Live(Live) }

/// Lines that exist only on the stream. Their kinds never collide with an entry's.
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Live {
    /// The first line of every attach, and again whenever the client fell too far behind:
    /// the session, its state now, and its last entry. The entries after the client's cursor
    /// follow it.
    Attached { session: SessionId, state: State, last: EntryId },
    /// Text as the model writes it. The `Text` entry for the same block supersedes it.
    Delta { text: String },
    /// The session's state changed.
    State { state: State },
}
```

A record line and a stream line, for the reader who will parse them:

```json
{"id":4,"at":1761603303123,"kind":"call","call":"toolu_01Hx","name":"Bash","input":{"command":"ls"},"native":{"claude":{"conversation":"6c2f…","uuid":"a1b9…"}}}
{"id":5,"at":1761603303180,"kind":"result","call":"toolu_01Hx","outcome":"success","output":"a\nb\n","native":{"claude":{"conversation":"6c2f…","uuid":"77d0…"}}}
{"id":7,"at":1761603305002,"kind":"turn_end","inputs":[2,6],"end":"done","usage":{"input":1200,"output":80,"cache_read":9000,"cache_write":0}}
{"kind":"delta","text":"Two files"}
```

## Data flow

1. A client's input reaches the API server, which hands it to `Controller::admit` as an `Input`.
2. The session's controller task appends it as `Body::Input`; the record gives it an `EntryId` and an `at`.
3. A model path reports what the model did as a `Body` plus a `Native` marker (the Claude bridge sends them as JSON lines in this shape), and the controller appends each one.
4. A `Call` is appended before its action starts. On the Claude path the SDK's `PreToolUse` hook holds every call, Claude Code's own tools included, until Razor has appended it (claude-slice.md).
5. The shell reports `JobEvent`s, which the controller appends as `Body::Job`.
6. Every appended entry is published once, as its stored line, to the session's attached clients. Text deltas are published as `Live::Delta` and never appended; state changes as `Live::State`.
7. A client that attaches gets `Live::Attached` first, then the entries after its cursor. A client that falls more than the stream's buffer behind is attached again from its cursor, so it gets `Attached` (with the state now) and the entries it missed; what it loses is only deltas.
8. The record file, the API stream and `--print stream` therefore carry the same entry lines, so the flows, the terminal app and the web app all read one schema.
9. `recall` returns an entry's stored line, unchanged, except a `Thinking` entry: the model's own reasoning handed back to it is refused upstream as reasoning extraction (Claude, 2026-10-29), so recall answers that entry with one sentence instead.
10. Nothing in `protocol` touches a file or a socket.

## The hard decision: what the API's event stream is

The choice is between one event per record entry and a separate projection made for clients. **Decision: the stream carries record entries verbatim, plus three live kinds that are never stored (`attached`, `delta`, `state`).**

- **Entries only.** The stream is the record and nothing else.
  - Writer: appending and publishing are the same act.
  - Reader: one schema.
  - Crash: a reattach replays exactly.
  - Cost: text reaches a client only when a whole content block completes, which on Claude can be paragraphs and seconds of nothing on screen. The state is never said; every client must work it out from the entries.
- **A separate projection** (message started, delta, tool started, tool finished, state).
  - Writer: a mapping per entry kind, plus a second schema to version. Every new entry kind needs a projection rule, which is a second definition of the same concept (RULES.md 5).
  - Reader: clients learn a shape the record does not have, and the flows must check two formats.
  - Crash: the projection has to be rebuilt from the record anyway, so it is either derived (the first option plus a mapping) or stored (a second log).
  - Later changes: two schemas move with every change.
- **The choice: entries plus three live kinds.**
  - Writer: the controller publishes exactly the line it appended, serialized once, and only three kinds are added.
  - Reader: one schema plus three kinds, the same lines the record file holds.
  - Crash, disconnect or overrun: one rule, attach from the cursor. It replays the record exactly and re-sends the state in `attached`; only deltas are lost, and each is superseded by its `Text` entry.
  - Later changes: a new entry kind reaches every client with no API change.
  - Cost: the record's line format becomes public API, versioned under `/v1`. Accepted, because the record's shape changes rarely and on purpose.

## Per-conversation account usage

`AgentView.usage: Vec<UsageWindow>` replaces the controller-wide `Limit` and `Live::Limit`. Each row holds at most 32 windows for its native conversation's sign-in, cleared when that conversation reopens. `FromBridge::Usage { windows }` replaces those readings as one snapshot; nothing is recorded or combined across sessions.

```rust
/// One account window: absent utilization is unknown, never a measured zero.
pub struct UsageWindow {
    pub name: String,                  // 5-hour, 7-day, or the server's model label
    pub utilization: Option<f64>,      // percent, 0–100 on both routes
    pub resets_at: Option<Millis>,
    pub rejected: bool,               // a reached limit produces an alert
}
```

A window is stale once `resets_at` has passed; there is no separate reading timestamp or SDK status string.

`Models.windows: BTreeMap<String, u64>` carries the profile table's known context windows so the client can draw an empty selected model's gauge without guessing Claude's window.

## Second pass: keep the declarations plain

The public types, variants, fields, functions and JSON forms stay unchanged. Keep the hand-written UTC date calculation: replacing it with a formatter saves source lines but adds formatting work and a new date-range failure case. A macro for repeated derives would hide the declarations every module reads; it would shorten the count, not the design.

Remove only work with no caller-visible effect: serialize `SessionId` directly as its newtype string instead of cloning it through `String`, remove redundant defaults on optional fields, and simplify re-exports and the single-cause error display. Remove `IMAGE_TYPES`, which no code in `src/`, `bridge/`, `flows/` references; the MIME strings on the wire stay unchanged. Keep the consuming `From<SessionId> for String` conversion public. No caller changes, dependencies or new vocabulary are needed. Compare serialized bytes for every record and wire variant, round-trip records produced by the binary, and run the same numbered flows before and after.

## A child's kind is not a protocol type

Trey, 2026-10-29: a reviewer or researcher differs from a worker only in the role text its first input carries, which the record already holds (`docs/design/children.md`). So `Child` loses `kind`, which nothing read after the start, and `ChildKind` leaves `src/protocol/children.rs` for a private enum in `src/children/agent.rs`, the one place that parses it. A record written before keeps parsing, since `Child` ignores a field it does not name.

## Glossary lines proposed

- **native conversation**: a model path's own conversation inside a session (on the Claude path, the SDK's). A rebuild starts a new one, and the record names it on each entry it mirrors.
- **cursor**: the id of the last entry a client has read. Attaching after it replays the rest.
