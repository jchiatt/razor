# Children, mail and the tree channel

Every child uses the destination server's `[children] startup`, default `["project"]`, independent of its parent and role. The controller chooses it whenever the record's summary has a child; resident threads retain their resident startup. Role, shared brief and task text remain in the first input, in their existing order. Built 2026-11-02.

## One kind of child, inherited defaults, nothing left behind at settle

Design for three changes to children (Trey, 2026-10-29), built on 2026-10-29; where the build differs from the first draft, this section says what was built. Read: `src/children/`, `src/protocol/children.rs`, `Controller::create`, `record::Summary` and `[children]` in `src/config/mod.rs`; flows 11 to 16, 19 and 40; git-worktree(1), probed on git 2.47.3.

```rust
// src/protocol/children.rs: `kind` goes, since nothing reads it after the start; `ChildKind` goes with it.
pub struct Child { pub parent: SessionId, pub name: String, pub worktree: Option<Worktree>, pub launched: EntryId }

// src/children/agent.rs, private: the start's `kind` input.
/// What a child is told after its contract; nothing else about a child depends on it.
#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind { #[default] Worker, Reviewer, Researcher }

// src/children/git.rs
/// Moves every path `git status` lists in the worktree at `path` (untracked and not ignored, a
/// wholly untracked folder as one path, or changed since its last commit), except deletions, to
/// the same path under `keep`, when the listing is whole. Once each has moved, resets the tracked
/// files to the commit, then asks git to remove the worktree, which it refuses while a change is
/// left, a submodule is initialized or the worktree is locked, and deletes `delete`, a branch git
/// must agree is merged. The result names what moved and what would not, then git's words.
pub(super) async fn remove(origin: &Path, path: &Path, keep: &Path, delete: Option<&str>) -> Result<String, Error>;

// src/config/mod.rs, `[children]`.
/// A file whose text replaces Razor's reviewer or researcher text, read at each start, at most
/// 64 KiB, absolute as a `startup` entry is; one that cannot be read refuses the start.
pub reviewer: Option<PathBuf>,
pub researcher: Option<PathBuf>,
```

Text, all data: `text/child.md` loses `{kind}` and its sentences on write authority, gains one clause (in a worktree of your own, commit what your parent should merge, since settling moves anything uncommitted out of the worktree), and holds `{role}` where the role text goes, before its closing line, filled last so a role file's own braces stay as written. `text/reviewer.md` and `text/researcher.md` are Trey's final versions of the reviewer and researcher role texts, copied word for word and compiled in as `child.md` is.

1. `agent start` reads the parent's summary once, as now. An unnamed `model` or `effort` is the summary's, the parent session's own (a heartbeat's or schedule's turn-only model is not inherited), and goes to `Controller::create` named, so the config's default never reaches a child and the controller does not change.
2. An unnamed `cwd` is the parent's folder, as now.
3. The first input is the contract, then the role text (a reviewer's or researcher's: the configured file, else Razor's; a worker's: none), then the shared brief, then the brief; the 64 KiB limit covers all four. The opening prefix (facts, memory page, skills, servers) goes ahead of this input in the conversation, so the seam is this input's text and nothing else.
4. Placement is `placement` alone, for every kind: `in_place` works in the folder; otherwise a child in git gets `sessions/<id>/worktree` on `child/<id>`, as now; outside git the child works in the folder itself. No scratch folder is made, so the start makes no folder at all.
5. `agent continue` is unchanged: a child keeps its own model and effort unless continue names them.
6. Settle is one path for `merge`, `merged` and `discard`: close, retire, merge for `merge`, then `git::remove(origin, worktree, sessions/<id>/uncommitted, branch for merge and merged)`. A merge git refuses records nothing, as now; past the merge, the decision is recorded whatever the removal says.
7. `remove` runs `git -C <worktree> status --porcelain -z --no-renames -unormal` from the origin and reads each name as bytes, never as lossy text. `--no-renames` lists a rename as a deletion and an addition, so each entry names one path; `-unormal` keeps a person's `status.showUntrackedFiles` from hiding untracked files; `-C` makes a missing worktree a refusal git words, not a failure to start git. Every entry moves except one git marks deleted in either column (`D` in the index or the worktree): a deleted path is not in the worktree, and a name under a folder replaced by a link would resolve through the link. When a deleted path's content is still there (`git rm --cached`), git lists it again as untracked, and that entry moves. `-unormal` collapses an untracked folder to `dir/` only when it holds no tracked files, so once deletions are left out no entry lies inside another.
8. Each held path moves by one `rename` into `sessions/<id>/uncommitted/<path>`, its parent folders made first; worktree and destination share the session folder, so it is one filesystem. A path whose folder or rename fails is named in the reply with the error, in one sentence, and stays in the worktree.
9. A listing is whole when git answers it and it is under the 64 KiB git output keeps; one that is not whole moves nothing. When the listing was whole, every entry moved and none is unmerged, `git reset -q --hard` in the worktree returns its tracked files to the commit, since a moved edit and a listed deletion would each block the removal; it leaves ignored files and submodules alone. The one thing settle loses is a staged version that differs from the worktree's copy (`MM`), which the reset discards, as a forced removal would. An unmerged entry (a `U` in either column) skips the reset, because a modify/delete conflict lists `DU` or `UD` for a file still in the worktree, which the `D` rule leaves unmoved and a reset would delete or overwrite with the child's edit in it (probed); the plain removal then refuses, and the worktree keeps it. Then the plain `git worktree remove`, never forced: git deletes the ignored files with the worktree, and refuses in its own words while anything listed is left, a submodule is initialized (its repository under `.git/worktrees/<id>/modules` and its branches stay) or the worktree is locked. Then `git branch -d` as now.
10. The reply: "Moved into <keep>, at their paths: `a`, `b/`." when anything moved, "Could not move `c`: <error>." for each path that did not, then git's text, or "The decision is recorded; git would not remove the worktree at <path>: " and git's words. The names come from one listing of at most 64 KiB, so the reply is bounded. A removal that fails outright (git cannot start) is the reply's text; the decision is recorded either way.

**Hard decision: what settle moves, and how it removes without force.** The request was to move every untracked file that is not ignored, then remove. git-worktree(1) refuses a worktree with "modified or untracked files", so a child that edited a tracked file and never committed still blocks the removal, and `--force` would delete that edit, which the rule that nothing is deleted unless it is ignored forbids. Probed: a worktree holding only ignored files is removed with them; `?? __pycache__/` is one entry; a staged rename is `R  new\0old\0`.
- **Chosen: move everything git lists except deletions, reset the tracked files once all of it moved, and never force.** It covers untracked, modified, staged, renamed and unmerged paths in one listing, reading only the `D` in its status letters. A listing cut at 64 KiB moves nothing and a failed move skips the reset, so in each case the plain removal runs and git refuses: one postcondition instead of a case per failure. Names are bytes for the move, because a lossy name would not exist in the worktree; the changed-files list stays text, since it goes into the record. A first version forced the removal when a second listing showed nothing left; that would delete an initialized submodule's repository and its local-only branches, and a deleted path's name could resolve through a folder replaced by a link, so settle never forces and deletions do not move. A moved edit then reads as a deletion that the plain removal refuses (probed), hence the reset.
- Untracked only: one listing and no force, but a child's uncommitted edit leaves its worktree behind, which is the complaint.
- Move the whole worktree folder and `git worktree prune`: one rename, but a full copy of the checkout per child stays on disk.
- Commit the leftovers to the child's branch: a merge would then land `__pycache__` in the parent's branch.

**Where moved files go: `sessions/<id>/uncommitted/<path>`.** Files go into the child's own session folder, keeping their relative path, in a folder of their own, because the session folder already holds `record.jsonl`, `worktree/` and `delivered/`, which a repository's own names could collide with; "uncommitted" is git's word for what they are. Against: one more folder name a person reads.

**`Child.kind` leaves the record.** Nothing in `src/`, the bridge or the flows reads it after the start, and the role's words are already in the child's first input. Against: a card that labels a reviewer would need it back (RULES.md 19 says build that then).

**Naming a replacement role text: two `[children]` keys, `reviewer` and `researcher`.** Each names a file, like a `startup` entry, made absolute against its config file's folder, and is read at each start, so it is live. A word in the `startup` list cannot carry it: that list is the system prompt, and a role goes in the first input.

**The agent tool's declarations** are profile data (`src/profiles/data/tools.json` and `claude.json`); the child changes edit only their `agent` entries, in four fields: `placement`: "start: in git a child gets its own worktree and branch from your last commit, so it sees none of your uncommitted edits; to review those, commit first or use in_place."; `kind`: "start: worker unless given; a reviewer or researcher also gets its role text, and nothing else differs."; `model` and `effort`: "start: yours unless given; continue: the child's own unless given."

**Flows**, red-proofed against the code before the change: 11 (merge) and 12 (discard) leave an untracked `__pycache__/` and an uncommitted edit to a tracked file, and assert the worktree is gone, the branch gone on merge and kept on discard, the files under `uncommitted/` at their paths, and the reply naming them; 12's child is a reviewer with a configured reviewer file, in its own worktree and branch, its first input holding the contract, that file's text and the brief in that order; 13's researcher, outside git, has its parent's folder as its `cwd` and Razor's researcher text between its contract and its brief, replacing the check that passed whatever happened (that flow's folder is not a repository, so no child there ever had a worktree); 16's parent runs with `--model sonnet --effort low`, and its child's `Start` names the same; 19's child writes `counted` in the parent's folder, not `scratch/`. 11 drops its "reviewer or researcher" assertion.

## A child ending without `finish`

Design for (Trey, 2026-10-28, 16:58): only a child's clean turn end can finish it; its prose never decides whether it is waiting. The implementation belongs in `src/children/`, with these additions to its public surface:

```rust
/// Constant-space turn facts; only closing-text ids, never the text, are retained.
#[derive(Clone, Copy, Default)]
pub struct Turn { /* private sent, grace, nudged, text */ }
impl Turn {
    /// Clears this turn's facts, retaining whether Razor's nudge has been admitted.
    pub fn start(&mut self);
    /// Marks the nudge after its input passes admission, even if a plugin changes its words.
    pub fn nudge(&mut self);
    /// Retains committed mail and closing-text ids, never prose or failed calls.
    pub fn recorded(&mut self, body: &Body, at: EntryId);
    /// Pending work or answered person input forbids completion; otherwise prepares
    /// the report after a nudge or without sent mail, or starts the mail grace.
    pub async fn end(&mut self, record: &Record, inputs: &[EntryId], end: &End, held: bool)
        -> Result<Option<(SessionId, Input, Option<Vec<PathBuf>>)>, Error>;
    /// Holds an open child's idle task through its grace and unresolved nudge.
    pub fn waiting(&self) -> bool;
    /// Offers Razor's nudge against the live setting; only admitted input marks it.
    pub fn tick(&self, settings: &mut Settings) -> Option<Input>;
}
/// Prepares the same parent, bounded report and changed files for both forms of finish;
/// no controller call can re-enter the session task that is preparing its own report.
pub async fn report(summary: &Summary, status: ReportStatus, text: String)
    -> Result<(SessionId, Input, Option<Vec<PathBuf>>), Error>;
```

1. `Session` holds `children::Turn`; committed `MailSent` facts set `sent`, so failed calls, channel reads and unmentioned posts do not count as mail.
2. Starting a turn clears `sent`, `grace` and the text id, not `nudged`; any admitted input wakes a child in grace through the existing admission path. `Session::continued`, after the parent's `Continued` fact and before taking its new task, replaces the whole `Turn`, including `nudged`, with its default.
3. The controller handles the model's clean `TurnEnd` only after draining job events; Escape, handover and the turn that answers the threshold input never enter this rule; a turn ending in an error reports `blocked` (next section). A rebuild requested while the turn ran does not stop a finish: the child finishes, and the rebuild waits for the turn after a continue.
4. It records the turn's usage once, releases foreground jobs into end notices, then applies the rule before starting another turn; answered person-input ids come from that turn's actual delivered set, not merely admitted inputs.
5. The work check is extracted from `Session::idle`, shared by unloading and this rule, and includes open questions; working state, warm conversations, attached clients and the grace itself remain unload-only holds, not pending work.
6. Pending inputs, notices, jobs, actions, questions or children mean waiting; the turn they wake is checked again, and a turn answering the person's own split input always stays open.
7. No pending work and no sent mail, or an admitted nudge, prepares `ReportStatus::Finished`: completion, not inferred success. Closing Text blocks after the last Call use `record::closing_text`, the same join as `--print`; over 64 KiB, a UTF-8-safe cut ends with their session and entry id or range. Explicit finish still refuses.
8. Explicit and implicit finish share report preparation and the controller's close/admit path: Results and usage are recorded before close, descendants close before the parent receives its report, and changed files are refreshed without querying the caller's task.
9. While grace is held, the existing once-a-second controller ticker refreshes the session's settings and calls `tick`; the nudge's exact wording lives in versioned profile data, never a controller template.
10. `40-no-finish.py` proves the four cases, UTF-8 report truncation, live grace changes and a continuation's fresh nudge; flows 14 and 36 keep pending jobs while exercising the working-child cap and terminal steering. Private turn fields require explicit start, admitted-nudge and idle-hold methods as well as end, tick and the fact setter.

**Hard decision: a live grace, checked by the existing ticker.** Add `[children] grace = "3m"` in `src/config`, reusing its duration parser. Each check reads the live value, including project overrides, against the original grace start; editing the setting does not restart the interval. Flows set this same key to seconds. A fixed duration makes every no-reply flow wait three minutes; a flow-only switch violates SPEC 7; a timer task duplicates the controller's ticker. Reusing it costs at most one second of delay and a settings refresh only for a child in grace. Late replies remain the agreed loss: the parent can continue the child.

## A child's turn ending in an error

Design for: a GPT reviewer's turn can end "The model response did not complete; send another input to try again.", and its parent's wait then heard nothing. Two paths end a turn with an error: the model path's failed turn end (`turn_ended`, including a rebuilt context too large for the model) and `abandon` (a conversation that closed by itself, or a failure inside the session's task). Both now record the end through one controller step, `ended`, which applies `Turn::end`, now given the end, to a child that was open when its turn ended. Its rule for an error:

- **Pending work holds the report back**, as for a clean end: a running job's end notice starts the next turn, which is checked again, and reporting now would close the child and kill that job. So `abandon`, once nothing has reported, takes the step every turn's end takes and starts the next turn, which a queued end notice opens; without it the notice waited for an input that never came. A child already closed is held as pending work is.
- **A turn that took a person's input does not report**: the person is reading the child and sees the error, and a report would close the child and hold their retry until the parent continues it.
- **No mail grace**: the turn did not choose to wait, so mail it sent does not delay the report. Escape (`Stopped`) never reports.
- **The text**: the error's sentence, first so the status view's one-line preview shows it; any closing text the turn wrote; then one line of profile data, `Continue {name} to try again.`, all within 64 KiB: the line is kept whole, the sentence is cut to what remains, and the closing text gets what is left after that, keeping its pointer to the whole text or left out when even the pointer does not fit, each cut on a character boundary. The whole sentence stays in the child's record. The name is in it because a report the parent is not waiting for reaches its model as bare text.

A child whose failed turn did not report (it took a person's input, or pending work held the report back and then went away, as a restart that killed its job did on 2026-11-03) waits with the error on its row; `agent continue` admits to it directly, as to any open child that is not running a turn (next section).

**Hard decision: a report with status `blocked`, not a note.** The report reuses everything: the parent's wait returns it as any report, the parent's record marks the child (status shows it done, its preview the error; `children.max` stops counting it; settle can discard a child that keeps failing), and `agent continue` already takes a reported child and clears its turn facts. Its cost: the child is closed until continued, so mail to it is refused and the retry reopens its conversation. A note keeps the child open, but `agent wait` returns only mail or a report, so the note would have to be mail the child never sent, with no `MailSent` in its record; nothing would mark the child stuck, and `agent continue` refuses a child that has not reported.

**Lines:** session controller 2,600 to 2,597: `ended` is paid for by moving the explicit finish's turn end into `settled`, its one caller, and by sending the report through the existing `close` function, which spawns the close and now the admission (the abandon path is reachable from admission, so that spawn must sit in a plain function, as `close` already did), and `abandon`'s next turn by `ended` passing a closed child as held; children 2,528 to 2,544; profiles 1,921 to 1,922; flows 6,539 to 6,593. **Flow:** 40 gains a GPT child given six incomplete responses: its parent's wait returns the `blocked` report with the error sentence, and a continue runs it to a `finished` report; red-proofed against the code before the change, where the wait times out with the child's error turn end recorded. Two further checks, each red-proofed against the first version : a GPT child whose context error carries 80,033 bytes reports the sentence cut to fit, where the first version refused the report; and a Claude child whose CLI is killed while a background job's end notice is queued runs that notice's turn and reports `finished`, where the first version left it waiting.

## A child's lifecycle across restarts

Repairs for five bugs a formal-methods pilot found (Trey approved the fix on 2026-11-03), each reproduced against the binary first. The cause they share: a child's lifecycle rested on facts kept only in memory, or on the parent's fact standing in for the child's own state. A report lived only in the task delivering it; a closed child's held inputs only in `pending`; whether a person's message continues a child only in the status row; a child's lost job's notice waited in memory for an input no one would send; and `agent continue` decided from the parent's fact, which counts every child that has not reported as working, while a paired parent recorded `Continued` before its request reached the child. Each fact now comes from the child's record or the child's own session.

1. **A report is part of the child's turn's end** (`TurnEnd.report`, for every parent, not only a paired one): the one entry that answers the turn's inputs carries it, so no crash leaves a reported task's inputs unanswered to reach the next task again. Delivery reads it back once the child's descendants and then the child have closed (`children::deliver`, `outgoing`), and the parent admits it through `remote::update`, which drops a report of another task. A parent stays live (`Summary::live`) while it is open, has not reported and has a child at work, so at the next start it is loaded and delivers each local child's recorded report it never admitted, as it already pulled a paired child's. Local task inputs now carry the parent's `Started` or `Continued` entry as `child_task`, as paired ones did, so a recorded report names its task.
2. **The child's own session decides `agent continue`** (`Session::take`): a running turn refuses with "Still working"; an open child takes the input as it takes a note, whatever its last turn's end; a closed one (finished, stopped, settled) reopens in the same queue operation: its parent records `Continued` (locally, or on a paired parent's server), the input carries that entry, and the child takes it, so two inputs arriving together reopen it once. Admission refuses a task input older than the child's current task, or naming the current one once it is reported, so a delayed continue never runs a replaced task again. A paired parent whose fact counts the child as working sends the fact's current task again instead of recording a new one, so a continue its server never got, or a failed turn that never reported, is reached by the next continue; the owner refuses a task the child has already reported, whose report reaches the parent by itself. The cap counts only a continue that adds a working child.
3. **The person's input to a closed child** reopens it through the same `take`, from the session's own hold, which a load derives from the child's record and its parent's fact; the status row no longer decides.
4. **A closed child holds its unanswered inputs again at a restart** (`Session::restart` puts them back in `pending`), so its next task's first request carries them, as it does without a restart.
5. **A child's lost job's notice opens a turn at start**: a root's still waits for the person, but a child has no one to send it an input, and the notice's turn ends in its report. A notice that opens a turn is recorded before its job's end, so no crash between the two leaves an idle child whose last job ended with nothing to wake it; the notice names its job as its `request`, so one recorded before a crash that lost the end is not made again.

**Hard decision: re-send the paired parent's current task rather than undo a `Continued` that was not sent.** The task id is the parent's `Continued` entry, so it must be written before the request; a failed request leaves it. Recording `Stopped` after a refused connection keeps the parent's fact true but does not help a request that may have run, nor a failed turn that never reported. Re-sending the current task covers all three with one rule; TLC showed it needs the owner's refusal of a task already reported, without which an old report in flight is taken for the re-sent work. Against: after a continue that was not sent, the parent's fact names a task the child has not got until the next continue, as the agent was told.

**Checked.** The pilot's TLA+ models (not published), with this design added as a switch: every children property holds (no turn ended or job killed by a planned restart, continue never refuses an idle child, no silent child, no orphaned input) up to three restarts, two faults and two of each input (3.8 million states); on the paired model, continue never refuses an idle child and the parent never takes a report the child is still running, and the parent counts the child as working on a task it was not given only after a continue that said it failed. After review, the model writes a job's end and its notice, and a report and its turn's end, as two steps a crash can fall between, with the session's own queue kept out of the gap. In the reviewed order TLC finds both reviewer cases: a child left silent by a crash between its job's end and the notice, and a finished task's brief taken into the next task after a crash between the report and the turn's end. In this order every property above holds, plus a new one: no running turn takes an input that a recorded report covered. The bounds are three restarts, two faults and two of each input (100 million states).

**Flows**, each red against the binary before the change: 40's job ends while the server is down and its notice's turn reports; its person case continues after a clean turn that took the person's words; a new case freezes the server the moment a finished child's turn end is written, kills and restarts it, and checks the report arrives, then that the person's first message continues the child with the words held from its finish (red separately with the report written after the turn's end, without the re-hold, and without the person's reopening); 49 continues while the child's server is down, then again once it is back. After review: 40 checks that a job's notice is recorded before its end, and that a failed turn that took the person's input carries no report (red on a build without that rule); 49 checks that the report is part of the turn's end and that a delayed continue of a task the child has moved past is refused (both red on the reviewed build).

## Public surface

```rust
/// One controller's child, mail and channel actions. It keeps no session state: every fact it
/// acts on is read from the controller when it acts.
pub struct Children { /* home, and the lock held from a start's name check to its record */ }
impl Children {
    /// Opens no files and scans no records; one serves the whole controller.
    pub fn open(home: PathBuf) -> Self;
    /// Beside the output, a finished child's report waits for its Result, usage and close.
    pub async fn run(&self, controller: &Controller, caller: &SessionId, at: EntryId, action: &str, input: Value) -> Result<(Value, Option<(SessionId, Input)>), Error>;
    /// Discards finished helpers with no branch commits after the parent task releases its record.
    pub fn closed(&self, controller: &Controller, parent: &record::Summary);
}
/// A sentence the model can act on, or a machine failure with what was being done.
pub enum Error { Refused(String), Other { source: Box<dyn std::error::Error + Send + Sync>, context: &'static str } }
```

The shared types (`Child`, `Worktree`, `ChildEvent`, `Decision`, `Report`, `ReportStatus`, `MailKind`, `MailId`, `ChannelEntry`, `Ask`) are in `src/protocol/children.rs`; the parent's view of its children is `record::ChildSummary`.

| Call | Input | Result |
| --- | --- | --- |
| `agent start` | `name, brief`, `shared_brief?` (a file), `kind?` (worker, the default; reviewer, researcher: a role text), `cwd?`, `model?`, `effort?` (each the parent's unless given), `placement?` (worktree, in_place) | `{session, cwd, branch}` |
| `agent continue` | `child, brief, model?, effort?` | `{}` |
| `agent finish` | `status` (finished, partial, blocked), `report` | `{}`, then the child closes |
| `agent settle` | `child, decision` (merge, discard, merged) | `{text}`: git's words |
| `agent stop` | `child` | `{}` |
| `agent wait` | `timeout_ms?` (600,000) | the next mail or report, as its whole entry; `null` at the timeout |
| `agent headline`, `agent progress` | `text`, one line | `{}` |
| `message note, question, answer` | `to?` (`parent`, the default; a child's name or id; `channel`, notes only), `text`, `question` for an answer, `options?` for a root's question | `{id}` |
| `message read` | `limit?` (20) | `{posts, file}`: the latest posts and the channel's file |
| `status` | none | `{agents}`, every agent in the tree but settled helpers and their subtrees: session, parent, name, headline, state, changed_files, branch |

## Data flow

1. A start is refused when the parent has `children.max` (live, 20) children neither finished, stopped nor settled; continue counts the same way. A name is unique in the tree, whose root is `main`. For this check the tree is the root, each helper not settled, and every child row those sessions recorded, settled ones included; a settled helper's own subtree left the tree at settle, so a name used only there is free again. A start reads only those unsettled records.
2. The first input is the contract (`text/child.md`), a reviewer's or researcher's role text, the shared brief and the brief, admitted as the parent's mail; a continuation is a later one, through `Controller::continue_child`. At most 64 KiB, checked before anything is made.
3. A child in git gets `sessions/<id>/worktree` on branch `child/<id>` at the commit its folder's checkout has now, with the parent's place in the checkout kept below it; dirty edits stay behind. `in_place`, and any child outside git, works in the folder given, the parent's by default. The record comes first; if the worktree then fails, the child is recorded stopped.
4. Mail is admitted to the recipient, then `MailSent` is appended to the sender. A child mails its parent or its own children; anyone else is mentioned in the channel. Mail to a child that has reported, stopped or been settled is refused (it would wait in that record forever, holding the child live), and a mention skips such a child; `agent continue` gives it more work. A root's parent is the person: a note goes to `residents::notify`, a question becomes an ask through `residents::ask`.
5. A channel post is one `O_APPEND` write of its JSON line to `channels/<root>.jsonl`, then mail to each agent named as `@name` or `@id`, once each. A read returns the latest posts in the file's last 64 KiB and the file's path, which holds every post.
6. Finish checks the report's 64 KiB limit, refreshes changed files and returns the parent's id and report input. The controller records Result, waits for Settled usage, records the report in the child's record before the turn's end, closes the child and its descendants, then sends that report; the parent cannot settle from an early report and discard the usage. Wait takes the next mail or report, once, as its result. A report carries a defaulted `child_no_changes` fact: true without a worktree, otherwise only when git resolves the child branch to its recorded base. It is computed once while preparing the report, forwarded with remote reports, and retained as `Report.no_changes` in the parent summary. A successful finished report with this fact leaves the agents list but remains continuable. Parent closeout schedules ordinary discard settlement for those finished helpers on a separate task, after releasing the parent task; partial, blocked and working helpers stay untouched by automatic settlement. A stopped child’s displayed report is cleared while its durable report remains; done helper rows with no report leave the agents list. Remote settlement remains a parent-side fact, so the owner’s stopped-looking row leaves through the same rule.
7. Stop closes the child and its descendants, refreshes its changed files and records it stopped. Settle takes a finished or stopped child once: closes it, merges for `merge`, moves the worktree's uncommitted files to `sessions/<id>/uncommitted/` and removes the worktree, deletes a merged branch with `git branch -d` (`merge`, `merged`), keeps the branch on `discard`, and records the decision.
8. Changed files are `git diff --name-only <base>...child/<id>`, refreshed at finish, stop and status (for children still at work), appended only when they differ. When git cannot say, the last list stands.

## Decisions

**Recipient records for mail; one channel file per tree.** Call, recipient's input, sender's `MailSent`, result. A crash before the result leaves the call unknown and never retried. No queue and no cross-record atomicity; rebuild carries admitted mail.

**Git is a program, run to its end.** At most 64 KiB of each stream is kept and the rest drained, so git never waits on a full pipe and nothing is spooled to disk. A merge goes into whatever the origin checkout has checked out; git protects dirty files. A merge that exits 1 (a conflict this call began) is aborted; a merge someone else began makes git exit 128 and is never touched. A refusal is git's own words.
