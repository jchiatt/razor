# Session controller

One task owns each session's record, admission, actions and conversation. Claude and Responses exchange the same protocol messages; the controller has no route-specific request loop. The API, children and residents pages govern this page where they differ.

## Public surface

```rust
/// Clones share command senders; only the session task mutates its record.
#[derive(Clone)]
pub struct Controller { /* private */ }
impl Controller {
    /// Recovers calls without replay and reattaches jobs; only an open turn rebuilds.
    pub async fn start(home: PathBuf, responses: Arc<Responses>) -> Result<Self, Error>;
    /// The same durable index/subscription used by actions and the API.
    pub fn deliveries(&self) -> residents::Deliveries;
    /// Writes Start only, or reopens a named nonchild; a child never reuses an existing id.
    pub async fn create(&self, cwd: PathBuf, model: Option<String>, effort: Option<Effort>,
        id: Option<SessionId>, child: Option<Child>) -> Result<SessionId, Error>;
    /// Appends in arrival order; person inputs are refused at 64 unanswered, internal inputs wait.
    pub async fn admit(&self, session: &SessionId, input: Input) -> Result<EntryId, Error>;
    /// Only facts use append; inputs always use admission.
    pub async fn append(&self, session: &SessionId, body: Body) -> Result<EntryId, Error>;
    /// The loaded fold or a read-only record snapshot; never starts a session task.
    pub async fn summary(&self, session: &SessionId) -> Result<record::Summary, Error>;
    /// Latest rows for the containing tree, following only parent/child links.
    pub async fn tree(&self, session: &SessionId) -> Result<Vec<AgentView>, Error>;
    /// Escape stops this turn's foreground jobs, not detached jobs or children.
    pub async fn stop(&self, session: &SessionId) -> Result<(), Error>;
    /// `/done`: admits Razor's closing input; when the turn that takes it ends, inputs still waiting are Withdrawn and the session closes.
    /// With no `closeout` (`/quit`) the turn ending now is the last: a running turn stops there, an idle session records an empty closed turn end, and the answer is entry 0.
    pub async fn done(&self, session: &SessionId, closeout: bool) -> Result<EntryId, Error>;
    /// Requests the threshold's continuation note; answers on admission, or None if already requested/rebuilding; refuses a closed child.
    pub async fn handover(&self, session: &SessionId) -> Result<Option<EntryId>, Error>;
    /// Close stops jobs and descendants and holds future inputs; continue_child, or for a session that is no child its next input, clears that hold.
    pub async fn close(&self, session: &SessionId) -> Result<(), Error>;
    pub async fn continue_child(&self, session: &SessionId, input: Input) -> Result<EntryId, Error>;
    /// The next mail, report or Razor notice (including a background job end), or timeout;
    /// person input and handover end every wait with no news.
    /// The waiting call recorded at `at` takes held news at once, else the next news; that call's Result answers it.
    pub async fn wait_input(&self, session: &SessionId, at: EntryId, timeout: Duration) -> Result<Option<EntryId>, Error>;
    /// Answers an open Call once, in its own session; withdraw records Withdrawn for an untaken input.
    pub async fn answer(&self, session: &SessionId, ask: EntryId, answer: Answer) -> Result<EntryId, Error>;
    pub async fn withdraw(&self, session: &SessionId, entry: EntryId) -> Result<(), Error>;
    /// A resident's night: at once when idle, after the turn its input `at` opened, or once the
    /// current batch of a turn that input waits behind settles; schedule due times come from the existing fold.
    pub async fn night(&self, name: &SessionId, at: Option<EntryId>) -> Result<(), Error>;
    pub async fn scheduled(&self, session: &SessionId) -> Result<BTreeMap<String, Millis>, Error>;
    /// Attached, replay and live lines; lag reattaches at the cursor without a subscription gap.
    pub async fn attach(&self, session: &SessionId, after: EntryId) -> Result<mpsc::Receiver<Arc<str>>, Error>;
    /// Agents snapshot and subscription share one lock; account readings belong to each agent row.
    pub fn status(&self) -> mpsc::Receiver<Live>;
    /// Creation model, first person input's first line and last record activity; children are omitted.
    pub async fn list(&self, cwd: Option<PathBuf>, limit: usize) -> Result<Vec<Listed>, Error>;
    /// Indexed, contiguous range: at most PAGE_BYTES, or one larger entry alone.
    pub async fn page(&self, session: &SessionId, before: Option<EntryId>, limit: usize) -> Result<record::Replay, Error>;
    pub async fn get(&self, session: &SessionId, entry: EntryId) -> Result<String, Error>;
    /// Sniffs JPEG/PNG/WebP bytes and stores directly; no decoded bytes enter a command queue.
    pub async fn store(&self, session: &SessionId, bytes: Vec<u8>) -> Result<Image, Error>;
    /// Opens against held directory descriptors without following symlinks; returns no path to reopen.
    pub async fn file(&self, session: &SessionId, name: &str) -> Result<tokio::fs::File, Error>;
    /// One UTF-8 draft per session, at most 1 MiB; admitting person input clears it.
    pub async fn draft(&self, session: &SessionId) -> Result<String, Error>;
    pub async fn set_draft(&self, session: &SessionId, text: String) -> Result<(), Error>;
    /// Bounded fresh read; invalid edits keep the last good value and publish one status snapshot.
    pub async fn config(&self) -> config::Config;
}
```

## Data flow

1. `main.rs` owns bootstrap and clients: create, then admit, including argv/stdin; bare `razor` creates only Start before the terminal attaches. Images come only through the API/composer. One Responses handle is shared by controller and API.
2. The API decodes base64; store sniffs type and owns `images/`. Limits: 10 MB base64/image, 20 images/input, 32 MiB HTTP body; Input.text is at most 1 MiB from person/program and 64 KiB for mail/reports. Paths in records are session-relative; the model receives absolute paths.
3. Inputs append during work or rebuild. Held person inputs join before a finished action's Result; call admission keeps that boundary for refusals and calls run by the model path. The first result takes the held inputs once, unless the conversation is already settling for a handover or finish; later inputs stay held for the conversation that can answer them. Claude's SDK reads them when its calls finish; Responses queues them after every call's output in the next request, never between a call and its output. The person's words join though they name another model, which takes over at the next turn; a switch's own note turn holds its words, and every input after them, for the new model. Internal inputs wait for turn end. Each heartbeat/schedule takes a turn alone; their selections never persist. Other held inputs batch in `pending`'s order into one native user message, each input's images followed by its text. The encoded-line budget takes a prefix only; remaining inputs stay held for the next turn, without changing the bridge's 64 MiB limit. The 64 unread inputs a model path holds count what goes ahead of the batch too (a rebuild's videos, a call's unsent facts), so a large batch never overflows it. Busy heartbeats append nothing. Finished/stopped children retain inputs without starting.
4. Each turn freezes config and calls Tools::configure. A coalesced registry watch refreshes profile declarations and sends Add while a turn runs, without polling or a second registry. `profiles::model()` alone chooses route/window; the selected Profile.version includes code mode and is recorded on Start/Rebuild. A profile change rebuilds; a model change within the profile applies in place unless it hands over; effort applies in place. Native resume requires a matching version. Responses owns retries and direct-result order.
5. Both model tasks emit Call Entry, await Admitted, emit Run and mirror exactly one Result after the controller answers. Claude's Rust task shares CodeMode with Responses; exec/wait stay outside the controller, and native byte permits remain held through admission. A FIFO tokio RwLock per session shares reads and serializes changes; first polls are in arrival order. Session actions use the same lock. Outer code-mode cells hold no lock; nested results commit as they finish.
6. Drain job events continuously and before sending or recording a Result. Started carries its Call EntryId, so even a reattached job's notice can read its original tool name/input. A durable Result showing an end suppresses its queued notice and releases the shell's handle; otherwise its notice is admitted at the next tool boundary, which releases the handle, and joins the turn there; with no turn running it opens one. Result.settle blocks another request after a handover or finish batch. Settled carries usage into the existing TurnEnd; finish then closes jobs/descendants before admitting its report to the parent. Notice wording and error/result rendering are profile data, not controller templates.
7. Stop drops action futures, waits for a job start under way to queue its Started (so that job is in the foreground, marked before its end is recorded), kills foreground jobs and records Unknown; until the path ends the turn, a `Run` it still sends runs nothing, and on Claude the bridge's task stops the turn's cells at once and refuses their later calls, as the Responses loop does by ending the turn for calls without mirrored results. Each job it kills is marked until its end notice is admitted, so the notice says the person stopped it, how long its call had run, and its last lines, and is admitted only with another input or at a running turn's next call, never opening a turn alone; after a restart the record gives the marks back (the live jobs whose calls a stopped turn ended unknown), and a Result that shows such a job's end drops the notice and the mark, as for any job; on Claude the CLI answers the stopped call with its own rejection text, which Razor cannot replace, and on Responses the stopped call's output says the person stopped it. A turn's abandoned waits leave their jobs running and eligible for end notices. Restart queues lost-job notices for the next input, but a job-only root stays idle; a child, which no one else sends an input, opens a turn on them. A second crash before any input loses a root's notice; its Ended{Unknown} stays in the record and status view. Close stops jobs/descendants and holds inputs; settlement forbids continuation.
8. Planned rebuild waits for the batch, never yielded cells. Record::rebuild(trigger, note, cut) stays in-task: only covered asks/parent mail, image paths, settled model entries and one line per other input. Post-cut inputs follow the rebuilt context. The batch a model switch or a restart that cannot resume rebuilds for is held out of the context and follows it in the same message, and so does every held input that would join at a tool boundary, so the new model's request ends with the person's own words rather than the rebuild's whenever there are any (5 November: a threshold rebuild during a live call had ended its request with the context, so the voice could speak the model's words about resuming before it had read the person's). TurnEnd answers an explicit delivered set, never a numeric prefix.
9. Transcript live lines are Attached{session,last}, Delta, Taken and Tail; no state/context line or active-text buffer. Tail is the last five lines, at most 1 KiB, at most once per 250 ms/job, even between model turns; unchanged sizes and I/O errors skip display updates, never fail a session. Agent rows carry state/context and at most 32 named account usage windows, and change only when a field other than activity changes. Each native conversation clears its previous readings before opening, since its sign-in can change. Only config errors belong to the controller-wide status snapshot; there is no controller-wide limit or limit event.
10. Configuration lives in src/config, one struct per TOML table, at most 1 MiB. Fresh bounded reads replace the mtime cache: same-timestamp edits were missed in the real-controller exercise. Invalid changes retain the last good value; status and server.log report changes once. Address/base_url changes require restart; no key lands before its reader.

## Immediate effort

`Controller::effort(session: &SessionId, effort: Effort) -> Result<Option<EntryId>, Error>`
records and applies an effort without admitting input or starting a turn. The API is
`POST /v1/sessions/{id}/effort` with `{"effort":"xhigh"}`, returning `{"entry":N}`,
or `{}` when both the live and recorded choices already match. The session task appends
`Body::Effort { effort }`, updates the live choice and sends the existing `ToBridge::Effort`,
retaining the active conversation's model and speed. The summary folds the fact, so list
rows and restart use the same selection; the fact is absent from transcript rows. Responses applies
queued settings before starting its next request. Claude captures effort for a native
turn, so a busy bridge coalesces a generic one-line notice into its existing held message.
At the next tool boundary it yields the batch and starts a native turn with the new
effort, within the same Razor turn. If the turn ends or is stopped first, the notice
is discarded. An idle bridge sends no notice and starts no request.
An older queued input uses the summary's later effort rather than undoing the choice;
schedule and heartbeat inputs retain their existing per-turn overrides.
All five efforts are accepted as on input; a chat route retains its existing cap.
The additive fact is necessary because an input would wake an idle session; reusing
or replacing admission would change that existing behavior. A held message keeps its
real input's UUID when an effort notice joins it, so that input is still acknowledged.

## The person's handover

1. `Controller::handover` enters the session task through `ask`, like `/done`.
2. A closed child is refused before any input is appended.
3. An existing threshold request, handover batch or rebuilt context returns `None`.
4. The threshold detector and the person's request share the continuation-note input.
5. A working conversation receives that input at its next tool boundary, as today.
6. An idle session admits it through the existing input path, opening its short note turn.
7. The conversation's existing `threshold` field names the admitted input in both cases.
8. The model calls `handover` with its note as the call's one field; the current batch settles first. A handover is never refused: a missing or blank note rebuilds with the latest note, as after a restart. (Before 2026-10-29 the note was the message's text and a note-less handover was refused; Claude often wrote the note only in its thinking, and the refusal asking for it as text was flagged as reasoning extraction.)
9. The existing `Trigger::Threshold` rebuild uses that note and the same record cut.
10. The API returns `{entry}` or `{}` immediately; subsequent inputs use the rebuilt context.

**Decision:** the person's handover is the threshold reached on demand, not a direct call to `Controller::rebuild`. Sharing the threshold path costs one optional-entry operation and an idle admission case, but preserves the model's fresh continuation note and batch ordering. Calling rebuild directly would save the short note turn but use a stale or absent note; a separate trigger or pending flag would duplicate state the conversation already owns. No record kind, prompt, config key or terminal change is needed.

## Record and protocol changes

Shared conversation messages live in protocol: Open prompt/tools, Add, Effort, Input{session, inputs: Vec<BridgeInput>}, Result.settle, optional Entry.native and both Resume variants. BridgeInput holds the durable entry id and rendered text/absolute image paths; batching changes only this conversation wire, not Body::Input or the record schema. Input gains model/effort/images; Start gains profile_version/child; Rebuild gains profile_version/cut; Change and Night are triggers. Named SessionIds support residents. Job Started gains its durable Call link. TurnEnd gains `context: Option<u64>`, the context tokens known at the boundary. Settled{usage} preserves handover/finish accounting without a new record kind.

The record folds profile version, persistent selections, rebuild cut, original start/activity times, person-only asks, parent-mail references, title, child lifecycle/report, headline/files, open person/mail questions and each schedule's maximum due time (64 names). `Withdrawn { inputs: Vec<EntryId> }` removes undelivered inputs from unanswered inputs, asks and parent-mail references without changing the last turn end. If the title's input is withdrawn, the title clears; the next person input sets it, rather than retaining taken-back words or rereading earlier inputs. Rebuild also omits withdrawn inputs from its one-line input entries. Older `TurnEnd { end: Stopped }` entries keep their existing meaning; no migration. Child, Headline, ChangedFiles and MailSent are fact entries; MailSent.id and MailSent.kind are mail_id and mail_kind on the wire, separate from the containing entry's id and kind. Report holds status/text/entry. Origin adds Heartbeat, Schedule, Program, Answer, Mail and Report. Answer names an EntryId in its own session. Paging uses the offset index; list omits children. Summary.responses holds only EntryIds carrying Responses native data since the last Rebuild; resume reads those indexed entries, never scans the record again or re-renders an admitted Input. Native additions persist actual sent inputs (including memory) and direct results. `Record::entry` exposes its existing indexed decoder. No second mail store or cross-record transaction.

Claude, GPT, children, residents, images, restart and rebuild flows run; flow 34 requires one reply for held inputs. Flow 28 is enabled after a real current-prefix measurement: 4,889 input tokens, previously 4,552, below 12,000. The code-mode adapter adds no record/protocol field or dependency; its yielded-cell proof also caught and fixed a whole-server QuickJS callback-ownership abort.

## Second pass

The rewrite keeps this surface and every behaviour above, in less code. Work reaches a session's task as a boxed closure on its queue (`ask` answers through a oneshot, `tell` does not), so there is no command enum. Each conversation owns the channel its lines arrive on, a branch of the session's loop: a dropped conversation's lines are never read, so no open number is checked. Dropping the conversation also ends it, since both model paths end the native conversation when their input channel closes; the controller sends neither `Stop` nor `Close`. A conversation's threshold input, handover call and finishing report live on it and go with it. Inputs a rebuilt context covers join the delivered set behind the rebuild entry; a job's end notice is rendered from its Started call before its end is recorded, and a Result that showed the end drops the notice once it is mirrored. Idle conversations past `sessions.idle` close longest-idle first. A record read that finds its own session missing ends its loaded task and removes its status row, publishing an `agents` snapshot to existing subscribers. Queued work preserves the missing session's id; it never forgets its caller when another session is missing. File reads use `summary` without starting a session task, and forget a session if opening the file fails and its folder is gone. Replay and page open the record before returning, so the API can refuse with 404 before sending headers.

## Idle tasks and held inputs

`sessions.idle` remains a count of warm idle conversations, not a duration. With no conversation, a task leaves after its queue drains and it has no working turn, unanswered input, job, running action, end notice, attached client or live child. The existing controller ticker rechecks after callers or clients leave; no timer or config key is added. A detached client's relay drops its subscription even when the session sends nothing. List and tree read unloaded records without starting tasks, retain the last published rows, and never repair a record while reading it. A reopened conversation uses the record position for its open number, so a late native line cannot match an earlier open after unloading.

Removal holds the same map lock as loading and checks both an empty queue and a single sender (the map's). A caller that already holds a sender prevents removal; a caller arriving after removal loads from the record. Nothing can enqueue to a removed task. A finished child's continue still admits while held, records `Continued` in the parent, then releases it; refusal changes neither hold nor parent fact.

The known rebuild duplicate stays open. Interrupting `wait` for a held person input is not a sufficient ordering rule: a threshold input is sent past held inputs too, and wait can pass held inputs from other origins. [Claude Code](https://code.claude.com/docs/en/how-claude-code-works) reads queued corrections when running calls finish; Codex distinguishes steering at request boundaries from cancellation of tools. SPEC section 3 promises the next tool boundary, not cancellation of a waiting call. The narrower wait change would alter that behaviour without removing the duplicate case.

### A rule for the duplicate

The case: the rebuild's cut is the latest input sent (`conversation.rs:302`). An input still held when a later one was sent sits at or before the cut, so the rebuilt context renders it (verbatim among the latest four asks, else as one line) and the held queue then delivers it again: the model reads it twice. Whatever passed it (a `wait` taking newer news, the threshold input, any input that is not news) the duplicate needs the same two things, a held input rendered and then delivered.

**The rule: a rebuild renders only what the model was sent.** A held input is left out of the rebuilt context wherever it sits and stays held; it reaches the rebuilt conversation as inputs past the cut do (one that would join at a tool boundary, such as the person's, in the rebuilt context's own request, the rest at the turn's end), and the turn that takes it answers it. So each input reaches the model once, and the rule does not depend on why the cut passed it.

What it would change: `Record::rebuild(trigger, note, cut, held)` treats the held ids as it treats withdrawn ones (out of the latest asks, the earlier asks' ids and the one-line inputs; the calls listed then start at the latest delivered ask), and the controller passes its held ids and leaves them out of the inputs the rebuilt turn answers. Flow 34 gains a session whose handover batch holds a `wait`: a person message sent during the batch stays held, mail the wait takes is sent after it, and the rebuilt context leaves the message out while the last request carries it once, after the context.

Why it holds where the two rejected fixes did not: dropping held inputs at or before the cut lost a held mail older than the latest ask, and here nothing is dropped; moving the cut below the first held input left a delivered report after it neither rendered nor answered, and here the cut stays the latest input sent, so every delivered input is rendered and answered. The wait rule is not needed.

What the model sees differently: a held input arrives after the rebuilt context instead of inside it, once. A person's correction held at the rebuild still reaches the model at the next tool boundary, so SPEC section 3's promise holds.

The other rule considered, by its cost: *render every held input inside the rebuilt context and drop it from the queue.* The model also reads it once, but the rebuild would have to render inputs outside the window it reads (a held mail older than the latest ask, which the one-line list never reaches), and the rebuilt turn would answer inputs before the model had acted on them.

## Escape keeps the conversation (29 October)

Pressing Escape in a long Claude session and then sending a message rebuilt the session from the record (about 280k of context: a stopped turn end, then the input, then a restart rebuild). `stop()` drops the conversation (since the rewrite), and `reopen()` resumes only after a `Done` turn, so every stopped turn rebuilds and loses the native history and the prompt cache. claude-slice.md designed Stop as kill the foreground job and `interrupt()`: the SDK ends the turn aborted, writes its own `tool_result` for the interrupted call, and the conversation goes on (probe B).

**What changes.** Escape stops the turn's running actions and kills its foreground jobs at once, as today, then sends the model path a new `ToBridge::Interrupt { session }` and keeps the conversation; the turn ends when the path reports its end.

- *Claude.* The bridge calls `query.interrupt()`, then answers the calls the controller stopped (their pending runs and admission gates), since no `Result` will come for them. The SDK ends the turn `aborted_tools` or `aborted_streaming` and writes its own `tool_result` for an interrupted call; for a call Razor ran and had not answered, the bridge leaves it unmirrored, so the turn's end records `Unknown` (below).
- *Responses.* The task drops the request in flight and stops its cells (dropping its turn does both), answers each unanswered call in its native history (with the call's own result when it had one, else `record::UNKNOWN`, the text of the `Unknown` result the controller records for it), and ends the turn stopped.

`turn_ended` already maps `aborted_*` to `End::Stopped`; the Responses task's `stopped` joins them. A stopped call thus has one result on each route, `Unknown`, since neither path's own interruption text carries Razor's answer: on Claude the model reads Claude Code's "The user doesn't want to proceed with this tool use…", on Responses `record::UNKNOWN`. Flow 05 expects `Unknown` on both; from this section's change until it expected the CLI's `error` on Claude.

**Matching the path's turn end to the stopped turn.** There is nothing to match: the path's own turn end is the stopped turn's end, as the slice designed. The controller no longer ends the turn first and then has to discard a late tail.

**An input already sent when Escape lands.** The turn end answers the inputs the path says the turn took. One sent but not yet taken stays sent: the SDK runs it as its next turn by itself (probe B), and the Responses task starts a turn for inputs it holds; that turn's end answers it. The record and the native history agree on who answered what. After a turn ends with nothing held, the session is working while any sent input is unanswered and needs you otherwise (claude-slice.md's Ending rule), so that turn shows as working and Escape stops it (it had shown needs-you, a second Escape did nothing and its job outlived the server). A stopped turn that answered the threshold input clears it from the conversation, so the next reading asks again; left set, every later turn ended without answering its inputs and the session hung.

**When the path does not answer.** A second Escape while the interrupt is pending, or an Escape with no conversation open, ends the turn in the controller and drops the conversation, as today, so a path that never answers cannot hold the turn open. `close()` drops the conversation first, so a closing session still ends at once. `reopen`'s `Done` rule stays for a cold reopen after a restart.

**Who ends the stopped turn**, by what each costs:

- *The path* (chosen). One end per turn, the inputs answered by what the path took, no tail to tell apart. The turn stays working until the path answers: milliseconds, and the second Escape covers a path that never does.
- *The controller, at once, discarding the path's late lines up to its turn end.* Two ends to reconcile: the SDK's mirrored `tool_result` for a call the controller already gave `Unknown` becomes a second `Result`; an input the SDK still runs as its next turn was already answered by the stopped turn's end; and the controller must tell the stopped turn's tail from the next turn's lines.

**Flow.** 36-stop, for Claude and GPT: after the stopped turn and a person input that carries the job's notice, the record holds no rebuild and each call one result; on Claude every native conversation id in the record is the same; on Responses the last request still carries the first call's id.

## The threshold after a rebuild (29 October)

`handover` refused while `rebuilt` was set, and `rebuilt` cleared only at the turn's end, so a turn that kept working after its threshold rebuild never got a second threshold input (Trey saw a child back at 85% of its window after one handover). The guard is there so the rebuilt request's own size cannot ask for another handover at once, which would rebuild again for nothing. It now ends at the first context reading of the rebuilt conversation: that reading is tested with the guard still up, then `rebuilt` clears, and later readings may ask again. The size it holds is only read for `prompt_too_long`, which comes before any reading. A person's `/handover` is refused on the same guard, so it too is refused only until the rebuilt request reports its size, not for the rest of the turn (flow 18's check of a request made after that response goes). Flow 17, on GPT: in one turn, a large response asks for the handover; in the rebuilt conversation the first response, large too, asks nothing, and the next large one gets the second threshold input and a second threshold rebuild.

## Opening a conversation from the startup list

No surface changes. `Session::open` (`session/conversation.rs`) keeps its order (render, prompt, `Before::Prompt`, first input's prefix, `Open`) and hands the new work to its owners:

1. Render the profile, as today.
2. `residents::identity(...)` gives a resident's identity record (None for any other session), in place of today's `residents::prompt(...)`, whose persona replaced the whole prompt.
3. `profiles::opening(&profile, config.startup_for(&self.id), root, s, identity)` reads the list, `s` the session's summary, which gives the folder and the session's name and id for the facts; `profile.prompt` takes its `prompt`. `root` is the session settings' local root (`Settings.project`), so the project walk and the project's settings agree on where the repository starts.
4. `Before::Prompt` plugins see and may change that prompt, as today.
5. A fresh open or rebuild builds the prefix with `Opening::page(memory, skills, mcp)` instead of the three pushes it does now, then `profiles::save(&dir, &self.profile, &page)` writes `startup.md`. A resume builds neither, as today.

`Run::act` (`session/actions.rs`) no longer writes a job's file path and call id into every Claude job result: a result's `path` is the shell's own, so a Claude shell result spills only past `SPILL`, as every other route's does (docs/design/profiles.md). Job notices still carry both.

No record kind, protocol message or config key is added here; `Config::startup_for` and the `startup` key live in `src/config/` (API and configuration row).

## A job's end notice joins the running turn (30 October)

Trey, 2026-10-30, to "A background job's 'finished' notice reaches a running turn at its next tool call, instead of waiting for the turn to end": "7. yes." The night before, an agent polling inside one long turn learned its job had died 95 minutes late, because an end notice waited for the turn's end (data flow 6). The notice now joins at the next tool boundary, by the same path as the person's words, a parent's mail and answers to the session's own questions.

SPEC section 3's sentence becomes: "a person's message to a working agent is delivered at its next tool boundary, so a correction arrives before the next action (what Trey uses today), and so are mail from the agent's own parent and an answer to a question it asked (Trey, 2026-10-29, after a coordinator's corrections reached its children only after they had committed), and a background job's end notice (Trey, 2026-10-30, after an agent polling inside one long turn learned 95 minutes late that its job had died); other mail and scheduled jobs wait for the turn to end". Data flow 6's "one admitted Razor notice releases it and waits for turn end" becomes "its notice is admitted at the next tool boundary, which releases the handle, and joins the turn there; with no turn running it opens one".

### Surface

No public change. Three private functions change, in `session/conversation.rs` and `session/actions.rs`:

```rust
impl Session {
    /// At a tool boundary, queued end notices are admitted first; then held inputs join the turn
    /// when they are the person's, an answer to the person's ask, direct mail from this
    /// session's parent, an answer to a question this session asked (Trey, 2026-10-29), or a
    /// job's end notice (Trey, 2026-10-30): any Razor input but `/done`'s. The rest, channel
    /// mentions included, wait for the turn's end, and those that join pass them (Trey,
    /// 2026-10-30), but not an input that switches the model. Not while the conversation settles.
    async fn boundary(&mut self) -> Result<(), Error>;

    /// Admits queued end notices while there is room, each releasing its job's shell handle,
    /// except the notice of a job whose end a Result not yet mirrored shows: mirroring that
    /// Result drops it, and if it is never mirrored it stays queued for the next turn.
    async fn flush_notices(&mut self) -> Result<(), Error>;

    /// An action finished: the job it leaves running leaves the foreground, and is marked shown
    /// when the result shows its end, before held inputs join; so the notice of an end this
    /// result shows never joins beside it.
    async fn ran(&mut self, ran: Ran) -> Result<(), Error>;
}
```

### Data flow

1. A job ends while a turn runs. Its `Ended` is recorded before any Result can overtake it, and its notice is rendered from its `Started` call, as now.
2. A foreground job's notice waits in its slot until its call returns. Any other notice goes to `notices`, as now.
3. At the next tool boundary (a call's admission, or a finished action before its Result), `boundary` calls `flush_notices`: each queued notice not shown by an unmirrored Result is appended as an `Origin::Razor` input, its shell handle is released, and it goes on `pending`.
4. The inputs on `pending` that join (up to one that switches the model) now take these notices with the person's held words, in one `ToBridge::Input`.
5. Each model path places them as it places the person's words: Responses after every output of the batch, Claude's SDK when its running calls finish.
6. The turn's end answers them, as any delivered input.

**Why no new origin.** The test is "a Razor input other than `/done`'s", which needs no record change. The other Razor inputs never wait in `pending` while a turn runs: the threshold input and MCP news are delivered at once, and the nudge is admitted only to a waiting session, which it starts. `/done`'s input keeps waiting, so the closeout stays a turn of its own. A new `Origin::Job` would say the same thing more plainly but costs a record schema change, and flows 05, 07, 19 and 35 match the notice's origin by name.

**The one hard part: an end the model already sees.** Today a notice waits for the turn's end, so a Result that shows the same end (a `wait`, `bash`, `send` or `kill` that saw the job finish) drops it first. Now that the notice moves at the next boundary, two orderings matter:

- *The result that shows the end is the boundary's own.* `ran` marks the job shown before the boundary, and `flush_notices` skips a shown job, so the notice never joins; mirroring the Result drops it, as now. Without the reorder every `wait` that returns at its job's end would deliver the notice too.
- *Another boundary falls between the end and that result.* A notice already admitted cannot be taken back, so the model reads the end twice: once as the notice, once in the result. This happens when the model calls `wait` on a job that has already ended unseen (the notice joins at that call's admission), or when a parallel call finishes in the instant between a job's end and a `wait` on it returning. It is the price of hearing early, and both readings say the same thing.

**Edge cases handled.**

- A notice queued behind an input that waits (a channel mention, other mail) passes it and joins at the next boundary, as the person's words do (Trey, 2026-10-30); one behind an input that switches the model waits for the next turn.
- A settling batch (handover, finish) runs no boundary; its notices stay queued for the next turn, as now.
- Escape queues the stopped turn's ended jobs' notices for the next turn, as now.
- A restart's lost jobs, ended unknown, join the first boundary of the next turn or open it, as now.
- At 64 unanswered inputs `flush_notices` stops and the rest stay queued, as now.

**As built.** The session controller went from 2,510 to 2,515 of 2,600; no protocol, record or config change. A scratch run against the debug build and the stand-in, outside the repository, checked the case no flow reaches. A GPT turn started a background job that ends after one second, then called `sleep 3`. The notice was admitted at that call's end and sent with its output, so the next request carried `Job job-1`, and the turn's one end answered it.

**Flows.** 07 (job notices), 05 and 36 (stop), 19 (restart), 35 (unknown outcome), 11 and 40 (children holding inputs), 42 (the mail rule's case) and 25. No flow ends a job during a later call of the same turn; that case was exercised in a scratch run against the built binary and the stand-in, outside the repository.

## Admission counts what the model has not read (30 October)

**The defect.** Admission refused a person's input, and held every other input, once 64 inputs were unanswered. An input that joins a running turn, or that a waiting call takes, stays unanswered until that turn ends, so a turn that never ends fills the 64 with inputs the model read long ago. A long-running coordinating session (Claude) worked in one turn until it held 64 unanswered inputs, 63 of them child mail and reports its own `agent wait` calls had taken and one a person's, and from then its children's message calls hung and mail from a paired server timed out. The reverted fix stopped counting an input once it was handed to the model path. But the Responses task holds handed inputs until its next request, with its own limit of 64, so a response with two calls let 64 in at the first call's end and more at the second's: the task overflowed, the turn failed, and inputs the model never saw were answered.

**The rule.** An input counts against the 64 from its admission until the model has read it: while Razor holds it, and while the model path holds it unsent or sent in a request whose reply has not begun. Nothing passes the 64, Razor's own inputs included: the threshold input and MCP news, which are sent at once rather than admitted, wait for room as an end notice does. The model path names, with each model message, the latest input that message's request carried. News a waiting call takes is that call's result, not an input to the path: the path sends every result of a batch with its next request and holds results nowhere else, so the news stops counting when the call takes it (63 of the incident's 64 were), and the call's Result answers it, since that Result keeps its words (one never-ending code-mode cell took 160 mails with no further request, and each stayed unanswered).

### Surface

The public changes are `admit`'s doc ("refused at 64 the model has not read") and `wait_input`, which takes the waiting call's entry. The record's `Result` gains `inputs`, the news it answers (serde default, empty omitted). The lines from the model paths change, and the session gains three lists and four functions, and loses `consumed`, which duplicated `delivered`:

```rust
// protocol/conversation.rs
pub enum FromBridge {
    /// One per model message: its request's input tokens, the window where the route knows it,
    /// and the latest input that request carried; the model has read it and every input the
    /// path received before it.
    Context { tokens: u64, window: Option<u64>, read: Option<EntryId> },
    /// The turn's result, usage and window, and the latest input it took, which the path will
    /// not hand the model again (a path takes inputs in the order it received them); None when
    /// it took none. Was every input it took.
    TurnEnd { success: bool, reason: Option<String>, message: String, usage: Usage,
        window: u64, took: Option<EntryId> },
    // The other lines are unchanged.
}

// controller/session.rs
/// The most inputs a session holds that the model has not read. (Was UNANSWERED.)
const UNREAD: usize = 64;

struct Session {
    /// Inputs sent to the model path and not yet read, in the order sent; with `pending`, at
    /// most UNREAD.
    sent: VecDeque<EntryId>,
    /// The inputs that opened the turn, which only its end answers; at most one message's.
    opening: Vec<EntryId>,
    /// News waiting calls took, with each call's Call entry, until that call's Result answers
    /// it; one per waiting call.
    taken: Vec<(EntryId, EntryId)>,
}
impl Session {
    /// UNREAD inputs held or sent unread: a person's input or an answer is refused `Full`, any
    /// other input waits for room, and end notices, the threshold input and MCP news wait.
    fn full(&self) -> bool;
    /// The model has read `latest` and every input sent before it: they leave `sent`, found by
    /// position (an id no longer there was read already), and inputs waiting for room wake.
    fn read(&mut self, latest: Option<EntryId>);
    /// What a handover's turn end answers: the inputs up to the previous rebuild that the model
    /// has read, and that rebuild's entry, except the turn's opening inputs and the asks and
    /// parent mail the record still lists. So a turn carries an input across one rebuild.
    fn carried(&self) -> Vec<EntryId>;
    /// The news the call took as it waited, which its Result answers; `taken` holds each pair
    /// (the call's entry, the news) until then, one per waiting call.
    fn took(&mut self, call: &CallId) -> Vec<EntryId>;
}
```

### Data flow

1. Admission, `flush_notices`, the threshold input and MCP news test `full()`. A threshold reading that finds no room asks again at the next reading; the person's `/handover` is refused `Full`; MCP news waits for the next finished action.
2. `deliver` appends each input to `sent` as it sends it; a rebuild's context is the new conversation's only sent input, alone in a fresh conversation.
3. Responses: `request()` takes every input the task holds into the request and keeps the last as the turn's `took`; each response's `Context` names it as `read`. An input in a request Escape drops is in the native history and is not sent again, so the stopped turn's `took` names it.
4. Claude: the CLI writes a `command_lifecycle` frame with state `started` when it takes an input in, whether that input opens a turn or joins between tool rounds, before the request that carries it. That is a candidate. It becomes the turn's `took` when the reply to that request begins: the first top-level `message_start` stream event or assistant frame after it, an API error's own frame included. At a turn's end the candidate is the turn's `took` too, Escape's included: an input whose request Escape aborted before any reply is in the CLI's conversation, followed by "[Request interrupted by user]", and the next request carries it. Escape's receipt (`still_queued`, sdk.d.ts:4836-4850) lists only inputs the CLI has not yet taken in, which stay sent and run later, and, in the first command's startup window, a batch it delivers once into a turn that starts aborted, whose own end answers it; so the bridge does not read the receipt. The first assistant frame of each new message, where the bridge already sends `Context`, names the turn's `took` as `read`. The bridge's map from an SDK uuid to its batch's entries goes: a batch's uuid is made from its last entry, and the latest is all the controller uses.
5. `context()` calls `read` before its threshold test, so a threshold input sent there waits for the next report.
6. At the turn's end the controller calls `read(took)`, and the turn answers every delivered input no longer in `sent`. What stays in `sent` is what the path holds for its next turn. When the path names none, nothing is read; the old rule that answered the first delivered input would answer an input the CLI is about to run again.
7. A handover's turn end (`settled`, and a finished turn that took the threshold input) answers `carried()` where it answered nothing; the rebuild then covers the rest.
8. `end_turn` removes answered inputs from `sent` as from `delivered`; `abandon` and `close_out` clear it; a second Escape and `finish` answer everything delivered, which empties it. A redirect answer taken while working joins `delivered`, so the turn's end answers it. News a wait takes goes to `taken`, not `delivered`; held news leaving `pending` wakes inputs waiting for room. The Result that hands it to the model names it in `inputs`, and the record's fold answers it there; an `Unknown` Result names nothing, and the news goes back to `pending` (at the end of this section). A rebuild's cut becomes the record's end, where it was the latest input sent: news a wait took reaches the record only through its Result, so the old cut left it out of the rebuilt context (flow 34 caught this). Held inputs are left out by name, as before.
9. Freed room wakes waiters in `read`, as it already does in `end_turn` and `withdraw`. Handing an input to the path frees nothing, since a sent input counts as a held one did.

### Where the read signal comes from

This is the hard decision. What each route can tell:

- **Responses**, exactly. The task drains every input it holds into a request, and that request's response is the model's reading.
- **Claude**, from probes against the stand-in with the real CLI and a bridge copy that logs every SDK frame. For each input with a client uuid the CLI writes `command_lifecycle` frames: `queued` when it arrives, `started` when it is taken in (after PostToolBatch, before the request carrying it), and `completed` at the turn's end, or `cancelled` if Escape ended the turn after its reply began. The probe's second and third requests carried the inputs whose `started` came just before them. The documented signal is `user_message_uuids`, on the result and on some frames, and it keeps only the first 64 inputs a turn took: in a 70-input turn, on a scratch build with the limit raised, the status frames and the result both stopped at the 64th, while the 71 `started` frames named every input. `command_lifecycle` is missing from the pinned SDK's TypeScript types, although its own comments refer to it and the CLI's output schema describes it. claude.md already reads `unifiedWindows`, which the pinned SDK's type also leaves out, on the same terms. A later SDK without the frame would fail flow 34.

The options, by what each costs:

- **The path reports (chosen).** It is exact on both routes and the only option that serves a turn past 64 inputs. Protocol +2; model paths about even (the bridge's list and map become two numbers, and the task's list one id); controller about +40.
- **The controller infers it** from the model messages it already sees: everything sent before the previous model message counts as read at this one. That needs no change to the lines, but it is wrong both ways. On Responses a yielded code-mode cell keeps running while a request is in flight; an input sent at one of its calls goes in the request after next, so it would count as read while still in the task's buffer. Everywhere else it is one message late: a turn of 64 inputs, read by the second response, would free room only at the third, which there is the turn's end. On Claude it depends on timing. Nor can it fix the defect below.
- **The frames' `user_message_uuids`** stop at a turn's first 64 inputs, so they fail exactly the turn this fixes.
- **Confirming at the first assistant frame** leaves an input sent forever when Escape lands after its reply has begun but before any content block completes: an Escape probe got `message_start`, then the interrupt, `cancelled` and an aborted result, with no assistant frame. The reply's start is the earliest sign the request reached the model, so that is the confirmation.

**A defect the probe found.** On today's bridge a Claude turn that takes more than 64 inputs answers only the first 64 at its end; the probe's turn answered 64 of 71. The other seven stay sent and unanswered; the SDK will not run them again, so the session shows working until Escape. Today's limit of 64 unanswered hides this, and this change would expose it. Taking `took` from `started` frames fixes it.

### Bounds

- **Unread inputs.** Admission stops while `pending` and `sent` together hold 64, Razor's own inputs included. `sent` holds at most 64, since a delivery never gives the path more than 64 less what it holds unread (`fit`), so the Responses task's buffer (part of `sent`) never passes its 64; its Size check stays as that buffer's written cap. News put back after an `Unknown` Result can take `pending` past 64.
- **A turn that never ends.** A cap at admission alone cannot bound it, since every rebuild appends an unanswered Rebuild entry and carries its predecessors forward, and Razor appends some inputs without admission. The rule that removes the case: a handover's turn end answers what the previous rebuild carried (`carried()`). Every input it answers was read: either before the previous rebuild, or in that rebuild's context, which the handing-over conversation read as its first request. So a turn holds unanswered at most what its last two conversations read, each bounded by its model's context window (the threshold rebuild comes as the context fills, and a conversation that cannot rebuild fails at its window), plus its opening inputs, the asks and parent mail the record lists (four through the cut, plus those after it), 64 unread and one Rebuild entry. News a wait takes adds nothing once the Result that hands it to the model answers it, which for a code-mode cell's waits is the next Result that reads the cell; so a cell the model does not read again holds what it took unanswered until its turn ends. That is how `responses` is already bounded. No number is added, and the 4,096 of the first page goes. summary.rs's comment changes to say this.
- **What the exceptions keep.** The opening inputs stay for the turn's end, so `/done`'s input still closes the session only when its turn ends, a schedule's failure notice still names it, and `--print` still waits for the turn's end. The asks and parent mail stay for the rebuilt turn, as flow 34 expects. Inputs answered earlier are those that already sit behind a second rebuild: a turn end after them no longer names them, so a failure reaches them only through the turn's own error.
- **The paths.** The Responses task's `took` was every input of the turn and becomes one id. The bridge kept a list and a map with an entry per batch the turn took; it now keeps two numbers.

The other bound considered was to wait at a fixed count, 4,096 unanswered, on every path. Admission and the inputs Razor appends can wait there, but a rebuild cannot, so the count still grows, and at the bound a session refuses input until its context fills and its turn fails.

### Record

One field: `Result.inputs`, empty and omitted on every Result but a waiting call's; that Result answers the news it names, and the turn's end does not list it again. Everything else lives in the controller and on the lines between the controller and its model paths, which are installed together. A handover's TurnEnd lists inputs where it listed none. Razor has not shipped, so no binary older than the record's writer reads it (Trey, 2026-11-04: "there should be *zero* backwards compatibility code"). claude.md's `Input` paragraph ("TurnEnd names all consumed inputs") and claude-slice.md's Ending rule change with step 4.

### The reproductions

- **cap.py** passes: each note is read by the response after the call it joins.
- **wakeup.py**: the second response reads the 64, so the internal input waiting for room is admitted when that response arrives, and so is the person's probe. The script now waits for its turn's end, so its six-second `sleep` no longer outlives the flow.
- **batch-overflow.py**: its `extra` input would be the 65th the model has not read, so it is refused (409), and the script's assertion expects the refusal. The turn ends done, having sent all 64 in its second request, with no overflow and nothing answered unseen.
- **synthetic-overflow.py**, inverted: the person's `/handover` with 64 sent and unread is refused `Full`, the turn has no Size error, and every note is in a request before its turn ends.

**Flows.** 05, 07, 16, 17, 19, 20, 34 and 36-stop, scratch runs of the reproductions, a turn of 70+ inputs on each route, and the Escape probe.

The case where the CLI puts an input back could not be driven from outside: the window between the CLI taking an input in and its request leaving is too short. Escape while a reply is held before its first event does stop the turn (the first probe of it never held the reply). Before this rule, that stopped turn answered nothing, the input stayed sent, and the session took no further turn.

**News whose Result ends Unknown goes back to `pending`** (30 October). News a waiting call takes is answered by the Result that hands it to the model: the call's own, or, while the call's code-mode cell has not returned, that cell's exec Result, since a nested call's Result reaches only the cell's code. A nested call's id is its exec's id, a colon and a number, so its owner is found by the last colon (an exec id may hold one, as an open model's `functions.exec:0` does) and must be an open exec Call, which the summary lists as `cells`; an unrelated top-level pair such as `base` and `base:1` shares nothing. An Unknown Result hands the model nothing, and carrying the words in it would not help: neither path shows the model the controller's Unknown output (the Responses task writes its own sentence into the native history, the CLI its own interruption marker). So `unknown_results` puts every news still taken back at the front of `pending`, in record order, and the next turn delivers it as an ordinary input; the turn's end does not list it, since only an answering Result adds to `answered`. Taken news still does not count toward the 64: counting it until the exec returned would refuse the person and stall mail for as long as a cell that has taken 64 runs, which is the incident again (nested-mail.py stalled at mail 65). Instead a delivery never gives the model path more than 64 less what it holds unread (`fit`), so news put back past 64 waits in `pending`, admission stays shut while `pending` and `sent` hold 64, and the Responses task cannot overflow; when the path already holds 64, turn start sends an empty batch, which both paths ignore, and the path runs what it holds. A restart mid-turn loses `taken` and leaves the news unanswered in the record, for the restart's rebuild to carry; that rebuild can today answer unread mail older than the latest person input without showing it, which the later fix addresses. A cell that never returns holds what it took unanswered in the record until it ends. Once a cell had yielded, its later waits answered at their own Result, so a stop before the model next read that cell lost what they took; the next section covers that. On an earlier revision, stop-mail.py found both cases: the nested wait's own Unknown Result answered the mail, or its known Result did while the exec ended Unknown; either way no request carried the mail's words.

## News a cell's calls take is answered by the next Result that reads the cell (31 October)

**The rule.** News a call takes is answered by the first Result the model reads that carries or follows it, and by nothing else. A waiting call's own Result carries it. A call a code-mode cell makes returns its Result to the cell's code, so its news is answered by the next Result that showed the cell: the cell's exec, if it has not returned, or a `wait` naming the cell, even after a yield. An `error` Result, a refusal included, delivered no news and showed no cell, so it answers and moves nothing. A Result the model path writes itself carries no answer of Razor's and is not mirrored, so a stop before a Result that showed the cell leaves the calls `Unknown` and `unknown_results` puts the news back, as before.

**The two defects.** (GPT, live): once a cell yielded, its exec was no longer open, so a later nested wait's own Result answered its news; a stop before the model's next `wait` on the cell lost it. (Claude code mode, off by default): on Escape the CLI writes "The user doesn't want to proceed…" for the running exec, the bridge mirrored it, and that Result answered every mail the cell had taken.

**Where the link lives (the hard decision).** A cell is named by its exec's call id, which the model passes to `wait` as `cell_id`, and a call the cell makes has that id, a colon and a number; code mode refuses a call id that cannot name a file. Call ids name spool files for the whole session, across rebuilds and restarts; both routes give every call a fresh id (`toolu_…`, `call_…`), so only a model reusing one would collide, and then exec fails to create the spool rather than overwrite it. The record's summary keeps `cells`, this turn's exec Calls, and `waits`, this turn's `wait` Calls whose `cell_id` names one of them, each with that exec Call; `Summary::reads` gives the cell a call reads. The turn's end clears both, since cells stop with their turn. `took` moves a nested call's news to its cell's exec Call (found by the prefix among `cells`, open or not), and a Result that is not an error answers the news filed under the cell it reads. The other homes cost more: the session would hold the same lists beside the record that already folds every Call, inside the controller's last lines; the code-mode runner knows each read but not the news, and on Claude the outer Results come from the bridge, so it would need a new line from both paths to the controller.

**Knowing a Result showed the cell.** The outcome decides, with one exception: every refusal is an error on both routes (Responses records it, Claude mirrors `is_error`) and answers nothing, but a cell whose code threw ends in an error Result headed by code mode's `FAILED` status line, "Script failed", which still shows the cell's output, so it answers the cell's news. Counting it as a refusal sent every mail the cell had printed to the model again, and once the cap held 64 it refused the next wait too. Only exec Calls resolve a `cell_id`, so a `wait` naming an earlier `wait`'s id reads nothing; code mode refuses it as no cell anyway.

**What a stop does.** News that no Result showing the cell answered goes back to `pending`, as `Unknown` did. Answering it at the cell's end instead would cover a cell that takes news, is read, and only later shows what it took, but every news a cell took and the model read would come back as a duplicate whenever the turn ended with the cell still running, and a cell pumping mail would hold all of it unanswered until then. So a Result that showed the cell answers the news taken before it, whether or not the code had shown it yet.

**On Claude.** A call stays in the bridge's `runs` until its answer goes to the SDK, which is when `run()` returns, after Razor's answer has waited behind the bridge's queue (an `Add`, an `Input` reading its images) and its own images are read. Escape notes the calls then in `runs`, and the CLI's own result for each is not mirrored, so its outcome is `Unknown`, as on Responses (flow 05). The note is exact as far as the pinned SDK's code shows: it writes the interrupt request when `interrupt()` is called, before any later answer, and the CLI reads its input in order, so every noted call's answer reaches the CLI after the interrupt and the CLI writes its own result for it. That is every stopped call Razor ran, not only exec: a plain `agent wait` whose Result waited behind a person's input with three large images when Escape landed lost its mail the same way (`queued-answer.py`; on an earlier revision the snapshot missed it). The error rule alone already keeps such news, since the CLI's result is an error; the note keeps the outcome right.

**Flows.** 36-stop's last block, on both routes: a yielded cell's first mail is answered by the `wait` that shows it; the second is taken while a gate holds back two refused waits (one with an invalid `terminate`, one naming the first `wait`'s id), which answer nothing; Escape stops a last `wait`, and the second mail reaches the model once, as an input of the next turn. 05 expects `Unknown` for Claude's stopped call.

## A cell's waits hold at most 64 news (4 November)

`taken` had no cap: a GPT cell that keeps calling `wait` without the model reading it took 160 mails with no outer Result. `TAKEN`, 64, now caps it (RULES 11). At the cap every `agent wait`, nested or top-level, is refused before it registers: "This turn's waits hold 64 news the model has not read yet; return from the cell, or wait on it by cell_id, so the model reads them, then wait again." A refusal is an error Result, which answers and moves nothing, and in a cell it rejects the call's promise, so a pumping loop throws and its exec returns, whose Result answers the 64; a null would spin the loop. News admitted at the cap stays in `pending`, as during any other long call. Taken news still does not count toward the 64 unread, so the person is admitted. Flow 34's last case drives a cell to the 65th wait, on both routes, with the refusal left uncaught.

## The person's words end a wait; the person's commands wait for the turn (1 November)

**The defect.** An agent called `agent wait` with a ten-minute timeout, and the person's messages sat queued until a child's mail ended the wait. `wake` gave a waiting call only news (mail or a report); the person's input went to `pending`, to join at the next tool boundary, which a blocked wait never reaches.

**The rule.** The person's input (a message, or an answer to the person's ask) ends every wait at once: `wake` drops the waiters' senders, so each wait answers `null`, as at its timeout, and the input joins at the boundary after that Result. A wait called after such an input was admitted, and before it joined, answers `null` at once. The `null` needs no reason: the model reads the person's words right after it. The `agent` tool's description says a wait answers null at its timeout or when the person writes.

**Commands.** `/done` and the person's `/handover` are Razor inputs carrying `command: "done" | "handover"` on the recorded Input, so any client draws them as the command rather than Razor's prompt text, after a relaunch too. Both routes answer `{"entry": id}`. `/done` waits for the running turn's end, at a boundary as any command does, and can be withdrawn with `DELETE /v1/sessions/{id}/input/{entry}`, which also forgets it as the last turn. The terminal draws a queued command as one short row.

**Handover takes the threshold's path (2 November).** The person's `/handover` was first held for the turn's end like `/done`, and one sat behind a long `agent wait` until the person's next message ended it. It is the context threshold reached on demand, so it now goes the same way: while the session works it is sent at once, as the threshold's is, and any handover request sent at once ends every waiting call with `null`, the request following that Result; a wait called while one is out answers `null` at once, since the threshold can arrive with the very response that calls the wait. An idle session admits it as a turn. `/handover` answers `{}` when a handover is already requested or rebuilding. A person's handover delivered in a batch becomes the conversation's threshold input.

**Flows.** 18-handover: during a wait, the person's message ends the wait with `null` and reaches the next request; during a second wait, the person's `/handover` ends it with `null`, is recorded with `command`, a second is refused `{}`, and its note rebuilds; on GPT, a response past the threshold that calls `wait` gets `null` at once and rebuilds. 36-stop's check that an input sent before Escape runs as the path's next turn now sends the handover while a code-mode cell runs, since it would end a wait, and the cell starts no job, so no end notice opens a turn and the session must stay working on the sent input alone. On Claude that turn's own cell used to be refused ("The turn has ended; no cell was started", also on 86084f4 with a threshold input): the bridge opens a held input's turn itself after a stop, and the Claude task reactivated only on a fresh input; it now also reactivates on that turn's first `exec` or `wait`. 20-done checks `command: "done"`. Flow 18 fails on the code before this change, and when wake's, the handover's or the wait's new line is removed; 20-done was red-proofed by disabling its line.

## Closed sessions stay closed (Trey, 1 November)

**The defect.** A closed top-level session reopened on its next input of any kind, and its Post watch survived the close and was rebound from `post.json` at every server start, so Post's rings revived sessions the person had closed for good.

**The rule.** A closed session that is no child reopens only for the person (a message or an answer to their ask), their commands (`/done`, `/handover`), and its resident's schedules and heartbeats. Admission refuses mail, a child's report and any program's input, Post's rings included, with "This session is closed; only the person can reopen it." Mail rarely gets that far, since `message` already refuses a session whose row reads done ("No agent is ..., or it has ended"). A stopped resident is not closed (its turn is stopped as by Escape), so mail to it still runs. When a session closes (`/done`, `/quit`, or a close from its parent or its own finish), `Post::closed` drops its watch, rewrites `post.json` without it and runs `post participant end` as its participant; if the person reopens it, its next `post` command binds it again.

**Flows.** 43-mail-server: mail to a root closed with `/quit` is refused and never recorded in it (red-proofed by disabling both `message`'s done filter and the admission check; either alone holds). 45-post: after `/quit` the watch is gone, `post.json` is empty, the participant reads `ended`, a ring through the local socket is refused, and no watch returns after a restart (red-proofed by disabling `Post::closed`, and the admission check, each in turn).

Job-end notices use the same waiter admission as mail and reports (2026-11-02). A background job ending while `agent wait` is open flushes its queued notice immediately; one waiting call consumes it, and its Result answers the input. The same notice held before a wait is consumed at once. A foreground result that shows its own job end still suppresses that notice, and the person's stop still holds stopped-job notices until the next input.

## A wait ends while the model path holds an unread input (3 November)

**The defect.** An agent's model, mid-reply, was sent the person's message, then called `agent headline` and `agent wait` in one batch. The wait ran until the person's next message, and the model read both together. The message had been admitted while the reply streamed, so it sat in `pending`; the boundary at the headline call's admission sent it to the model path, moving it to `sent`. There it waits for the batch's results: the Claude bridge holds a mid-turn input until `PostToolBatch` and the Responses loop puts it in the next request. `wait_input` looked only at `pending`, for the person's input admitted after the call, so it waited on an input already sent, which no boundary would bring back.

**The rule.** A wait answers `null` at once while `sent` holds anything: an input on the model path that no request with a begun reply has carried reaches the model only after the batch's results, whatever its origin. That covers the handover request the threshold guard was there for (sent at once, read only by the next request, on GPT with the very response that calls the wait), so the guard goes; after the model has read the request, a wait waits as any other. The person's input admitted after the call and still in `pending` ends the call as before.

**Flows.** 67-wait-unread, on both routes: the person writes during a reply streamed slowly before its `agent wait` call (20 s timeout); the wait answers `null` within 10 s, the request that carries that `null` (on Claude the bridge's yield after `PostToolBatch`) carries the input after it, and the turn's end answers it. On the code before this change both routes waited the full 20 s. 18-handover's threshold check passes on the `sent` rule alone.
