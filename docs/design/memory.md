# Memory: Razor's own memory files

Razor keeps each memory as a Markdown file in the server's home. Until 2026-11-02 Memorum could be configured as a second store, so a person who used it kept sharing memory with Claude Code and Codex; Trey dropped it that day, and a one-time import outside the product copies its memories into the files. The sections below are newest first: the 2026-11-02 design and what was built, then the files store as built on 2026-11-01, whose Memorum parts are history.

## Descriptions, standing rulings first, a nightly tidy, and leaving Memorum

Built 2026-11-02; "As built" at the end of this section records what changed. Trey agreed to each change on 2026-11-02 ("i like all of this"); this section designs them. Tasks, which replace a proposed "commitment" kind of memory, share this module, its files and its startup page.

**The problem, with its evidence.**
- *Facts are written and never read.* The startup page shows the state note and rulings only, so a fact reaches a session only if it thinks to search for it.
- *One-time answers crowd out standing rulings.* One session's startup page, on 2026-11-02, held eight rulings, nearly all one-time yeses to line budgets ("yes", "yes 111 is ok", "oh and to ur questions, yes, you can increase that budget"); another held a one-time permission recorded for every project, which then read as standing in every session.
- *Nothing consolidates.* When Memorum's dreaming was removed, Trey required its functions back (2026-10-25: "I can be onboard w/ removing it so long as we recover the feature and functions, which are analogous to those in humans for human dreaming"): consolidating episodes into lessons, forgetting and expiry, merging duplicates, and correcting stale claims. Razor's files have none of these.
- *A resident's state note goes stale or is overwritten.* A resident's page on 2026-11-02 showed a state note another session wrote on October 29 about something else; any session in its folder can replace it, and residents never run `/done`.
- *Four places to put a thing, and no rule for which.*
- *Trey is dropping Memorum*, and switching stores copies nothing, so every ruling would vanish from every page the moment the setting changes.

**1. A description on every fact and note.** `write` gains `description`: one line, at most 200 bytes, required for `fact` and `note` (refused without it: "A fact or note needs `description`: one line saying what it is, for the startup page.") and ignored on a ruling or a state note. It is a header line, `description: …`. A file without one, such as a person's, shows its text's first line instead. Search hits show the description as their summary when there is one.

**2. The startup page lists what is known.** The page becomes, in order:

```text
## Memory: this project's state
<the state note, at most 1,500 bytes>

## Memory: the person's rulings; `memory get` with a line's id gives its context
- <standing rulings, newest first> (ruling-…)
- (until 2026-11-09) <one-time rulings, those with an expiry, newest first> (ruling-…)

## Memory: what is known here; `memory get` reads one whole
- <a fact's or note's description> (fact-20261101-203800-0a1b2c3d)
- and 31 more; `memory search` finds them

## Tasks: …
```

- Rulings keep their 20 lines in 4,000 bytes, but standing ones (no `expires`) come first, so a burst of one-time answers cannot push an old decision off the page.
- The list of what is known holds the project's and every project's facts and notes, newest first, at most 24 lines in 2,400 bytes, then a count of the rest. It is a table of contents: the agent sees what it knows and opens a file when it matters. Search stays word search; nothing ranks by meaning.
- The page grows from about 1,500 tokens to at most about 2,500 with tasks. A child's page is the same, so the GPT child prefix (SPEC section 5: under 12,000 tokens) is measured again in the build.
- The `memory` declaration's `expires` line becomes: "write: a date after which it no longer applies; give a ruling that answers one question for now (a yes to one raise, one push) a date a week out, or do not record it; a standing ruling has none."

**3. Expired memories are put away.** `best`, the one reader under search, the page and the person's list, already leaves out a memory past its `expires` date; it now also moves that file into the retired tree by one rename as it passes it, so expiry is forgetting and folders do not fill with dead files. The page reads rulings and the state note only, so one-time rulings leave it the day after their date with no other step.

**4. The night tidies, before its rebuild.** Today the night rebuilds, then sends the home session the day's threads and fresh runs when there were any. Now the residents task first sends one input, always, then asks for the rebuild, which the controller holds until that input's turn ends (both pass through the session's one queue in order; the build checks that the admitted input has set the session working before the rebuild request is handled). The input is `TEXT.day`, rewritten, then the threads and runs as today, then every memory's description line (the same list as the page's, up to 200 lines in 16 KiB):

> Your night began. Before your context is rebuilt: record what today taught you as facts, each with a description; where two memories say the same thing, replace them with one; replace any fact today proved wrong; close the tasks you finished, with their evidence, and make sure every promise you made today is a task with a due date; then rewrite this project's state note. Never retire or replace one of the person's rulings. These of your threads and fresh runs were active since your last night, each with the first line of its last reply; keep what matters from them. These are your memories, newest first:

Tidying before the rebuild means the day is still in the context when it is consolidated, and the rebuilt context opens with the new state note and the tidied list. A home session busy at night gets the input after its current turn, past the rebuild, as now.

`/done`'s closing text (`src/controller/session/done.md`) gains the same work for a project session, before the state note: record what this session learned as facts with descriptions, replace facts it proved wrong or that say the same thing, close the tasks it finished, and never retire a ruling.

**How Memorum's dreaming is recovered** (Trey's four functions, 2026-10-25):

| Function | How |
| --- | --- |
| Consolidating episodes into lessons | The night's input, while the day is still in the resident's context, and `/done` for a project, ask for the lessons as facts with descriptions; the record keeps every episode whole, one recall away |
| Forgetting and expiry | `expires` on one-time rulings and facts; reads leave expired memories out and move them into the retired tree (3) |
| Merging duplicates | The night's input lists every description; the resident replaces duplicates with one memory (`write` with `replaces`) |
| Correcting stale claims | The night and `/done` ask for each fact the day proved wrong to be replaced; replacing retires the old file, which stays readable in the retired tree |

None of it is new machinery: the model does the judging with the tools it has, and Razor's part is the input, its timing and the list it carries. The one thing the tidy may not do is retire a ruling; that rule is in its words, consistent with SPEC's two guards, and a mistaken retirement is one move back out of `retired/`.

**5. In a resident's folder, only its home session writes the state note.** `Role::of` takes the residents' configuration: a session that is neither a child nor a thread but works inside a resident's folder (its `cwd` starts with the resident's) and is not the home session itself has the thread's role for this rule, and its state write gets the thread's refusal, which tells it to send the home session a note instead. Threads were already refused. The night (4) asks the resident to rewrite its state note, since residents never run `/done`.

**6. One home for each kind of thing.** The system prompt (`src/profiles/data/razor.md`) gains one paragraph, and the `memory` and `task` descriptions agree with it:

> Keep each kind of thing in its one place: the person's decisions are rulings, quoted from their words; anything owed, your own promises included, is a task with a due date; what is true about the project or the world is a fact, with a description; stories and narrative go in a journal file of your own, if you keep one.

A resident's night journal is a schedule in its own configuration (a job at its night that writes the day's entry into a file in its folder), not a Razor feature.

**7. Leaving Memorum: a one-time import, then (if Trey agrees, question 1) deleting the store.** A script outside the product, run on each home Trey uses while `memoryd` still runs:

- *Which projects*: the distinct project folders of every session in that home (each record's `Start` line, through the same rule as `roots`) and each resident's folder. For each, `memoryd project resolve` in that folder gives its Memorum project; a Memorum project no Razor folder maps to is skipped (most belong to other tools).
- *What*: read with `memoryd select` (read-only, recording no recall). Standing rulings keep their bodies unchanged, since Loom's form (`the human said (…): "…"` then `agent's reading:`) is what the page already reads, with `source` from `source_ref`; a ruling whose `valid_to` has passed is dropped, and a future one becomes `expires`. The project's newest active state note becomes `state.md`. Facts and notes keep their text, with Memorum's summary as `description`. The person's all-projects preferences go into `all/`.
- *Files*: `<kind>-<created>-<eight hex of the Memorum id>.md`, each file's modification time set to Memorum's `updated_at` so newest-first order holds; a second run skips ids already written.
- Then the person removes `memorum` from their settings.

**8. Memory kept in git** is already supported: `<home>/memory` (or one project's folder) can be a link into a repository, and a nightly commit, pull and push is the person's own setup (a cron job or a resident's schedule), not product code. Every memory file has a unique name and is written once, so two machines never change one file, with one exception: a project's `state.md`, replaced whole by each machine's closeout. Two machines closing the same project between syncs conflict there; the setup should take the newer commit's `state.md`, since a state note replaces the last by definition.

### Surface changes

```rust
// src/memory/mod.rs
impl Memory {
    /// The page, as above. `session` is new (tasks); the store is the files.
    pub async fn page(&self, cwd: &Path, session: &SessionId) -> String;
    /// Every fact's and note's description line, newest first, at most 200 lines in 16 KiB:
    /// the page's list, longer, for the night's input.
    pub async fn index(&self, cwd: &Path) -> String;
}
impl Role {
    /// As before, and a root inside a resident's folder that is not its home session reads as a
    /// thread for the state note.
    pub fn of(s: &Summary, residents: &BTreeMap<SessionId, Resident>) -> Role;
}
// private: `Write` gains `description: Option<String>`; `files::write` writes its header line;
// `best` ranks with the header in view (standing first) and retires an expired file it passes.
```

### Lines

| Row | Change | Estimate |
| --- | --- | ---: |
| Flows | flow 24 (memory) checks the required description, the page's list, standing rulings first and an expired memory moved into `retired/`; flow 21 (residents), which no night has covered until now, sets a resident's `night` a minute ahead and checks that its input comes before the rebuild and carries the list, and that a state write by another top-level session in the resident's folder is refused | +40 |

### SPEC changes, to land with the build

- Section 3, **Memory**: the startup page is the state note, the rulings with standing ones first, a one-line description of each fact and note (newest first, capped), and the session's tasks, about 2,500 tokens; a fact or note is written with its description. If question 1 is yes, the sentences keeping Memorum as a configured store go, and section 5's dependency sentence on Memorum with them.
- The closing paragraph's "the startup memory page holds the state note and the rulings and nothing else" becomes the page above (Trey's word of 2026-11-02, "i like all of this").
- Section 3, **Residents**: "At its night, after the rebuild, the home session gets one input listing the threads and fresh runs…" becomes: at its night, the home session first gets one input asking it to tidy its memory and tasks and rewrite its state note, with the threads and fresh runs and its memory's description lines; its context is rebuilt when that turn ends.
- Section 3, **Continuity**: the closeout also records what the session learned as facts and closes the tasks it finished, before the state note.

### Open questions for Trey

1. You are dropping Memorum: once the import has run on every home, delete Razor's Memorum store (about 330 lines), so Razor's own files are its only memory? That reverses your 2026-11-01 ruling that kept it as a configured store for sharing with Claude Code and Codex.
2. One-time rulings already recorded (47 of Memorum's 369 quoted rulings were under 40 characters when counted on 2026-10-29, mostly yeses): should the import give those a one-week expiry, or copy every ruling as it is and leave retiring them to you?

### As built (2026-11-02)

- **Descriptions, the page and expiry** as designed. The page reads `state.md`, the rulings (standing first, then those with an expiry shown as `(until <date>)`, each newest first, 20 lines in 4,000 bytes) and the facts' and notes' lines (24 in 2,400 bytes, then "and N more"), then the tasks part when the project has an open task. A file without `description:` shows its text's first line, at most 200 bytes. `best` moves an expired memory into the retired tree as it passes it, logging a failed move and leaving the file out either way.
- **The night** sends its input always, then asks for the rebuild; a rebuild requested while the home session works waits for its turn to end (`Session::plan`), and admitting the input starts the turn before the request is handled, so the tidy runs first. The input is the rewritten `day` text, the threads and runs, then `Memory::index` (200 lines in 16 KiB).
- **The resident-folder rule lives in memory, not `Role::of`.** The project lookup finds the deepest resident whose folder holds the session's folder or its main checkout and keys memory and tasks to it; a state write there by any session but the resident's home session gets the thread's refusal, reworded for both. `Role` went: the caller says only whether it is a child. The lookup reads the residents from the controller's last good machine settings (review, 2026-11-02), so a person's settings file with a mistake in it no longer sends a resident's memories to its project's folder; every memory call therefore takes the controller.
- **The prompt**: the system prompt gained one line ("Keep each thing in its one place: …"), shorter than the design's paragraph, and `/done`'s text the closeout steps; the `memory` declaration gained `description` and the shorter `expires` wording ("a ruling that answers one question for now (one raise, one push) gets one a week out"), all to stay under flow 28's 12,000-token prefix.
- **Memorum deleted** (question 1): `memory/memorum.rs`, the socket setting and its machine-key entry, the doctor's probe, the residents list's socket, flow 61 and its eight recorded replies, and the public manifest's two lines for them. A `memorum` key left in a settings file is ignored (settings ignore unknown keys), so nobody's settings break.

## The files store, as built (2026-11-01)

```rust
// ---- src/memory/mod.rs: the tool, the startup page and the rules both stores share ---------

/// Memory for one server. Razor keeps its own memory files under `<home>/memory` unless the
/// machine settings name Memorum's socket in `memorum`. Each call reads the person's file and
/// the pack again on a blocking thread (a stat each when unchanged; a bad edit keeps the last
/// good value), so a change to the key applies at the next call. There is no probe and no
/// fallback between stores. Between calls it holds only Memorum's project ids, at most 64.
pub struct Memory { /* home; its machine settings, opened at the first call; Memorum's client */ }

impl Memory {
    /// Memory for the server whose home is `home`. It reads and writes nothing until a call.
    pub fn new(home: &Path) -> Memory;

    /// Unchanged on both stores: the project's state note (at most 1,500 bytes), then its
    /// rulings, newest first, at most 20 lines in 4,000 bytes, each the person's words and then
    /// its id. It never fails: a store that cannot answer makes the page one sentence saying why.
    pub async fn page(&self, cwd: &Path, child: bool) -> String;

    /// The `memory` action, with the same input on both stores. The shared rules run first, and
    /// all of them before anything is retired: a child does not write; a thread writes no state
    /// note; a state note at most 1,500 bytes, never `all`, and with no `replaces`, since it
    /// replaces the project's last by definition, with or without text; `expires` a date
    /// (`2026-12-31`); a ruling cites an input or answer of the person's, and its text is
    /// composed from the record; the composed text, a ruling's quoted words with it, at most
    /// 16 KiB. The store receives one composed memory. Answers are each store's own (table below).
    pub async fn run(&self, caller: Caller<'_>, input: Value) -> Result<Value, Error>;

    /// For the person's phone: `{"hits": [{id, kind, summary, updated_at}], "omitted_count"}`,
    /// newest first, at most 64 on the files store (a search with no words): the shape the
    /// phone already reads from Memorum's `select`.
    pub async fn list(&self, cwd: &Path) -> Result<Value, Error>;

    /// At the person's word, deletes memory `id` of `cwd`'s project or of every project, words
    /// and all, not into the retired tree (`cwd` is new: the files store looks only there).
    pub async fn forget(&self, cwd: &Path, id: &str) -> Result<Value, Error>;

    /// For `razor doctor`: the memory files' folder, or Memorum's socket and protocol, or why
    /// Memorum did not answer. It creates nothing.
    pub async fn check(&self) -> Result<String, Error>;
}
// Unchanged: Role, Caller, Cited, cited(), Error.

// ---- src/memory/files.rs (private): Razor's own store, synchronous, run on a blocking thread
// ---- src/memory/memorum.rs (private): today's client, moved; its socket comes from the key

// ---- src/config: one machine key, and the project root made public for memory --------------
pub struct Config {
    /// Memorum's socket (`~/` is the home folder); absent, Razor keeps its own memory files.
    /// Only the person's file and the pack may set it.
    pub memorum: Option<PathBuf>,
}
/// The local root, and the checkout whose `.razor` files apply: a linked worktree's main one.
pub fn roots(cwd: &Path) -> Result<(PathBuf, PathBuf), Error>; // was private to config
```

**The files store.** Memories for every project are in `<home>/memory/all/`. A project's are in `<home>/memory/projects/` followed by its folder's own path, component by component (`projects/home/you/code/app/`), where the folder is `roots(cwd).1`, so no two projects share one; a read takes only the files directly in a folder, never a nested project's. A retired or replaced memory moves to the same place under `<home>/memory/retired/` (`retired/projects/home/you/code/app/`), where search, `get` and the page never look; a separate tree rather than a `retired/` folder beside each project's, because a project whose own folder is named `retired` would otherwise read its parent's retired memories as its own. A memory is `<kind>-<time>-<eight hex digits>.md` (`ruling-20261101-203800-0a1b2c3d.md`), and its id is the name without `.md`: the name says the kind, so the startup page opens only `state.md` and the ruling files. The project's one state note is `state.md`, id `state`. A file of any other name, such as one a person adds by hand, is a note. A file is a header between `---` lines (each line ending in LF or CRLF, for a file edited by hand), then the text (a ruling's is the composed form the page already reads); a file with no header is all text:

```
---
source: razor:20261101-120000-abcd:l-12
expires: 2026-12-31
---
the agent asked: "Should every merge wait for the flows?"

the human said (razor:20261101-120000-abcd:l-12): "yes"

agent's reading: merges wait for the flows.
```

| action | the files store's answer |
| --- | --- |
| `search` | `{"hits", "omitted_count"}` as `list`, at most `limit`: memories whose text holds the most of the query's words, matched as whole words in lowercase, newest first among equals; every memory when the query has no words. A hit's summary is its page line for a ruling, else its text on one line, at most 400 bytes |
| `get` | `id`, `kind`, and `file`: the file as it is written, header and all |
| `write` | `{"id", "replaced"}`: the id it retired, or null |
| `write`, `replaces` and no text | `{"retired": id}` |

## Data flow

1. The controller makes the one `Memory::new(home)`, and the residents task uses the controller's; `razor doctor` makes its own. The files store holds nothing between calls.
2. Every call takes the caller's `memorum` setting (the session's settings for the page and the tool, the residents' settings for list and forget, `Config::load` in doctor): absent means the files store, present means Memorum at that socket. Memory keeps no copy of the settings.
3. `run` parses the call and applies the shared rules above; a ruling's words come from the entry the controller fetched before the call queued (`cited`, unchanged).
4. Files: the project is the checkout whose `.razor` files apply (a worktree's main checkout, else the repository root, else the folder), so children share their parent's memory.
5. Every write is a temporary dotfile with a random name, `sync_all`, then a rename onto its name, so a reader or a crash sees the old file or the new one, never half of one. A state note is that rename onto `state.md`: the last state note is replaced in one step, and earlier ones live only in the session records, which hold every state note ever written. A state write ignores `replaces` (2026-11-02, under Trey's approval of the startup-text changes): agents named `state` or an old note's id there, and the refusal that once answered them failed 21 writes in a day, though the new note replaced the last either way.
6. A write with `replaces` first moves exactly the named file into the retired tree, by one rename, and writes the new file only after that succeeds. Only one rename of a file can succeed, so of writers replacing one memory exactly one writes; the others are refused with a sentence that the memory was already replaced or retired. A crash between the two steps leaves the old words in the retired tree and no new file, the accepted trade against two current versions. Retiring is that move alone, so a person's hand-written words stay on disk; the person's `forget` deletes the file, since they asked for the words to be gone.
7. `search` and `list` read every memory file of the project's folder and `all/`; the page reads `state.md` and the ruling files only. All on a blocking thread, one file at a time, at most 17 KiB each, keeping only the best 64 or `limit`; a write's temporary file is not a `.md`, and memories past their `expires` date are left out (kept through the date, as the tool's description says; Memorum's `valid_to` ends at the date's first second, unchanged). Newest means the file's modification time, so a person's edit counts as new, and then the id, since ZFS gives files written in the same few milliseconds one time.
8. The page holds hand-edited files, and on either store, to the written bounds: the state note is cut to 1,500 bytes, and the rulings pass through the same 20 lines in 4,000 bytes, so the page stays about 1,500 tokens.
9. `get`, retiring and `forget` open `<id>.md` in the two current folders; an id holding `/` names no memory, so no call reaches outside them. A link counts as the file or folder it points to: memory is not a boundary against the person's own agents, whose shells can already write anywhere the person can. A file is opened without waiting on a pipe, and one that is not a regular file is no memory, so a named pipe cannot hang a read. A new file is created, never opened over.
10. Memorum: today's client unchanged, except that the socket comes from the key, not from `MEMORUM_SOCKET`, `MEMORUM_RUNTIME` or `MEMORUM_REPO`. `check` names the folder without creating it, or asks Memorum for its protocol as today.

## The hard decision: storage and search with no new dependency

**Storage: one file per memory (chosen).** Every write is one rename onto a single name, so no two writes share a file and none needs a lock, and nothing ever scans to decide what to retire. The folder is the memory a session sees: a person can read it, edit or delete a file, or add one, and the next call sees the change.
- *One append-only file per project.* An append is crash-safe and a read is one file. But a replacement or retirement becomes an appended event, so a `forget` that removes the words needs a rewrite and a rule for compacting. A person's edit and Razor's append race on the same file, and a log of events is not something a person reads as their memory.
- *One file for everything.* Every write rewrites the whole store under a lock, and a person's edit made during a write is lost.

**Search: scan on demand (chosen).** It adds nothing to idle memory, each call holds at most 64 files, and its cost grows with the number of memories in two folders, not with how long the server runs. It is a tool's own work on a blocking thread, not the harness's per-call overhead, so it is held to a stated size instead of SPEC's 2.5 ms. **Supported: up to 2,000 memories in a project and 2,000 for every project.** Measured through the release binary at that size (4,001 files of 150 to 1,300 bytes, 3.3 MB; nine `memory` searches in one session, each timed from its call entry to its result entry in the record): a median of 67 ms (range 62 to 89) with the files in memory, on a ZFS home folder; the person's list through the API took 50 ms. Where the filesystem keeps no file contents in memory (a scratch volume with a cold cache), the same searches took a median of 1.2 s (1.1 to 1.5): every file is a disk read, and that is poor; on a normal machine the page cache holds them. Memorum's own `search` takes about 265 ms. The startup page opens only the rulings and the state note, so its cost follows the number of rulings, not of memories.
- *A word index held in memory.* It holds every memory's words for the server's whole life, several times the text, against an idle limit of 24 MiB where the server measures 12 to 13 (`docs/design/perf.md`). It is built by reading every file at start, and it goes stale as soon as a person edits a file; noticing the edit means a stat of every file per query, which is most of a scan's cost anyway.

**Several servers: each home keeps its own memory.** Paired servers do not share it, and a child on another server reads that server's memory. The alternative is one owner that its peers ask, as `[server] shared` does for saved views: that needs API routes for every memory action, forwarding, and a rule for when the owner is unreachable, on an API row with 5 lines left, for a need nobody has named, and Memorum already meets it.

**What a person gives up compared with Memorum:**
- memory shared with Claude Code, Codex and other harnesses;
- one memory across machines and homes (Memorum syncs through git, so several servers can share one);
- ranked full-text and embedding search (here a memory matches the query's words exactly);
- Memorum's git history, consolidation, focus notes, and memories hidden from children;
- a project identity that survives moving the folder (here the folder's path names it; moving the memory folder to match follows a move).

**Choosing the store: an explicit machine key (chosen).** `memorum = "<socket>"`; absent means the files store. When Memorum is down, the page says so and calls are refused, as today, with no fallback. A server that used Memorum through the environment needs the key in its settings no later than it runs this binary, because a new binary without the key writes memory files.
- *Detecting a running Memorum.* While Memorum restarts, that minute's rulings and state notes go into the files store, where Memorum's sessions never see them, and the next page shows Memorum's older note. Every call would pay a probe, and a stranger never has Memorum running, so detection serves only a person who already configures Razor.
- *A name key (`memory = "memorum"`) plus today's environment lookup.* That puts the answer to "where does memory go" in two places, and environment variables are not live.

**The ruling check stays in the shared path, where it already is.** Razor holds the record, so `cited` and `cite` refuse anything but the person's input or answer for both stores, and the ruling's text is composed from the record's words, never from the model's. Memorum cannot read Razor's record; the files store receives text that is already composed; so the rule lives in one place. A memory file written by hand, an agent's included, is not checked, just as a file written into Memorum's repository is not: the check binds the `memory` tool, and Razor is not an authentication layer over a person's own files.

## Lines

**Flows.** Flows 24 and 20 need Memorum, because imported records, focus notes and `memoryd project init` belong to it; so does the second doctor run in 01. Each starts its own scratch Memorum (`driver.memorum`), never the machine's. Flow 02 and the first doctor run in 01 use the files store. With the default now the files store, four flows change, each keeping its line count: 24 and 20 change `env={"MEMORUM_SOCKET": …}` to `config='memorum = "…"'`; in 01, a fresh install's Memory line reads found, and the second run writes the key to its home config; 02 looks for the page's headings in place of the Memorum sentence. The same commit rewrites SPEC.md's memory paragraph (section 3), its memory-client bullet and dependency sentence (section 5), and the README glossary's line for memory.
