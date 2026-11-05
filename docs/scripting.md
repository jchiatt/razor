# Using Razor from scripts

You can use Razor without the terminal app. `razor --print` sends one message and prints the answer, which suits scripts, cron jobs and other programs. Anything it does not cover, your own programs can do through the local API, the same one the terminal app and every other client use.

## One answer with `razor --print`

`razor --print "message"` starts a new session in the current folder, sends the message, waits for the turn it starts to end, and prints the reply:

```sh
cd ~/code/your-project
razor --print "List the TODO comments in this repository."
```

With no message argument it reads the message from standard input, so you can pipe text in:

```sh
{ echo "Review this diff for bugs:"; git diff; } | razor --print
```

The reply is the text the agent wrote after its last tool call, its closing words rather than its working notes. The message is one argument, so quote it; an empty message is refused. `--print` alone is the same as `--print=text`; the other two formats are [below](#json-and-streamed-output).

The exit code says how it went:

| Code | Meaning |
| --- | --- |
| 0 | The turn finished; the reply is on standard output. |
| 1 | The turn failed or was stopped, or Razor could not run it; the reason is one sentence on standard error, such as `Claude is not signed in; run razor setup.` |
| 2 | The command line was wrong, such as an unknown flag, two message words, or `--resume` with `--continue`; the sentence ends with `Run razor --help.` |

`--print` starts the server if none is running, as every `razor` command that needs one does.

## Keeping a conversation across calls

Each plain `--print` starts a fresh session. To carry on a conversation, send the next message to the same session:

- `--continue` sends it to the session most recently started in the current folder. With none there, it fails with `There is no session in this directory to continue.`
- `--resume <id>` sends it to the session with that id, wherever it started. The id is in `--print=json`'s output and in `GET /v1/sessions` below.

```sh
id=$(razor --print=json "Read the README and wait." | jq -r .session)
razor --print --resume "$id" "Now list its install steps."
```

`--model <name>`, `--effort <low|medium|high|xhigh|max>` and `--speed <standard|fast|ultrafast>` choose how a new session runs; without them it gets your settings' `model`, `effort` and `speed` ([Configuration](configuration.md#models-and-sessions)). Given with `--continue` or `--resume`, they switch the existing session from this message on. A switch keeps the conversation when the new model can continue it, such as Claude to Claude, and otherwise starts the new model from a continuation note ([Concepts](concepts.md#when-the-context-fills)).

```sh
razor --print --model sonnet --effort low "Which files changed most this month?"
```

## JSON and streamed output

`--print=json` prints one line once the turn ends, with the session's id and the reply. When the turn fails, the line holds `error` instead of `message`, the same sentence also goes to standard error, and the exit code is 1. A missing sign-in is such a failure, whether or not `--model` names the model: Razor makes the session and its turn fails, so you still get the session's id. A failure before any session exists, such as no server starting, prints no JSON line, only the sentence on standard error, also with exit code 1.

```text
{"session": "20261104-211641-6cae", "message": "The reply."}
{"error": "Claude is not signed in; run razor setup.", "session": "20261104-211641-6cae"}
```

`--print=stream` prints the session as it happens, one JSON object a line, and stops after the line that ends the turn. Every line has a `kind`. The first is `attached`; then come the record's entries as they are written, each with its `id`: the `input` you sent, the agent's `text`, each tool `call` and its `result`, and finally `turn_end`, whose `end` is `"done"`, `"stopped"` or `{"error": "<sentence>"}`. Between them come lines that are never recorded: `delta` (reply text as it streams), `thought` (a thinking summary as it streams, numbered by `n`, `start` on its first words; one with `void` drops the unfinished thoughts through its `n`, before a cut response is tried again), `taken` (a waiting message reached the agent), `pending` (the messages still waiting) and `tail` (a running command's latest output). A turn that failed for want of a sign-in looks like this:

```text
{"kind":"attached","session":"20261104-211641-6cae","last":6,"pending":[]}
{"id":6,"at":1793827016333,"kind":"input","from":"person","text":"Stream it."}
{"id":7,"at":1793827016507,"kind":"turn_end","inputs":[6],"end":{"error":"Claude is not signed in; run razor setup."},"usage":{"input":0,"output":0,"cache_read":0,"cache_write":0,"requests":0,"waited":0},"context":null,"threshold":0.8}
```

## What a headless session is

A `--print` session is an ordinary session that no terminal app happens to show. The server runs it, so:

- it works the way any session does: it reads and edits files, runs commands, starts helpers, uses the project's memory, and writes everything to its record;
- killing the script stops only the printing; the turn goes on in the server;
- it appears in the agents list, and `razor --resume <id>` opens it in the terminal app, where you can read what it did and keep talking;
- Razor never pauses a turn to ask permission for a command, so a headless turn runs to its end on its own; the two command guards still apply ([Security](security.md#what-agents-can-run)).

A question the agent asks you does not hold the turn open. The agent asks and usually ends its turn there, so `--print` prints what it said and exits. Your answer, from the terminal app or the API, starts the session's next turn.

## The local API

Every client reaches the server through its API, and so can your programs. On this machine it listens on a socket in Razor's home, `~/.razor/server.sock`, and every request carries the local key from `~/.razor/device.key`, readable only by you. Requests and answers are JSON; streams are JSON lines. This shell function wraps curl for the examples on these pages:

```sh
razor_api() { curl -sS --fail-with-body --unix-socket ~/.razor/server.sock -H "Authorization: Bearer $(cat ~/.razor/device.key)" "$@"; echo; }

razor_api 'http://localhost/v1/sessions?limit=5'
razor_api -X POST -H 'content-type: application/json' -d "{\"cwd\": \"$PWD\"}" http://localhost/v1/sessions
razor_api -X POST -H 'content-type: application/json' -d '{"text": "Run the nightly checks.", "from": "nightly"}' http://localhost/v1/sessions/<id>/input
curl -sS -N --unix-socket ~/.razor/server.sock -H "Authorization: Bearer $(cat ~/.razor/device.key)" 'http://localhost/v1/sessions/<id>/events?after=0'
```

With `RAZOR_HOME` set, both files are in that folder instead. The API does not start a server; if nothing answers, `razor restart` starts one. A refusal comes back with an HTTP status (400 for a request Razor will not take, 401 for a missing or revoked key, 404 for an unknown session or route, 409 when a session is busy or full) and `{"error": "<one sentence>"}`.

The routes a script needs:

| Request | Body | Answer |
| --- | --- | --- |
| `GET /v1/sessions?cwd=<folder>&limit=<n>` | | Top-level sessions, most recently started first, each with its `session` id, `cwd`, `model`, `effort`, `name` and `title` (your first message's first line). 50 unless `limit` says otherwise, at most 1,000. |
| `POST /v1/sessions` | `{"cwd": "<folder>"}`, and optionally `model`, `effort`, `speed` | `{"session": "<id>", "name": "<name>"}`: a new session, idle until its first input. |
| `POST /v1/sessions/<id>/input` | `{"text": "…"}`, and optionally `model`, `effort`, `speed`, `from`, `request`, `images` | `{"entry": <n>}` once the message is recorded. A message sent mid-turn reaches the agent before its next step. |
| `GET /v1/sessions/<id>/events?after=<entry>` | | The session as JSON lines: `attached`, then every entry after that id, then each new line as it happens, until you close the connection. `--print=stream` prints exactly this. Entries leave out `native`, the model's own replay data; add `&native=true` for it. |
| `GET /v1/sessions/<id>/entries?before=<entry>&limit=<n>` | | One page of the record as JSON lines, oldest first, ending before that entry (or at the end); 256 entries unless `limit` says otherwise, at most 1,024. `&native=true` as on `events`. |
| `POST /v1/sessions/<id>/asks/<entry>` | `{"option": <n>}`, counting from 0, or `{"text": "…"}` | `{"entry": <n>}`: the answer, recorded as the session's next input. Refused once the question has an answer. |
| `POST /v1/sessions/<id>/stop` | | `{}`: what Escape does. The turn and the command it is running stop; background commands and helpers keep going. |
| `DELETE /v1/sessions/<id>/input/<entry>` | | `{}`: withdraws a message the agent has not taken yet. |
| `POST /v1/sessions/<id>/done` | | `{"entry": <n>}`: what `/done` does. The closing turn runs on in the server, and the session closes when it ends. |
| `POST /v1/sessions/<id>/quit` | | `{}`: what `/end` does. The session closes now, with no closing turn. |
| `PUT /v1/sessions/<id>/name` | the new name, as plain text | `{}`. |
| `GET /v1/sessions/<id>/files/<name>` | | A file in the session's folder, such as one an agent delivered, a command's saved output, or `startup.md`. |
| `GET /v1/status` | | JSON lines: first `agents`, a row for every open session with its `state` (`working`, `needs_you`, `done` or `stuck`), `headline` and open questions (`asks`), then a replacement row each time one changes. |
| `GET /v1/deliveries?limit=<n>` | | Your inbox, newest first: the files, links and notes agents sent you. 50 unless `limit` says otherwise, at most 256. |
| `GET /v1/search?q=<words>` | | Past conversations holding every word, here and on paired servers ([Concepts](concepts.md#the-record)). |
| `GET /v1/models` | | The models Razor can run, each one's efforts, and your defaults. |

Three optional fields of an input are for programs:

- `from` names the program that sent it, such as `"nightly"`. The record then shows the input came from that program, not from you, and only your own words can become [rulings](memory.md#how-your-decisions-become-rulings).
- `request` is your own id for the message, 1 to 64 letters, digits and hyphens. Sending the same id again within the session's newest 64 answers with the first message's entry and sends nothing, so a script can retry safely.
- `images` is a list of pictures as plain base64, JPEG, PNG or WebP, at most 20 to a message and 10 MB of base64 each.

To answer a question, find it in `GET /v1/status`: each row's `asks` lists its open questions with their `text`, `options` and `id`, which is the session and the entry to put in the `asks` route. The always-on agents' routes are on [Always-on agents and schedules](always-on-agents.md#controlling-it-from-a-script), and the voice settings' on [Browser, voice and search](browser-voice-search.md#spoken-notes-from-agents). Every other route, the ones the terminal app and the phone client use, is described as it was designed in the [API design page](design/api.md).

## From another machine or program

A program on this machine can use the socket and the local key, as above. To give a program a key of its own, which you can take back without touching anything else, enroll it:

```sh
razor devices add nightly
```

```text
nightly is enrolled. Its key, shown only now:
0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
```

The name is 1 to 32 lowercase letters, digits and hyphens. Keep the key like a password: it can do anything the terminal app can, sign-ins and settings included. The program can use it on the socket, and, once you give the server an address, from another machine over HTTP. Set `[server] address` to this machine's address on a private network such as Tailscale, never `0.0.0.0` or a public address, run `razor restart`, and send the key on every request ([More machines](machines.md#2-give-each-server-an-address)):

```sh
curl -sS -H "Authorization: Bearer $KEY" 'http://100.101.102.103:7433/v1/sessions?limit=5'
```

The traffic is plain HTTP, so the private network is what keeps it secret. `razor devices` lists every enrolled program and paired server, and `razor devices remove nightly` revokes the key at once: its next request gets 401, and its open streams end.
