# Security

Razor's agents run commands as you, with your files, your network and your credentials. Razor does not sandbox them. It keeps its own keys out of what agents see by accident, refuses two kinds of dangerous command, and listens only where you tell it to. Decide what you let an agent do the way you would for a person you give a shell to.

## What the server exposes

By default the server listens only on a Unix socket in its home, `~/.razor/server.sock`, and every request must carry a key: the local key in `~/.razor/device.key`, readable only by you, or the key of a device or server you enrolled with `razor devices add`. One request is exempt, `GET /v1/mcp/callback`, where the browser returns from an MCP server's sign-in and cannot carry a key; it finishes only a sign-in the server is waiting on, matched by that sign-in's secret `state` value. Nothing listens on the network.

`[server] address` adds a TCP listener for paired servers, the Razor servers on your other machines that you connect to this one ([More machines](machines.md)), and for other devices. It speaks plain HTTP: requests, replies and the bearer keys they carry are not encrypted by Razor. That is why remote use needs a private network such as Tailscale, which encrypts the traffic and limits who can reach the port at all. Bind to the machine's Tailscale address, never to `0.0.0.0` or a public address.

A device key grants everything the local terminal can do, and a paired server's key is a device key: a server you pair can do on this one anything its terminal can, devices, settings and sign-ins included. Pair only machines you trust as you trust this one. What a server passes along for you is narrower: operations on a session that lives on the other server, and listing or starting sessions and listing models and repositories there, each forwarded once and never relayed onward. `razor devices remove <name>` revokes a key at once.

## Where keys live

| What | Where |
| --- | --- |
| Web and voice keys from `razor keys` | `~/.razor/keys.toml`, mode 0600 |
| ChatGPT sign-in | `~/.razor/auth.json`, mode 0600 |
| OpenAI API key, for GPT at `api.openai.com` and the phone client's voice calls | `~/.razor/key`, a file you create; make it mode 0600 |
| Claude sign-in | Claude Code's own store: `~/.claude`, or the login keychain on macOS |
| MCP servers' sign-in tokens | `~/.razor/mcp-auth.json`, mode 0600 |
| The local key, enrolled devices and paired servers | `~/.razor/device.key` and `~/.razor/devices.json`, mode 0600 |
| Other models' keys | The server's environment, under the names their `[models]` rows give |

No setting needs a key's value: a model row names the variable that holds its key, and an MCP server's headers and client secret are `{ env = … }` or `{ file = … }`. The one exception is an MCP program's `env` table, which is passed to the program as written, so a value put there sits in the file. `razor doctor` says which sign-ins and web keys are present and never prints one.

## What agents can run

An agent's shell is bash, as your user, in the session's folder. It starts from the server's environment minus the web and voice keys and Claude's sign-in variables (`EXA_API_KEY`, `PARALLEL_API_KEY`, `FIRECRAWL_API_KEY`, `ELEVENLABS_API_KEY`, `CLAUDE_CODE_OAUTH_TOKEN`, `ANTHROPIC_API_KEY` and `ANTHROPIC_AUTH_TOKEN`), so a command that prints its environment does not put them in the record. MCP servers and script tools start the same way. That is protection against accidents, not against an agent that sets out to read a file: anything your user can read, an agent can read, `keys.toml` included. Keys you export for other models, such as `GEMINI_API_KEY`, stay in agents' environments.

Razor refuses two kinds of command, by checking the command's text before it runs:

| Refused | What counts | Can it be turned off |
| --- | --- | --- |
| A kill by pattern | `pkill`, `killall`, `kill -1`, or `kill` fed by `pgrep` or `pidof`. A plain `pkill` whose pattern matches the session's own running jobs stops those jobs instead. | No |
| Deleting what the session did not create | `rm`, `rmdir`, `unlink`, `shred`, `find -delete`, `find -exec rm`, `git clean` without `-n`, or a patch that deletes a file, aimed at a file that existed before the session started or whose creation time the filesystem does not record, or at a path written with a wildcard, `~`, a variable or a command substitution | Yes: `[guards] delete = false` in your own file or a pack (a folder of default settings, such as the starter pack Razor ships) |

Both are best effort. The check sees through `sudo`, `env`, `nohup`, `timeout` and similar wrappers, but a program an agent runs can still kill or delete whatever you could, a command can be written in ways the check does not see, and script tools' commands are not checked. Plugins can refuse more under their own rules ([Configuration](configuration.md#plugins)).

## Screen and control on macOS

Turning on Razor under Accessibility (Device Control and Data Access on macOS 27) and Screen & System Audio Recording lets every agent on that Mac, in every Razor home, see the screen and control the computer: read any window, type, click and answer dialogs as you. macOS gives the permission to Razor.app, the program the background service starts first, not to one agent or one home, and `razor install` keeps Razor.app at one path so that the permission can outlast updates. Leave both off unless you want agents to do that; `razor doctor` says whether the running server has them, and turning Razor off in System Settings takes them back.

## Opening an untrusted repository

Opening a session in a repository starts the MCP servers its settings name, without asking, so only open sessions in repositories you trust, as you would only run their build scripts if you trusted them. A repository's Razor settings are `.razor/config.toml` and `.razor/config.local.toml` at its root.

What a repository can do when you open a session in it:

- start MCP servers: programs, named in its settings, that run as you as soon as the session opens;
- add script tools, commands an agent can choose to call;
- steer the agent through its `AGENTS.md` and `CLAUDE.md` files, at the root and in each folder down to the session's, which go into every conversation's opening;
- add skills, in its `.agents/skills` and `.claude/skills` folders, which every conversation's opening lists;
- change project-level settings such as the model, the web providers and the helper limits.

What it cannot do: set anything about you or the machine, such as the server, plugins, guards or the notification programs ([the full list](configuration.md#what-only-your-own-file-may-set)).

To look before you open one, read its settings and instructions, and list its skills:

```sh
cat .razor/config.toml .razor/config.local.toml 2>/dev/null
find . -name AGENTS.md -o -name CLAUDE.md
ls .agents/skills .claude/skills 2>/dev/null
```

## What is kept on disk

Every session's record, `~/.razor/sessions/<id>/record.jsonl`, holds the whole conversation in plain text: your messages, the agents' replies, every command and its output. Records and memory files are written with your account's default file permissions, which on many systems let other users read them, so Razor makes its home readable only by you (mode 0700) when it creates it. A home that already existed keeps its mode; on a machine other people can log in to, check it with `ls -ld ~/.razor` and make it private with:

```sh
chmod 700 ~/.razor
```

When an agent types a password into an interactive program, it can send it from a file so that the password never enters the record; what the program prints back is recorded as usual.
