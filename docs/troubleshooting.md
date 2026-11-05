# Troubleshooting

Start with `razor doctor`. It starts no server, Razor writes nothing while it runs, and it prints no key's value; the Claude sign-in check it runs is Claude Code's own, which may create `~/.claude` and `~/.claude.json` if they are missing. It prints one line for each thing Razor runs on, found or missing with what to do, then the models it can run on.

```sh
razor doctor
```

```text
ChatGPT sign-in: missing. ChatGPT is not signed in; run razor login.
Claude: found.
EXA_API_KEY: found.
FIRECRAWL_API_KEY: missing. For optional web search, run razor keys.
PARALLEL_API_KEY: missing. For optional web search, run razor keys.
ELEVENLABS_API_KEY: missing. For optional voice, run razor keys.
Browser: found (agent-browser executable; Chrome launch untested).
Server: running.
Service: installed.
Razor can run on Claude.
```

It exits 0 when your settings load and Claude or GPT can run. A missing ChatGPT sign-in or API key does not matter if you use Claude, and the reverse; missing web keys only mean agents use their model's own web tools, and a missing voice key that nothing is read aloud. The OpenAI API key line matters only if you bill GPT through an API key instead of a ChatGPT plan. `Server: stopped; your next razor command starts it.` means no server is running for this home, which is normal before your first `razor` command; if a command then fails to start one, see [the server and settings](#the-server-and-settings) below. On a Mac with the server running, a `Mac permissions` line also says whether agents can see the screen and use the keyboard and mouse ([Security](security.md#screen-and-control-on-macos)).

Two places hold more detail. The server writes its errors to `~/.razor/server.log`; when it runs as the service that `razor install` sets up (the same server, started by systemd or launchd), read them on Linux with `journalctl --user -u razor`. The service is named after its home's folder without the leading dot, so a `RAZOR_HOME` of `~/.razor-work` has `razor-work`. Each session's folder, `~/.razor/sessions/<id>/`, holds its record and `startup.md`, which shows exactly what the conversation was sent at its start (`/startup` shows it in the terminal app).

## Restarting the server

Some changes apply only when the server starts: new keys in `keys.toml`, `server.address` and `responses.base_url`. Restart this home's server with:

```sh
razor restart
```

It waits for the old server to stop, then starts the installed background service, or starts a detached server if you have no service. To stop without starting again:

```sh
razor stop
```

Stop succeeds if no server is running. A later `razor` command, `razor --print` and the `razor mcp`, `login` and `devices` commands start it again: the installed service if there is one, otherwise a server in the background. Set `RAZOR_HOME` on either command to select a different home. Commands the agents started keep running through a stop, so that the next server can take them back.

Sessions that were working when the server stopped resume by themselves if their last entry is less than an hour old. Older ones show as interrupted; send them a message, or type `/continue all` to continue every interrupted top-level session.

## Installing

**`razor: command not found` after installing.** The installer links `razor` into `~/.local/bin` and, when that folder is not on your `PATH`, prints the command that adds it. For sh, bash and zsh, add this to your shell's profile; for fish, run `fish_add_path "$HOME/.local/bin"`:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

**`Unsupported platform; use Linux x86_64 or aarch64, or macOS arm64.`** Releases and the install script support only those three platforms.

**`The Claude bridge at … has no node_modules; run npm ci there.`** A build from source is missing the packages of its bridge, the small Node program Claude sessions run through. Run `npm ci` in the `bridge` folder of your checkout, or install with `scripts/install.sh --link`.

**`Run razor setup in a terminal.`** Setup, and `razor keys`, ask their questions in a terminal. For a script, sign in with `razor login` for ChatGPT, and if you use only ChatGPT put `model = "sol"` in `~/.razor/config.toml`; for Claude, sign in with Claude Code itself if you have it, or export `ANTHROPIC_API_KEY` where the server starts. `razor install` sets up the background service without asking, and `~/.local/share/razor/current/agent-browser install` downloads the Chrome the browser drives. Then run `razor doctor`. Optional keys can go straight into `~/.razor/keys.toml` ([its format](configuration.md#keys)).

**`Release checksum mismatch; download again from the release URL.`** or **`Node checksum mismatch; download again from nodejs.org.`** A download was cut short or changed on the way, and the installer stopped before installing anything. Run the one-line installer again; each run downloads afresh. An error starting `curl:` instead, such as `curl: (22) The requested URL returned error: 404`, means the download itself failed: check your connection and the address in the command.

## Signing in

**`Razor can run on neither Claude nor GPT yet`** Do what the `Claude:` or GPT credential line above it says, such as `razor setup` for Claude (`razor setup --console` for an Anthropic Console account), `razor login` for ChatGPT, or installing Node, then run `razor doctor` again. Doctor shows `ChatGPT sign-in:` for the default `responses.base_url`, or `OpenAI API key:` for the public API endpoint.

**`ChatGPT is not signed in; run razor login.`** Run `razor login`, open the link it prints on any device, and enter the code it shows.

**`Claude is not signed in; run razor setup.`** Claude's sign-in is missing or expired. `razor setup` runs it again, or `razor setup --console` for an Anthropic Console account. On a machine with no browser, open the link it prints on any device and paste the code back.

**`The Claude bridge needs Node 22.18 or later on PATH (…); this install has no private Node; install Node, then run razor doctor.`** Claude sessions run through a small Node program. The installer downloads a private copy of Node only when it finds no Node 22.18 or later on your `PATH`; when it found yours, Razor keeps using the `node` on the `PATH` of whatever starts the server, and the line shows that `PATH`. Put Node 22.18 or later on it, run `razor install` again if the background service needs the new `PATH`, then `razor restart`.

**`… is not set in the server's environment.`** or **`The model host refused or never got a key; set … in the server's environment.`** A model on a chat endpoint needs a key variable the server does not have, or the host refused the key it got. The first sentence is the model picker's reason for not offering the model; the second ends a turn on it, such as one from `razor --print --model gemini-3.8-flash`. Export it where the server starts, or add it to the service ([Models and sign-in](models.md#other-model-families)), then run `razor restart`.

## The server and settings

**`Settings: invalid. <file> does not parse: …`** from `razor doctor`. A settings file has a mistake, named with the file and the problem; the server will not start until it loads, so doctor exits 1. Fix that file and run `razor doctor` again.

**`… keys.toml may set only EXA_API_KEY, FIRECRAWL_API_KEY, PARALLEL_API_KEY, ELEVENLABS_API_KEY, as nonempty strings.`** `keys.toml` holds a name it may not, or an empty value, and neither the server nor doctor starts until it is fixed. A model's key, such as `GEMINI_API_KEY`, goes in the server's environment instead ([Models and sign-in](models.md#other-model-families)).

**`razor serve exited (exit status: 1) before answering; see …/server.log; last line: "…"`** The server a command started stopped at once, and the quoted line from `~/.razor/server.log` says why. The usual reason is a settings file that does not parse, named with the problem: fix the file and run the command again.

**`No Razor server answered within 10 seconds; … says why.`** A server started but did not answer in time. The message names where its errors are: `~/.razor/server.log`, or, when the background service started it on Linux, `journalctl --user -u razor.service`, whose name for another home is that home's folder without its leading dot. Read the last lines there.

**`Razor cannot listen on …/server.sock: …`**, usually as the last line of a `razor serve exited` message. The server could not make its socket in Razor's home, and the error after the path says why. `Operation not permitted` means something outside Razor forbids a socket there, such as a sandbox that an agent tool or a container runs commands in: run Razor outside it, or set `RAZOR_HOME` to a folder where it may make one. `path must be shorter than SUN_LEN` means the socket's full path is too long, which is over 107 bytes on Linux and 103 on macOS: set `RAZOR_HOME` to a shorter path.

**`A Razor server is already running for …`.** `razor serve` found another server using the same home, often the service and a server the terminal app started in the background, both trying. Run `razor restart` to stop this home's server and start it again. To install the background service, run `razor install`; it stops the old server first.

**`… cannot set …; move these keys to the person's file.`** A project's `.razor/config.toml` holds a setting only your own file may set ([Configuration](configuration.md#what-only-your-own-file-may-set)). Move it to `~/.razor/config.toml`.

**`This session has … working children and children.max is …`.** A session reached its limit of working helpers. Wait for one to finish, or raise `[children] max`; it applies at once.

**A session shows as stuck.** It has had unanswered input for 15 minutes with no command running, no question open and no working helper. Open it to see what it last did; Escape stops its turn, and a new message gives it something to do.

## More machines

**`server.address changed; restart Razor to apply it.`** Run `razor restart`. If the server then fails with `Razor cannot listen on server.address`, the address is not one of this machine's; use its Tailscale address.

**`This server has no server.address, so the other could not answer it`.** Pairing, connecting the Razor servers on two machines, needs both servers to listen on an address. Set `[server] address` on this machine, run `razor restart`, and pair again.

## Voice, notifications and the browser

**`speech: there is no ElevenLabs key; run razor keys to add one.`** Reading aloud and agents' spoken notes need `ELEVENLABS_API_KEY`. Run `razor keys` (Enter keeps the keys you already saved), then `razor restart`.

**No desktop notifications.** On macOS, allow notifications for Script Editor in System Settings, under Notifications. On Linux, the starter pack (the default settings Razor ships) uses `notify-send`; on a machine with no desktop there is nowhere to show them, so send them to your phone with ntfy ([Configuration](configuration.md#notifications)).

**`Browser: missing or unresponsive. Reinstall the agent-browser executable, then run agent-browser install.`** from `razor doctor`. The agent-browser program beside Razor, `~/.local/share/razor/current/agent-browser`, is missing or does not run. When it is missing, Razor uses the `agent-browser` on your `PATH`, so `npm install --global agent-browser`, then `agent-browser install` to download its Chrome, brings it back ([Browser, voice and search](browser-voice-search.md#the-browser)).

**The browser will not start (Linux).** Chrome lacks system libraries. Run `~/.local/share/razor/current/agent-browser install --with-deps`, which uses `sudo`. `~/.local/share/razor/current/agent-browser doctor --offline` says what else is wrong, with a test launch.
