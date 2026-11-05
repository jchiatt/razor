# Configuration

Razor's settings are TOML files that it rereads when you save them; a change applies at the next turn, with the few exceptions below. Most people only ever edit one file, `~/.razor/config.toml`. API keys go in a separate file, `keys.toml`, that only you can read, and sign-ins are kept by each provider's own login.

## Where settings live

Razor reads up to four files. Each later one wins over the ones before it:

| File | What it is for |
| --- | --- |
| The pack's `config.toml` | A pack is a folder of default settings, prompt files and skills. Razor ships one, the starter pack, which a new install uses. Its file, `packs/starter/config.toml` in Razor's install folder, holds worked examples of the common optional settings as commented lines. Don't edit it: an upgrade replaces it. |
| `~/.razor/config.toml` | Your settings. With `RAZOR_HOME` set, it is `$RAZOR_HOME/config.toml`. |
| `<project>/.razor/config.toml` | A project's shared settings, at the root of its git repository (or the folder the session started in, outside git). |
| `<project>/.razor/config.local.toml` | The same project's private settings for this machine. It sits in the checkout, and an MCP server's `env` values are stored in it as written. When Razor creates it in a git checkout, as `razor mcp add` does, it adds the file once to the main checkout's `.git/info/exclude`, so it stays out of git without a change to any tracked file; Razor never edits `.gitignore`. If you create the file yourself, add it to `.git/info/exclude` or `.gitignore`. |

A helper working in its own worktree reads the project files of the main checkout. A folder whose `.razor` is Razor's home, such as your home folder, has no project files: its `.razor/config.toml` is your own file, read once.

## How the files combine

1. A plain value replaces the one beneath it: `model = "sonnet"` in your file wins over the pack's.
2. A table merges one level deep: `[children] max` in your file keeps the pack's `[children] grace`.
3. An entry with a name of its own, such as `[mcp.files]` or `[residents.scout]`, comes whole from the last file that has it. So does any table two levels down, such as `[profiles.codex]`: yours replaces the pack's whole.
4. A list replaces the list beneath it and is never joined to it.

## Paths in settings

A setting that names a file or folder (`pack`, `skills`, `startup` entries, a variant, key or secret file, a resident's `cwd`) counts a relative path from the folder of the file that names it, and `~/` from your home folder. So to change the starter pack's `startup` list, copy the whole list into your file and copy the files it names (`persona.md`, `rules.md`) beside your file. Two exceptions: a program in `ask`, `notify`, `speech`, `transcribe`, `capture` or `play` named without a `/` is looked up on `PATH`, and a plugin's relative `file` counts from Razor's home.

Commands are not rebased. An MCP server's `command` starts in the project's folder, so give it an absolute path or a program on `PATH`; a scripted tool's `command` is run by bash in the session's folder.

## What only your own file may set

Settings about you or the machine may be set only in your own file, `~/.razor/config.toml`, or in a pack, never in a project's files. A project file that sets one is ignored whole, and Razor keeps that file's last good settings, with a message naming the file and the keys, such as `/home/you/code/app/.razor/config.toml cannot set ui; move these keys to the person's file.` `razor mcp` and `/mcp` refuse to change such a file and show that message; nothing else shows it yet. These are the settings:

- the server and models: `server`, `models`, `claude`, `responses.base_url`, the whole `[sessions]` table, `call_model`, `call_effort`;
- what conversations start with: `startup`, `children.startup`, `skills`, `pack`;
- programs Razor runs for you: `ask`, `notify`, `speech`, `transcribe`, `capture`, `play`, `plugins`;
- safety: `guards`;
- always-on agents: `residents`;
- display: `brand`, `ui`;
- `phone`, `push` and `live`, which serve a separately distributed phone client (`live` sets where its voice calls go: OpenAI by default, or `url`, and `names`, the names its voice will hear).

`[tasks]` is the other way round: only a project's `.razor/config.local.toml` may set it ([Tasks](#tasks)).

## When a change applies

- Most changes apply at the next turn.
- `server.address` and `responses.base_url` apply when the server restarts, and Razor says so when you change them ([Troubleshooting](troubleshooting.md#restarting-the-server) says how to restart).
- `keys.toml` is read when the server starts, so run `razor restart` after `razor keys`.
- A file that does not parse, or holds a wrong value, is ignored, and Razor keeps that file's last good settings. For your file and the pack's, the terminal app's status pane and `server.log` name the file and the problem.
- A server whose own file or pack is broken does not start, and `server.log` says why.
- A settings file is at most 1 MiB.

## Keys

API keys are read from the server's environment. `razor keys` asks for the optional ones and saves them in `~/.razor/keys.toml`, readable only by you, and the server fills any of them its environment lacks from that file when it starts; an exported variable wins. `keys.toml` holds only these:

| Variable | Used for |
| --- | --- |
| `EXA_API_KEY`, `PARALLEL_API_KEY`, `FIRECRAWL_API_KEY` | The web tools `search`, `fetch` and `answer` ([Browser, voice and search](browser-voice-search.md)) |
| `ELEVENLABS_API_KEY` | The bundled speech program |

Each name is optional, and the file is readable only by you (`chmod 600 ~/.razor/keys.toml` if you write it yourself). Any other name, or an empty value, stops the server and `razor doctor` from starting, with a sentence naming the four it may hold:

```toml
# ~/.razor/keys.toml
EXA_API_KEY = "your-exa-key"
PARALLEL_API_KEY = "your-parallel-key"
FIRECRAWL_API_KEY = "your-firecrawl-key"
ELEVENLABS_API_KEY = "your-elevenlabs-key"
```

One more key lives in its own file: `~/.razor/key` holds an OpenAI API key, which Razor uses when `[responses] base_url` points at `api.openai.com` (see [Model routes](#model-routes)), and for the phone client's voice calls. Model sign-ins are kept by each provider's own login, and other models' keys come from the server's environment ([Models and sign-in](models.md#other-model-families)).

## Terminal keys

Every terminal keyboard action can be changed in your own `config.toml`. Tables name the context, and each action has a list of keys:

```toml
[ui.keys.global]
prefix = ["Ctrl-B"]
status = ["Ctrl-Y"]

[ui.keys.list]
up = ["k", "Up"]
down = ["j", "Down"]
```

Missing actions keep their defaults; `[]` unbinds an action. The prefix is unset by default. After setting it, prefix then n/p opens the next/previous root in the same project, as a click on it does, and prefix then h/j/k/l navigates the agents list. Prefix n/p leaves typing in the composer.

Each table is a context, the place its keys apply: `global`, `transcript`, `list` (the agents list), `prefix`, `composer`, `line_start` (the start of a composer line), `targets` (selecting click targets), `children`, `input_history` (earlier inputs), `queued_input` (withdrawing queued input), `attached_images` (entering the image selection), `question`, `voice`, `picker`, `panel`, `mcp`, `start`, `emoji`, `completion`, `shortcode`, `attachments` (while selecting an image), `editor` and `skill`.

Run `/keys` for every action, its context and effective keys. The [shipped catalogue](https://github.com/treygoff24/razor/blob/main/src/terminal/data/keys.toml) supplies the defaults and descriptions in the same format. Keys accept `Ctrl-`, `Alt-`, `Shift-` and `Super-`, named keys such as `Enter`, `Esc`, `Space`, `Left`, `PageUp` and `F2`, or a single character. Each action accepts up to eight keys. Changes apply while the terminal is open, and hints update with them. An invalid name, key or duplicate within a context shows an error and keeps that terminal's last valid bindings.

## The file about you

`~/.razor/about.md` is plain Markdown that every conversation starts with, through the `about` part of the `startup` list. Razor does not make one: write it yourself, or ask an agent to, with a first line `# About <your name>`, then a few lines about you.

```markdown
# About Sam

I build web apps, mostly in TypeScript, and I prefer small pull requests.
```

## Every setting

Durations are a number and a unit: `90s`, `3m`, `2h`, `1d`. Times of day are `HH:MM` in local time.

### Models and sessions

| Setting | Default | What it does |
| --- | --- | --- |
| `model` | `"opus"` | The model new sessions start on ([Models and sign-in](models.md)). |
| `speed` | `"standard"` | New root sessions use `standard`, `fast` or `ultrafast`; `razor --speed` overrides it. Children start at Standard unless explicitly given a speed. `/fast` opens the speed row; `/fast on`, `/fast off` and `/fast ultrafast` stage the next input. The picker shows prices and unavailable reasons. |
| `effort` | `"high"` | How much work the model puts into each step in new sessions: `low`, `medium`, `high`, `xhigh` or `max`. |
| `call_model` | `"terra"` | The model a session started for a voice call runs on, such as the phone app's Talk; any model Razor can run. A call to a conversation already under way keeps that conversation's model. |
| `call_effort` | `"low"` | The effort of a session started for a voice call. The phone app can choose its own model and effort for calls, which win over these two. |
| `[rebuild] threshold` | `0.8` | How full a context may get, as a fraction of its size, before Razor asks for a continuation note and starts a fresh one ([Concepts](concepts.md#when-the-context-fills)). |
| `[sessions] handover_minutes` | `50` | Idle minutes before requesting a continuation note on every model route, residents included. Running turns and jobs hold it off; working children do not. `0` disables it. |
| `[sessions] handover_tokens` | `50000` | Minimum context size for an idle handover, for asking whether a model switch keeps the conversation, and for a switch's fresh note; `0` removes the floor. |
| `[sessions] close_minutes` | `0` | Idle minutes before a top-level conversation closes without a model turn. Residents' home sessions, children and sessions with pending work stay open. The next person input resumes the same conversation. |
| `[sessions] idle` | `4` | How many idle sessions keep their model conversation open. On Claude each one holds a Claude Code process; the longest idle closes first and reopens at its next input. |
| `[stuck] after` | `"15m"` | How long a working session may sit with unanswered input, no command running, no question open, no `agent wait` within its timeout and no working helper before it shows as stuck. |

### Model routes

These change how Razor reaches Claude and GPT; most people never set them. `[profiles.claude]` covers the Claude models and `[profiles.codex]` the GPT models, named after Codex, whose tools GPT is shown.

| Setting | Default | What it does |
| --- | --- | --- |
| `[profiles.claude] code_mode` | `false` | Code mode: instead of separate tools, the model gets one `exec` tool and writes JavaScript that calls Razor's tools, which lets it batch many reads in one step. This turns it on for Claude. |
| `[profiles.codex] code_mode` | `true` on the ChatGPT sign-in, `false` at `api.openai.com` | The same for GPT models. It cannot be turned on at `api.openai.com`. |
| `[profiles.codex] image_gen` | `false` | Gives GPT on the ChatGPT sign-in Codex's image generation, which makes and edits images on your subscription. Off, its 2,500-byte description is left out of every GPT conversation. It takes effect at the next turn; a project may set it. |
| `[responses] base_url` | `"https://chatgpt.com/backend-api/codex"` | Where GPT requests go. `"https://api.openai.com/v1"` bills an OpenAI API key instead of your ChatGPT sign-in: the key alone in the file `~/.razor/key`, which is not part of `keys.toml`. Needs a restart. |
| `[claude] sign_in` | `[]` | A command and its arguments that print a Claude sign-in token. Razor runs it for each conversation it opens on Claude, so a script of yours can pick the account. Empty: Claude Code's own sign-in. |
| `[claude] usage_file` | none | A JSON file that another program keeps with each Claude account's plan usage; Razor reads it to show the usage of the account `sign_in` picked. |

### Models

`[models."<name>"]` adds a model or changes a shipped one. For Claude and GPT only `window` and `threshold` may be set; the rest are for chat endpoints.

| Setting | Default | What it does |
| --- | --- | --- |
| `base_url` | the shipped row's, else required | The OpenAI-compatible endpoint; Razor posts to `<base_url>/chat/completions`. |
| `key` | the shipped row's, else none | The name of the environment variable holding the endpoint's key. Never the key itself. |
| `id` | the shipped row's, else the row's name | The model id the endpoint expects. |
| `speed_ids` | empty | Chat model variants, for example `speed_ids = { fast = "accounts/fireworks/routers/kimi-k3-fast" }`. The base URL, credentials, profile, effort and context limits stay the same. Missing variants are unavailable. Gemini Fast uses its priority tier directly; a conflicting `request.service_tier` is refused. |
| `window` | the shipped or reported size | The context size in tokens. |
| `threshold` | `[rebuild] threshold` | This model's own rebuild threshold. |
| `replay` | `"streamed"` | How the model's earlier reasoning is sent back with the next request: `streamed` as it arrived, `reasoning` or `reasoning_content` in the field of that name, which some hosts require, or `none`. |
| `request` | none | Extra fields sent with every request, such as an option only that host has. |
| `variant` | the shipped row's, else four plain tools | A JSON file describing the tools this model is shown, in place of the default. |

### The system prompt and skills

These decide what every conversation is told before your first message, and which skills, folders of written instructions, its agent can load.

| Setting | Default | What it does |
| --- | --- | --- |
| `startup` | `["razor", "project"]`; the starter pack's is `["persona.md", "razor", "about", "project", "rules.md"]` | The parts of a top-level conversation's system prompt, in order. Three words are not files: `razor` is Razor's own text about its tools, `project` is the `AGENTS.md` and `CLAUDE.md` files from the repository root down to the session's folder, and `about` is `~/.razor/about.md`. Anything else is a file, or a folder whose first 64 `.md` files are read. |
| `skills` | `[]` | More folders of skills: written instructions an agent loads by name, one folder per skill with a `SKILL.md`. These are searched after the pack's `skills` folder and the project's `.agents/skills` and `.claude/skills`. |
| `pack` | the starter pack | The pack folder. Naming one replaces the starter pack; removing the name brings it back. |

The opening lists the pack's and project's skills with descriptions, up to 100. Additional `skills` folders remain available by name and through slash-command search.

### Helpers (`[children]`)

An agent can start a helper as a reviewer or a researcher, which gives it Razor's instructions for that role.

| Setting | Default | What it does |
| --- | --- | --- |
| `[children] startup` | `["project"]` | The system prompt for every helper, including a resident's helper, using the same entries as `startup`. Independent of the parent's list; `[]` sends only the model route's mandatory text. Only the pack and your own file may set it. A remote helper uses its destination server's list and files. Read again at a fresh opening or rebuild; a native resume retains its recorded prompt. |
| `[children] max` | `20` | How many working helpers one session may have at once. |
| `[children] grace` | `"3m"` | How long a helper that stopped without reporting is given before Razor asks it once to finish. |
| `[children] reviewer` | none | A file whose text replaces Razor's instructions for a helper started as a reviewer. |
| `[children] researcher` | none | The same for a helper started as a researcher. |
| `[children] watcher` | `"gemini-3.8-flash"` | The model of the watcher: a helper Razor starts by itself when a client sends a video (the terminal app does not) to a session whose model cannot watch video, and which reports a description and a timeline of key moments. Its key goes in the server's environment like any other model's ([Models and sign-in](models.md#other-model-families)); for the default that is `GEMINI_API_KEY`. Without the key no watcher starts, and the session gets only the video's file path. |

### Guards

| Setting | Default | What it does |
| --- | --- | --- |
| `[guards] delete` | `true` | Refuses an agent's command or patch that would delete a file that existed before its session started. A check of the command's text, not a sandbox ([Security](security.md#what-agents-can-run)). |

### Notifications

| Setting | Default | What it does |
| --- | --- | --- |
| `notify` | `[]`; the starter pack's is Razor's `scripts/notify.sh` | A program and its arguments that carry an agent's note to you; the note is its last argument. |
| `ask` | `[]`; the starter pack's is Razor's `scripts/notify.sh` | A program that carries a question to you, as its last argument. If it prints something and exits 0, that is the answer; if it prints nothing, the question waits for you in a client. |

The starter pack's `notify.sh` shows a desktop notification through `osascript` on macOS or `notify-send` on Linux, and does nothing on a machine with neither. On macOS the notifications come from Script Editor, which you may have to allow once in System Settings. To reach your phone instead, install the [ntfy](https://ntfy.sh) app, subscribe to a topic no one could guess, and put it in both lines:

```toml
# ~/.razor/config.toml
notify = ["curl", "-fsS", "-o", "/dev/null", "https://ntfy.sh/your-unguessable-topic", "--data-raw"]
ask = ["curl", "-fsS", "-o", "/dev/null", "https://ntfy.sh/your-unguessable-topic", "--data-raw"]
```

`-o /dev/null` matters: without it, ntfy's reply would become the question's answer.

### Voice

| Setting | Default | What it does |
| --- | --- | --- |
| `speech` | `[]`; the starter pack's is `scripts/speech.py` | A program that reads a reply's text on standard input and writes MP3 audio to standard output, for replies read aloud and agents' voice notes. |
| `transcribe` | `[]` | A program that reads a voice note's audio on standard input and writes its words to standard output. Empty: recording is refused. |
| `capture` | `[]` | A program that writes 16 kHz mono 16-bit PCM from the microphone to standard output. Empty: no recording. |
| `play` | `["afplay", "-r", "{rate}"]` on macOS, `[]` elsewhere | The player; `{rate}` becomes the speed, and the audio file's path is added last. |

[Browser, voice and search](browser-voice-search.md) has working values for each platform.

### Web

| Setting | Default | What it does |
| --- | --- | --- |
| `[web] search` | `[]`; the starter pack's is `["exa", "parallel", "firecrawl"]` | The providers the `search` tool tries, in order. |
| `[web] fetch` | `[]`; the starter pack's is `["firecrawl", "exa", "parallel"]` | The same for `fetch`, which reads a page. |
| `[web] answer` | `[]`; the starter pack's is `["exa", "parallel"]` | The same for `answer`, a cited answer. Firecrawl has none. |
| `[web.<provider>] base_url` | the provider's public address | Another address for `exa`, `parallel` or `firecrawl`, such as a proxy. |

A web tool appears only while one of its providers has a key. With none, agents keep their model's own web tools.

### MCP servers

`[mcp.<name>]` is one MCP server. `razor mcp add` and `/mcp` write these entries for you; `razor mcp add` writes to the current project's private file, `.razor/config.local.toml`, which Razor keeps out of git ([Where settings live](#where-settings-live)), unless you pass `--scope global` (your file) or `--scope project`, the shared file others who clone the repository get; in a folder whose `.razor` is Razor's own home, which has no project files, every scope writes your file. Any other folder, even an empty one outside git, gets its own `.razor/config.toml` or `config.local.toml`.

```sh
razor mcp add files --scope global --config '{"command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "/home/you/notes"]}'
razor mcp list
```

`razor mcp list` shows each server's scope and whether it is on (`configured`) or off (`disabled`); whether it starts, and its tools, show in `/mcp` in a session.

In the terminal app, `/mcp` opens the list of servers, with their state and tools; `/mcp add <name> <url>` adds one reached over HTTP (`--header NAME=env:VAR` for a header read from the server's environment), and `/mcp add <name> -- <command> <args…>` one that runs as a program. To bring over the servers you already set up in Claude Code, Codex or OpenCode, type `/mcp import <file>` with that program's settings file, such as `~/.claude.json`, a project's `.mcp.json` or `~/.codex/config.toml`: Razor shows what it would add and writes it when you press Enter, leaving out any field that holds a secret for you to fill in. `razor mcp import <file>` shows the same preview without writing anything. `razor mcp sign-in <name>` starts a server's sign-in, and if the browser then lands on a page that does not load, `razor mcp complete '<that address>'` finishes it.

| Setting | Default | What it does |
| --- | --- | --- |
| `command`, `args`, `env` | none | A server that runs as a program, with its arguments and extra environment. It starts in the project's folder. |
| `url` | none | A server reached over HTTP or HTTPS instead. |
| `headers` | none | HTTP headers, each `{ env = "VARIABLE" }` or `{ file = "path" }`, so no secret is written in the file. |
| `oauth` | none | `issuer`, `client_id` and `client_secret` (as `{ env = … }` or `{ file = … }`) for a server that needs a registered client. Other servers sign in on their own: Razor sends you the link. |
| `enabled` | `true` | `false` turns the server off, including one a file beneath defines. |
| `search` | automatic | Whether agents find this server's tools through a search instead of seeing them all at the start. Automatic means yes once its tool declarations pass 8 KiB. |
| `timeout` | `600000` | How long one tool call may take, in milliseconds. |
| `tools` | all | The only tools this server offers, by name. |

A project's servers start without asking when a session opens there.

### Script tools

`[tools.<name>]` turns a script into a tool. Each call runs the command with bash in the session's folder, with the call's input as JSON on standard input; the output is the result.

```toml
[tools.forecast]
command = "~/bin/forecast"
description = "Today's weather forecast for one city."
input = { type = "object", properties = { city = { type = "string" } }, required = ["city"] }
read_only = true
```

`read_only = true` lets it run beside other read-only calls in the same batch. `input` must be a JSON schema whose top is `{ type = "object" }`.

### Plugins

`[[plugins]]` entries run in the order listed. `file` is a program or a `.js` file; a relative path counts from Razor's home. A program gets each event as JSON on standard input, and allows, changes or refuses it by what it prints and its exit code; a JavaScript file registers its own events and may add tools and commands. `razor plugins` lists what loaded. The full contract is in [docs/design/extensions.md](design/extensions.md).

```toml
[[plugins]]
file = "~/razor-hooks/check-calls"
before = ["call"]
```

| Setting | Default | What it does |
| --- | --- | --- |
| `file` | required | The plugin's file. |
| `before` | `[]` | The events a program sees before they happen, and may change or refuse: `prompt` (a conversation's system prompt as it opens), `input` (a message before it is recorded), `call` (a tool call before it runs), `rebuild` (a fresh context before it replaces the old). |
| `after` | `[]` | The kinds of record entry a program is shown once written, such as `input`, `text`, `call`, `result` and `turn_end`. |

### Always-on agents

`[residents.<name>]` defines an agent that runs as part of the server. Its keys are on [Always-on agents and schedules](always-on-agents.md#every-setting).

### Tasks

| Setting | Default | What it does |
| --- | --- | --- |
| `[tasks] remote` | none: plain files | A remote of the project's checkout, or a git URL, through which its tasks are shared, on the hidden reference `refs/razor/tasks` ([Memory](memory.md#tasks)). Only a project's `.razor/config.local.toml` may set it; any other file that sets it is refused. |

### The server

| Setting | Default | What it does |
| --- | --- | --- |
| `[server] address` | none: the local socket only | A `host:port` the server also listens on, for other devices and for paired servers, the Razor servers on your other machines that you connect to this one; plain HTTP, so use a private network ([More machines](machines.md)). Needs a restart. |
| `[server] https_url` | none | An HTTPS address that reaches this server, used to finish MCP sign-ins. Without it, you paste the address the browser landed on. |
| `[server] name` | empty | This machine's short name: the first word of the name of each new session you start, and the machine name other servers show. Empty: sessions are named after their project. |
| `[server] label` | `name`, else the machine's name | How clients label this home. |
| `[server] realm` | empty | A label for this home, such as `work` or `personal`, that saved views can group sessions by. |
| `[server] shared` | empty: this home | The id (from `server.id` in its home) of the paired server that keeps the shared saved views and voice settings. |
| `[server] wrapper` | empty | A command the background service runs the server through, such as a script that sets up its environment: `razor install` puts it before the server's own command, which it receives as its last arguments and must run, for example with `exec "$@"`. Read when you run `razor install`. |

### The terminal app

| Setting | Default | What it does |
| --- | --- | --- |
| `[ui] motion` | `true` | `false` draws everything still, in its settled form. |
| `[ui] blocks` | `"octant"` | Pictures are drawn with octants, Unicode characters that split a cell into eight blocks. `"half"` uses half blocks instead, for fonts that lack octants. |
| `[ui] stage` | `"workshop"` | The status pane ends in a small animated workshop scene. `"off"` hides it and gives its rows to the pane. |
| `[brand] name` | `"Razor"` | The product name clients show. |
| `[brand] colours` | none | Replacements for the theme's colours, each name mapped to `"#rrggbb"`; the names are below. |

The theme's colour names:

```text
glass  bezel  bezelHi  recess  strip  agentBg  violet  violetDim  white  data  gray  grayDim
green  cyan  magenta  amber  red  redBg  cyanBg  youBg  chip  button  addedBg
```

```toml
[brand.colours]
violet = "#b48cff"
```

Saved arrangements of the agents list are not settings: `/views` edits them.
