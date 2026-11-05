# Razor

![Razor's terminal app: the agents list on the left with a tree of helpers at work, a conversation in the middle, and the status pane on the right](docs/razor.png)

Razor is an agent harness, like Claude Code or Codex, built for running a crew of agents rather than one. It runs Claude through Anthropic's Agent SDK, which is Claude Code underneath, and GPT through the same ChatGPT sign-in Codex uses, so the subscription you already have is enough. Razor's conversations run on a server rather than inside the terminal you type in, so many run at once and each keeps going after you close the terminal, and an agent can hand work to helpers that each work on their own git branch, then merge what they bring back. It also has agents that wake on a schedule, memory that each project keeps, and one list of every agent on every machine you pair, which means connecting the machines' Razor servers to each other over a private network.

After installing you have a `razor` command that opens a terminal app in any project, and a server in the background that keeps your sessions running after you close it. The installer then runs `razor setup` in your terminal, which signs you in, or, with no terminal, prints the command to run. Razor runs on Linux (x86_64 or arm64) and on Macs with Apple silicon.

```sh
curl -fsSL https://github.com/treygoff24/razor/releases/latest/download/install.sh | sh
```

## What Razor is

Razor is one program, `razor`, that is both a server and its clients. The server does the work: it talks to the models, runs the agents' commands, and writes every conversation to a log on disk, its record. Because the work lives in the server and not in your terminal, a session keeps running when you quit, you can watch and steer it from another terminal or another machine, and scripts can use it through `razor --print`.

It reaches three kinds of model: Claude, on your Claude subscription or an Anthropic Console account; GPT, on your ChatGPT login or an OpenAI API key; and any model behind an OpenAI-compatible chat endpoint, such as Gemini, DeepSeek, GLM or one running on your own machine.

A session keeps going for as long as the work does. When the model's context, what it can hold at once, fills up, the agent writes a few lines on the work in hand and Razor starts a fresh context from the record, with your latest requests word for word. No model summarizes the conversation, and the earlier part stays in the record for the agent to look up ([Concepts](docs/concepts.md#when-the-context-fills)).

## Install

Razor runs on Linux (x86_64 or arm64) and on macOS with Apple silicon.

You need curl and trusted CA certificates to download a release, and git to work in repositories. On a minimal Debian or Ubuntu system, ask an administrator to run `apt-get update && apt-get install curl ca-certificates git sudo`. The browser also needs Linux system libraries; [the browser guide](docs/browser-voice-search.md#the-browser) gives the command to install them using sudo.

This downloads the latest release, checks its checksum, and installs it under `~/.local/share/razor` with a `razor` link in `~/.local/bin`. It uses Node 22.18 or later from your `PATH` if you have it, and Razor then keeps using the Node on its `PATH`; otherwise it downloads a private copy that only Razor uses.

```sh
curl -fsSL https://github.com/treygoff24/razor/releases/latest/download/install.sh | sh
```

The first install starts setup in your terminal. Without a terminal, it prints the command to run later. If your shell cannot find `razor`, run the PATH command the installer prints. Running the same command again installs an update; it leaves your server running, so run `razor restart` to use the new version.

To build from source, you need Rust, Node 22.18 or later, git, Python 3 and curl. The script builds a release from a clean checkout, downloads the agent-browser, ripgrep and jq programs, and installs it the same way, setup included:

```sh
git clone https://github.com/treygoff24/razor.git
cd razor
scripts/install.sh --link
```

## First run

Setup asks which provider you use (Claude, ChatGPT or both) and whether to start the server at login, then runs the provider's own sign-in if you are not signed in yet. It then downloads the browser agents use, runs `razor doctor` to check the machine, and offers to open the terminal app for your first message. The [Quickstart](docs/quickstart.md) walks through it.

To rerun setup, or after a noninteractive install:

```sh
~/.local/bin/razor setup
```

Claude uses your subscription by default; `razor setup --console` signs in to an Anthropic Console account instead, billed at API prices. Optional web-search and voice keys can be added later with `razor keys`, followed by `razor restart`. Ask an agent to remember your name or preferences when you want them saved.

Send your first message from inside a project:

```sh
cd ~/code/your-project
razor "What does this repository do, and what would you change first?"
```

Escape stops what the agent is doing. Ctrl-C twice quits the app; the session keeps running on the server, and `razor --continue` opens it again.

## What you get

| Feature | What it does |
| --- | --- |
| Sessions that keep going | Each conversation is saved on disk and reopens with `razor --resume <id>`; a message you send mid-task reaches the agent before its next step. |
| Helpers on their own branches | An agent hands a task to a helper that works in its own git worktree and reports back; its work is then merged or discarded. |
| Agents on a schedule | An agent defined in your settings wakes at set times, in its own folder with its own memory, and sends you notes or questions. |
| Memory | Each project keeps a note on where the work stands and your decisions in your own words, and every new session starts with both. |
| Voice | Replies read aloud, voice notes you record, and short spoken notes from agents. |
| A browser and web search | Agents drive a real browser with [agent-browser](https://github.com/vercel-labs/agent-browser), which ships with Razor, and search the web through Exa, Parallel or Firecrawl, or their model's own search. |
| Several machines | Pair the Razor servers on your machines over Tailscale, and see each one's agents from the others. |
| Extensions | MCP servers, your own scripts as tools, plugins that can change or refuse what agents do, and `razor --print` for scripts. |

## How the code is kept small and correct

Razor's whole source is held to a line budget the build enforces, and tested only end to end. [SPEC.md](SPEC.md) says what Razor must do and why, in the words of the person who decided each behaviour, and what was deliberately left out. [RULES.md](RULES.md) gives twenty-five rules for the code, one line each, such as no abstraction before its second use and nothing unbounded in memory.

A script, `scripts/size.py`, counts every module's lines against its row of the spec's budget table and fails the build when one goes over. Five speed and memory limits sit beside it:

- 15 ms from opening the terminal app to its first frame;
- 24 MiB of server memory at rest;
- 2.5 ms of Razor's own time per tool call;
- 4 ms of Razor's own time per turn;
- 40 ms to draw a 10,000-line conversation.

No part of Razor is faked in its tests, and there are no unit tests. Each test is a script that runs the real `razor` binary, the terminal app included. The only stand-ins are for services outside Razor, the model providers above all: a small server that speaks the real Claude and OpenAI protocols and replays recorded answers, including the failures that matter, such as a reply cut off mid-stream, a tool call with broken arguments, and a crash between running a command and recording its result.

## Docs and license

The [docs index](docs/README.md) lists every page; an installed Razor carries the same pages in `~/.local/share/razor/current/docs/`.

- [Quickstart](docs/quickstart.md): from installing to merging a helper's branch.
- [Concepts](docs/concepts.md): sessions, helpers and how their work is merged, agents on a schedule, memory, and a glossary.
- [Models and sign-in](docs/models.md): Claude, ChatGPT, API keys, other model families, and what each costs.
- [Configuration](docs/configuration.md): every setting, its default and what it does.
- [Memory](docs/memory.md): what is kept, how your decisions are recorded, and where the files are.
- [Always-on agents and schedules](docs/always-on-agents.md): an agent that wakes on a schedule and keeps its own memory.
- [More machines](docs/machines.md): pairing the Razor servers on several machines over Tailscale.
- [Security](docs/security.md): what the server exposes, where keys live, and what agents can run.
- [Browser, voice and search](docs/browser-voice-search.md): agent-browser, the voice programs and the search providers.
- [Using Razor from scripts](docs/scripting.md): `razor --print`, its JSON and streamed output, and the local API.
- [Troubleshooting](docs/troubleshooting.md): `razor doctor` and common failures.
- [Contributing](docs/contributing.md): the spec, the rules, the budgets, the tests, and where to send a change.
- [Uninstall](docs/uninstall.md): removing the program, the background service that `razor install` sets up, and your data.

Razor is dual-licensed under the [MIT license](LICENSE-MIT) and the [Apache License 2.0](LICENSE-APACHE), at your choice. Some of its code and prompt text is adapted from OpenAI's Codex under the Apache License 2.0; [NOTICE](NOTICE) lists those files.
