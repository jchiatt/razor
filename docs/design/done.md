# `/done` and `razor doctor`

Design page, 2026-10-28, updated as built. Two behaviours the plan names that had no code: `/done` (SPEC.md section 3, Continuity; flow 20) and `razor doctor` (flow 1).

## Surface

```rust
// ---- src/controller: one new operation --------------------------------------------------

impl Controller {
    /// `/done`: admits the closing input (Razor's words, `done.md`, from `Origin::Razor`, so it is
    /// no ask of the person's and waits for a running turn to end) and clears the draft, then
    /// answers at once. The turn that takes it is the session's last: no budget, deadline or turn
    /// cap applies to it, and when it ends, however it ends, every input still waiting is recorded
    /// as `Withdrawn` and the session closes as a finished child does (jobs and descendants stopped).
    /// The session's next input reopens it. Withdrawing the input before it is taken cancels it.
    pub async fn done(&self, session: &SessionId) -> Result<EntryId, Error>;
}

// ---- src/api: one new route -------------------------------------------------------------
// POST /v1/sessions/{id}/done   (no body)  ->  {entry}, once admitted; errors as for `input`.

// ---- src/terminal: `/done` ---------------------------------------------------------------
// Sends the route for the addressed session through the draft-ordered write queue. Once it is
// admitted the draft clears and the transcript shows the closing turn as it runs; when the
// session's status row reads it closed (after the closing turn ends and its jobs are stopped),
// the app leaves, or a split goes back to the root (Trey, 2026-11-01). `/quit`, `/quit!` and `/end`
// do the same with no closing turn. A refusal stays on screen.

// ---- src/main.rs: `razor doctor` ----------------------------------------------------------
/// One line per thing Razor runs on, each found or missing with what to do, and never a
/// credential's value: the Claude sign-in as the SDK's own bundled Claude Code CLI reports it,
/// the ChatGPT sign-in (`$RAZOR_HOME/auth.json`), the OpenAI key (`$RAZOR_HOME/key`), each web
/// key by its variable's name, Memorum's socket (`Memory::check`), the Claude bridge's directory
/// and Node. Starts no server and writes nothing of Razor's. Exit 0 when Claude (sign-in,
/// bridge, Node) or GPT (either credential) can run; 1 otherwise, with the sentence saying what
/// to do first.
async fn doctor(home: PathBuf) -> Result<ExitCode, Error>;

// ---- what doctor reads from its owners ----------------------------------------------------
/// src/claude: the bridge's directory, beside the binary or else the source tree's, with its
/// `node_modules`; `Missing` says what to do. `Bridge::start` uses the same function.
pub async fn dir() -> Result<PathBuf, Error>;
/// src/claude: the sign-in the bundled CLI in `dir` finds in this environment, from its own
/// `claude auth status`: the method (`claude.ai`, `api_key, ANTHROPIC_API_KEY`), never a value.
pub async fn sign_in(dir: &Path) -> Result<String, Error>;
/// src/tools/web: each provider's key variable and whether it is set; never a value.
pub fn keys() -> impl Iterator<Item = (&'static str, bool)>;
```

## Data flow

1. The person types `/done`; the terminal posts `/v1/sessions/{id}/done` behind any queued draft write and, once the server answers `{entry}`, clears the draft: the transcript shows the closing turn as it runs, and once the session's status row reads it closed the app leaves, or a split goes back to the root (Trey, 2026-11-01: "the ending turn happens but the TUI never closes and the session never ends"). On 2026-10-30 he had asked it to stay open, when the terminal exited the moment `/done` was admitted and left his pane blank; it now leaves only once the closing turn has ended.
2. The API calls `Controller::done`. On the session's task, one queued step admits the input through the usual admission path (64-input cap, plugins, the record) and marks its entry as the last turn's, so no turn can end between the two.
3. Idle, the session starts the turn at once; working, the input waits for the turn to end, as mail does, and then opens its own.
4. The model reads `done.md`: bring durable state up to date, tell live children to finish, write the project's state note with `memory` (`write`, kind `state`), end. Memory already makes each state write supersede the project's last (residents.md), so `/done` has no memory code.
5. While it runs, inputs follow the usual rules: a person's words join at a tool boundary; mail and reports reach a waiting `agent wait`; the rest wait.
6. A threshold, `handover` or restart rebuild carries the closing input as one of the rebuilt context's input lines, and the turn goes on. Loading the session restores its closing intent from an unanswered `Input.command == Done`; an answered or withdrawn closing input restores nothing.
7. When a turn's end is about to answer the closing input, the session first records every other unanswered input in one `Withdrawn { inputs }`, removing them from rebuilt context without ending a turn, then records the closing turn's own end; it cancels a planned rebuild, holds its turns, and queues `Controller::close` for itself: jobs and descendants stop, its row reads done.
8. Nothing it holds is unanswered, so its live marker goes, it can unload, and a restart never revives it. Its next input clears the hold and opens a turn, which resumes the native conversation, since the record's last end is the closing turn's: `--resume` and `--continue` reopen a closed session.
9. `razor doctor` asks the bridge's bundled Claude Code CLI for `auth status` (the CLI's own lookup: its environment variables, its config directory, the macOS keychain; about 0.2 s, and it keeps its own `~/.claude.json` as at any start), checks the GPT credential its configured `[responses] base_url` uses (the ChatGPT sign-in, or the key file for the public API), the web and voice keys' variables, the settings, the browser, the server and the service, runs `node --version`, prints its lines and exits; it never starts the server. Asking the CLI, rather than reading its files, keeps one definition of how Claude signs in, and it is the CLI every Claude session runs.

## The hard decision: what holding the session's single worker means, and what happens to inputs that arrive during the closeout

In Razor a session's worker is its task and its one conversation. The closeout is a turn on them like any other, so "holding" needs no mechanism: a working turn keeps the task loaded whether or not a client is attached, no second turn starts while it runs, and nothing in Razor limits a turn's time or tokens (an earlier harness's 600-second closing-turn limit is what "no budget or cap" removes). What needs deciding is what the session does with inputs, during the turn and after it.

**A. Refuse every input once `/done` is admitted.** One refusal sentence, nothing held. It cuts the closeout off from its own children: a child's `finish` report would be refused (the child's report never reaches its parent's row), and `agent wait` inside the closeout could never return. A person's correction from the phone ("also note X") would bounce. It adds a refusal the spec does not name.

**B. Hold every input after `/done`, the closed child's rule.** Nothing is lost while the session runs. But a held input stays unanswered, so the session keeps its live marker: it never unloads, and at the next restart the controller rebuilds every live session with unanswered inputs and runs them, which is a turn after the last turn. A root has no `continue_child`, so nothing releases the hold except that crash.

**C. The usual rules during the closeout; at its end, what is still waiting is `Withdrawn`, and the next input reopens. Chosen.** The closeout can wait on its children and take a person's correction at a tool boundary, as any turn does. Its end writes `Withdrawn { inputs }` for the inputs never delivered, as taking back a waiting input does; they leave unanswered inputs and rebuilt context without ending a turn, and no restart runs them. Costs: a person's words that arrive after the closeout's last tool boundary, and a report that arrives with no `agent wait` running, are recorded and shown but never reach the model.

The closing intent survives a crash or restart after admission (Trey, 2026-11-02, about 19:20: "yes", to making a crash during `/done` still close the session). This reverses the earlier choice to lose that intent at a crash. No new stored field is needed: loading reads the record's existing unanswered inputs by index and restores the latest `Done` command. `Withdrawn` and `TurnEnd.inputs` already remove cancelled and answered commands, so a completed close is never repeated and continuing after it restores no old intent. Stopping a closing turn still closes it when its end answers `/done`, however that turn ends; withdrawing an untaken `/done` cancels the close. The rule applies to roots and children alike; a closed child still waits for continuation through its parent.

The closing input's origin is part of the same decision. As `Origin::Person` it would join a working turn at its next tool boundary (so the closeout would not be a turn of its own), count as one of the person's asks, retitle nothing but be citable by a ruling as the person's words. `Origin::Razor` is what it is: harness text the person asked for, as the earlier harness's closing input was ("harness context, never the human's own words").

After the close, the next input reopens rather than being held, because a root closed by `/done` is still the newest session in its folder: `razor --continue` lands on it, and holding the person's first words there would lose them silently. The hold could not last anyway: a reloaded session is held only when its parent's record says it is a closed child, so a closed root is released the moment it unloads, and the rule makes a loaded root behave as a reloaded one does. Children keep their rule: only `continue_child` releases them.

## Flows

- **Flow 20** (`flows/20-done.py`, Claude route, a real Memorum): a first turn starts a background job; `/done` through the API answers while the closing turn's first call is still waiting on a file, so the answer came before the turn ended; a program's input sent then waits and is recorded as `withdrawn`, and its text reaches no request; the closing turn writes the state note through `memory`; the turn ends done, the background job ends by signal, the row reads done and the live marker is gone; `--resume` reopens the session for one more turn, with no rebuild; then the flow writes a focus note for the project straight to its Memorum (`focus_write`, pinned, as the earlier harness's closeout did after its state note; residents.md, "A state note over a pinned note"); a second session in the same folder has the first note on its startup page and not the focus note's text, and its `/done` supersedes the first note (the write succeeds and its `old_id` is the first note's id). Before the change the page shows the focus note and the write is refused.
- **Flow 1** (`flows/01-doctor.py`): a copy of the built binary in an empty directory, run with an empty `HOME` and `RAZOR_HOME` and no Node on `PATH`, reports each sign-in, key and Node missing, each with what to do, writes nothing in `RAZOR_HOME` and exits 1 (the bridge is found: a copy falls back to the source tree it was built from); with a stand-in ChatGPT sign-in, a planted Claude credentials file and web key, and Node on `PATH`, it reports them found and exits 0, and no planted value appears in its output.
- Flows 1, 20 and 24 each run a Memorum from their own directory, so the driver gains one `memorum()` context manager yielding its socket, and flow 24 uses it too.

## Glossary line proposed

- **closeout**: a session's last turn, asked for with `/done`: it writes the project's state note, and the session closes when it ends. (SPEC.md uses the word; code says `done`.)
