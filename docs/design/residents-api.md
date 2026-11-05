# Residents for the phone: schedules, runs, notes, Run now and threads

Design page, third revision, 2026-10-31, approved for building. It folds in a review of the first revision and Trey's rulings. Every line number below is that of the code it was written against.

Three rules carry the design, and none adds a store:

- **A run is one heartbeat or schedule input, or one night rebuild, followed to the closure that answers it**: the `TurnEnd` or `Withdrawn` that names its entry. The record's summary fold already sees every entry once, so it keeps the last 24 hours of runs in memory. There is no scan on read.
- **A note to the person is a delivery with neither file nor url.** `deliver` requires exactly one of them (`residents/deliveries.rs:262`), so the index, its list, the status stream, paired servers, read receipts and push carry notes unchanged. Each reader that shows deliveries presents one with neither as a note.
- **A thread is an ordinary session started as a resident.** Its `Start` entry names the resident, so it opens with that resident's identity record, startup files and folder, and so its memory, but has its own record and a fresh context. A schedule marked `fresh` runs in a new thread of its own.

## Cuts

**Controller: 10 lines, each leaving one definition where there were several.**
- `controller/surface.rs:284`: `scheduled` goes (−3). Its one caller, the residents task, reads `summary().scheduled`.
- `ChildSummary::working()` exists (a child with no report, not stopped, not settled), but the controller still spells out that condition, or its negation, by hand in four places.
  - `controller/session.rs:784` (`pending`): `s.children.iter().any(|c| c.working())` (−2).
  - `controller/mod.rs:581`: `!child.working()`.
  - `controller/session.rs:166`: `!f.working()`.
  - `controller/surface.rs:197`: `fact.working()`, equivalent because a settled child has already been refused two lines above.
- `controller/mod.rs:565`: the remote facts' change check compares `insert(..).as_ref() != Some(&fact)` across eight lines; `insert(..) != Some(fact)` says the same in one (−5, with the condition above it).

**Elsewhere:**

| where | change | lines |
| --- | --- | ---: |
| `api/push.rs:214` `resident`, `api/devices.rs:364` | both build the whole residents list (every resident's due times, and after this its runs and identity file) only to ask whether a session is a resident; both call a three-line `Residents::has` | −5 API, +3 children |
| `api/mod.rs:797` `resident_list` | `Listed` derives `Serialize`; the handler returns `Json(app.residents.list().await)` | −3 API |
| `record/summary.rs:418` a report's preview | one `preview` shared with a run's line: the first line, even an empty one, then 200 characters, as today | −3 record |
| `residents/deliveries.rs:264` `deliver`'s tail | lock, append, enrich and broadcast become `add`, shared with `note`, still under the writer lock | +2 children |

Kept: no deliveries early stop (rows appended with times 200, 100, 150 would make a one-row page return 150), and `next` stays beside `upcoming` (at 03:01 on the night the clocks fall back, the next night is 1,499 minutes away).

**Behavior changes, not cuts.** `notify` runs detached, as `ask` does, under the same bound of 16 commands. `failed` (`residents/mod.rs:405`) loses its own permit, config copy and spawn (about −12 children). Both callers change: the `message` tool's note to the person (`children/message.rs:117`) and `failed`. Callers no longer wait for the command or learn that it failed, timed out or that none is configured; a failure is logged, and with 16 commands already running the notification is dropped with a log line. A note no longer fails without a notify command, since it has reached the inbox. Failed-run notices still go only through the notify command.

## Loading residents

After a server start an idle resident is never loaded. `Controller::create` reopening an existing record goes straight to `summary` (`controller/surface.rs:77`), and `summary` never loads a session (`:93`), so every wake, at least once a minute (`residents/mod.rs:294`), parses the resident's whole record from disk (`record/mod.rs:365`). Confirmed from the code, not timed.

**The rule: the residents task holds each configured resident's home session loaded.** `Controller::load` starts the session's task if it is not running and returns a `Loaded` that holds its queue. The idle check removes a task only when the controller's map holds the last sender of that queue (`controller/session.rs:810`), so a held `Loaded` keeps the task. The residents task takes a `Loaded` at each wake, keeps it beside the start time it already keeps per resident, and loses it in the retain that already drops a resident removed from the config. A paused resident stays loaded, and `sessions.idle` still closes idle model conversations.

Held per resident, not measured: the session task with its record (an open file, an index of 8 bytes per entry, and the summary with at most 512 runs), its 1,024-slot transcript channel, its shell and tool-service handles, its plugins, its settings (a copy of the whole config), its profile, a 256-deep work queue and its job-event queue. No model process is held.

## The phone's five requests

**1. The timetable: build, smaller.** The row carries only what the phone draws: `heartbeat` and each schedule as `{at, days, deliver, does, fresh, latest}`, where `does` is the first sentence of its text (at most 200 characters) and `latest` is its newest run; then `night`, `next` and `upcoming`. `upcoming` comes from `calendar::due`, the function that fires the timers, run per timer and then sorted. The unchanged `calendar.rs` handles five cases correctly (a repeated fall-back time fires once, a skipped spring time moves on, weekday filters, a next night past 24 hours, a late wake). These are planned times, not promises: a heartbeat that finds the resident busy is skipped, times missed while the server was down collapse into one run, and a night can wait behind work. No `zone`: the phone draws every instant in its own day (`the phone client`).

**2. Runs: build, smaller.** A `runs` field on the rows holds every run opened in the last 24 hours, newest first, at most 512 per resident.
- **What closes a run.** Only a turn end or withdrawal naming its entry. A handover's intermediate turn end leaves it open. A restart rebuild answering several runs closes them all at once. A turn abandoned on error (`controller/session.rs:754`) fails every unanswered input, so a waiting schedule can fail without having run.
- **States.** `ok`, `failed` and `stopped` come from the closing entry; a withdrawal is `stopped`. An open run is `interrupted` while its session's interrupted flag is set; otherwise it is `running` once it has a `start` (below) and `waiting` before. The controller takes held inputs in arrival order and runs a heartbeat or schedule alone (`controller/session.rs:374`), so a run starts when a turn's first work entry comes while its entry is the oldest unanswered input; a night's run starts at its own rebuild, and a restart's rebuild starts every open run, since the rebuilt turn covers them all. A run is never shown as `running` unless its turn is in progress. A fresh run is `starting` from its home marker until its input reaches its thread.
- **Retention.** The fold keeps a closed run until a run opens more than 24 hours after it, capped at 512. It also keeps the newest run of each name, whatever its age, so `latest` survives for a weekly job: at most the heartbeat, the night and 64 schedule names, the bound the missed-run marker already has; the row shows `latest` only for configured names. A new name at capacity evicts the oldest kept run; the missed-run marker likewise evicts the lowest due time among its 64 names. A configured name's older history can therefore be lost when more than 65 other names run since its last run, requiring schedule renames beyond the configuration's 64-schedule capacity within one of that job's periods (accepted 4 November 2026).
- Your conversations are not runs; they are threads. `started_ms` is cut, since the record has no entry for a turn's start; `start` (below) marks where a run's own work begins.

**3. `display_name` and `role`: build as asked.** Read from the home session's `identity.toml` through `Controller::file`; each is empty when absent or when the file does not parse.

**4. Notes: build, smaller (Trey: notes are inbox items, listed, counted unread and pushed, in the terminal too).** A top-level session's note to the person calls `Deliveries::note`, then the notify command when one is configured. The row's `line` is the note's first non-empty line, at most 2,000 characters; the whole text is the `message` call at the row's entry. The push is the existing `delivery` alert; there is no new push kind. These readers change:
- **Terminal** (about +3 lines, on a row at its cap): `terminal/status.rs:367` labels it "note"; `terminal/keys.rs:617` opens it at its entry in its session's transcript and marks it read.
- **Phone:** `Deliveries.swift:42` and `:48` (the list's title and icon), `Residents.swift:215` and `:217` (the resident page's cards), `Deliveries.swift:65` (opening), and `NotificationService.swift:64` (the "Delivered something" subtitle).

**5. Run now: build as asked.** `POST /v1/residents/{name}/run/{schedule}` admits the schedule as when due, through the same function the task uses, with `due` set to now. A home schedule waits behind a busy turn as usual; a fresh schedule starts its own thread. It runs while the resident is paused, since pausing stops timers, not inputs.
- It advances the schedule's latest-due marker, so the next due time after now fires as usual.
- An older occurrence still overdue is subsumed by the manual run.
- A timer firing at the same moment as a manual run can admit both.

An unknown resident gets 404; an unknown schedule gets 400, "Resident <name> has no schedule <name>."

## Where a job runs (Trey: "Some jobs should be main thread some should be fresh context cheaper model")

- **The setting.** A schedule takes `fresh = true` (default false) and keeps its own `model` and `effort`; one boolean says whether a schedule runs at home or in a fresh thread. The heartbeat stays in the home session. Its purpose is to wake the resident's own continuing context, and its skip-when-busy rule only means something in that one session; in a fresh thread every heartbeat would be a new session with no memory of the last check. Fresh periodic work is a schedule with those times.
- **A fresh run.** At the due time, or on Run now, the residents task does four things, in order:
  1. Creates a thread through the thread creation path, `Controller::create` with `Thread { resident, job: Some(name) }`, on the schedule's model and effort.
  2. Names it for the job and its due time ("morning · Nov 1") with a `Name` entry.
  3. Appends `Fresh { schedule, due, session }` to the home session. This advances the missed-run marker, exactly as a home schedule's input does, and is the home's pointer to the run.
  4. Admits the schedule's input in the thread.

  The home's fact comes before the admission, so a crash between them loses the run rather than repeating it, which is the scheduler's existing promise. `Fresh` joins the entries `Record::append` syncs to disk (`record/mod.rs:236`), so the marker is durable before the thread runs. Between steps 3 and 4 the row is `starting`: the residents task keeps the threads it is admitting in memory, and a row it is admitting is never cached. Once no admission is in progress and the thread holds no run (the server stopped between the steps, or the admission failed), the row is `failed`, "The run's input never reached its thread.", with `entry` 0 and no range.
- **The run row.** The home's row for a fresh run is the thread's own run row, which the thread's fold makes from its schedule input like any other, so `session` names the thread. Once that run has closed, the residents list caches the row, so a closed thread is read once, not on every poll. The cache holds only threads the home's runs and configured `latest` still name.
- **Close when the run ends: yes.** Rule: a thread's schedule input is its last. On admission the controller marks it as `/done` marks its own input (`controller/session/inputs.rs:58`), so the turn that answers it closes the session (`controller/session.rs:528`): no conversation stays open, its jobs stop, and its task unloads unless a question it asked is still open. Loading a job thread restores the same mark from its open run, so a run resumed after a crash still closes (`Session::load` starts with none).
  - For: on the Claude route an open thread keeps a Claude Code process warm until `sessions.idle` evicts the longest idle, and job threads are always the newest, so they would push your own sessions out.
  - Against: a run that ends by asking you a question also closes. Your answer reopens the thread for a turn, and it then stays open like any session.
- **Out of the thread list: yes, always.** `record::list` skips a `Start` whose thread names a job (`record/mod.rs:431`, +1 record), so `GET /v1/sessions`, naming and the repository list never see job threads.
  - For: a daily job adds 30 threads a month, and a list of 50 newest could become nothing but job runs. You reach a job's runs through `runs` and each job's `latest`, which is what you asked for.
  - Against: a job run older than 24 hours that is not its job's latest is reachable only by its id. An opt-in `?jobs=true` would cost about 4 lines if you ever want the history.
  - As built later (2026-11-02, because a resident's routines live in fresh runs): `GET /v1/sessions?resident=<name>` lists that resident's threads with its fresh runs, each row carrying `job`; the plain list still leaves them out.
- **Other effects.**
  - A job thread sends no finished or stuck alert, as its home session doesn't (`api/push.rs:198`, +1 API).
  - A job thread is attributed to its resident in the failed-run notice.
  - While it runs, a job thread shows in the live status like any session; hiding it there would need a field on the status row and a controller line.
  - Every fresh run is one more session folder, which `record::list` opens when it lists or names sessions.

## Seeing what a run did (Trey: "a way to see/click into the latest run of each job, not to message it, but to just see what it did")

- Each run row carries its `session` (the home session, or a job's thread) and its range: `entry` (the input or rebuild that opened it), `start` (the first entry of its own turn) and `closed` (the turn end or withdrawal).
- `start` is the first work entry (a reply, a call or a rebuild) of the turn that took the run, so a schedule that waited behind your conversation does not show that conversation as its work, and an intermediate handover or overflow turn end does not move it. It is null for a run that never ran: a withdrawal, an input failed while waiting (`controller/session.rs:754`), or a fresh run whose input never reached its thread (+5 record).
- A client reads the range through the existing route: `GET /v1/sessions/{session}/entries?before={closed+1}&limit={closed-start+1}`, paging again from the first entry returned when the range exceeds a page, plus `entry` alone when it precedes `start`. While a run is open, it reads back from the session's newest entry, or attaches to its stream.
- **No route change.** A forward-reading `after` on the entries route would let one call start at `start`, for about 5 lines across the API, the controller and the record. It is not worth it.
- **The latest run of each job** is `latest` on that job's timetable entry: one read of `GET /v1/residents`.

## Resident threads (Trey: "start new sessions, new threads, with the resident with a fresh context but same resident … the resident can name the sessions for me or I can name them")

- **A thread** is a top-level session whose `Start` holds `Thread { resident, job }`, where `job` is set only for a fresh run. Identity and startup files resolve through the resident (`controller/session/conversation.rs:130-131`, +1 controller). The identity file is the home session's, so an edit from any thread is the resident's own. The folder is the resident's, so memory is the same project's.
- **Create:** `POST /v1/sessions` with `resident` instead of `cwd`; the API takes the folder from the resident's config and keeps the request-id repeat rule. An unknown resident gets 404.
- **List:** `GET /v1/sessions` rows gain `resident` on a person's thread.
- **Rename, for you and the agent:** one entry, `Name { name }`, which the fold makes the session's name, the one every row shows. You use `PUT /v1/sessions/{id}/name`; the agent uses a `name` action on the existing `agent` tool. Both go through one function in `children` that refuses a child and anything but one line of 1 to 64 characters. Duplicates are allowed, since mail to a shared name already refuses and lists the ids (`children/message.rs:347`). The rename route sits among the session routes, so a paired server forwards it like `draft`.
- **Closeout (the resident's state note belongs to the home session).** The memory module refuses a `state` write from a thread with a sentence, so a thread's `/done` writes rulings, facts and notes but never the state note. Its identity heading also tells it so, so it does not try. Zero controller lines: the action's `child: bool` (`controller/session/actions.rs:126`) becomes `memory::Role::of(summary)` (root, child or thread) on the same lines, and `memory/mod.rs:261` checks it.
- **Push and follow.** A person's thread gets finished and stuck alerts and Live Activities like any top-level session.
- **The night's list of the day's threads: kept, priced separately.** After asking for the night rebuild, the residents task admits one input from Razor to the home session. It has one line per thread active since the previous night: its name and the first line of its last reply; and one line per fresh run: its job name and its run's line. The input waits for the night turn to end, then takes one more turn in the fresh context.
  - Price: about +21 children lines and one extra model turn per resident per night.
  - Reads, once a night: one listing of top-level sessions, one entries page per active person thread, and the home's runs.
- **SPEC.md, proposed.**
  - Section 3, Residents: replace "A resident has a durable session, a state directory, a persona, optional memory and a schedule." with "A resident has a home session, whose id is its name, where its heartbeat, its night and its schedules run, and any number of threads: sessions started as the resident, with its identity record, folder, startup files and memory, each with its own record, a fresh context and a name the person or the resident gives it. A schedule may instead run in a fresh thread of its own, on its own model, that closes when its run ends. A resident also has a state directory, a persona, optional memory and a schedule."
  - Section 3: replace "A resident handles one input at a time; a heartbeat arriving mid-conversation is skipped." with "Each of a resident's sessions handles one input at a time; a heartbeat arriving mid-turn in the home session is skipped."
  - Section 3, Continuity: after "the closeout asks the agent to write the project's state note through memory", add "except in a resident's thread, whose state note is its home session's."
  - Section 3, API server: add "rename" to the session routes a paired server may forward.
  - Section 5: "enforces one worker per resident" becomes "enforces one worker per session".

## Field contract for the phone

Times are absolute milliseconds since the epoch; draw them in the phone's own zone. `HH:MM` strings are the server's local time, for labels only.

**`GET /v1/residents`**: one object per configured resident.
- `session`: the resident's name, which is also its home session's id.
- `stopped`: true while paused.
- `display_name`, `role`: from its `identity.toml`; empty when absent.
- `heartbeat`: null, or a timetable entry (below).
- `schedules`: an object of timetable entries keyed by schedule name.
- `night`: `"HH:MM"`.
- `next`: null, or `[what, at]`, the soonest due time within a week.
- `upcoming`: `[[what, at], …]`, every due time in the next 24 hours, soonest first; empty while stopped. These are planned times: a busy heartbeat is skipped, times missed while the server was down collapse into one run, and a night can wait behind work.
- `runs`: every run opened in the last 24 hours, newest first, at most 512.
- `sky`: absent unless the resident's config names a `sky` file, then `[{day, n}, …]`, oldest first: `day` is `"YYYY-MM-DD"` and `n` the commitments the resident closed that day (below).

**`sky`** (the phone draws a star on a resident's dial for each day it closed a commitment). A resident's config table may set `sky = "<path>"`, relative to its `cwd` or absolute, naming a JSON file the resident keeps itself (for example `sky.json`); Razor reads no ledger and knows no resident. Each list poll reads the file, at most 64 KiB, off the loop, as it reads `identity.toml`, and passes through only `day` and `n` of each element. A file that is missing, longer than 64 KiB (cut mid-list, so it does not parse), not a list or not made of `{day, n}` objects with `n` a whole number leaves `sky` out of the row; the row and the route are unaffected, and, as with an unreadable `identity.toml`, nothing says why, since a resident's config has no place for warnings. `day` is not checked as a date: the file is the resident's own record, and the phone draws what it is given. A typical file is about 1.4 KB.

A **timetable entry** is:
- `at`: `["HH:MM", …]`.
- `days`: `["mon", …]`; empty means every day.
- `deliver`: `"none"`, `"ask"` or `"notify"`.
- `does`: the first sentence of its text, at most 200 characters.
- `fresh`: true when each run gets its own thread; always false for the heartbeat.
- `latest`: null, or its newest run, whatever its age.

A **run** is:
- `session`: the session it ran in, the resident's home session or a fresh run's thread.
- `entry`: the entry that opened it, in that session.
- `start`: the first work entry of the turn that took it; null until it starts, and for a run that never ran.
- `closed`: the turn end or withdrawal that closed it; null while open.
- `at`: when it was admitted, not when work started.
- `what`: `"heartbeat"`, the schedule's name, or `"night"`; the same word as in `next` and `upcoming`.
- `due`: the schedule's due time (for Run now, when it was asked for); for the others, `at`.
- `state`: `"starting"` (a fresh run whose input is on its way to its thread), `"waiting"`, `"running"`, `"interrupted"`, `"ok"`, `"failed"` or `"stopped"`.
- `ended`: when its closing entry was recorded, or null. `ended` minus `at` is the time from admission to close, so for a run that waited it includes the wait.
- `line`: the first line, at most 200 characters, of the last note or question it sent you, else of its last reply; empty while open or when it said nothing.
- `error`: for a failed run, the turn end's error sentence; otherwise null.

Every field comes from the record's own entries: no client reads entries to draw a run row. To show what a run did, read `GET /v1/sessions/{session}/entries?before={closed+1}&limit={closed-start+1}`, paging again from the first entry returned if the range exceeds a page, plus `entry` alone when it precedes `start`. While a run is open, read back from the session's newest entry, or attach to its stream.

**`POST /v1/residents/{name}/run/{schedule}`** returns `{"entry": n}`, the entry admitted in the home session, or the `Fresh` entry there for a fresh schedule. 404 for an unknown resident, 400 for an unknown schedule.

**Notes** are `GET /v1/deliveries` rows with neither `file` nor `url`. `line` is the note's first line, at most 2,000 characters. The whole text is the `text` field of the `message` call's input object at `GET /v1/sessions/{session}/entries?before={entry+1}&limit=1`. A note's push is the existing `delivery` kind.

**Threads.**
- Create: `POST /v1/sessions {"resident": "scribe", "model"?, "effort"?, "request"?}` returns `{session, name}`.
- List: `GET /v1/sessions` rows carry `resident` on a thread and `resident_project` on a thread started in a project. Job threads are left out; `GET /v1/sessions?resident=scribe` lists that resident's threads with its job threads, each carrying `job`. `&jobs=false` leaves the job threads out of that list, and its `limit` counts only what it returns, so fresh runs do not crowd the person's threads out of the newest dozen. Listing puts each session's row into the status stream.
- Rename: `PUT /v1/sessions/{id}/name` with the name as the body text returns `{}`. It works on a top-level session only, and takes one line of 1 to 64 characters.

## Surface

```rust
// ---- src/protocol ---------------------------------------------------------------------------

/// One heartbeat or schedule input, or one night rebuild, followed to the closure that answers it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Run {
    /// The session it ran in: the resident's home session, or a fresh run's thread.
    pub session: SessionId,
    /// The entry that opened it; its own turn's first entry; the entry that closed it.
    pub entry: EntryId,
    pub start: Option<EntryId>,
    pub closed: Option<EntryId>,
    /// When `entry` was recorded: admission, not the start of work.
    pub at: Millis,
    /// `heartbeat`, the schedule's name, or `night`, as `next` and `upcoming` name them.
    pub what: String,
    /// The schedule's due time (for Run now, when it was asked for); `at` for the others.
    pub due: Millis,
    pub state: RunState,
    /// When `closed` was recorded.
    pub ended: Option<Millis>,
    /// The first line, at most 200 characters, of the last note or question its turn sent the
    /// person, else of its last reply; empty while open.
    pub line: String,
    /// A failed run's error sentence, from its turn end.
    pub error: Option<String>,
}

/// The fold records `Waiting`, `Running` from its start, and the closing states; the residents
/// list shows an open run `Interrupted` while its session's flag is set, and `Starting` while a
/// fresh run's input is on its way.
pub enum RunState { Starting, Waiting, Running, Interrupted, Ok, Failed, Stopped }

/// What makes a session one of a resident's threads: the resident, and for a fresh run, its job.
pub struct Thread { pub resident: SessionId, pub job: Option<String> }

// Body::Start gains `thread: Option<Thread>`.
// Body::Name { name }: the session's new name, from the person or the agent.
// Body::Fresh { schedule, due, session }: in the home session, a schedule's run started in a
//     fresh thread; it advances the missed-run marker as a schedule input does.
// Listed (GET /v1/sessions) gains `resident: Option<SessionId>`.

// ---- src/record -----------------------------------------------------------------------------

pub struct Summary {
    /// Runs opened in the 24 hours before the newest, at most 512, oldest first; for a fresh
    /// run, a row whose `session` names its thread.
    pub runs: VecDeque<Run>,
    /// The newest run of each name, whatever its age.
    pub latest: BTreeMap<String, Run>,
    pub thread: Option<Thread>,
    // … and the last turn end's entry and the open run's note and reply lines.
}
// `list` skips a Start whose thread names a job.

// ---- src/controller -------------------------------------------------------------------------

/// Keeps a session loaded while it lives; dropping it lets an idle session unload.
pub struct Loaded(/* the session's queue */);

impl Controller {
    /// Starts `session`'s task if it is not running.
    pub fn load(&self, session: &SessionId) -> Loaded;
    /// As today, with `thread` written into `Start`. A thread's schedule input is its last:
    /// the turn that answers it closes the session.
    pub async fn create(&self, cwd: PathBuf, model: Option<String>, effort: Option<Effort>,
        id: Option<SessionId>, child: Option<Child>, thread: Option<Thread>)
        -> Result<SessionId, Error>;
}
// `append` also accepts `Name` and `Fresh`. `scheduled` goes.

// ---- src/residents --------------------------------------------------------------------------

/// One resident as `GET /v1/residents` lists it (see the field contract).
#[derive(Serialize)]
pub struct Listed {
    pub session: SessionId,
    pub stopped: bool,
    pub display_name: String,
    pub role: String,
    pub heartbeat: Option<Due>,
    pub schedules: BTreeMap<String, Due>,
    pub night: Time,
    pub next: Option<(String, Millis)>,
    pub upcoming: Vec<(String, Millis)>,
    pub runs: Vec<Run>,
}

/// One of a resident's timers as the phone shows it.
#[derive(Serialize)]
pub struct Due {
    pub at: Vec<Time>, pub days: Vec<Day>, pub deliver: Deliver,
    /// The first sentence of its text, at most 200 characters.
    pub does: String,
    pub fresh: bool,
    pub latest: Option<Run>,
}

// Schedule (config) gains `fresh: bool`, default false.

impl Residents {
    /// Every configured resident: one identity file read and one copy of its loaded summary
    /// each, and a fresh run's thread summary until that run has closed; never a home record read.
    pub async fn list(&self) -> Vec<Listed>;
    /// Whether `name` is a configured resident.
    pub async fn has(&self, name: &SessionId) -> bool;
    /// The resident's config; `NoResident` when there is none.
    pub async fn resident(&self, name: &SessionId) -> Result<Resident, Error>;
    /// Runs `schedule` now as when due, `due` now, at home or in a fresh thread; its entry.
    pub async fn run(&self, name: &SessionId, schedule: &str) -> Result<EntryId, Error>;
}

/// Sends `text` through the notify command, detached, under the bound of 16 commands that asks
/// share; a failure, a 30-second timeout or a full bound is logged.
pub fn notify(config: &Config, text: String);

impl Deliveries {
    /// Keeps a note an agent sent the person as a delivery with neither file nor url, at its
    /// `message` call's entry: its first non-empty line, at most 2,000 characters.
    pub async fn note(&self, session: &SessionId, entry: EntryId, text: &str) -> Result<(), Error>;
}

// ---- src/children and src/memory -----------------------------------------------------------

/// Renames a top-level session, for `PUT /v1/sessions/{id}/name` and the `agent` tool's `name`
/// action: one line of 1 to 64 characters; refuses a child.
pub async fn rename(controller: &Controller, session: &SessionId, name: &str) -> Result<(), Error>;

/// Who is calling `memory`: a thread writes rulings, facts and notes but no state note.
pub enum Role { Root, Child, Thread }
```

## Cost and memory

- **`GET /v1/residents`, per resident, per call:**
  - one config copy, as today;
  - two copies of the loaded home summary, one inside `Controller::file`'s check;
  - one `identity.toml` read, at most 64 KiB and usually under 1 KiB;
  - the `due` calls for `next` and the 24 hours: libc time calls bounded by the configured times;
  - one summary per fresh run still open (it is loaded while it runs).

  No home record is read. A closed fresh run is read once, folded from its own small record, and then cached. A typical day of an hourly heartbeat, five schedules and the night is 30 runs of about 300 bytes.
- **Runs:** at most 512 per resident (about 150 KB at the cap), plus one per job in `latest`; none for other sessions beyond a fresh run's own row. They stay bounded with no scan because the fold sees each entry once, at open and at each append.
- **Fresh runs:** each is a closed session folder on disk. Once closed it holds no conversation, and its task unloads, unless a question it asked is still open: that keeps the task loaded until answered. Also, `record::list` opens every folder when it names a new top-level session or lists repositories, so a frequent fresh job slows those. **Threads:** each costs what any top-level session costs.
- **`GET /v1/deliveries`:** unchanged; it reads the whole index each call, and notes make the index grow faster.

## The hard decision: where runs come from

- **Folded on read from the record.** Each call would read a resident's record back to 24 hours ago: every entry of the day, one indexed read and parse each, per resident, on every poll. RULES 10: "a request is rendered from state, never from re-parsing the whole file."
- **Recorded as their own entries.** Turn ends and withdrawals already hold what such an entry would, so the record would say each thing twice, and older records would have no runs. It also costs controller lines at both closures and still needs an index in memory. (`Fresh` is not this: it is the home session's only knowledge of a run that happens in another record, and the missed-run marker needs it.)
- **An index file per resident on disk.** A crash between two writes leaves it disagreeing with the record, the window the residents design rejected SQLite for.
- **Chosen: folded into the record's summary, in memory.** The record stays the one store (SPEC.md section 5), runs are a function of it recomputed whenever it opens, and a fresh run's row is its own thread's fold. It costs about 50 record lines for runs, at most 512 rows per resident, and a copy per read. Against it: `Controller::summary` copies the summary whole, runs included.

## Glossary lines proposed

- **run**: one heartbeat or schedule input, or one night rebuild, followed to the turn end or withdrawal that answers it.
- **thread**: a session started as a resident, with its identity record, folder, startup files and memory, and its own record and fresh context.
- **home session**: a resident's own session, whose id is its name.
- **fresh run**: a schedule's run in a thread of its own, which closes when the run ends.
- Amend **delivery**: "a file or link handed to the person's inbox, or a note an agent sent the person."
- Amend **resident**: "… a home session and threads; its home session's id is its name."

## As built

### Idle home summaries (2026-11-01)

An idle home unloads like any session; `Controller::load` and `Loaded` go. The
controller configures the record module's `Summaries` with exactly the resident
names on each config refresh, dropping removed homes. Summary requests use the
loaded session's fold while it is present and the saved fold otherwise. On unload,
the controller hands over the final `Record` so its summary moves into that saved
fold. The residents task retains only the home's start time separately.

Unloaded summary reads resume from their last committed boundary. Admitting an
input still opens the record and rebuilds its entry index from the beginning.
The cursor's read boundaries, replacement rules and memory bounds belong to
The record supplies idle summary reads.

The runs fold is its own file, `src/record/runs.rs` (145 lines), and the residents list is `src/residents/list.rs`, which keeps `src/residents/mod.rs` under the 800-line review line.

Departures from the page:
- **The terminal opens a note by copying its line,** as it opens a link by copying its address, rather than jumping to its entry: jumping to a transcript the terminal has not loaded takes more than three lines.
- **One residents rule, `Residents::timed`, replaces `has`.** It covers a resident's home session and a fresh run's thread, so push and follow refuse both with one check.
- **`Residents::thread` gives a thread's folder and Start.** The person's `POST /v1/sessions` and the fresh path both create through `Controller::create`.

Flows 21, 22, 23, 24, 42, 43 and 48 pass.
