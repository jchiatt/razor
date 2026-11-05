# What a stranger gets out of the box

Three parts: the starter pack a fresh install uses, notifications that need no program of the person's own, and agent-browser at runtime.

## Surface

```rust
// src/config/pack.rs, inside `parse`: in `ask`, `notify`, `speech`, `transcribe`, `capture` and
// `play`, a program named by a path containing `/` counts from its file's folder (`~/` from the
// home folder), as every other path in a settings file does; a bare name is still found on PATH.

// src/profiles/startup.rs
/// As before, plus one word: `about` is the person's `about.md` in Razor's `home`, which
/// `razor setup` writes; a missing one is left out without a line in the facts.
pub async fn opening(profile: &Profile, list: &[String], root: Option<&Path>, s: &Summary,
                     identity: Option<String>, home: &Path) -> Opening;
```

`src/api/speech.rs`: a reply whose speech program fails is refused with `Couldn't read it aloud:` and the program's own sentence, in place of "see the server log". `scripts/speech.py`: with no `ELEVENLABS_API_KEY` that sentence is `speech: there is no ElevenLabs key; run razor setup to add one.`

`scripts/notify.sh`: the starter's `notify` and `ask` program. Its one argument becomes a desktop notification titled "Razor", through `osascript` on macOS or `notify-send` on Linux, with `--` before the text so no character of it reads as an option; where neither program exists it exits 0 and says nothing. It prints nothing, so an ask it carries waits for a client's answer.

`src/config/pack.rs`, `Settings`: a person's file that names no `pack` gets `packs/starter` from `config::INSTALL_DIR` (the one resolution of the binary's folder, made before the runtime starts), else from the source tree the binary was built from, read and layered as any pack (docs/design/packs.md); the lookup is a stat in Settings' own blocking read. A file that names a pack gets that pack alone, and removing the name brings the starter back. With neither folder holding one, there is no pack and no error.

**The starter pack** (`packs/starter/`, text only):

| File | What it holds |
| --- | --- |
| `config.toml` | `startup = ["persona.md", "razor", "about", "project", "rules.md"]`, with the instruction that a person who changes it copies the list and the two files beside their own file, since a relative path counts from the file naming it; `notify` and `ask` set to `../../scripts/notify.sh`; `speech` set to `../../scripts/speech.py`; `[web]` listing Exa, Parallel and Firecrawl, each verb live only while one of its keys is set (`src/tools/web/mod.rs:83-94`), the model's own web tools otherwise. Every other optional feature is a commented example with one sentence: ntfy for a phone, `play` and `capture` for each platform, model and effort, `memorum`, guards, children, a local chat model, an MCP server, a resident with a schedule, a plugin, the terminal's motion. Each example parses when uncommented. |
| `persona.md` | Who the agent is to the person: a collaborator who brings judgment and says when it disagrees, in general terms, with nothing personal. |
| `rules.md` | How it talks and reports, kept apart from what Razor's own prompt already says. |
| `skills/agent-browser/` | A self-contained guide: every command from agent-browser's own documentation, run by hand against v0.41.0, plus a named browser per agent (`--session "$RAZOR_SESSION"`; the unnamed one is shared by every agent on the machine), covered clicks and `doctor`, less its date-field line, whose `.value` assignment fires neither `input` nor `change`. It credits agent-browser's documentation (Apache-2.0, 2025 Vercel Inc.). There is no pointer to `agent-browser skills`, which the native binary Razor ships lacks. |
| `skills/razor-settings/` | Changing Razor's own settings for the person: the four files, keys, `about.md`, MCP servers, notifications to a phone, residents. |
| `skills/writing/`, `skills/review/` | Prose a person will read; reviewing a change. |
| `skills/new-steward/` | Setting up a steward, a resident that decides a project's design, product and size-limit questions for the person: the interview, a charter template, the resident's settings and the paragraph that routes other agents' questions to it. |

**The about-you file**, shared with `razor setup` (docs/design/setup.md): `$RAZOR_HOME/about.md`, plain Markdown, first line `# About <name>`, then free lines. Setup writes it, never inside the pack. A pack cannot name it by path: a pack's relative paths count from the pack and `~/` from the home folder (`parse`), and neither reaches `$RAZOR_HOME` when it is not `~/.razor`. Hence the word.

## Data flow

1. The person's file names no pack, so Settings reads `<install>/packs/starter` (a release) or `<checkout>/packs/starter` (built from source).
2. The starter's `config.toml` sits beneath the person's file; its relative paths count from the pack, so `../../scripts/` is the bundled programs in a checkout and in a release, which keeps the source layout.
3. A conversation opens with persona.md, Razor's own text, `about.md` when it exists, the project's files, then rules.md: 5,018 characters with a two-line `about.md`.
4. Its first input lists the starter's five skills with descriptions; that line is how an agent learns agent-browser is there, so no other startup text is added.
5. Web verbs appear for each verb with a provider key in the server's environment (setup's `keys.toml`); without keys the model keeps its own web tools.
6. Speech runs the bundled program; with no key the reply's ▶ and an agent's voice note are refused with the setup sentence, as an install with no `speech` refuses today.
7. A root's note to the person goes to the inbox, then through `notify` to the desktop.
8. A root's question goes through `ask` to the same notification; the script prints nothing, so a client answers.
9. A headless Linux server has neither notifier (libnotify is a desktop package): the script exits quietly; the person reaches a phone by copying the ntfy lines, which send through `curl`, already an install requirement.
10. A person's file that names another pack gets that pack alone: no starter, and only the startup words and programs that pack sets.

## The hard decision: what fires with no configuration

| | Desktop, always | Only when no terminal is attached | Nothing until setup turns it on |
| --- | --- | --- | --- |
| A question while the person is away | banner | none, if a terminal was left open on that session | none |
| The person watching the session | banner beside the question band | none | none |
| Headless Linux | silent | silent | silent |
| Cost | a configured script; no Razor code | the ask and note paths would look up the session's attached terminals; a resident's failure note has no session the person attaches to | a feature setting setup does not write |

**Chosen: desktop, always, as the starter's configuration.** "Attached" is not "looking": a terminal left open in a background window or overnight would silence every notification, at exactly the time the person needs one. The status row's terminal count is the only attachment signal on Linux, and the activity samples the voice code uses come only from the macOS terminal. Nothing-until-setup cannot work, since setup writes keys and no feature settings, so a stranger's first agent that asks a question would wait unseen. A missed question stalls an agent for hours; a duplicate banner costs a glance. On macOS the banners come from Script Editor, which the person may have to allow once; the starter says so.

This is configuration, not a fallback in code: Razor with no `notify` or `ask` still runs nothing, so `notify = []` and any other pack stay silent as before, and the residents module is unchanged. The phone needs no new key either: the ntfy lines are `curl` with `-o /dev/null`, without which curl's reply would become the ask's answer.

Every flow now runs on the starter, as a stranger's install does. Two assumed bare defaults and set the key they assume in their own config: flow 28 pins `startup = ["razor", "project"]`, since its 12,000-token limit is Razor's own GPT prefix and the starter's persona and rules (about 510 tokens) are configured startup text the spec lets grow it (12,022 estimated with them); flow 29 empties the `[web]` verbs whose absence it checks. On a Mac, eight flows (22, 26, 37, 47, 48, 53, 56, 57) would raise 21 desktop notifications through osascript; a no-op `osascript` and `notify-send` at the front of the flows' `PATH`, stand-ins for other people's programs, would keep them quiet without touching any flow's config.
