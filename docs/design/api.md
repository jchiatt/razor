# The API server

## Paired sessions and the inbox

```rust
// protocol/api.rs: additive fields default for old clients and index rows.
pub struct Server {
    // Existing id/label remain.
    /// Enrollment name for qualified addresses; empty for the connected home.
    pub name: String,
    /// Reported server.name or hostname, independent of the free-text label.
    pub machine: String,
    /// Optional server.realm, in words.
    pub realm: String,
    /// Last contact when unavailable; None while reachable.
    pub silent: Option<Millis>,
}
// Live::Agents gains defaulted servers: Vec<Server>; its agents include directly paired homes.
// Live::Server(Server) is emitted only for an opted-in heartbeat reader.
pub struct Delivery {
    // Existing session/entry/at/line/file/url stay.
    /// Sender derived through the record's Start naming rule.
    pub name: String,
    /// Project at delivery time; empty in old rows.
    pub project: String,
    /// Independent shared inbox state, defaulting to false in old rows.
    pub read: bool,
    pub archived: bool,
    /// Owning home's absolute copy path, enriched for the API.
    pub path: Option<PathBuf>,
}
// api/peers.rs: server-owned, transient snapshots; one reader per directly paired device.
/// Forward the registered person session routes to an enrolled owner, once, with its key.
async fn forward(State(app): State<App>, Extension(caller): Extension<devices::Enrolled>,
    request: Request, next: Next) -> Result<Response, Refusal>;
// residents/deliveries.rs, under its existing single-writer lock.
/// Sync and broadcast one full replacement, preserving identity and the other state bit.
pub async fn read(&self, session: &SessionId, entry: EntryId, read: bool) -> Result<(), Error>;
pub async fn archive(&self, session: &SessionId, entry: EntryId, archived: bool) -> Result<(), Error>;
/// Every lowercased query word matches line, sender name, project, file name or url, ignoring case.
impl Delivery { pub fn matches(&self, words: &[String]) -> bool; }
// record/mod.rs
/// Read only Start and apply Summary's existing naming rule, without opening a session task.
pub async fn name(dir: &Path, session: &SessionId) -> Result<String, Error>;
```

**Ten lines of data flow.**

1. The API owns one reader per directly paired Device address/key, cancelled on removal. Readers request local-only status, preventing recursive aggregation and duplicate rows. Local status never waits for a peer.
2. Each peer's native ids, parent references, ask identities and deliveries gain @enrollment-name in the API view. Native transcript bytes stay unchanged; clients qualify links using the enclosing session.
3. The API retains each peer's latest snapshot and delivery window only in memory, with one 8 MiB byte bound per peer, reusing the stream line bound. No global byte accounting or disk checkpoint.
4. Two-second initial contact and independent reconnects keep local status available. A sleeping machine silences every server on it; one stopped server leaves another on the same machine reachable. Before first contact show the paired host as provisional machine words.
5. Presence goes out on change. Retain a 15-second heartbeat and a 45-second silence limit: a scratch TCP connection stayed open while its API process was stopped, even with 1-second idle/interval and one keepalive retry, because the kernel still acknowledges keepalives. This checks API-process unavailability, not physical machine sleep.
6. A qualified session route resolves only an enrolled peer, strips its suffix and forwards once. Bodies retain the 32 MiB bound; Content-Type, Range and If-Range are forwarded and response streams are passed through. No body is recorded or parsed by the forwarding server. A closed client response drops the upstream response; the owning API already controls its enrolled-device stream.
7. Input at the owner remains from the person, with the input's `device` naming the authenticated server (since 2026-11-03, when it took the place of `via` on the origin so a phone is named the same way). No new origin kind or mail-only pairing.
8. Read receipts live in the owning home's deliveries.jsonl, shared across its devices. No read directory, database, startup unread scan, global counter or decrement-once logic. Unread covers the newest 256 deliveries only.
9. Later receipts preserve original positions and do not send a second push. Restart restores read state by listing the index, without a counter.

**Authority, approved by Trey.** One kind of pairing grants the already existing full device key (mail.md, "Trust and failure"); what is new is servers using that key as the person. Forwarding is mounted once on the existing session router, before follow/device/global routes are added; it has no second route allowlist. The exact forwarding surface is GET events, entries, files/*, draft, skills, settings, project and marks; POST input, stop, done, quit, handover, asks/{entry} and recap; DELETE input/{entry}; PUT draft and seen; PUT/DELETE deliveries/{entry}/read and deliveries/{entry}/archive, all beneath /v1/sessions/{id}. These are session operations, including admitting input, answering asks, stopping turns and reading saved output. Devices, config, sign-in, mail, global MCP mutations and session creation are never forwarded. Forwarding refuses dot segments, encoded slash or backslash separators and literal backslashes in the qualified path. An authenticated paired server cannot ask this server to forward; the hop header independently refuses a second forward. The owner derives the input's device from the authenticated pairing, never a caller's claimed header.

**Alternatives weighed.** Direct connections forecast 747 net product lines (API/config 110, protocol 45, children/residents 124, record 18, terminal 450), versus the original aggregation forecast 959 (API/config 470, protocol 65, children/residents 90, controller 4, terminal 330). Direct saves server lines but moves 120 lines into the terminal, its tightest row. The approved product exposes paired homes through one connection for every client without distributing keys to clients; that and Trey's ruling decide aggregation. The implementation replaces that forecast; no performance improvement is claimed.

**Persisted inbox state.** Only `$RAZOR_HOME/deliveries.jsonl` on the delivery's owner stores `read` and `archived`, independent bits shared across devices. PUT/DELETE `deliveries/{entry}/read` marks read/unread; PUT/DELETE `deliveries/{entry}/archive` archives/unarchives. All four are idempotent and preserve `at/session/entry` and the other bit. A changed state appends a full replacement under the writer's lock, syncs before acknowledgement, then broadcasts. An unchanged state syncs without appending or broadcasting. Archived rows are excluded from the layout's unread census. Reverse scans keep the newest complete copy whole; read is no longer monotonic. Replacement lines can mark unread. No migration or startup id set is added, and at most 256 rows are retained per page. Incomplete tails are never acknowledged. The terminal hides archived rows in the inbox and its unread count, retaining them in its bounded newest-256 cache so conversation voice notes stay playable and unarchiving a cached row is a replacement. It takes each replacement's read state.

`project` is captured with `name` from the summary lookup at delivery time (`children::project`), empty in old rows. `GET /deliveries?q=` requires every whitespace-separated word, ignoring case, in line, name, project, file name or url. Filtering happens during the local scan before limiting and over each peer's cached newest 256 rows in the peer merge before limiting; cursor pages still belong to the connected home. Every replacement has identical searchable fields, so every copy matches or none does. No `archived=` or `read=` server filter is offered: clients choose which states to show. The internal deliveries broadcast carries `(delivery, new)`; status forwards both new and replacement rows, while push alerts only for a new unread delivery. This discriminator is required because an unread replacement must not re-alert the phone.

**Shape.** Extend the existing replacement writer and bounded reverse scan; share a private mutation path between read and archive and one search predicate between local and peer rows. Separate receipts or a second index would duplicate inbox facts and require additional reverse-scan state; deleting the writer would distribute its lock and sync obligations across routes. No new dependency or storage is needed. `Delivery` is boxed inside `Stored` and `Live` to keep those enums small as the stored project is added; their JSON stays the same.

The peer cache keeps an incremental byte total within 8 MiB: changed agent rows, delivery replacements and evicted deliveries add or subtract only their serialized sizes. Snapshots recount the cache; a small reserve covers timestamp and separator growth. Idle heartbeats skip serialization when metadata is unchanged.

**Acceptance.** Flows 25, 26, 43 and 44 in separate systemd units. Two real paired scratch servers: identical ids, input/withdraw/stop/answer, native event/page/file forwarding, no recursive rows, peer disconnect/return and key revocation. Verify provenance, forbidden destinations, shared read state across restart, migration, window unread, local old-page receipts and no second push. Real pty frames at 40×30, 120×40, 160×50 and motion on/off; compare the approved combined.html and unchanged existing frames. Fmt, clippy, size; release perf versus main alternating three times with machine load recorded.

Design page, 2026-10-27; revised 2026-10-28. Module `src/api/`, and `src/main.rs`'s server and clients. Budget row: "API server and configuration", 4,000 lines, shared with `src/config/`, `src/main.rs` and `src/lib.rs`.

The server is the one door every client uses: the terminal app, `--print`, `razor login`, `razor devices`, and later the web app and plugins. It holds client connections; [devices and paired transport](devices.md) owns enrollment and keys. It reaches sessions through the controller, residents and deliveries through the residents module, and sign-in through `Responses::login`.

## Surface

```rust
/// The local socket and the local device's key (mode 0600), in the state directory.
pub const SOCKET: &str = "server.sock";
pub use crate::devices::{Error, KEY};
/// The base URL of a client bound to `SOCKET`; the host is not used.
pub const LOCAL: &str = "http://localhost";

/// Serve until this future is dropped.
/// - Listens on `home/server.sock`, replacing a stale file (the caller holds `server.lock`), and
///   on `address` (`server.address`, the tailnet address) when one is given.
/// - Makes `server.id` at the first start and writes `server.json`: `{pid, address}`. The
///   controller has already opened device enrollment and keys through `crate::devices`.
/// - Every request must carry `Authorization: Bearer <key>`: the local key or an enrolled
///   device's. A body is at most 32 MiB.
/// - The ChatGPT sign-in answers 503 until `serve` is handed the model client.
pub async fn serve(home: PathBuf, address: Option<String>, controller: Controller,
    residents: Residents, deliveries: Deliveries) -> Result<(), Error>;

/// `razor login`: the ChatGPT sign-in runs in the server; each instruction is printed as it comes.
pub async fn login(client: &reqwest::Client, key: &str) -> Result<(), Error>;
/// `razor devices`, `razor devices add <name>`, `razor devices remove <name>`: the text to print.
pub async fn devices(client: &reqwest::Client, key: &str) -> Result<String, Error>;
pub async fn enroll(client: &reqwest::Client, key: &str, name: &str,
    address: Option<&str>, given: Option<&str>, own: Option<&str>) -> Result<String, Error>;
pub async fn revoke(client: &reqwest::Client, key: &str, name: &str) -> Result<String, Error>;
```

`main.rs` builds every client with reqwest's `unix_socket(home/SOCKET)` and reads `KEY`; `razor serve` starts the controller, opens `Residents` and `Deliveries`, and calls `serve`. The controller opens `crate::devices`, which owns keys, `devices.json` and the shared error type. `api/devices.rs` holds the enrollment routes and authentication middleware.

**Before the model client is wired in**, `serve` takes no `Arc<Responses>` and `POST /login` answers 503 with one sentence. Once it is, `serve` takes `responses`, and `sign_in` (31 lines) runs `Responses::login(show, stop)` on its own task and sends `true` on `stop` when the client leaves.

### Routes, under `/v1`

A refusal is a status code and `{"error": "<one sentence>"}`:
- 400: a malformed body or id; a picture that is not JPEG, PNG or WebP, or past the limits; an ask already answered; an input already taken; a file name that leaves the session's directory.
- 401: no key, or a key that no longer works.
- 404: no such session, resident, device or file.
- 409: 64 inputs are already waiting, the session is busy, or the device name is taken.
- 413: the body is over 32 MiB (a draft: over 1 MiB).
- 500: anything else; 503: the sign-in, until the model client is wired in.

| request | body | answer |
| --- | --- | --- |
| `GET /sessions?cwd=&limit=` | | `[Listed]`, newest first. `limit` is 50 by default, 1,000 at most. |
| `POST /sessions` | `{cwd, model?, effort?, call?}` | `{session}`: a session holding only its `Start`, idle until its first input. With `call: true`, a model or effort left out is `call_model`'s or `call_effort`'s (docs/design/call.md). |
| `POST /sessions/{id}/input` | `ClientInput` | `{entry}` once admitted. Any session: a child, or a resident, whose session id is its name. A person's input that is one landed sign-in address (the callback address with its query, nothing else) completes its sign-in instead, as the callback does: it answers the ask of whichever session began the sign-in, else signs in directly, and the reply is `{signed_in: true}`. Its code reaches neither the record nor the model. |
| `DELETE /sessions/{id}/input/{entry}` | | `{}`. The controller records `Withdrawn { inputs: [entry] }`, so the input is never delivered or carried into rebuilt context, across a restart too; no turn ends. Refused once the model has taken it. This cancels an accepted scheduled job. |
| `POST /sessions/{id}/stop` | | `{}`: Escape. Children and detached jobs keep running. |
| `POST /sessions/{id}/done` | | `{entry}` once `/done`'s closing input is admitted; that turn runs on in the server, and when it ends the session closes (docs/design/done.md). |
| `POST /sessions/{id}/quit` | | `{}` once the session is closed with no closing turn: the turn ending now is its last, as `/done`'s closing turn would be, so a running turn stops, inputs still waiting are Withdrawn, its jobs and children close, and a closed turn end is recorded (an empty one when idle); its next input reopens it (`/quit`, Trey 2026-10-30). |
| `POST /sessions/{id}/handover` | | `{entry}` once the threshold's continuation-note input is admitted, or `{}` if already requested/rebuilding; never waits for the rebuild. A closed child gets 400 with a sentence. |
| `POST /sessions/{id}/asks/{entry}` | `Answer` | `{entry}`: the input carrying the answer. Refused once the ask is answered from any device. |
| `GET /sessions/{id}/events?after=N&native=` | | NDJSON: the session's stream. |
| `GET /sessions/{id}/entries?before=N&limit=L&native=` | | NDJSON: the stored lines of up to `L` entries before `N` (default: the end), in record order. A page is at most 8 MiB, except that one entry larger than that comes whole and alone; entries are never split. `L` is 256 by default, 1,024 at most. Entry `N` alone is `before=N+1&limit=1`. |
| `GET /sessions/{id}/project` | | `{"commits": N, "merges": M, "branch": "main"}` from the session folder's git history; absent facts are omitted, including the branch on a detached head; `{}` outside git or after two seconds. |
| `GET /sessions/{id}/marks` | | `[[entry, kind], …]`, oldest first, newest 4,096 at most: `you`, `question`, `rebuild`, `error` or `failed` at the failed result's entry. |
| `GET /sessions/{id}/files/{name…}` | | A file in the session's directory: a picture, a delivered file, a job's output, `startup.md` (what the latest fresh opening or rebuild sent ahead of the first input, docs/design/profiles.md); `Accept-Ranges: bytes`. A single byte range returns 206 with `Content-Range` and `Content-Length`; an invalid or unsatisfiable range returns 416 with `bytes */size`. Multiple ranges, unknown range units or an unmatched `If-Range` are ignored (200). The controller's checked, opened handle is never reopened. |
| `POST /sessions/{id}/effort` | `{effort}` | `{entry}` after appending an effort fact, or `{}` if both live and recorded effort already match; no recorded input or new Razor turn. The next model request uses it, even mid-turn. A busy Claude turn receives one generic notice at its next tool boundary; an idle session receives none. Accepts `low`, `medium`, `high`, `xhigh`, `max`, with the existing chat cap; an unknown session is 404 with `{error}`. Invalid JSON is 400, an unknown or missing effort 422. Paired session ids forward as other session routes do. |
| `GET`, `PUT /sessions/{id}/draft` | text | The unsent words the controller keeps for that agent. An empty `PUT` removes them. |
| `GET /status` | | NDJSON: the status stream. |
| `GET /models` | | `Models {models, windows, efforts, unavailable, model, effort, call_model, call_effort}`: named choices and each one's effort list, plus defaults from the controller's current good config, a call's among them. `efforts` maps every name to levels in ascending order and replaces `tops`; authenticated like every other route. Prefix-accepted names remain valid when typed, not a falsely closed list. |
| `GET /residents` | | The residents' rows: `{session, stopped, next}`, `next` being `[what, at]` or null. |
| `POST /residents/{name}/start`, `/stop` | | `{}`: the start and stop of a resident's schedule. |
| `GET /deliveries?session=&before=&limit=&q=` | | `[Delivery]`, newest first, before the delivery `session` and `before` name together. `limit` is 50 by default, 256 at most; every word of `q` matches line, name, project, file name or url, ignoring case. |
| `PUT /sessions/{id}/deliveries/{entry}/read` / `DELETE` | | `{}`. Mark read / unread, idempotently, preserving archive state. |
| `PUT /sessions/{id}/deliveries/{entry}/archive` / `DELETE` | | `{}`. Archive / unarchive, idempotently, preserving read state. |
| `GET /devices` | | `[Device]`, without keys. |
| `POST /devices` | `{name}` | `Device` with its key, shown this once and never again. |
| `DELETE /devices/{name}` | | `{}`. That device's key stops working and its open streams end. No other device is touched. |
| `POST /login` | | NDJSON: `show {text}` lines, then `done` or `failed {error}`. A client that leaves stops the sign-in. |

**The session's stream** is the transcript, plus live lines that are never recorded:
1. `attached {session, last}`.
2. The stored lines after the cursor.
3. Then, as they happen: entries, `delta`, `thought` (the words of a thinking summary as the model writes them), `taken` (a working turn took this queued input at a tool boundary), and `tail` (a running job's last lines and when it last wrote).

On both, an entry leaves out its `native`, the model path's replay data that no client draws (on a GPT response about 100 KB, nearly all of the entry); `native=true` sends each stored line byte for byte. The record and `recall` keep it, and a paired server's `recall` asks for it.

A client's cursor is the last entry it consumed, never `attached.last` or a live line. A client 1,024 lines behind gets `attached` again and the entries it missed: the session's task takes the replay range and the subscription in one step, so no entry falls between them. Only live lines are lost, each superseded by something the client still gets: a delta by its `Text` entry, a thought by its `Thinking` entry, `taken` by the `TurnEnd` naming that input, a tail by the next tail or the job's end. `--print stream` prints exactly these lines.

A thought is `{"kind": "thought", "n": 7, "text": "…", "start": true}` for its first words and the same without `start` for the rest. `n` rises by one with each thought, from 1 when the server loads the session, so a client can tell which thoughts it missed. Each `Thinking` entry supersedes the oldest thought it holds that no entry took and no void line voided, and that thought's texts joined are the entry's text: a GPT reasoning item is one thought, each later summary part beginning with the line break that joins it; Claude's thinking block is one, and its entry lands right after it; a chat host's reasoning is one per response, from `reasoning_content`, else `reasoning`, chunk by chunk, which matches the entry for a host that sends one of the two or both in every chunk. A redacted or omitted Claude thought sends nothing. When a response is cut and tried again (the Responses loop's retry, or Claude Code starting the cut message anew), `{"kind": "thought", "n": 9, "void": true}` comes before the retry: every thought numbered 9 or below that no entry took is void, the client drops it at once, and the retry's thoughts go on from 10. A thought a stop, an error, a handover or a cut response leaves unfinished is recorded nowhere, and a client drops what is left at the turn's end. On GPT a full display channel drops a thought's line as it does a delta's, but the route numbers each thought before any of its lines can be dropped, so a later line still carries its own thought's `n`: a client that sees an `n` without its `start` lost that thought's head, shows none of it, and waits for its entry.

**The status stream** is everything about the agents now. It has no cursor.
1. `agents {agents, config, brand, ui}`: a row for every session the controller holds open and the config's present error, if any. Each row carries its own conversation's account usage windows; accounts are never combined across rows.
2. Then `agent` rows, each replacing the client's row for that session (a new session is a new row), and `delivery` lines. There is no controller-wide limit or separate limit line.

Each row's `calls` lists unresolved tool calls running now. `calls_made: u32` counts every tool call recorded over the session, and `failed` counts results that were errors. The record summary folds both totals during appends and recovery. A fresh session reports `calls_made: 0`.

`looked_at` is when a reader last opened or closed the session's stream, or reported `seen`: the last entry it showed the person, which a client reports with `PUT /v1/sessions/{id}/seen?entry=N` as he leaves the session (an entry past the last recorded counts as the last). Both live on the row in memory and are gone after a restart, like the row's other readings; a report is a truer look than a stream's open or close, and replaces it. A client recaps a session the person comes back to when ten minutes have passed since `looked_at` and his own last input, with entries after `seen`.

`agents` is sent again after a lag, when the controller closes a session (its row leaves), and when the config's error changes. The controller takes the snapshot and its subscription together, as the session's task does for attach, so no change falls between them. A row is sent when any of its fields changes, except `last_activity`, which is current in every row sent. Rows may repeat; a replacement never leaves a gap. An entry id that a row names is always already recorded.

Deliveries come from the deliveries broadcast, beside the controller's lines. A client reads `/deliveries` once it has connected, so a delivery may arrive twice; its session and entry say so. A stream the broadcast laps ends, and the client connects again.

### Phone meetings

The authenticated PUT/GET/DELETE/POST routes beneath `/sessions/{id}/meetings/{meeting}` are registered after peer forwarding, like the call route: qualified sessions are refused and the phone connects directly to the owning machine. Live chunks trim overlap, refuse gaps by returning the synced prefix, and never write audio headers; finalized pieces are digest-checked, synced and renamed before 200. The pending end manifest is `pieces/expected.json`; `complete.json` appears only after its complete live prefix and listed pieces are held. Audio is an opened `room.wav` handle passed to `files::serve`, preserving one byte-range implementation. The readers resume from the existing startup summary publication or lazily from a phone request; only the folder and recorded lifecycle survive a restart.

### Protocol types

Every type this page puts on the wire is in `src/protocol/api.rs`: `ClientInput`, `Answer`, `Listed`, `Delivery`, `Device`, `Login`, `AgentView`, `UsageWindow`, `Live`, and the limits `IMAGE_BASE64` (10 MB), `IMAGES_PER_INPUT` (20) and `BODY_BYTES` (32 MiB). `Report` and `Ask` are in `children.rs`. What they mean here:
- `Live` holds the stream-only lines above. `Tail` is a running job's last 5 lines with the call entry that started it (`call`; 0 for a job recorded before calls were linked), at most 1 KiB, at most one every 250 ms per job.
- `AgentView` is the children module's row plus model, effort, context, window, threshold, calls, asks, launched and `headline_at`. A fact not known yet is absent, never zero.
- A row's `report` is the one `Report { status, text, entry }`. The API sends its text cut to the first line, at most 200 characters; the whole text stays in its entry.
- `ClientInput` is admitted from the person, or, when `from` names an outside program, as `Origin::Program { name }`.
- `Answer` is `{"option": 1}` (from 0) or `{"text": "…"}`. `Listed.at` is when the record last changed.
- `ClientInput.images` is plain base64, no data-URL prefix. `AgentView.asks` is `Vec<Ask>`, so the question band needs no fetches. The config's error is a fact on every `agents`. `PAGE_BYTES` (8 MiB) bounds a page.

### What the API calls

From the controller: `create(cwd, model, effort, None, None)`, `admit`, `stop`, `done`, `handover` (the optional admitted entry, not a direct rebuild), `attach`, `list`, `store(session, bytes)` (the type read from the bytes), `page`, `file` (checked and opened in one step), `status() -> mpsc::Receiver<Live>`, `answer`, `withdraw`, `draft` and `set_draft`. From the residents module: `Residents::{list, start, stop}` and `Deliveries::{list, subscribe}`. From the model client: `Responses::login(instructions, stop)`.

Errors map in one place: `NoSession`, `NoResident` and a named file that is not there are 404; `Full` and `Busy` 409; `Invalid` and `Refused` 400; `Other` 500.

## Data flow

1. `razor serve` holds `server.lock`, starts the controller, opens `Residents` and `Deliveries`, and calls `serve`, which binds the socket and the optional address.
2. One check in `devices.rs` passes every request: its bearer key must match the local key or an enrolled one, each compared in constant time. The match tags the request with its device.
3. An input arrives as `ClientInput`, with at most 20 pictures. The API decodes them one at a time and `store` writes each, then `admit` appends the `Input` and the controller clears the draft. Until the model takes the input, it can be withdrawn with `Withdrawn { inputs: [entry] }`; that removes its words from rebuilt context, not from the recallable record, without ending the running turn.
4. A client draws a session by reading its last page, one contiguous read of the record, then attaches after that page's last entry.
5. A client draws every other agent from the status stream. The question band is the union of the rows' `asks`, with their text and options. A report's whole text is a one-entry page of the parent's record.
6. Answering, stopping and steering a child are the session routes, addressed by the child's session id. No client launches a child.
7. A delivery shows on the status stream and in `/deliveries`. Its file downloads through `files`, which streams the handle the controller opened and never opens a path itself.
8. A session stream that falls behind sends `attached` and the missed entries; the status stream sends `agents`. A client reconnects to a session with its cursor, and to the status stream with nothing.
9. `/login` runs `Responses::login` on its own task and streams each instruction, then `done` or `failed`.
10. Removing a device removes its key and ends that device's open streams; its next request gets 401.

## The hard decision: how the facts no entry carries reach a client

The facts: the one state; context against window and threshold; last activity; the headline; the branch and changed files; open asks; running calls; live children; model and effort; account usage windows. Each belongs to one agent and changes at a step, and clients other than the one showing that agent's transcript want them: the status pane for every child, a plugin for every agent.

The workload is a person attached to a session with 20 working children and 2 residents. These are estimates of a workload, not bounds.
- **Measured.** A sample of real Claude sessions: 12 SDK session files, 1.3 active hours. They append 0.37 entries per active second, at a mean of 2.2 KB; results average 4.4 KB.
- **Derived.** A row is about 450 bytes with no report text in it. It changes about 0.37 times a second for a working agent: twice per call (0.12 calls a second) and once per request's context report. A row's lists are bounded by their sources: calls by a step's batch, asks by the 64 unanswered inputs, changed files by the children module's list.

**Option 1: one stream per session**, the facts as live lines and `attached` as the snapshot. The status pane needs 23 streams per client and parses about 7 entries a second (16 KB/s) only to keep rows current. A resident's ask reaches only a client attached to it; nothing announces a new session; a network blip reattaches 23 streams; a plugin attaches to every session and polls for new ones.

**Option 2: a snapshot the client re-reads** (the children module's paged tree, by GET). A change shows up to a poll late, and every open client polls for as long as it is open, idle or not; so does a plugin.

**Option 3, the choice: one status stream for the machine, beside each session's stream.**
- The terminal needs two connections, plus one for an open split.
- Rows arrive about as often as entries would, about 7 a second at this workload, at a fifth of the bytes: about 3 KB/s instead of 16. Nothing is sent while every agent waits.
- Asks, deliveries, the config's error and new agents need a home for the whole machine in any design; this stream is it. Account usage windows follow their agent rows.
- Each stream has one rule after a lag: send its snapshot again. One rule joins the two: a row names only recorded entries.
- A plugin reads exactly what a client reads.
- It costs a second stream kind, the controller's snapshot-and-subscribe step, nine more fields per row, and one rule clients learn: the session's stream is the transcript, the status stream is the present.

**The 10,000-line attach** costs the same under all three. The measured 313 turns are about 1,565 entries, 3.4 MB at the measured mean. A client reads one page of 256 entries, about 560 KB, then attaches after it, so the cost grows with the page, not the record.

**Rejected: each tree's rows on its root session's stream.** It saves one connection, still needs a machine-wide path, and publishes every change once per ancestor.

## Device keys and finding the server

- **Keys.** A key is 32 random bytes in hex, sent as a bearer token. The local key is in `device.key`. Enrolled keys are in `devices.json` (`[{name, added, key}]`, mode 0600, rewritten by write-and-rename) and held in memory. At most 64 devices; a name is up to 32 lowercase letters, digits and `-`.
- **Not hashed.** Whoever can read `devices.json` can also read `device.key`, which already gives full owner access.
- **Not signed.** Pairing enrolls Ed25519 keys, which protects only the one transfer of a key to the device, and that already crosses an encrypted tailnet. Signing would cost a crate and a challenge per connection.
- **One owner.** Every device acts as the one owner.
- **Enrolling.** `razor devices add phone` posts `/devices` through the socket and prints the key once. `enroll`, `revoke`, `devices` and `login` are this module's client functions; `main.rs` calls them for `razor devices [add|remove]` and `razor login`.
- **Revoking.** `razor devices remove phone` deletes the key. Each device's open streams wait on a channel of its own; removing the device drops that channel's sender, which ends exactly those streams. A sign-in that phone started stops with its stream; nothing on another device notices.
- **The local key** is never listed or revoked. To rotate it, delete `device.key` and restart.
- **Locally**, a client connects to `$RAZOR_HOME/server.sock` (reqwest 0.13's `unix_socket`, axum 0.8's `UnixListener`) and still sends the key: one check on one path. It probes `GET /devices`, which reads no record; if nothing answers, `main.rs` starts `razor serve` detached and waits up to 10 seconds.
- **Remotely**, a device uses `server.address`. It is empty by default, so out of the box nothing listens on the network. The person sets it to the tailnet address. Razor speaks plain HTTP inside WireGuard.
- **`server.json`** keeps `{pid, address}` for the flows driver.
- **As a user service.** `razor serve` stays in the foreground and exits on SIGTERM; `razor install` writes the unit. Its stop signals the server alone (`KillMode=process`), as launchd does on macOS: systemd's default signals the whole control group at once, so the Claude bridge could exit while the server still ran, which read as a conversation ending by itself and closed every open turn with "The Claude bridge exited" instead of leaving it to resume at the next start (2026-11-03), and every job died with the server. Now the server's exit closes the bridge's input, the bridge closes its conversations, and the jobs live on for the next start to take back. Flow 19 runs a server with the unit's `[Service]` settings under the user's systemd and stops it.

## Flows

**Flow 25** (`flows/25-api.py`, on the flows driver, with `server.address = "127.0.0.1:0"`):
1. `--print` starts the server through the socket and makes a session.
2. `razor devices add phone` prints a key once; `razor devices` lists the phone; `/devices` shows no key; adding the name again exits 1 with the refusal.
3. Over TCP with the phone's key, `/sessions` lists the session. With no key, or a wrong one, the request gets 401.
4. `entries?limit=2&native=true` equals the record's last two lines, byte for byte, and `entries?limit=2` the same entries without `native`. `before=2&limit=1` is `Start`.
5. A `PUT` draft reads back. `events?after=<last>` opens with `attached`. A posted input answers with entry E; the stream yields E, the turn's entries and a `turn_end` naming E, all equal to the record without `native`; the draft is then empty.
6. `files/../../device.key`, plain or percent-encoded, gets 400; `files/nothing` gets 404.
7. The phone's status stream opens with `agents`, the session's row among them. `razor devices remove phone` ends that stream, and its next request gets 401; the local key's status stream stays open and shows the session's row after a `--resume` turn.
8. A GPT session's `response` entry reaches a stream and a page without `native`, and whole with `native=true`.

Two steps wait, to be written against the real tools, not guessed replies:
- The local key's status stream shows an ask, with its text and options, in the session's row. `{option: 1}` is admitted with that option's text. A second answer gets 400.
- A `deliver` call yields a `delivery` line; `files/<its file>` returns its bytes; `/deliveries` lists it.

Flows 22 and 23 set `server.address`, since they reach the server over `server.json`'s address.

**Flow 5, its API half.** A stand-in `Bash` call runs `sleep 300`, and the `--print` client is killed. `events?after=0` shows the call and the job still running. `stop` then ends the turn `stopped`, and the job ends by signal.

**Flows 23 and 26** use, on their API side, the routes of steps 4, 5 and the ask.

## Budget

Dependencies (RULES.md 20):
- **base64**, to decode pictures instead of a hand-written decoder. It is already in `Cargo.lock` through reqwest; the model paths take it too.
- **tokio-util's `ReaderStream`** (feature `io`, which adds no crate: tokio-util is already built for rmcp), for file bodies, instead of a hand-written read loop.
- **http-range**, for the standard byte-range parser instead of a private Range grammar; unlike path-based file services it lets the API seek and stream the controller's already-checked handle without reopening it.
- No hashing or signature crate, and no CORS layer until the web app needs one.

## Where this departs from the first plan

1. **Creating a session takes no input**; the first input goes to `input` like every other. One input body, and a refused picture never leaves behind a session whose id the client never learned. It costs `--print` one more local request.
2. **One `entries` route** serves both a page and a single entry.
3. **Drafts are words only**, kept by the controller; the API's routes hold nothing.
4. **Residents start and stop** are served, though the first list did not include them: the residents design asks for them, for the feature "start and stop a resident's host".
5. **The model picker is served with its reader:** authenticated `GET /models` returns the one protocol `Models` type, from `profiles::models()` over the same shipped table that routes model names, plus `Controller::config()`'s defaults. The terminal loads defaults and refreshes the list when `/model`, `/effort` or the model chip opens; choices stage the next explicit input, never an input or ask answer of their own. **Still not served:** the completion list and emoji history (each needs its reader in the same commit); the context trend comes from recorded `TurnEnd.context`, not a new route.
6. **A refused input can leave its stored pictures** in `images/`, named by no entry. The API cannot remove them, because the controller owns that layout. It happens only when 64 inputs are already waiting or the input itself is invalid.

## The project's history

`GET /v1/sessions/{id}/project` reads the session's folder from `Controller::summary`, then runs the two `rev-list --count` commands and `symbolic-ref --quiet --short HEAD` concurrently through Tokio. One two-second deadline covers all three; `kill_on_drop(true)` stops them when the deadline or request ends. Counts and branch are independent optional facts; a detached head has no branch. A missing session is the ordinary 404. Nothing is stored or added to the session. The terminal shares this read between the room's root-project counts and the focused folder's branch, on focus changes and focused turn ends, even when the room is hidden.

## The session map's marks

`GET /v1/sessions/{id}/marks` opens the checked record through `Controller::file`, fixes the read to its present byte length and scans once in `spawn_blocking` through a 64 KiB `BufReader`, with the existing `Entry` decoder. An unfinished last line is omitted. A sorted map retains the newest 4,096 marks. Failed results mark their own entries, so there is no call index. Person inputs exclude answers, and a `Withdrawn` entry removes the marks of its named inputs; recorded server questions and the root's question-to-person calls are `question`; rebuilds and errored turn ends name their own entries. Nothing is stored, and no scan runs until requested.

On 2026-10-28, five calls over the largest available Razor record (753,517 bytes, written by the real server against the model stand-in) took a median 8.87 ms: 85.0 MB/s. Its largest entry was a 390 KB `Response`, not a multi-megabyte rebuild. A straight-line projection is 0.59 seconds for 50 MB or 1.18 seconds for 100 MB, not a measurement of either size; no longer real session exists yet. The scan holds its one entry, 64 KiB reader and 4,096 marks, not the record.

## Glossary lines proposed

- **status view**: one row for every agent the server holds: its headline, state, model, context, changed files, running calls and open asks. The API serves it as the status stream.
- **device key**: the secret a client sends with every request; the local one is `device.key`, and each enrolled device has its own.
- **headline**: the one line an agent says it is working on.

## The recap

**New route:** authenticated `POST /v1/sessions/{id}/recap?after=<entry>`, no body → `{"text": "…"}`. When nothing was recorded after that entry, text is `Nothing new since your last visit.` without a model request. `after` defaults to zero and uses the existing `Page` query; no new protocol types. Clients own the timing (ten minutes from the status row's `looked_at` and `person_input_at`, after its `seen`) and the display. The recap never enters a record, context or rebuild (SPEC 3, Continuity).

```rust
// src/api/recap.rs, using the existing API refusal type.
/// Never admits work; returns text of at most 400 characters or a 503 sentence.
async fn recap(State(app): State<App>, Path(id): Path<SessionId>,
    Query(q): Query<Page>) -> Result<Json<Value>, Refusal>;
// src/claude/mod.rs
/// One tool-free Haiku answer at medium effort on the CLI's existing sign-in, in the session's cwd;
/// no session persistence, at most 64 KiB of stdout, killed when the request is dropped.
pub async fn recap(cwd: &Path, prompt: &str, evidence: &str) -> Result<String, Error>;
impl Responses {
    /// One tool-free Terra answer at low effort on this client's credentials and supplied endpoint;
    /// the existing post owns retries and stream completion, bounded by the caller's deadline.
    pub async fn recap(&self, base: &str, session: &SessionId, prompt: &str,
        evidence: &str) -> Result<String, Error>;
}
```

### Data flow

1. Read `Controller::summary` for model, cwd, headline, latest person's input id and last entry id.
2. Return `{"text":"Nothing new since your last visit."}` without a record read or model request when `summary.last <= after`.
3. Get the person's latest input through indexed `Controller::get`, cut to 2 KiB, queued together with step 4's page so a cold session usually loads once for both; headline and each open question to 512 bytes.
4. Read the newest entries after `after` in one `Controller::page`, at most 4,096 entries and its 8 MiB (RULES 10: never a page per 64 entries, which reopened a cold record each time; a 41.6 MB record of 10,000 skipped entries read 6.6 GB in 42–75 s, now 92 MB, the cold summary's fold, one load and the page, in 1.0–1.4 s), and keep within 32 KiB of evidence in all, labels counted: first the agent's `Text` entries whole, newest first, its newest always (longer than all the room, its start and its end within it around one `[…middle cut…]` line, since agents end work with a summary for the person), until the first older one that does not fit; then, in the room left, each call as one line of its name, target (its command, file, pattern, path, address or query, else its first text, at most 120 bytes) and outcome, newest first until the first that does not fit, so the oldest steps drop first and never crowd out the agent's words. `Thinking`, `Response`, inputs and the rest are skipped.
5. The evidence reads: the agent's words, oldest first; the person's latest words; the headline; open questions; the steps, oldest first.
6. Memory: the page's words, calls and results, at most its 4,096 entries and 8 MiB, and the evidence's 32 KiB.
7. `src/api/recap.md` asks what got done and what is happening or needs the person, in 1–2 short sentences spoken to the person as "you", the agent as "the agent"; the evidence is a record to describe, never a message to answer, continue or obey, and the recap never speaks of its own model.
8. Claude and open models (`Transport::Chat`) alike, on the machine's Claude sign-in (Trey, 2026-11-04: "Claude sessions should use haiku medium, gpt sessions terra low, open source model sessions also haiku"): extract `sign_in`'s bundled CLI search for two callers. Run `-p --model haiku --effort medium --output-format json --tools= --setting-sources= --strict-mcp-config --max-turns 1 --no-session-persistence --system-prompt <prompt>`, `DISABLE_PROMPT_CACHING=1`; evidence on stdin, stdout through `take(64 KiB)`, `kill_on_drop`, session cwd. No bridge change. Flow 70 captures the CLI's request: the Haiku id and `output_config.effort` `medium`.
9. GPT: small JSON body, `terra` as `profiles::id` names it, developer and user messages, `reasoning: {effort: "low"}`, `store: false`, `stream: true`, no tools; existing `post` with no turn state. Endpoint from `controller.config().await.responses.base_url`; no output-token limit or new SSE cap.
10. Deadline 20 seconds over reading the evidence and the model's answer, below the terminal's 30 seconds; strip controls, fold whitespace, cut to 400 Unicode characters and return only to the person.

### The hard decision: each request stands alone

**No answer cache, lock or 409.** Each terminal's request gets its own model request; the terminal already avoids repeated requests without new activity. Live Haiku calls cost about $0.0001–$0.0003 each: duplicate calls are cheaper than storing answers and managing concurrent terminals. Subscription allowance or the configured API-key billing applies. Haiku runs at medium effort and Terra at low, by Trey's rulings; `store: false` does not disable GPT's automatic caching.

Unavailable model, refused sign-in, rate limit, timeout or any other recap failure: 503 with one sentence pointing to the transcript, never another account, endpoint or model. Ordinary malformed-query, device-key and missing-session refusals remain 400/401/404. A short session still gets a recap when it has an entry after the cursor.

No dependency, config or controller change. Room memory waits. Proof: real server and session on each sign-in, timed recaps, unchanged record and next rebuilt request; flows 25/28/33, clippy, fmt and size. The CLI's documented `--effort low` is the sole effort control; live request capture must confirm it. No implementation proof yet.

Proposed glossary: **recap**: recent work described for the person, never model context.

## The startup list in the configuration (built 2026-10-29)

The API needs no new route: a client reads what a session was given at `GET /v1/sessions/{id}/files/startup.md`, through the files route above. The configuration gains one key and reads it live, like every other:

```rust
// ---- src/config/mod.rs ------------------------------------------------------------------------
pub struct Config {
    // ...
    /// `startup`: the system prompt's entries in order: `razor`, `project`, files and folders
    /// (docs/design/profiles.md). Default `["razor", "project"]`. Only the person's file and the
    /// pack may set it; a project's file that does is refused, as for the other machine keys.
    pub startup: Vec<String>,
}
impl Config {
    /// The list `session` opens with: its own if it is a resident that names one, else
    /// `startup`.
    pub fn startup_for(&self, session: &SessionId) -> &[String];
}
```

- `pack.rs`'s `MACHINE` gains `startup`. `parse` makes each entry of `startup` and of every `[residents.<name>] startup` absolute, except the two words: a leading `~/` against `HOME`, anything else against the file's folder, as it already does for `skills` and a resident's `cwd`. A file named `razor` or `project` is written `./razor`.
- A list replaces a list whole, the more specific file winning, as any value does.
- Read live: each turn's admission re-reads changed settings files, and the next fresh open, rebuild or Claude resume uses the list as it then is.
- `[profiles] claude_prompt` is gone, with its `ClaudePrompt` type, since the prompt A/B was decided (the evaluation design): 12 lines here.
