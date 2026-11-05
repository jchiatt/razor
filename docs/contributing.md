# Contributing

Changes go to the GitHub repository, [github.com/treygoff24/razor](https://github.com/treygoff24/razor): report a problem or propose a feature as an issue, and send code as a pull request. A change is judged as much by what it leaves out as by what it adds: Razor is held to a written spec, a hard line budget, and tests that run the whole program, and a new module starts as a design page, described below, before any code.

## What to read first

- [SPEC.md](../SPEC.md): what Razor must do, what was deliberately removed, its architecture and its budgets. A behaviour is in Razor because the spec names it.
- [RULES.md](../RULES.md): twenty-five rules, one line each, that every line of code keeps to. Reviews check against them.
- The vocabulary: [the glossary in Concepts](concepts.md#glossary) and the [words used in the code](#words-used-in-the-code) below. A name in neither is sent back in review; propose a new word as a line for one of them.
- [AGENTS.md](../AGENTS.md): how an agent works in this repository, which applies to people too.

## Design pages before code

A new module, or a change to a module's public surface, starts as a design page in `docs/design/<module>.md`: the public surface as types and function signatures, a doc comment on each saying what it guarantees, about ten lines on how data flows through it, and the one hard decision, argued with what each option costs. The page is reviewed before code is written, because a structure that is wrong at the start cannot be reviewed back into shape later, and a page can be thrown away for free. The existing pages in [docs/design](design/) are the examples. A page keeps its design as it changes, without dated build logs or approvals.

## Line budgets

Razor's size is held to a budget: the spec's budget table gives each module a limit in non-blank, non-comment lines of Rust, TypeScript, Python and shell, and the whole program a total. `scripts/size.py` counts every row and fails when one is over:

```sh
python3 scripts/size.py
```

A change that would take its module over its row says so, with the count, before it is written; a module near its limit cuts before it asks for more. A granted raise changes only the numbers in the spec's table. The spec's five performance limits sit beside the line budget, measured on Linux by `scripts/perf.py`.

## Tests

There are no unit tests and no mocks or fakes of Razor's own code. The tests are the numbered flows in `flows/`: end-to-end scripts that each drive the built binary the way a person uses it, the terminal app included under a pseudo-terminal, in a scratch home, against a stand-in server that replays recorded model responses over the real Claude and OpenAI protocols. On the Claude route the real Agent SDK and its Claude Code run against the stand-in. [docs/flows-and-tasks.md](flows-and-tasks.md) lists what the flows cover.

Run flows by number or name; the command prepares the build, bridge packages and pinned Python environment:

```sh
scripts/flows.sh 02 session
```

With no arguments it runs every numbered flow, eight at a time; repeated selectors run each file once, and `--repeat N` runs the selection N times. `RAZOR_BIN` may name another binary under test; otherwise the command uses this checkout's debug build.

Each flow prints `PASS 02-session` or its failure, then a `FLOW` line with its time and exit code. A change runs the flows nearest it, and a change that removes a feature removes that feature's flow. A new flow must be able to fail the way the product fails; prove it by breaking the code it covers and watching it fail.

## The gate

One script runs everything a change must pass: the line budgets, `cargo fmt`, `cargo clippy` with warnings denied, debug and release builds, the bridge's type check, `shellcheck`, every flow, and the performance limits on the release build.

```sh
scripts/gate.sh
scripts/gate.sh --static    # everything but the flows
```

The flows need the tools they start: Rust with a C compiler, Git, Node 22.18 or later, Python 3 with `venv`, the `openssl` command, and, for flow 45 only, [Post](https://github.com/treygoff24/post) (`post` on your `PATH`); `ffmpeg` is used if present. The gate adds `shellcheck`. It runs on Linux.

## Sending a change

For a new module or a change to a module's public surface, send the design page first, as its own pull request, so the structure is agreed before the code. Keep a pull request small, review your own diff before anyone else does, and say in its description which flows and checks you ran and what they showed.

A pull request is not merged as it stands. When one is accepted, a maintainer applies it with you as a co-author of the commit, and closes the pull request with a link to the release that ships it.

Unless you explicitly state otherwise, any contribution you intentionally submit for inclusion in Razor, as defined in the Apache-2.0 license, is dual-licensed under the [MIT license](../LICENSE-MIT) and the [Apache License 2.0](../LICENSE-APACHE), as Razor is, without any additional terms or conditions.

Reviews here subtract. A review comment asks for something to be deleted, merged or renamed, or asks a question; a request to add a check, a mode or a layer needs a reason the spec already gives.

## Words used in the code

These are the words the code and its reviews use beyond [the glossary in Concepts](concepts.md#glossary); together the two lists are the whole vocabulary.

### Models and the API

- **request**: what is sent to a model for one step, rendered from the record. A **response** is one model reply on Razor's own loop: the output items of one request, recorded whole once complete, or not at all.
- **native conversation**: a model path's own conversation inside a session (on the Claude path, the SDK's). A rebuild starts a new one.
- **model path**: the code that talks to one kind of model: the Claude path (the SDK), the Responses loop (GPT), the generic loop (everyone else).
- **profile**: what one model family sees: its tool declarations, prompt register and continuation rules.
- **action**: what a tool does. A **declaration** is how a profile shows it to a model. A **transport** is the wire it travels.
- **code mode**: a profile that shows one `exec` tool running JavaScript against the tools, behind a config flag. A **cell** is one `exec` call's JavaScript run, named by an id; it may yield, and `wait` resumes it.
- **overflow**: a host rejection because a request exceeds the model's context window; Razor rebuilds once from the record, and pauses if the rebuilt request is also rejected.
- **cursor**: the id of the last entry a client has read; attaching after it replays the rest.
- **program status**: what the terminal app tells its terminal through OSC 7501 (the Program Status Protocol): the state of the attached session and of each live descendant, written only after the terminal has answered the protocol's query.
- **layout**: a view resolved by the server into ordered sections.
- **section**: a heading and its ordered session ids within a layout.
- **search file**: a top-level session's `search.txt` beside its record: a line per searched entry, its text folded for case and accents and as written. It is derived from the record, and a deleted one is rebuilt at the next start.
- **hit**: a search's answer for one conversation: its newest entry holding every word, or with `session` each such entry.

### Tests and checks

- **flow**: one end-to-end test script that drives the installed binary against a **stand-in**, a scripted fake of a model's API.
- **gate**: the one check on merged code: build, lints, flows, budgets.
- **budget**: a number the gate enforces: lines per module, and the five performance limits.

### How the work is organised

- **design page**: the page a lane commits before its code: the module's public surface and its one hard decision.
- **lane**: a child building one module of this repository.
