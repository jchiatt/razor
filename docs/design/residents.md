# Residents, schedules and the memory client

Design page, 2026-10-27; revised 2026-10-28 after a cold review. It covers two modules, `src/residents/` and `src/memory/`. Their budget row is "Children, mail, schedules, memory client" (6,000 lines); these two modules get 1,200 of it and children and mail the rest.

Read: `main`, the record, API and children pages; the rulings and the cold review; a resident's configuration and service units; the configured ask and notify commands; Memorum's source and `memoryd schema --json`.

A resident is a session whose id is its name. Its config is `[residents.<name>]`, and the one `razor serve` process runs it. The service that `razor install` writes starts that process and restarts it after a failure. One task in that process fires every resident's heartbeat, schedules and night, and each of those is either an input or a rebuild through the controller. A resident's state, its open asks and its waiting inputs are its session's, so this module holds none of them.

## Surface

```rust
// ---- src/config/residents.rs, read by this module -----------------------------------------
// `[residents.<name>]`, `ask` and `notify` are fields of config's `Config`: `residents` (each
// resident by name, its session's id), `ask` and `notify` (the programs that carry an ask and a
// notification to the person). Every key is live: the schedule re-reads the config on every wake
// (at least once a minute), and the rest are read at their next use.

/// One resident: plain config a pack can hold.
#[derive(Clone, PartialEq, Deserialize)]
pub struct Resident {
    /// Its home: its session's working directory, fixed when the session is made, as any
    /// session's is.
    pub cwd: PathBuf,
    /// Its own startup list, in place of the person's `startup`; its persona file is one of the
    /// entries, wherever the person puts it (docs/design/profiles.md). Absent: the person's list.
    /// Paths are made absolute when the file is parsed, as `cwd` is. (2026-10-29: replaces
    /// `persona`, which replaced Razor's whole prompt, so a resident was never told how Razor's
    /// tools work.)
    pub startup: Option<Vec<String>>,
    /// When its night comes, in local time. Default 03:00.
    pub night: Time,
    pub heartbeat: Option<Heartbeat>,
    /// By name; the first 64 by name are read.
    pub schedules: BTreeMap<String, Schedule>,
}

/// The input that wakes a resident at set times. A heartbeat that finds its resident busy is
/// skipped, and one missed while Razor was down is not made up.
#[derive(Clone, PartialEq, Deserialize)]
pub struct Heartbeat {
    pub at: Vec<Time>,
    /// The weekdays it runs on; empty is every day.
    pub days: Vec<Day>,
    pub text: String,
    /// For the turn this input opens only. Absent: the session's own.
    pub model: Option<String>,
    pub effort: Option<Effort>,
}

/// A named input that comes due at set times: Trey's "named job" ("job" is the shell's word).
/// One that comes due while its resident is busy waits for the turn to end. One that came due
/// while Razor was down runs once at the next start, if the record holds an earlier due time of
/// the same name.
#[derive(Clone, PartialEq, Deserialize)]
pub struct Schedule {
    pub at: Vec<Time>,
    pub days: Vec<Day>,
    pub text: String,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub deliver: Deliver,
}

/// What the schedule's turn is told to do with its result, in one sentence (from `text.toml`)
/// after the schedule's text. `Ask`: end by asking the person, through `message`. `Notify`: end
/// by sending the person a note through `message`, which reaches the notify command. `None`
/// (the default): nothing.
#[derive(Clone, Copy, PartialEq, Deserialize)]
pub enum Deliver { None, Ask, Notify }

/// A time of day in local time, written `HH:MM` or `HH:MM:SS`. A time that a clock change skips
/// comes due at the time the OS's `mktime` gives for it; a time that occurs twice comes due once.
pub struct Time(/* seconds after midnight */ u32);
/// A weekday, written `mon` to `sun`.
pub enum Day { Mon, Tue, Wed, Thu, Fri, Sat, Sun }

/// Every resident's heartbeat, schedules and night: one task in `razor serve`. Cheap to clone.
#[derive(Clone)]
pub struct Residents { /* the controller; the names stopped by a person; when each resident
                          started; a wake signal */ }

impl Residents {
    /// Start the task. On every wake, for each resident in the config that is not stopped:
    /// - On its first wake in this process it is started: `Controller::create` with the name as
    ///   the id, in its cwd, which makes the session the first time and opens it after that.
    /// - A schedule is admitted once when its latest due time is later than the latest `due` of
    ///   that name in the record, or, when the record has none, than the resident's start in
    ///   this process. That one comparison fires schedules, makes up missed ones at a start, and
    ///   never fires one twice when the clock is set back. The first occurrence of a schedule the
    ///   record has never seen is lost if Razor is down or crashes at its due time.
    /// - A heartbeat that came due since the last wake is admitted; when the controller answers
    ///   `Busy`, it is skipped and nothing is recorded.
    /// - A night that came due since the last wake asks the controller to rebuild.
    ///
    /// The task sleeps until the next due time, and at most 60 seconds. So a suspend, a clock
    /// change or a config edit takes effect within a minute.
    pub fn open(controller: Controller) -> Residents;

    /// Every resident in the config, by name.
    pub async fn list(&self) -> Vec<Listed>;

    /// Start a stopped resident, as at the server's start: its missed schedules run once.
    pub async fn start(&self, name: &SessionId) -> Result<(), Error>;

    /// Stop a resident: nothing of its schedule fires until `start` or the server's next start,
    /// and its current turn is stopped as by Escape. Its session and record stay. Input sent to
    /// it still runs, and that is how the person wakes it.
    pub async fn stop(&self, name: &SessionId) -> Result<(), Error>;
}

/// One resident in the list. Its state, last activity, open asks and waiting inputs are its
/// session's, from the API's facts.
pub struct Listed {
    pub session: SessionId,
    pub stopped: bool,
    /// What comes due next (`heartbeat`, a schedule's name, or `night`) and when. None while
    /// stopped.
    pub next: Option<(String, Millis)>,
}

/// A resident's identity record as its first input carries it, after the session's facts: the
/// heading naming `identity.toml` in its session's directory (made from a template at its first
/// start), then the record, at most 64 KiB. A record that cannot be read, or does not parse,
/// becomes one sentence saying so, so the resident can fix it. None for a session that is not a
/// resident. (2026-10-29: replaces `prompt`; the persona is now an entry in its startup list.)
pub async fn identity(config: &Config, session: &SessionId, dir: &Path) -> Option<String>;

/// Send the person a notification through the notify command. `Refused` when none is
/// configured, with the sentence for the model.
pub async fn notify(config: &Config, text: &str) -> Result<(), Error>;

/// Carry an ask to the person through the ask command, and return at once. The command runs
/// detached, at most 16 at a time; when it exits 0 and prints an answer, Razor answers the ask
/// with `Controller::answer`, which refuses an ask already answered from a client.
pub fn ask(controller: &Controller, config: &Config, ask: &Ask);

/// Write and enable the service that runs `razor serve` and restarts it 60 seconds after a
/// failure: a systemd user unit on Linux, which starts at boot when lingering is on and at login
/// otherwise; a launchd agent on macOS, which starts at login. The service carries this binary's
/// path, and `PATH` and `RAZOR_HOME` as they are now; running this again rewrites them. Returns
/// the sentence to print, which says when it starts, or that no user manager answered.
pub async fn install(home: &Path) -> Result<String, Error>;

pub enum Error {
    /// No resident by this name in the config.
    NoResident(SessionId),
    /// One sentence for the model: the `deliver` action's refusal, or no notify command.
    Refused(String),
    /// Any other failure, with its cause and what was being done.
    Other { source: Box<dyn std::error::Error + Send + Sync>, context: &'static str },
}

// ---- src/residents/deliveries.rs -----------------------------------------------------------

/// Every delivery on this machine: one line each in `$RAZOR_HOME/deliveries.jsonl`, append-only,
/// for listing. A delivery is identified by its session and its `deliver` call's entry. A
/// delivered file is copied into its session's directory. One per server; cheap to clone.
#[derive(Clone)]
pub struct Deliveries { /* the index's path; its append handle behind a lock; a broadcast of
                           new deliveries */ }

impl Deliveries {
    /// Open the index. A line that a crash left without its newline gets one, so the next line
    /// starts clean; `list` skips a line that does not parse.
    pub async fn open(home: &Path) -> Result<Deliveries, Error>;

    /// The `deliver` action, which the controller runs for any session.
    /// - Input: `line`, one line for the person; and exactly one of `path` or `url`. A `path` is
    ///   a regular file of at most 64 MiB, relative to `cwd`, copied as it is now to
    ///   `dir/delivered/<entry>/<its name>`. A `url` is http or https, and is never fetched.
    /// - Output: the `Delivery`.
    /// - The copy is made before the index line is written, so a listed delivery always has its
    ///   file.
    pub async fn deliver(&self, session: &SessionId, entry: EntryId, dir: &Path, cwd: &Path,
        input: Value) -> Result<Value, Error>;

    /// Deliveries newest first, those before the delivery `before` names, at most `limit`
    /// (capped at 256). The index is read front to back once, holding at most `limit` lines.
    pub async fn list(&self, before: Option<(SessionId, EntryId)>, limit: usize)
        -> Result<Vec<Delivery>, Error>;

    /// Each delivery as it is made, for the API to announce on its stream.
    pub fn subscribe(&self) -> broadcast::Receiver<Delivery>;
}

// ---- src/memory/mod.rs ---------------------------------------------------------------------

/// The client of Memorum, the memory service. Each call is one connection to its socket, with
/// one JSON line out and one in: Memorum's own frame, at most 64 KiB each way. One per server.
pub struct Memory { /* the socket's path; project ids by cwd, at most 64 */ }

/// The session making a `memory` call: what the call needs from it. The controller fills it.
pub struct Caller<'a> {
    pub session: &'a SessionId,
    /// The call's own entry. A memory it writes cites `razor:<session>:l-<at>`.
    pub at: EntryId,
    pub cwd: &'a Path,
    /// A child reads as a child (Memorum hides root-only memories from it) and does not write.
    pub child: bool,
    /// The owned entry at `ref`, or the latest person input when no `ref` is given.
    /// Fetched before queueing, so a memory call never borrows the live record owner.
    /// Memory validates that its origin is Person or Answer itself.
    pub cited: Option<Entry>,
}

impl Memory {
    /// A client of the daemon that Memorum's own clients use: `$MEMORUM_SOCKET`, else
    /// `$MEMORUM_RUNTIME/memoryd.sock`, else `<$MEMORUM_REPO or ~/memorum>/.memoryd/memoryd.sock`.
    /// Nothing connects yet.
    pub fn new() -> Memory;

    /// The startup page for a session in `cwd`, about 1,500 tokens, and nothing else (SPEC.md,
    /// last paragraph):
    /// - the project's state note, its newest active `status`/`project` memory (at most 500
    ///   tokens);
    /// - its rulings: the standing memories that apply to this project, newest first, at most 20
    ///   lines in 4,000 bytes (about 1,000 tokens), each the person's words on one line followed
    ///   by the memory's id. A line that does not fit is passed over, not cut.
    ///
    /// It never fails. When Memorum does not answer within 2 seconds, the page is one sentence
    /// saying this session runs without memory, and that is the only place it is said.
    pub async fn page(&self, cwd: &Path, child: bool) -> String;

    /// The `memory` action, whose input and output are below. Each call waits at most 10 seconds.
    pub async fn run(&self, caller: Caller<'_>, input: Value) -> Result<Value, Error>;

    /// For `razor doctor`: the socket and Memorum's protocol version, or why it did not answer.
    pub async fn check(&self) -> Result<String, Error>;
}

pub enum Error {
    /// Something the model should read and act on, in one sentence: Memorum did not answer
    /// (naming the socket; the session goes on without memory), input that does not fit, a
    /// ruling that cites no words of the person's, a child's write, or Memorum's own refusal.
    Refused(String),
    Other { source: Box<dyn std::error::Error + Send + Sync>, context: &'static str },
}
```

**The `memory` action.** Run by the controller (`session: true`, not read-only). Its declarations are profile data. Every output is Memorum's own answer, unchanged.

| action | input | output |
| --- | --- | --- |
| `search` | `query`; `limit` (default 8, at most 20) | Memorum's search answer: `hits` (`id`, `kind`, `summary`, `snippet`, `namespace`, `source_ref`, `status`, …). It covers this project and `all`, which Memorum scopes from the cwd |
| `get` | `id` | Memorum's memory: `id`, `kind`, `body` (at most 4 KiB, Memorum's own cap), `standing`, `source_ref`, `status`, … |
| `write` | `kind`: `ruling`, `fact`, `note` or `state`. `text`: at most 16 KiB. `all`: for every project; not allowed for `state`. `expires`: a date, after which search leaves the memory out. `replaces`: an id; the new memory supersedes it, and with no `text` the old one is retired. `ref`: for a ruling, below | Memorum's write outcome: `status`, `id` (null when nothing was kept), `warnings`, `next_actions`, … |

How each kind is written:

| kind | Memorum `kind`, `type` | standing | source | rule |
| --- | --- | --- | --- | --- |
| `ruling` | `preference`, `decision`, namespace `me`, `applies` this project or `all` | yes | `user`, `razor:<session>:l-<ref>` | `ref` is the entry of the person's words the ruling comes from, by default the last of `Summary::asks`. Razor reads it from the record and refuses it unless it is an input from `Origin::Person` or `Origin::Answer`. The body is not the model's text: it quotes the question the cited words answer, when there is one, and the person's words, both verbatim, then the model's `text` as the agent's reading (below, "Rulings in the person's words"). The provenance is the cited entry, which Memorum stores as the source. Replacing a ruling needs the same |
| `fact` | `fact`, `claim` | no | `agent_primary`, the call | A changed fact is written with `replaces` |
| `note` | `lesson`, `pattern` | no | `agent_primary`, the call | |
| `state` | `status`, `project` | no | `agent_primary`, the call | One per project: the newest active memory of this kind and type in the project's namespace, whichever harness wrote it. Each write supersedes it. A pinned one is not a state note (below, "A state note over a pinned note"). `/done`'s closeout writes the state note this way, so `/done` needs no memory code of its own |

Every write also carries `harness: razor`, `session_id` and the session's cwd, from which Memorum resolves the project; no idempotency key, since Razor never sends a call twice and a resident's entry numbers restart if its record is ever removed. A `fact` or `note` with `all` goes to Memorum's `me` namespace. The startup page shows the standing memories as rulings; Memorum governs the standing flag, and Razor is not an authentication layer over it.

## Data flow

1. `razor serve` makes one `Memory`, one `Deliveries` and, after `Controller::start`, `Residents::open`. The service `razor install` wrote starts it, and so does any client that finds no server.
2. A resident's first wake creates or opens its session.
3. The task sleeps until the next due time across all residents, and never more than 60 seconds. On each wake it reads the config and `Controller::scheduled`, and compares every due time with the wall clock.
4. A heartbeat is admitted as `Origin::Heartbeat` with its own model and effort. `Busy` means skipped, and nothing is recorded.
5. A schedule is admitted as `Origin::Schedule { name, due }`, with its own model and effort and its text plus the delivery sentence. The record holds it while the controller holds its turn, and the person can withdraw it until the turn starts.
6. A night is `Controller::night(name, at)`, `at` being its tidy input's entry. When that input opened the turn (the home session was idle), the controller rebuilds once that turn ends, so the rebuilt startup page shows the tidying. When it waits behind a turn already running, that turn's current batch of tool calls settles as a handover's does, the context is rebuilt there with the latest note and the turn goes on in it, and the tidy input runs after the turn (Trey, 2026-11-04, agreeing to "the rebuild happens after the current round of tool calls. Otherwise a busy coordinator may never get its nightly reset"). Before, the rebuild waited for the running turn to end, which for a coordinator could be hours. No note is asked of the model, and a wait in the batch is not cut short. Any other rebuild while the night waits (a threshold the same batch reached, the person's handover) is that night's: it gives the fresh context, keeps its own trigger, and the night does not rebuild a second time.
7. The schedule turn's last act is the model's own `message`: an ask, a Call entry answered through `Controller::answer` from a client or the ask command; or a note, which reaches `notify`. After a schedule's error or stopped turn end, or a `Withdrawn { inputs }` naming its undelivered input, is committed, the controller calls residents to send a sentence naming the resident, schedule and reason through `notify` (a withdrawal uses the stopped sentence), on a detached task sharing the asks' 16-command bound and the notify command's 30-second limit; failure or saturation is logged. A normally ended turn sends no automatic note, even if the model omitted the requested ask or note.
8. `deliver` copies a file into the session's directory, appends one line to `deliveries.jsonl` and broadcasts it. The API lists, serves and announces deliveries from there.
9. Each native conversation opens with `Memory::page` ahead of its first input. Each `memory` call is one socket connection.
10. Nothing here keeps a session's state. The record holds every admitted input and every schedule's latest due. The index holds deliveries, and Memorum holds memory.

## The hard decision: what holds the schedule and its pending inputs

The task: nothing may be lost or run twice across a crash or a night, and a busy resident's heartbeat must be skipped, not queued.

**Decision: an in-process schedule read live from the config, with the resident's own record as its memory, and no SQLite.**
- A schedule input carries `due`. The record's summary folds the latest `due` of each schedule name (`Summary::scheduled`, at most 64).
- Inputs that wait are entries in the record, held by the controller's own turn-end rule.
- A busy heartbeat is refused by admission, so nothing holds it.

It promises admission, not exactly-once effects in the world.

The options compared:

- **OS timers that call the API** (systemd timers with `Persistent=true`; launchd's `StartCalendarInterval`). The OS owns the clock.
  - *A crash between firing and admission.* systemd stamps the timer when it elapses, not when the input lands (systemd.timer(5)), so a client that fails because the server is down still counts as run, and the job is lost. A retry would admit it twice unless admission dedups on `(name, due)` against the record, which is the decision's fold.
  - *A missed job.* On Linux, `Persistent=true` is the one rule. On macOS a job missed while the machine was off "does not execute until the next designated time occurs" (Apple, *Scheduling Timed Jobs*), so the Mac would need Razor's own catch-up too.
  - *Linux and macOS.* Two unit generators (`OnCalendar` strings, `StartCalendarInterval` dictionaries), rewritten on every schedule edit; Loom's "must be re-run after any calendar edit" breaks RULES.md 12.
  - *The status view.* It would parse `systemctl list-timers`; launchd has no query for the next firing.
- **One in-process task, read from the config (chosen).**
  - *A crash between firing and admission.* Firing and the append happen in one process.
    - A crash before the append leaves the record without that `due`. When the record holds an earlier `due` of that name, the latest due time is later than the recorded one at the next start, and the schedule is admitted once.
    - When the record holds none (the schedule's first occurrence), that occurrence is lost. Making it up would fire every new schedule at install (a "daily 09:00" configured at 15:00 would run at 15:00) to recover a sub-second window.
    - A crash after the append leaves it in the record, and the restart's own rule carries the open turn on.
    - Every wake makes the same comparison, so a clock set back never fires a schedule twice.
  - *A missed job.* The same comparison at start, on both OSes. A schedule the record has never seen is not made up, so installing a resident never fires last Friday's review.
  - *Linux and macOS.* One code path, with local time from libc's `localtime_r` and `mktime`. The OS only starts `razor serve`, through one unit no schedule edit changes.
  - *Suspend.* tokio's clock does not count time asleep, so the task never sleeps past 60 seconds and compares wall-clock due times on every wake.
  - *The status view.* The next due time comes from the same function that fires it; the rest is the session's own facts.
  - *Cost.* About 250 lines, no new dependency.
- **SQLite with an occurrence table.** The claim row and the record's append are two writes to two files, so it has the same window plus a second store. No write here spans two records; SQLite left the dependencies.

The busy heartbeat: only `Origin::Heartbeat` is refused with `Busy` while a turn is open, and nothing is appended. A schedule admitted during a turn waits by the controller's turn-end rule, like mail (SPEC.md section 3). A held input that carries its own model or effort (a heartbeat or a schedule) takes a turn alone, so each runs on its own; other held inputs share a turn as before. The scheduler holds nothing.

## The memory client: the socket, one connection per call

Measured on 2026-10-27, against a live Memorum daemon, with read-only calls:

| | cost per call | when Memorum is down |
| --- | --- | --- |
| **CLI per call** (`memoryd …`) | 20 to 30 ms wall for `status`; a 43 MB binary peaking at about 115 MB resident, per call; `memoryd` on the service's `PATH` | exit 75 after the process starts |
| **Socket per call (chosen)** | connect 0.012 ms; handshake 1.3 ms; `status` about 10 ms; no process | `connect` fails at once |
| **Socket held open** | saves the 0.012 ms connect, but the daemon closes a silent connection after 300 s and answers one frame at a time, so it needs a pool and reconnect logic | as per call, plus a stale connection |

`search` takes about 265 ms either way (Memorum's own work); a 1,200-token `select` of rulings, 158 ms.

One thing goes through the CLI: a cwd's project id, which the page and the state note need for Memorum's `project:<id>` namespace. Only the CLI resolves it (`memoryd project resolve`, `cli/project.rs:30`), so the client runs it once per cwd and keeps at most 64. For Memorum: `select` taking a `cwd`, as `search` does, would remove that process.

When Memorum is down, the startup page says so in one sentence and the session runs without memory. Each `memory` call is refused with the same sentence; there is no queue and no retry (an offline queue is cut). A daemon that hangs costs one call 10 seconds, or the page 2.

The spec's "a scheduled job consolidates" is a schedule whose text asks for it: no code.

The socket's frame is Memorum's (`memorum-protocol` `MAX_FRAME_BYTES`, 64 KiB). That is why a memory's text is capped at 16 KiB and search returns summaries. Memorum accepts `razor:<session>:l-<n>` as a source (`is_harness_log_ref` takes any harness name, `grounding.rs:247`), so no Memorum change is needed.

## Rulings in the person's words

Design of 2026-10-29, asked for by Trey, built against Memorum, whose client Razor later dropped (docs/design/memory.md); what remains is the rule for a ruling's words. The page shows a ruling's verbatim words, never an agent's paraphrase or a cut summary. The write follows Trey's ruling of 2026-10-29 ("if the ruling is contingent on me literally sending a detailed confirmation, then I'm not going to do that. I'm just going to say yes to questions. So that means rulings need to change."): a ruling's body is composed from the record, never taken from the model. It is `the agent asked: "<question>"` when the cited input answers one, then `the human said (razor:<session>:l-<ref>): "<the person's words>"`, both verbatim, then `agent's reading: <text>`, the model's, which the page never shows. For an `Origin::Answer` the question is the ask's text; for a person's input, the Text blocks that closed the last turn before it, joined as `--print` joins them, read back from the input past job events only, so an input that came mid-turn or after another input answers no question. The controller reads it together with the cited entry before the call queues (`memory::cited`). Each page line is `- <words> (<id>)`, or `- Asked "<question>", answered "<words>" (<id>)`, with a question over 300 bytes shortened to its end.

**What verbatim costs.** The person's words often mean nothing alone ("Yes sure", "Yes to all go"). That is why each line ends with its id: the record's context is one `memory get` away.

## What neighbours build

- **Protocol.**
  - `Origin` gains `Heartbeat`, `Schedule { name, due: Millis }`, `Answer { ask: EntryId }` and `Program { name }`; `Trigger` gains `Night`.
  - `SessionId` also accepts a resident's name, `[a-z][a-z0-9-]{0,31}`.
  - `Ask { id, text, options }` is the shape of an ask's call (API and children); `Answer` is the API's.
  - `Delivery { session: SessionId, entry: EntryId, at: Millis, line: String, file: Option<PathBuf>, url: Option<String> }`, the API's shape, where `file` is relative to the session's directory; `Live::Delivery` carries it.
- **Controller.**
  - Session creation and admission:
    - `create` takes an optional id, and opens an existing record with that id.
    - `admit` refuses `Heartbeat` with `Busy` while a turn is open and appends nothing. `Schedule` and `Program` wait for the turn's end, like mail.
    - A heartbeat's or a schedule's model and effort apply only to the turn it opens, which takes no other held input.
  - New operations:
    - `config()`, public;
    - `night(name, at)`: as at a restart, at once when idle, after the turn its input opened, or after the current batch of a turn it waits behind, and nothing is asked of the model first;
    - `withdraw(session, entry)` and `answer(session, ask, Answer)`;
    - `scheduled(session)`, which returns `Summary::scheduled`.
  - Session actions: it runs `memory` (filling `Caller`) and `deliver`.
  - Each native conversation: it calls `Memory::page` at the start and puts the page ahead of the first input, outside the system prompt.
  - Wiring: `razor install` in `main.rs` calls `install`, and `razor serve` opens `Residents` (both done here); `Memory` and `Deliveries` are made where the tool service runs `memory` and `deliver`.
- **Record.** The `Summary::scheduled` fold. `--continue` skips names. A rebuild's calls count from the later of the latest ask and the previous rebuild, so a resident's stays bounded.
- **Prompt.** A resident's session opens with its own startup list (`Resident::startup`, or the person's), its persona one of the entries, and `identity(...)` in its first input's facts (2026-10-29; before, `prompt(...)` replaced Razor's whole prompt). A pack can give each resident its own `startup` list, its persona first.
- **Tools.** The registry gains `memory` and `deliver`, both `session: true` and not read-only. `deliver` joins SPEC.md section 3's default set, as its Residents paragraph keeps it.
- **API.**
  - Residents: list them (`Residents::list` joined with each session's facts), and start and stop them.
  - Sessions: `withdraw` for any waiting input, and `answer` through `Controller::answer`. The input route's optional `from` becomes `Origin::Program`.
  - Deliveries: `Deliveries::list`, whose `before` is a delivery's session and entry; file download from the session's directory; and `subscribe` on the status stream.
  - A resident is reached at `/v1/sessions/<name>`, as any session, so `razor --resume <name>` and `--print` cover "open the terminal app on a resident" and "send input and wait for its reply" with no code of their own.
- **Children and mail.** A `message` from a session with no parent:
  - a question becomes an ask, and returns once the ask is recorded; when `ask` is configured, it also calls `residents::ask`, which waits for nothing;
  - a note goes to `residents::notify`.

  `stuck.after` is shared: a resident shows as stuck only when it has an input admitted or a turn open and its record has not moved for `stuck.after`. An idle resident with nothing waiting is not stuck.

## Where this departs from the spec

1. **One service unit, not one per resident.**
   - SPEC.md section 3 (line 55) says "A resident is a systemd or launchd unit".
   - Section 5 (line 92) says "one process per machine", and section 3 says one API server per machine. A unit per resident would be either a second process or a unit that starts nothing.
   - So `razor install` writes one unit, for `razor serve`, which runs every resident.
   - "Starts at boot" holds on Linux with lingering on; a macOS launchd agent starts at login (before login needs a LaunchDaemon: Trey's call, not needed now). Flow 21 checks the unit's text, not a real boot.
2. **Loom's job `deadline` and `missed` keys are not carried.** The one rule replaces `missed`, and a deadline belongs in the schedule's text. A resident's "every 2h within 08:00 to 22:00" is written as its eight times.
3. **The identity record has no edit operation of its own.**
   - The spec's "harness operation that records who changed what" is the resident's own edit of `identity.toml`: its record holds the call, which says who and what. The person edits the file, or asks the resident to.
   - Its fields (id, display name, role, authority, values, relationships, operating patterns, version, who may change it) are the template's keys. They are shown to the model and enforced by nothing.
   - This is a question for Trey, below.
4. **The startup page holds no first-task brief**, though Loom's kept features name one: SPEC.md's last paragraph limits the page to the state note and the rulings.

## Flows

| flow | what it checks |
| --- | --- |
| 21 | `razor install` under the flow's HOME writes a unit whose `ExecStart` is the `server.wrapper` the flow sets, then `<razor> serve`, with `Restart=on-failure` (a plist with `KeepAlive` on macOS), and says no user manager answered: the flow puts a `systemctl` and a `launchctl` that answer nothing first on `PATH`, so no real manager is touched on either OS. Another session's input starts the server, and the resident starts with no input of its own. A heartbeat due 5 s after the config is written opens a turn whose `Bash` call sleeps 4 s; the persona, named in the resident's own `startup` list, is in that request's system prompt in the list's order, and the identity record is in its first input. A heartbeat due mid-turn is absent from the record. A schedule due mid-turn is admitted mid-turn, is absent from the turn's next request, and opens the next turn after the first turn's `TurnEnd` |
| 22 | A schedule naming another model and effort: the stand-in's request for its turn names them, and the next turn's names the session's own. With `deliver = "notify"`, the notify command (a flow script) receives the turn's note. With `deliver = "ask"`, the session's status row lists an open ask |
| 23 | A resident's `message` question with no parent returns once recorded, as an open ask. The ask command (a flow script) prints an answer after 1 s, and the resident's next request holds it as `Origin::Answer`. A second ask answered first through the API is not answered again by its command |
| 24 | Memorum runs for real, from the flow's own directory, through `MEMORUM_SOCKET`. A ruling citing the person's message by default is written standing, and one citing the session's `Start` entry is refused. Search finds the ruling. A `state` write supersedes the previous one. In a second session, a fact replaced with `replaces` is gone from search and its replacement is found, and the first request holds the new state note and the ruling, not the old note. Each ruling on that page is one line, its words and then its id: Razor's ruling as the person's words it cites; a bare "yes", given in a session whose turn closed with a 472-byte question, after "…" and that question's whole words in its last 300 bytes, and none of its head; and a ruling the flow writes straight to Memorum in Loom's form as the words it quotes, whitespace made single spaces |

Flows 21 to 24 are written (`flows/21-resident.py` to `flows/24-memory.py`).

## Size

Sentences, delivery instructions, the page's headings and the identity template are data in `text.toml` files. No new crate: tokio's `UnixStream`, `serde_json`, `toml` and `libc` are in the build already.

## Questions for Trey

1. A schedule delivers its result through its own last `message`, so a job that fails or is stopped sends you nothing: should Razor send you a notification when a scheduled turn ends in an error?
2. Is editing a resident's `identity.toml` by hand, or asking the resident to, enough for you, with the resident's own edits recorded as its calls?

## Glossary lines proposed

- **schedule**: one of a resident's named inputs that comes due at set times (a 07:10 morning brief), with its own text, model and effort, and what to do with its result; Trey's "named job".
- **night**: the time each day a resident's session is rebuilt.
- **notification**: a line sent to the person through the configured notify command, to read and not to answer.
- **inbox**: where deliveries wait for the person.
- Amend **input**: "… mail from another agent, the person's answer to an ask, or a program's message through the API."
- Amend **resident**: "a session that runs in the server, with a persona, memory, a heartbeat, schedules, a night and an identity record; its session's id is its name."

**Home defaults, 2 November 2026.** `Resident.model: Option<String>` and `Resident.effort: Option<Effort>` are passed to session creation for a resident home. Absent values use the global defaults; existing homes retain their recorded settings. Schedules still override their own turns. A saved view named Auto may name a resident in its optional `wake`; that resident is sent the view's new unmatched roots, so it can sort them.
