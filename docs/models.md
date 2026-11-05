# Models and sign-in

A Claude subscription or a ChatGPT subscription is enough to run Razor; no API key is needed. Razor reaches Claude through Anthropic's Agent SDK, GPT through OpenAI's Responses API, and every other model through an OpenAI-compatible chat endpoint. Each session runs on one model at a time: switch with `/model` in the terminal app, start on one with `razor --model <name>`, or set the default with `model` in your settings. With no `model` set, new sessions start on `opus` at `high` effort.

The names in the table are the short names Razor accepts: `opus`, `sonnet`, `haiku` and `fable` are Anthropic's model families, and `sol`, `terra`, `luna` and `astra` stand for OpenAI models listed under [GPT](#gpt).

| Family | Names | Sign-in | What it costs |
| --- | --- | --- | --- |
| Claude | `opus`, `sonnet`, `haiku`, `fable`, or a full `claude-…` id | Claude's own sign-in, run by `razor setup` (`--console` for a Console account) | Your Claude plan's usage limits; or API prices on an Anthropic Console account or key |
| GPT | `sol`, `terra`, `luna`, `astra`, or a full GPT id that `/model` lists | `razor login` | Your ChatGPT plan's usage limits; or API prices with an OpenAI API key |
| Gemini, DeepSeek, GLM | `gemini-3.8-flash`, `deepseek-v4.1-flash-1017`, `glm-5.3-flash` | An API key in the server's environment | The provider's API prices |
| Anything else | A name you choose | Its endpoint's key, or none | Whatever that endpoint charges; nothing for a model on your own machine |

Effort, how much work the model puts into each step, is `low`, `medium`, `high`, `xhigh` or `max`, set with `/effort` or `--effort`. Some chat models stop below `max`; asking for more gives them their highest.

## Claude

If Claude Code is already signed in on this machine, Razor is too: Razor runs Claude through the copy of Claude Code inside Anthropic's Agent SDK, and Claude Code keeps one sign-in for your user account, under `~/.claude` (in the login keychain on macOS). The `Claude:` line of `razor doctor` says whether it found one.

Otherwise `razor setup` runs Claude Code's sign-in, and usage counts against your Claude plan's limits. To bill an Anthropic Console account per token at API prices instead, run `razor setup --console`. If the sign-in goes missing or expires, run `razor setup` again; with a working sign-in it does not sign you in again.

There are two other ways in. Export `ANTHROPIC_API_KEY` where the server starts, and Claude sessions are billed at API prices; for the background service, add it to the service as [below](#other-model-families). Or set `[claude] sign_in` to a command and its arguments, such as `sign_in = ["/home/you/bin/claude-token"]`, that prints a Claude sign-in token on its first line, and optionally an account name on its second, within 30 seconds. Razor runs it for each conversation it opens, so a script of yours can choose which of several Claude accounts each conversation uses.

Claude sessions use Claude Code's own file tools, with Razor's shell in place of Claude Code's, beside Razor's own tools ([Concepts](concepts.md#tools)). Claude's own web search and fetch are always there, within your subscription; Razor's `search`, `fetch` and `answer` join them when a provider key is set.

## GPT

Sign in with your ChatGPT account:

```sh
razor login
```

It prints a link and a code: open `https://auth.openai.com/codex/device`, enter the code, and Razor saves the sign-in in `~/.razor/auth.json` and refreshes it by itself. `/login` in the terminal app does the same. This is the sign-in OpenAI's Codex uses, and usage counts against your ChatGPT plan; when the plan's limit is reached, the request fails at once rather than retrying.

The short names stand for full model ids: `sol` is `gpt-6.1-sol`, `terra` is `gpt-6.1-terra`, `luna` is `gpt-6-luna` and `astra` is `gpt-6-astra`. If you chose only ChatGPT in setup and your settings named no model, setup set `model = "sol"`.

On the sign-in, GPT runs in code mode by default, as Codex runs these models: it sees an `exec` tool that runs JavaScript against Razor's tools, so it can batch independent reads in one call, and a `wait` tool for a run that is still going. `[profiles.codex] code_mode = false` gives it plain function tools instead.

### With an OpenAI API key

To bill API use instead of a plan, put the key alone in the file `~/.razor/key`, readable only by you, and point Razor at OpenAI's public endpoint:

```sh
touch ~/.razor/key && chmod 600 ~/.razor/key
"${EDITOR:-vi}" ~/.razor/key
```

```toml
# ~/.razor/config.toml
[responses]
base_url = "https://api.openai.com/v1"
```

Run `razor restart` for `base_url` to apply ([Troubleshooting](troubleshooting.md#restarting-the-server)). At the public endpoint GPT uses plain function tools, and turning code mode on there is refused.

## Other model families

Every other model needs its provider's key in the server's environment, the variables the server process starts with. Razor's own key file, `keys.toml`, holds only the web search and voice keys, so these go elsewhere:

| How the server runs | Where the key goes |
| --- | --- |
| You start it with `razor serve`, or your first `razor` command starts it | Export the key in that shell first. |
| The background service, the same server registered by `razor install` with systemd or launchd so it starts at login; it sees only `PATH` and `RAZOR_HOME` | On Linux, run `systemctl --user edit razor`, add the lines below, save, and run `systemctl --user restart razor`. On macOS, add the key to `EnvironmentVariables` in `~/Library/LaunchAgents/razor.plist` and run `razor restart`; running `razor install` again rewrites that file. The service's name is its home's folder without the leading dot, `razor` for `~/.razor`. |

```ini
[Service]
Environment=GEMINI_API_KEY=your-key
```

Three chat models ship with their settings filled in:

| Model | Provider | Key |
| --- | --- | --- |
| `gemini-3.8-flash` | Google's Gemini API | `GEMINI_API_KEY` |
| `deepseek-v4.1-flash-1017` | RunInfra (`api.runinfra.ai`) | `RUNINFRA_API_KEY` |
| `glm-5.3-flash` | RunInfra (`api.runinfra.ai`) | `RUNINFRA_API_KEY` |

Any server that speaks OpenAI-compatible chat (`POST <base_url>/chat/completions`) works too, a local one included. Add a `[models.<name>]` table for it in your settings, then start a session on that name:

```toml
# ~/.razor/config.toml
[models.local]
base_url = "http://127.0.0.1:1234/v1"
id = "the-model-the-server-loaded"
window = 32768
```

```sh
razor --model local
```

`key` names the environment variable that holds the endpoint's key (`key = "OPENROUTER_API_KEY"`); with none, Razor sends no key. Set `window`, the model's context size in tokens: without it Razor cannot tell when the context is filling, so it cannot rebuild in time. A model you add gets plain file and shell tools (`read`, `write`, `edit` and `bash`, with `wait`, `send` and `kill` for running jobs) beside Razor's own; the shipped DeepSeek and GLM rows get only the shell of those. Chat models have no web tools of their own. [Configuration](configuration.md#models) lists the table's other fields.

## Checking what works

```sh
razor doctor
```

The last line names the routes Razor can run on, such as `Razor can run on Claude and GPT.` It checks Claude's sign-in, the one GPT credential `[responses] base_url` uses (the ChatGPT sign-in for the default endpoint, the key file for `https://api.openai.com/v1`), and that the web keys are present. It does not check chat models' keys, or whether a plan has usage left.
