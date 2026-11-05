# The agents view

The list uses a project gutter, cyan borders and the chosen top bar. Trey subsequently removed failed-call counts and their legend from the list. It supports three group levels and configurable terminal keys; Trey removed its previews on 3 November (see Opening). This page replaces the earlier list design; the top bar and its layout are the terminal app's own and are not designed here.

The approved visual references (prototype pictures kept outside the repository) fix the layout and the cyan border colour. Trey's later decisions remove the empty-group footer and calls graph shown in those pictures, replace idle waiting text with the first sentence of the latest reply, and omit the small "working on" label. Group definitions remain in the server's saved views; the prototype's proposed per-session path is superseded.

## Layout and rows

At a list width of at least 50 columns, each top-level group has a 13-column left gutter followed by a rule and a space. Its name appears once, in its colour, wrapped to two lines when long, with its agent and helper counts underneath (Trey, 2026-11-03, removed the status census: the cards already carry each state). Folded, a group is one line: its name, and beside it what it holds. The right side contains its agents and the heading rows for projects and sub-projects. Heading names indent two columns at each level; the status glyphs for agents and helpers remain in one column. Helpers have tree branches after that column. A group with one project and no directly assigned agent folds that project's name into the gutter when it fits; a long name keeps its heading row. This visual compression never changes the stored hierarchy or the fold target.

Root agents occupy two lines: identity, location, context and age; then their current call, idle last reply, question or error. Helpers occupy one line, keeping the shared status column, branch, name, state, clipped detail and age. A helper waiting for settlement says `◆ merge`; an otherwise completed helper says "✓ done". The selected row and the session being addressed are distinct: keyboard selection has its mark, while the addressed session retains its cyan edge and background.

Counts include every listed descendant of the group, including locally folded rows, and exclude settled helpers. Count roots as agents and descendants as helpers. A parent group sums its own rows and every child group, without counting NEEDS YOU or PINNED copies twice. Folded groups keep their counts. Project and sub-project headings carry their own rolled-up counts on the right.

Below 50 columns, replace the gutter with ordinary indented heading rows and keep the same order, counts, fold targets and status column. Names clip before state and context; clipping uses terminal-cell widths. At wide terminal sizes the list starts at a third of the screen, between 32 and 64 columns, beside the session; dragging its right edge, or `<` and `>` (`list.narrower`, `list.wider`) by four columns, sets its width, at least 32 and leaving the session 40, and the home's `list-width` keeps it for the next start. Below 80 columns the list or session fills the screen according to which owns keys. A narrow full-screen list still previews its selected row when the session becomes visible; Enter opens it.

Residents use the same group layout and aligned glyph column, with a row per resident, its state and next wake. The timeline strip is removed. Opening a resident still addresses its home session; the existing run history remains in the resident/session transcript. The server summary stays above the groups, with its existing expandable load, memory and account-usage table. Empty groups, including a parent with no agents anywhere below it, are hidden. Loading, unavailable views and an empty list each get one plain status line. Earlier stays folded by default. The header leaves room for a future search control; it has no search field or search key in this change.

The server includes a built-in `residents` section before the other sections. Each home is a depth-zero row; its live helpers and open threads the person spoke in follow at increasing depths, including helpers on a directly paired server. Threads use their resident as their display parent without changing recorded parentage or delivery routing. Finished and settled resident activity leaves the section and never moves into folder groups or Earlier, except a person's open threads, which stay as [below](#a-residents-threads); an active descendant of a completed helper uses the next visible ancestor. A hidden fresh run and its descendants stay out of this section. Resident homes remain visible through the existing resident metadata, and the terminal renders their descendants through the same helper cards as the saved groups. Tree navigation follows the resolved row depths. The selection shows in the fold's column, a space before the status glyph: a cyan `❯`, or a parent's own fold arrow turned cyan so its fold state stays visible. Selection leaves the status glyph, name and click target in the same columns. Residents stand a blank row apart, and each one's "+ New thread" sits under the status glyphs.

## A resident's threads

Approved by Trey on 3 November 2026: "I wanna be able to make separate fresh context threads with residents ... TUI agents view should live and feel excellent". A thread is an ordinary session started as a resident ([residents-api.md](residents-api.md)); it needs no server path of its own.

```
 ▾ Residents │▾❯○ ember                     night 03:00
 1 resident  │   + New thread
 ○1          │  ○└ tc-five  flow done                 3m
 next 03:00  │
```

- **Rows.** A resident's row has a fold arrow like a parent's: Space, `h`/`l` and a click on the arrow fold its block, `l` unfolds it, and a folded block that hides the addressed thread tints the resident's row. Under it sits the "+ New thread" row, then its threads as the same one-line cards helpers use, branch mark and all, so selection, click, Enter and the addressed tint work unchanged. A thread's clock is the time since it last did anything, as a root's is.
- **Which threads.** The server's Residents section lists a resident's open threads, the sessions with a person's input in them, whether or not they are live, most recently active first. A closed thread leaves its resident for Earlier, as a closed root does (Trey, 4 November 2026: resident threads "don't go away when closed"; until then the three most recently active closed ones stayed, the rest behind a finished line). An idle close is not the person's, so its thread stays. A fresh run is a thread nobody spoke in, so it stays out of the section. A thread just created appears once its first input from the person lands.
- **After a restart.** Only a session with work in flight (or idle maintenance due) is loaded when the server starts, so every idle session used to be absent from the status until something read it. At start the controller now reads, in a background task and without starting any session task, the summaries of the roots whose records changed in the last twelve hours (the idle-root rule's window; a stat of the record decides before any of it is folded) and of each resident's twelve most recently changed threads, whatever their age (`Controller::recent`). Job threads, children and older roots stay out. The agents list after a restart therefore holds what the layout shows, with no client asking. Measured on release builds against hard-linked copies of two real homes' sessions (517 records, 824 MB, 45 roots in the window; 199 records, 274 MB, 23 rows), with no live markers: the API answers in 0.08 s before and after; the rows settle in 0.44 s with 0.27 s more CPU and 4 MB more peak memory on the first, 0.18 s, 0.11 s and 7 MB on the second.
- **Starting one.** Enter or a click on "+ New thread", `n` with a resident or any of its threads or their helpers selected (`list.thread` in `keys.toml`), or `/thread [resident]` aims the right pane's composer at a new thread: its top bar says "New thread · ember", its placeholder "New thread with ember", the list row keeps the `❯`. Like a new agent, the session is made by the first message (`POST /v1/sessions` with `resident`, through `?server=` for a paired resident), so an abandoned aim leaves nothing behind. `/thread` with no name uses the selected row's resident while the list has the keys, else the addressed session's; anything else gets one sentence naming the way. The server takes the folder from the resident, and stores the thread's name as `<resident>-<noun>` (`ember-ferry`): the noun is the last word of the new id's word name, from `names.txt`, and a name already in use (a listed agent's, or a root's changed in the last week) gets the lowest free `-2`, `-3` …, as a root's does. The person's or the resident's rename replaces it; a fresh run's thread is renamed `<schedule> · <date>` at once, since that says more.
- **The list's stops.** A parent's own fold is not an arrow stop: before, each parent card, and now each resident, held an invisible stop between the row above and itself.

A resident's finished line and its start-time threads count the most recently active, not the newest made, and a voice note as a thread's first message is made on the resident's own server by the same route as typed words (`inputs::creation`).

## Shared groups and filing an agent

One saved `Views` definition belongs to the configured shared server. Other servers forward GET/PUT once and resolve those definitions against their local and directly paired agents. The existing version check prevents one editor overwriting another's save. Retain the 256 KiB stored-definition limit, 8 MiB layout limit, complete view-name order, changed-layout streaming and at-most-once-per-second publication.

Validation and layout resolution use bounded depth-first walks supporting depths 0, 1 and 2. A fourth level is refused. Assign missing stable ids and reject duplicates at every level. `Section.depth` already carries the third level, so no new group-path field goes on a session or its record. An empty parent whose descendants are populated must remain in the layout. Remove a group only when its entire subtree is empty.

`/group Venture / Project / Sub-project` files the addressed root into that existing group in the selected view. Paths have one to three names, with optional spaces around `/`; an unknown or ambiguous path is refused with a sentence pointing to `/views`. A helper files its root, because helpers follow roots. The command reads the current definition, removes that root's session claim elsewhere in the selected view and adds the exact session claim to the destination, then PUTs with the original version. Existing folder and repository claims remain available for automatic placement. The command creates no groups and retries no conflicting save; `/views` remains the editor for names, colours, claims and ordering. `/group` with no path shows the addressed root's current group.

Matches retain their specificity: session, folder, repository; ties choose the first group in depth-first order. Stable ids preserve folds through renames. Fold handling uses each section's depth, closing the complete subtree at any of the three levels. Built-in NEEDS YOU, PINNED, Everything else and Earlier retain their current partition and order. Closed roots go to Earlier immediately; unread deliveries stay in the inbox. Settled children leave and their children become roots. Finished helpers past their parent's newest three remain behind that parent's finished line.

**Smart sort, 2 November 2026.** `Group.why: Option<String>` is carried into its layout section; PUT refuses explanations over 200 UTF-8 bytes or containing a newline. `View.wake: Option<SessionId>` opts Auto into automatic sorting and names its resident; absent is off. The existing once-per-second resolver hands the resolved snapshot to Residents without waiting for admission. A single resident delivery task batches new depth-zero Everything else rows into one `smart-sort` program input; while admission waits for queue space, a new snapshot cancels that admission and retries from the latest roots and current resident membership. Only admitted roots count as sorted. A blocked resident never stalls layout publishing. The wake names the resident's local home session; only a configured resident receives admission. Successfully admitted ids are remembered for this process while retained in the bounded agent snapshot, so a restart or a root disappearing and returning can wake again. Other views, children, Earlier roots and residents cause no wake. No model turn polls for new roots. The resident is personal pack data; version conflicts and preserving human edits are its instructions.

## Opening

The right pane shows only the addressed session, `App.focus`. Clicking or pressing Enter on an agent switches to it through the existing guarded focus path, which saves the old session's draft and refuses while a note or input is pending. Nothing else changes the shown session. Only a group's arrow folds the group.

There are no previews. The list used to show a session's conversation, with a Preview label, while the pointer rested on its row for 500 ms or the selection rested for 300 ms, leaving the composer on its recipient. Trey removed that on 3 November 2026: "pls remove the thing where whenever I mouse over an agent it swaps to that agent in teh session feed, i don't like it, no previews, i might revisit one day". The mechanism was deleted rather than switched off: the list's preview target and deadlines, the hover handling, the Preview label and the preview's place among the card readers. The terminal now asks the terminal emulator for presses, drags and the wheel only, not bare pointer motion, so a terminal no longer sends a report for every pixel the pointer moves.

Keyboard navigation in the list uses j/k and Up/Down, which move the highlight and nothing else. Space toggles the selected heading or parent's children. h/Left moves to the containing group; l/Right enters a group or its first visible child. Enter opens an agent, or folds a selected heading. Escape closes an opened question's options and never stops an agent. The existing full-screen child split remains available outside the list.

From the composer, an optional prefix followed by n/p opens the next/previous root in the same project, through the same switch as a click, opening the list if it is closed. The project is the one each row already carries (`AgentView.project`, the folder's name); helpers use their root's. The order is the list's, across every heading: saved groups and state headings only order the project's roots, so a working root and an idle one are neighbours (until 3 November 2026 n/p stayed inside the current root's section, and a project split by state headings skipped its other roots). NEEDS YOU is skipped, since it repeats rows listed below it, and so is Earlier: n/p never reopens a closed session, and an idle one opened from Earlier is live again while a terminal has it open. At either end, stop at the first/last root. If the project contains only the current root, leave the view alone. Prefix h/j/k/l enters list navigation. Prefix is unset by default. Escape cancels an armed prefix; a key that has no prefix action is discarded with the available actions shown, rather than becoming an accidental send or edit. The prefix consumes one following action and then disarms. Prefix n/p leaves keyboard input in the composer; F2 and prefix h/j/k/l explicitly enter list navigation.

## Short titles and the last reply

Agent instructions ask for a headline of five words or fewer at the start and whenever the goal changes. Keep this to a few lines in the shared profile text, within flow 28's 12,000-token prefix cap. A headline is human wording; no runtime truncation to five words and no model request are added.

The record summary already folds each `Body::Text` to set `spoke_at`. In that same pass, retain a bounded first sentence of the latest reply and expose it as defaulted `AgentView.last_reply`. Collapse whitespace; stop at the first sentence-ending punctuation followed by whitespace or the end, and cap at 512 UTF-8 bytes on a character boundary. A reply with no such punctuation uses its bounded text. This is a display excerpt, so abbreviations may end it early. An idle row shows this excerpt instead of "waiting for you"; with no reply yet, use the headline, then "No reply yet". Questions, interruptions and errors retain their existing priority. No per-row transcript fetches or model calls are needed. Replaying the record after restart reconstructs the same excerpt, and peer status streams carry it normally.

## Configurable terminal keys

One table per context under `[ui.keys]` maps plain action names to arrays of key strings, for example `[ui.keys.global]` with `prefix = ["Ctrl-B"]`. Missing names inherit shipped defaults; `[]` unbinds an action. `[ui.keys.global] prefix = []` is the default. People and their agents edit the same plain TOML file. The live configuration stream carries changes to the terminal, where bindings compile only when `ui.keys` changes, rather than parsing strings on every keypress. `/keys` lists every action, its context and all effective keys, including unbound actions and the prefix.

The shipped `terminal/data/keys.toml` catalogue owns names, contexts, default chords and plain descriptions. It is loaded through `include_str!` and parsed with the same parser as personal overrides. Keys use `Ctrl-`, `Alt-`, `Shift-`, `Super-`, named keys such as `Enter`, `Space`, `Left`, `PageUp` and `F2`, or one printable character. The protocol carries only a defaulted nested map of contexts and actions to key strings. The terminal parses that map into native key events once per settings change. Invalid names, invalid chords and duplicate chords for different actions within the same context produce a config error in the terminal and leave its last valid compiled bindings in place. Separate contexts may reuse keys, as Enter already means different things in the composer and a picker. Bindings are bounded by the fixed action catalogue and at most eight keys per action.

Every terminal keyboard command goes through the catalogue: quit/stop, list and project navigation, prefix, status, questions and option numbers, child navigation, transcript paging and expansion, copy/cut/quote, attachments, model/effort pickers, voice, emoji, completion and shortcodes, all editor movement, selection, deletion, newline, undo and redo, and modal movement/accept/cancel. Ordinary printable text and bracketed paste remain input. The current modal and question precedence stays explicit; a key has an action only when that action is available in the active context. Existing defaults remain except for the list's approved j/k, h/l and Space behavior. On-screen hints use effective bindings; no hard-coded F2, Ctrl-S or Enter hint survives a remap.

A helper that is done without a report also leaves the list, matching the card’s “stopped by its parent” state. A stopped child’s durable report stays in its parent record, but is cleared from its displayed row. Remote lifecycle facts remain on the parent’s server; a finished remote helper with branch work remains listed there, while its owner’s list lacks those parent facts.

A finished helper whose report says its branch has no commits beyond its base leaves the list as soon as the report reaches its parent. Helpers without a branch follow the same rule. Continuing clears the report and returns the helper to the list; only settlement ends continuation. This uses the parent summary’s recorded report, with no git or file reads while resolving the layout.

## Internal surface and data flow

```rust
// Existing protocol views: add no per-session group path.
/// Stable ids and at most three levels; children follow their root's matches.
pub struct Group { /* existing fields, including groups: Vec<Group> */ }
/// Flat depth-first sections; depth 0–2 is independent of agent parent depth.
pub struct Section { /* existing fields */ }
/// Defaulted, bounded display excerpt reconstructed from the record.
pub struct AgentView { /* existing fields */, pub last_reply: String }
/// Live key overrides, checked against the single action catalogue.
pub struct Ui { /* existing fields */, pub keys: BTreeMap<String, BTreeMap<String, Vec<String>>> }

// terminal/agents.rs, within the existing private terminal module.
/// Draws the same server-resolved sections in the list and the status pane.
pub(super) fn rows(app: &App, width: u16, now: u64) -> Vec<status::Row>;
/// Consumes semantic list actions; only Enter opens, through `press`.
pub(super) fn key(app: &mut App, key: KeyEvent);
/// Explicitly opens an agent or folds a heading through the existing action path.
pub(super) fn press(app: &mut App, action: Action);
```

1. The shared owner's definitions remain the only source of group names, rules and nesting.
2. API validation and resolution walk the same three-level shape; each session is placed once.
3. A layout streams ordered sections; each section contains only its own roots and helpers.
4. The terminal counts roots and helpers from row depths and hides folded subtrees. It builds the card status census once per top-level gutter entry; every gutter label clips to 12 cells.
5. The existing card formatter owns status and identity; the shared list renderer owns gutter, headings, connectors and fold hits.
6. Resident rows join the same visual grammar while retaining their existing server metadata and home-session targets.
7. The record summary supplies the latest reply excerpt in every agent row; idle rendering uses it directly.
8. Enter, a click or prefix n/p calls the existing guarded focus switch; composer input always targets focus.
9. Changed key settings compile the single action table; translation returns the key, context and action explicitly, and active contexts choose actions and the same table supplies hints and `/keys`.

The redesign's hard decision was to give preview its own displayed-session id, so that navigating never changed the recipient or draft. With previews removed that id is gone too: the displayed session is always the addressed one.

## Checks and evidence

Flow 56 passes shared three-level placement, refusal of a fourth level and duplicate ids, `/group`, versioned external editing, paired preview, idle excerpts, the 49/50-column gutter threshold, folding a complete group and a live Ctrl-Y status remap. Flow 20 checks “closing after this turn” in the working line and agent list.

Modified deletion, movement and selection chords are in the key catalogue. Empty-option questions retain `/answer`, numbered hints stop at nine, and scheduled threads omit the `/done` closing notice. The key contexts use the plain names documented in Configuration, quit hints use effective bindings, and unchanged key settings never recompile.

## Layout server identity

Layouts need only server identity and the configured resident home ids, so they skip
calendars and load. One private `identity(app, config) -> Server` derives id, label,
machine and realm for both layouts and full status; `residents::Config::homes()` adds
each configured home id with empty upcoming wakes and past runs. Status still retains
the full snapshot: load, voice capabilities, upcoming wakes and past runs. A configured
home with no agent row never appears.
