# Browser, voice and search

Three capabilities come from programs and online services outside Razor: a real browser for agents, voice in both directions, and web search. Each works out of the box once its program is installed or its key is set, and each is optional.

## The browser

Agents can drive a real Chrome browser from their shell with [agent-browser](https://github.com/vercel-labs/agent-browser), a command-line browser from Vercel Labs (Apache-2.0) that ships beside the `razor` binary. The server puts it on its agents' `PATH`, so any agent can open a page, read it as a list of the page's elements, each with a short reference it can click or type into, wait for content, run page JavaScript and take screenshots.

The starter pack, the folder of default settings, prompt files and skills that Razor ships, includes a skill (written instructions agents load by name) that teaches them agent-browser's commands. It also tells each agent to name its own browser with `--session "$RAZOR_SESSION"` (Razor sets `RAZOR_SESSION` to the session's id in every agent's shell), since without a name every agent on the machine shares one browser.

`razor setup` downloads the Chrome that agent-browser drives. To do it yourself, use the copy beside Razor, or the one on your `PATH` if you built from source:

```sh
~/.local/share/razor/current/agent-browser install
```

On minimal Linux systems, Chrome needs system libraries even when its download succeeds. This installs them, and needs `sudo` and permission to install packages. On Debian or Ubuntu without sudo, ask an administrator to run `apt-get install sudo` first. `razor doctor` does not check whether Chrome can launch; `agent-browser doctor --offline` does, with a test launch.

```sh
~/.local/share/razor/current/agent-browser install --with-deps
```

Built from source without the release script, Razor has no copy of its own; install agent-browser from npm, and it is used from your `PATH`:

```sh
npm install --global agent-browser
agent-browser install
```

## Web search, page reading and answers

Agents get three web tools and never choose a provider: `search` (titles, links and excerpts), `fetch` (a page's text, optionally only the lines matching a phrase) and `answer` (a cited answer). Each tries the providers you list in order and moves to the next when one fails or returns no text; the result's first line names the provider that answered.

| Provider | Key | Used for |
| --- | --- | --- |
| [Exa](https://exa.ai) | `EXA_API_KEY` | search, fetch, answer |
| [Parallel](https://parallel.ai) | `PARALLEL_API_KEY` | search, fetch, answer |
| [Firecrawl](https://firecrawl.dev) | `FIRECRAWL_API_KEY` | search, fetch |

`razor keys` saves these keys in `~/.razor/keys.toml`; run `razor restart` after changing them. The starter pack lists all three providers, and a tool appears only while one of its providers has a key. The order is yours to change:

```toml
# ~/.razor/config.toml
[web]
search = ["parallel", "exa"]
fetch = ["exa"]
answer = ["exa"]
```

Claude and GPT also keep their own web tools, with or without these keys: Claude's web search and fetch, within your Claude subscription, and GPT's hosted search, billed the way your GPT sign-in is. Razor's three tools join them when a key is set. Other models have no web tools of their own, so without a key they have none.

## Voice

Voice has three parts. Each needs its own program, and Razor ships one of them.

### Replies read aloud

Click the ▶ beside the newest reply, or type `/listen`, to hear it. The starter pack's `speech` program, `scripts/speech.py`, sends the text to [ElevenLabs](https://elevenlabs.io) and needs `ELEVENLABS_API_KEY`, which `razor keys` saves; without the key, the ▶ answers with a sentence saying how to add it. It reads code blocks, tables and links as short phrases rather than spelling them out. With `ffmpeg` installed it also sets the volume and measures the waveform the terminal draws.

macOS plays the audio with `afplay`. Linux has no player by default; with `ffmpeg` installed, this one works:

```toml
# ~/.razor/config.toml
play = ["ffplay", "-nodisp", "-autoexit", "-loglevel", "error", "-af", "atempo={rate}"]
```

Any program that reads text on standard input and writes MP3 audio to standard output can replace `speech`. Its last argument names the speaker: a resident's name, or the session's id, so one agent can have its own voice. It gets the chosen voice, gain and expressiveness in `RAZOR_VOICE`, `RAZOR_GAIN_DB` and `RAZOR_EXPRESSIVENESS`, and run with `--voices` it prints its voices as JSON, for the voice list below.

### Voice notes you record

Ctrl-R (or `/record`) records a voice note: Enter sends it, Escape stops it for review. It needs two programs: one in `capture` that reads the microphone, and one in `transcribe` that turns the recording into words. Razor ships no transcriber, so bring your own speech-to-text program; it must read the recording, a WAV file (16 kHz, mono) from the terminal app, on standard input, write the words to standard output, and on failure make the last line of its standard error the sentence you should see. The microphone, through `ffmpeg`:

```toml
# ~/.razor/config.toml, macOS
capture = ["ffmpeg", "-nostdin", "-f", "avfoundation", "-i", "none:default", "-ac", "1", "-ar", "16000", "-f", "s16le", "pipe:1"]
transcribe = ["/path/to/your/transcriber"]
```

On Linux with PulseAudio or PipeWire, replace `-f avfoundation -i none:default` with `-f pulse -i default`.

### Spoken notes from agents

An agent can send you a short spoken note when it finishes or gets stuck, using the same `speech` program. A note appears in your inbox with a ▶, and the terminal app you used last plays a new one by itself if its machine has a player (`play`; macOS has one by default). By default an agent may speak at any time, at most 3,000 characters a day. With `when` set to `away`, it speaks only while you are away, unless you asked for a voice note: you count as present for a minute after you type in a terminal app on a machine that can play audio, not over SSH, and on a Mac after any keyboard or mouse use.

These settings are kept by the server, not in a settings file:

| Field | Default | What it does |
| --- | --- | --- |
| `when` | `"always"` | When agents may speak: `always`, `away` or `never`. |
| `voice` | `""` | The voice id to speak with, from `/v1/voice/voices`; empty is the speech program's own. |
| `families` | `{}` | Different voices for different agents: Razor gives each agent one of the families the terminal app draws (the characters in its workshop scene) by its id, and this maps a family's name to a voice id; an unknown name is dropped. Empty: every agent uses `voice`. |
| `gain_db` | `9.5` | How much louder to make the audio, in decibels (needs `ffmpeg`). |
| `expressiveness` | `0.5` | From 0 (steady) to 1 (lively). |
| `memos` | `false` | Allows notes of up to 1,000 characters instead of 300. |
| `quiet` | `null` | A time range with no spoken notes, such as `{"from": "22:00", "to": "07:00"}`. |
| `daily_characters` | `3000` | The most characters agents may speak in a day. |
| `call_voice` | `""` | The voice of live voice calls from the separately distributed phone client. |
| `call_daily_dollars` | `10` | The most those calls may spend in a day, in US dollars. |

Read and change them through the server's API, here with the `razor_api` shell function from [Using Razor from scripts](scripting.md#the-local-api). A change sets the fields it sends; the others keep their value. With paired servers, the settings live on the server that `[server] shared` names, and the others refuse a change:

```sh
razor_api http://localhost/v1/voice                # the current settings
razor_api http://localhost/v1/voice/voices         # the voices the speech program offers
razor_api -X PUT -H 'content-type: application/json' http://localhost/v1/voice -d '{
  "when": "away", "quiet": {"from": "22:00", "to": "07:00"}
}'
```
