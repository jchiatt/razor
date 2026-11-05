# Mail across sessions and machines

Module `src/children/` (mail and Post), with small additions to `src/protocol/`, `src/api/`, `src/tools/` and two lines of `src/controller/`.

Trey's words ask for five things beyond what runs today: any Razor agent on any paired server finds another by name, project or server and mails it; mail or a mention wakes its reader and nothing else does; the woken agent reads who wrote what in a few tokens; Post, when it is installed, wakes a Razor agent too; and every root session has a name of its own, as a child does. Parent and child mail, the tree channel, mention-to-wake and the one admission path stay as they are.

## Surface

```rust
// ---- src/protocol --------------------------------------------------------------------------

pub enum Origin {
    // ... every variant it has today; `Mail` gains `from`:
    Mail {
        id: MailId,
        kind: MailKind,
        /// The sender as its reader sees it: a tree member's name, or `<name> in <project>`
        /// from outside the tree, `<name>@<server> in <project>` from a paired server. Empty
        /// on a brief, which renders bare.
        from: String,
    },
}

pub enum MailKind {
    Note,
    Question,
    Answer { question: MailId },
    /// A channel post that names its reader; the channel file holds the post.
    Mention,
}

/// Mail to an agent outside the sender's tree: what `message` hands `receive`, and the body of
/// `POST /v1/mail` when the agent is on a paired server.
pub struct Mail {
    /// The recipient as the sender gave it, without `@server`: a session id, a name, or a project.
    pub to: String,
    /// The sender's session and `message` call; an answer names it as its question.
    pub id: MailId,
    pub kind: MailKind,
    /// The sender's name and project; `receive` makes the origin's `from` of them.
    pub from: String,
    pub project: String,
    pub text: String,
}

pub struct Device {
    pub name: String,
    pub added: Millis,
    pub key: Option<String>,
    /// Set when the device is another Razor server: this server calls it at `http://<address>`
    /// with the same key, and an address names it as `@<name>`.
    pub address: Option<String>,
}

pub struct AgentView {
    // ... every field it has today, and:
    /// The last name of the folder the session started in (a worktree child's: its origin
    /// checkout's), from the fold the row is built from; no file is read for it.
    pub project: String,
}

impl SessionId {
    /// The name a session that is no child answers to: a named id (a resident's) is its own;
    /// a generated id's is two words its four random hex digits draw from `names.txt`, so it
    /// is fixed at creation and stored nowhere. A child's name is its `Child::name`.
    pub fn name(&self) -> String;
}

// ---- src/children --------------------------------------------------------------------------

/// Admits `mail` to the one agent on this server that `mail.to` names, and returns that agent
/// and the input's entry. `server` is the paired device the mail came through; None from here.
/// - Candidates are the status view's agents not in state done, the sender aside, in its
///   tree or out of it. `to` matches a session id or a name; only when none does, the project
///   of a session that is no child.
/// - No candidate, or more than one: refused with one sentence naming up to ten by id;
///   nothing is admitted.
pub async fn receive(controller: &Controller, mail: Mail, server: Option<&str>)
    -> Result<(SessionId, EntryId), Error>;

// `record::heard` (src/record/text/input.rs, its words in `input.toml`) makes the words a model
// reads for an input: mail with a sender gets a one-line header (whose child, parent or channel
// post it is, and a question's id), and a mention shows its first 2,000 characters.

/// The last name of the folder `s` started in, its origin checkout's for a worktree child.
pub fn project(s: &Summary) -> String;

/// Post, when it is installed: the participant each session acts as, and one `post watch`
/// per participant, at most 32. Every call of the `post` program is in this file's two
/// functions, bind and watch, so a change in how Post is called touches one place.
#[derive(Clone)]
pub struct Post { /* home; per session its participant, folder and watch; post.json */ }

impl Post {
    /// Reads `post.json` and binds and watches each session it names again. Starts nothing
    /// when Post is absent, and never fails.
    pub fn open(home: PathBuf) -> Post;

    /// The participant `session`'s jobs act as. When `runs_post` and the session has none,
    /// binds one in `cwd` and starts its watch first. None while unbound, or when `post` is
    /// absent or refuses; never an error.
    pub async fn participant(&self, session: &str, cwd: &Path, runs_post: bool)
        -> Option<String>;
}
```

What the model sees, in `message` (its declarations are profile data):

| type | input | result |
| --- | --- | --- |
| `note`, `question`, `answer` | `to` also takes an address: any agent, in your tree or not, by session id, name or project, with `@<server>` for a paired server | `{id, to}`, `to` naming the session that received it |
| `list` | optional `query` | `{agents: [{to, name, project, state, headline, tree}], more, silent: [server]}`: every agent not done but you, here and on each paired server, newest activity first, `tree` marking your own tree's; with `query`, only those whose id, name, project, repository or headline holds it, in any case; at most 100 per server, `more` counting the rest; a server that does not answer in 2 seconds is in `silent` |

API: `POST /v1/mail` takes a `Mail` and answers `{session, entry}`, or a refusal's status and sentence as every route does. `razor devices add <name> --address <host:port> [--key <key>]` pairs another server: without `--key` it makes one and prints the command that pairs this server on the other side.

## Data flow

1. `message` with a `to` that is not `parent`, a child or `channel` builds a `Mail` from the sender's name and project; its id is the call.
2. Without `@<server>` the mail module calls `receive` here. With one, it uses that device's current enrollment through [Devices](devices.md) and posts the `Mail` to its `POST /v1/mail` with 5 seconds to connect and 5 more to answer; that server runs `receive` with `server` set to the name the key is enrolled under there, and the sender's session id becomes `<session>@<server>` from that name, so it never matches a local parent or child.
3. `receive` takes the status view's snapshot, finds the one candidate and admits the sender's words unchanged through the one admission path, the origin carrying `from`. The sender appends `MailSent` with `to` as `<session>` or `<session>@<server>` and `text` as the message's first line, at most 200 Unicode characters. Mail within the tree records the same preview.
4. Admission is the wake. An idle recipient starts a turn on it; a working one holds it until its turn ends, and every input held by then arrives as one message.
5. When a request is rendered, `rendered` (`src/controller/session/conversation.rs`) sends `record::heard(...)` in place of the bare text: the header and the mention's cut happen there, so the record keeps the sender's words alone and the terminal draws its own row from `from`.
6. `message list` reads this server's snapshot and, at once, the first line of each paired server's `GET /v1/status`, 2 seconds each.
7. `guard::command`, which already finds each command's program after `env`, `sudo`, assignments and paths, also says whether one of them is `post`; `curl -X POST` runs `curl`. Binding is detected from the command's own words, so a `post` inside `$(...)` or backticks is missed: that call gets Post's own "no participant" error, and the next plain `post` command binds. A `bash` call that runs `post` in a session with no participant first runs `post participant bind --harness razor --key <session> --json` in the session's folder and starts `post watch --from now` as that participant; `post.json` gains the session.
8. From then on every `bash` job of the session carries `POST_PARTICIPANT`, so the agent's own `post` commands act as it. Razor loads no Claude Code hooks (`settingSources: []`), so Post's `POST_HOST_PARTICIPANT` has no reader and is not set.
9. Each watch line whose `reason` is `mail` or `mention` becomes one input, `Origin::Program { name: "post" }`, admitted through the controller like any program's input (the controller hands itself to Post with `Post::start` once it exists); `reason: channel` is dropped, and a line over 64 KiB is skipped.
10. At the server's start `Post::start` binds and watches every session in `post.json` again. A watch runs in its own process group, killed with its task, so stopping the server ends every watch; one that exits is logged and starts again at the session's next `post` command; past 32 the oldest is dropped. When a session closes, its watch stops, `post.json` forgets it and its participant ends (`post participant end`); a closed session refuses a ring as it refuses any program's input (controller.md, "Closed sessions stay closed").

## The questions

**Addressing and discovery.** Inside a tree: `parent`, a child's name, or the channel with `@name`; since 2026-11-01 (Open messaging, below) any other agent, in the tree or not, takes the same address. An address is `<who>` or `<who>@<server>`, where `who` is a session id, a name, or a project. A project names only sessions that are no child, so "the agent in repo C" is one session even while it has five children. Without `@<server>` an address is looked up on this server alone, with no network. `@<server>` names a paired server by the name it was paired under, and a second account's server on the same machine is addressed exactly as another machine is: every server pairs with each of the others under a name such as `office`, `laptop` or `laptop-personal`, one pairing per pair. One form is simpler than a realm word beside a machine word: a realm on the same machine is just another server at another address, so it takes the same table, route and failure sentences. A send to two matches is refused, naming each by id; the sender sends again to one. The listing covers only the sessions this server has touched since it started: those it created, reopened, attached, mailed or restarted live, and every resident, which it opens at start. A session nobody has touched since the last restart is not listed and cannot be addressed until something opens it; closed sessions and finished children are left out.

**Root names.** Razor has no name generator to reuse: a child's name is the one its parent gives `agent start` (`src/children/agent.rs:175-179` refuses an empty one), and every root is `main` (`src/controller/mod.rs:503`). The smallest version is `SessionId::name`: two words, an adjective and a noun, drawn by the four random hex digits that end a generated id from two lists of 256 in `src/protocol/names.txt`. It is computed wherever a name is wanted and stored nowhere, so it is fixed at creation, every old session has one, and a resident, whose id is its name, keeps its own. Two listed roots share a name once in 65,536 pairs; the address refusal then names both by id. The status row, the channel's `@name` and the headers all take it, and the child contract drops "The root's name is main." The terminal's top bar would read, on its place line, `copper-heron · razor · main` (name, project, branch); the bar is the terminal's to change, as are the four places it writes `main` itself (`screen.rs:62`, `split.rs:288` and `312`, `strip.rs:183`).

**Reach across machines.** The servers call each other; see the hard decision below.

**Waking.** A turn starts for exactly these: mail to you from your parent, your child or any agent by your address; a channel post naming `@you`; Post mail to your Post participant, and a Post channel post that mentions you. Nothing else is admitted, so nothing else wakes: a channel post naming nobody stays in the channel file for `message read`, and Post channel traffic naming nobody stays in Post for `post chat`. A busy agent's mail waits for its turn to end, except direct mail from its own parent and an answer to a question it asked, which join its turn at the next tool boundary as the person's words do (Trey, 2026-10-29); a channel mention, even the parent's, still waits. The model reads one header, then the words:

```
haiku-whimsy (your child): the parser fix is on its branch; flows 13 and 15 pass.
A child asks: which marker?
haiku-whimsy (your child) reports finished: the parser fix is on its branch, ready to merge.
copper-heron (your parent) answers #41: copper-heron.
oak15 in the channel: @copper-heron the review is ready ...
amber-otter@laptop in razor: I take the GPU from 14:00 to 16:00.
sol-helper on Post in #general: can you check the parser … (post chat general)
```

Direct mail arrives whole: it is already capped at 64 KiB, and its one reader is the agent it was written for. A mention shows its first 2,000 characters, then `... (the rest: message read)`; the record and the channel file keep it whole. A Post input shows Post's own preview (80 characters, cleaned by Post) and the `post` command that reads the rest; Razor never reads a Post body. A child's report that reaches its parent as an input, not through `agent wait`, gets the same kind of header with its status (2026-10-31: until then it rendered bare, and a parent with several children could not tell whose it was). A brief, and every input in a record from before `from` was kept, renders bare.

**Post when installed.** Data flow 7 to 10. A session's Post participant is made in its folder, so it joins the Post workspace registered there and `@<workspace>` mentions reach it. Sending to Post is the agent's own `post` command. When Post is absent the bind cannot start, nothing is watched and Razor says nothing; the agent's `post` command fails in its own shell. One watch per participant is today's Post contract (Trey, 2026-10-29: the cost is fine for now): against a scratch mail root a bind took 3 ms and a watch held 5.7 MB and 6 file descriptors and rang about a second after the send, so 32 watches cost about 180 MB at most. A watch is owned as a job is: its own process group, killed with its task when the server stops, and the flows check that no process of theirs outlives the server.

**Residents.** A resident's session id is its name, so its address is its name: `scribe` on its own server, `scribe@workstation` from another. The server opens every resident that is not stopped at its start, so such a resident is always listed. Its mailbox is its record: mail waits for its turn to end like any input, a heartbeat that finds it busy is still skipped, and mail to a stopped resident still runs, as any input does. Its Post address is its Post participant, made at its first `post` command (Trey, 2026-10-29) and watched again after every restart.

**Trust and failure.** Any agent may mail any listed agent on any paired server. Mail carries words, never authority and never a sign-in: each server keeps its own accounts, and the mail runs under the receiving server's settings. There is no contact step and no block list. Pairing enrolls each server as a device of the other with one shared key, so the key on a request names the server that sent it; the key is a full device key, as the phone's is (Trey, 2026-10-29). So pairing gives the other server the person's full API authority, not mail alone: with the key a paired server can list sessions, enroll a device and post input as the person; Trey approved the full key. Nothing else is checked: one owner, plain HTTP inside WireGuard or on loopback. The sender reads one sentence for each failure: no such agent, or several (named); no server by that name is paired; `laptop did not answer; nothing was sent.` when the connection is refused or not made in 5 seconds, which is what a stopped server and a sleeping laptop give (Trey, 2026-10-29: fail at once, hold nothing), or `laptop did not answer; the mail may not have arrived.` when the answer is lost after connecting; `laptop refused this server's key; pair them again.` (401). A recipient holding 64 unanswered inputs makes mail wait for room, as it does today; across servers that wait ends at the answer's 5 seconds. Nothing is retried and nothing is queued: a send is one request, and a crash before its result leaves the call unknown and never re-run.

**`src/children/message.rs`.** Stays: `mail` (admit to the recipient, then `MailSent` to the sender), the channel's `O_APPEND` file and its 64 KiB read window, `mentions`, and a root's note and question to the person. Changes: `mail` sets `from`; a mention is admitted as `MailKind::Mention` with the whole post; a `to` that is not `parent`, a child or `channel` becomes a `Mail` for `receive` here or on a paired server; `read` gains a sibling, `list`; `heard` renders the headers. Goes: nothing; a direct child is still found first, by `find`.

## Open messaging

Trey agreed on 2026-11-01: "Any agent can message any other agent, on any paired server, by session id or name: parent, children, siblings, other trees alike. The list gets a search instead of a cap. Mail is information, never orders (labeled with its sender); mail from anyone but your parent waits until the recipient's turn ends." Two failures that day prompted it: a sibling addressed by its id was refused as outside the tree, and a child of another tree on a server with over 300 agents was missing from `list`, which showed the newest 100.

`to` now has one rule: `parent`, `channel`, or any agent by session id, name or project, with `@<server>` for a paired one. The sender's own children are looked up first, so a parent's child name never collides with another tree's; everything else goes to `receive`, which no longer sets the sender's tree aside, only the sender. A name two agents share (a sibling and a child of another tree, say) is refused, naming both by id. Nothing else changes: the sender label (`<name> in <project>`, with `@<server>` from a paired server) still comes from the sender's own server, mail is still never authority, a busy agent still holds mail from anyone but its parent (and answers to its own question) until its turn ends, and mail to an agent that has ended is refused. `list` drops its newest-first cut for a search: `query` filters on id, name, project, repository or headline, the answer still holds at most 100 agents per server and says how many more matched, and agents of your own tree are listed with `tree` set. `receive` no longer reads the sender's tree, so a send costs one status snapshot and no record walk.

Flow 43 proves it with two roots, a child named `twin43` under each and a sibling `scout43` beside the first, all older than 101 sessions made after them: `scout43` lists 100 agents with `more` and neither twin, finds both twins with a query (its sibling marked `tree`), mails its sibling and the other tree's twin by id, each note waiting for the busy twin's turn to end, and is refused when it sends to `twin43`, with both ids named. On the build before this change the sibling's mail was refused, the query was ignored, and the shared name went silently to the other tree's twin.

## The hard decision: how mail crosses machines

| | Servers call each other (the lean) | A shared mailbox and a relay | Post as the transport |
| --- | --- | --- | --- |
| Lines in Razor | about 100 beyond mail on one server: one route, a client, pairing | several hundred before the relay: a mailbox writer, an arrival watcher that remembers what it admitted, presence files for listing; Post's relay alone is a 3,735-line Python sweep | the Post part, plus every address routed through Post participants |
| Latency | one request: a paired server on the same private network answers in a few milliseconds | the relay's tick: Post's bridge runs every 15 s, plus the other machine's interval | Post's bridge tick, as left |
| Moving parts | the servers that already run, device keys, `server.address` set on each | a relay on each machine (a timer, a launchd agent), its store, SSH keys | Post, its bridge (Python, Git, OpenSSH) and a relay repository on both machines; a participant per session |
| When the other machine sleeps | the send fails within 5 s, nothing kept | the mail waits and arrives when it wakes, maybe hours late | as the relay |
| Other failures | an answer lost after admission reads as "may not have arrived" | queued is not delivered; a crash between admitting and marking admits twice or never; listings go stale | with Post absent there is no mail between machines at all, against "not as a dependency" |

**The lean: the servers call each other.** Each already lists its agents (`GET /v1/status`), admits input through the one path, and checks a device key on every request; a program's input carries no sender to answer, so mail takes one route of its own. A send is admitted in the other server's record before the call returns, so "sent" means sent. Trey's answers keep this lean whole: a full key, and a sleeping machine fails at once.

**The case against.** A machine can sleep, or run its servers only while someone is logged in, so an agent on another machine cannot leave a note for later for an agent there; a relay would hold it. A held note for an agent that is not running is the outbox queue section 4 cut, and a note delivered hours late to a context that has moved on misleads more than it helps; the sender learns at once and can tell the person instead. The cost that remains is setup: every pair of servers needs a pairing, two commands each, where one relay would serve all.

## Flows

| flow | what it checks |
| --- | --- |
| 13 | Unchanged: the record still holds the child's and parent's bare words, and the request holds them. |
| 15 | Changed: the root's status row names it with its two words, not `main`, and the child mentions it by that name, which the flow reads from a first turn's `status`. |
| 42 | In a tree: a child's note, question and the parent's answer reach the model with their headers while the record keeps the bare words and `from`; a 10,000-character mention reaches it as 2,000 characters and the pointer, whole in the record and the channel file. |
| 43 | One server, roots in two projects: `list` shows the other root with its name and project and neither the sender's tree nor ended children; mail by name arrives as `<name> in <project>: ...` and a reply by project comes back; mail to a working root opens its next turn after `TurnEnd`; a project two roots share is refused with both named. |
| 44 | Two servers paired with `razor devices add --address`: `<name>@<server>` is admitted there with `@<server>` in `from`; with that server stopped, and to a server whose connections never complete (an asleep machine's, made with a full accept queue), a send fails saying nothing was sent, no `MailSent` is recorded, and `list` names both in `silent`. |
| 45 | The real `post` under a scratch `POST_MAIL_ROOT`: the first `post` command binds `razor-...` and a later job sees `POST_PARTICIPANT`; a Post note and a mention each open a turn with Post's preview, and unaddressed channel traffic opens none; after a server restart a note still opens one; no watch outlives the server; with no `post` on `PATH` nothing is bound and nothing errors. |

## Lines

The headers, the Post input and the 512 name words are data files; the tool text is profile data. It needs no relay, no queue, no new store beyond `post.json`, and no new crate.

## Where this departs from the spec

1. **Today's mail reaches the model as bare words.** `rendered` (`src/controller/session/conversation.rs:214-223`) sends `input.text` alone, and `mail` (`src/children/message.rs:168-186`) admits the sender's words unchanged, so a parent of five children cannot tell which wrote. `heard` fixes it at render time; flow 13 keeps passing, since the record still holds the bare words.
2. **The spec's mail is parent and child only.** The glossary's **mail** ("between a parent and a child") and section 4's cut of "cross-session channels, rooms, signing and locks" need Trey's 2026-10-29 words beside them: direct mail now spans sessions, realms and machines, while channels still do not.
3. **"One authenticated API server per machine"** (section 3): a machine can run more than one, so an address names a server, not a machine.
4. **The 2026-10-27 keep list has "residents' Post address and mailbox".** A resident's Post address is made at its first `post` command, not at its first start (Trey agreed, 2026-10-29); its Razor address works from the start.
5. **No server listens on the network by default.** `server.address` is unset; pairing needs it set on each server, one restart each.
6. **"Reuse the children's name generator"**: there is none; children are named by their parents. `SessionId::name` is the generator, and only roots use it.

## Names from the server and the project

Trey: "can we make the automatically assigned names format be structured like this: [project-name]? Will make them easier for me to ID, and y'all, instead of having to remember which random name is working on which thing", then "What if it was [machine shorthand name-project-number if multiple-name]? That would neatly solve it", and on whether a random part stays: "Y'all decide whatever yall like best". The decision was `<machine>-<project>[-<n>]` with no random part: `laptop-razor`, `workstation-billing-2`. This replaces the two words of **Root names** above for every root started from now on; children, residents and older records keep the names they show.

```rust
// ---- src/config: [server] -------------------------------------------------------------------
pub struct Server {
    pub address: Option<String>,
    pub https_url: Option<String>,
    /// The name the other servers pair this one under (`workstation`, `laptop`): the first word of
    /// every root's name. Empty, the default: a root's name starts with its project. A machine
    /// key, read at each root's start, so a change names the next root.
    pub name: String,
}

// ---- src/protocol: Body::Start gains -----------------------------------------------------------
    /// The name Razor gave a root as it started. Empty for a child (its name is its
    /// `Child::name`), for a resident (its id is its name) and in every record written before.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    name: String,

// ---- src/record ------------------------------------------------------------------------------
pub struct Summary {
    // ... every field it has, and:
    /// The name the session answers to, fixed by its Start: a child's own, a root's stored
    /// one, else its id's (`SessionId::name`), so a record from before keeps the name it
    /// showed. Every status row, mail header, mention and lookup reads this one field.
    pub name: String,
}
impl Record {
    /// As before, with the root's name for its Start.
    pub async fn create(home: &Path, session: &SessionId, cwd: PathBuf, model: String,
        effort: Effort, profile_version: String, name: String, child: Option<Child>)
        -> Result<Self, Error>;
}

// ---- src/controller --------------------------------------------------------------------------
impl Controller {
    /// As before; a root Razor starts (no id, no child) is named `<server.name>-<project>`,
    /// or that with the lowest `-2`, `-3` … no listed agent on this server is named, stored in
    /// its Start. Two roots starting at once never take one name.
    pub async fn create(&self, cwd: PathBuf, model: Option<String>, effort: Option<Effort>,
        id: Option<SessionId>, child: Option<Child>) -> Result<SessionId, Error>;
}

// ---- src/children: `name(s: &Summary)` goes, its two callers reading `Summary::name`; in its
// place, beside `project`, the rule itself, which `create` calls with the rows' names in hand:
/// The name Razor gives a root starting in `cwd` on the server `machine` names:
/// `<machine>-<project>`, the project being `cwd`'s last name, each run of characters a mention
/// splits a word on (all but ASCII letters, digits and `_`) made one `-`; then the first of it,
/// `-2`, `-3` … not in `taken`. Empty when both parts are, so the record's fallback names it.
pub fn name(machine: &str, cwd: &Path, taken: &[&str]) -> String;
// ---- src/protocol: `SessionId::name` stays, now only for a Start with no name. ---------------
```

**Data flow.**

1. Every root starts at `POST /v1/sessions`, which calls `Controller::create` with no id and no child: the terminal, `--print` and the web app alike.
2. `create` takes the controller's naming lock, reloads the settings as it does now, and reads `server.name` from them.
3. It reads every root's Start line and its record's change time with `record::list`, the same scan `--continue` and `GET /v1/sessions` make. Then, under the status view's lock, it makes `<machine>-<project>` and takes the first form held by no row and by no root whose record changed in the last week. The rows are every agent this server has listed since it started, in any state, children and residents included, so a session's name is not handed out again while it shows or while it is likely to be reopened.
4. `Record::create` writes the name into Start. A child's or a resident's Start has none.
5. The session's task opens and folds Start into `Summary::name`: a child's own name, the stored one, or, with neither, `SessionId::name`, which is a resident's id or an older root's two words.
6. The task publishes the row with that name, and `create` reads the summary back through the task, so the row is listed before the naming lock is let go.
7. Every lookup reads the row's or the summary's name, as it read the derived name before: `receive` (where an exact session id or name now wins, and a project counts only when nothing matches exactly, so mail to `alpha` reaches the root named `alpha` even while `alpha-2` shares its project), mentions, the child-name check against the tree, `message list`, the sender in mail headers, `GET /v1/status` and the terminal's rows. The startup facts, which said `main` for every root, say the name too.
8. After a restart, `open` folds the same Start, so the name comes back unchanged. A record with no stored name derives the two words it always showed.

**Two servers that each have a `razor` session.** With `server.name` set, they are `workstation-razor` and `laptop-razor`, and nothing is ambiguous. Without it, both are `razor`. An address without `@server` is looked up on the sender's own server only, so `razor` reaches the local one and `razor@laptop` the laptop's. A header from the laptop reads `razor@laptop in razor`, and `message list` gives each row its `to` with `@<server>`. Both machines read the same pack, so `server.name` goes in each home config beside `address`, and it is the machine's shorthand, as Trey said it. Two servers on one machine have separate lists, and an address across them carries the other's pairing name, such as `@workstation-personal`, so a `workstation-razor` in each never meets the other.

**Recurrence.** A name is unique among the agents a server lists and the roots whose records changed in the last week. After a restart nothing idle is listed (**Addressing and discovery** above), so the disk is what keeps a reopened session from showing a name a newer root took. The week is the rule: never reusing a name would make a busy project's numbers climb for good, against Trey's "easier to ID", while a session idle for a week is rarely reopened and is still exact by its id. A session closed with `/done` keeps its name for that week too, since Start alone cannot say a session is closed. The scan reads only first lines and change times. On a copy of a server's 116 records it took 2.4 ms in a release build (6.3 ms in debug), and 26.5 ms over 1,000; a root's create took 0.9 ms before it.

### The hard decision: where the name is chosen, and how a number is picked without a race

| | Chosen at creation in the controller, stored in Start (the choice) | Derived when read, stored nowhere (today's way) |
| --- | --- | --- |
| Stability | fixed for the session's life and across restarts | a number derived from rank among same-project sessions moves when one before it closes, restarts or is forgotten, so an agent's name changes under its correspondents |
| Old records | keep their two words through the fallback | keep them |
| Machine part | read from config once, at start | a changed `server.name` renames every session at once |
| Cost | the name in Start, one field in the fold, a lock held across one create | none stored, but every read needs the server's whole list to rank against |

The number needs the server's list, and only the controller holds it, since the status view is private to it. A derived number would rename sessions as others come and go, which defeats the point of a name Trey can learn. So the name is chosen in `Controller::create` and written into Start, the one entry every reader already folds.

**The race.** Two `POST /v1/sessions` calls can both see `workstation-razor` free before either row is listed, since the row appears only after the record is created and its task has loaded. Three ways to prevent it:

- Reserve the name in the status view before writing the record. That needs a row with no record behind it, sent to every client, or a second set of pending names, cleared on every failure path.
- Re-check after the record is written. By then the name is in Start, and Start is never rewritten.
- Hold one async lock from the choice to the listed row. This is the choice. It is a `tokio::sync::Mutex<()>` on the controller, the same shape as `Children::starts`, which holds a child's name check to its record. It is held for every create, roots, children and residents alike, so there is no condition to get wrong. A create takes a few directory and file operations, and starts are rare and come from a person or a parent, so serializing them costs nothing anyone sees. A child's start holds `starts`, then this lock inside `create`; nothing takes them in the other order, so they cannot deadlock.

Eight roots created at once in one folder on a scratch server took `workstation-razor` through `workstation-razor-6` and kept them across a restart; with the lock taken out, three runs of the same check gave five or six roots the same name. After a restart with nothing listed, two new roots beside four on disk, one of whose records was eight days old, took that one's `workstation-razor-3` and then `workstation-razor-5`; with the scan's names left out, they took `workstation-razor` and `workstation-razor-2`.
