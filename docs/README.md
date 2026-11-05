# Razor's documentation

These pages describe the Razor they ship with. In a standard install this index is `~/.local/share/razor/current/docs/README.md`, in the folder of the `razor` program itself (`~/.local/bin/razor` is only a link to it); in the source repository the pages are its `docs` folder. `razor --help` lists the commands, and `razor doctor` says what this machine has and what is missing.

## Start here

- [README](../README.md): what Razor is, how to install it, and a first run.
- [Quickstart](quickstart.md): from nothing installed to a helper's work merged into your repository.
- [Concepts](concepts.md): how Razor works: the server and its clients, sessions, the record, rebuilds when the context fills, helpers and how their work is merged, agents talking to each other, the tools agents have, and a glossary.

## Using Razor

- [Models and sign-in](models.md): Claude, ChatGPT, API keys, other model families, and what each costs.
- [Memory](memory.md): what each project keeps across sessions, how your decisions become rulings, tasks, and where the files are.
- [Always-on agents and schedules](always-on-agents.md): an agent that runs as part of the server and wakes at set times, and the API calls that control it.
- [More machines](machines.md): pairing the Razor servers on several machines, so each shows and reaches the other's agents.
- [Browser, voice and search](browser-voice-search.md): the browser agents drive, replies read aloud, voice notes, and the web search providers.
- [Using Razor from scripts](scripting.md): `razor --print` and its JSON and streamed output, keeping a conversation across calls, the local API with its routes, and keys for other programs.

## Reference

- [Configuration](configuration.md): every setting, its default and what it does, the settings files and how they combine, MCP servers, script tools and plugins.
- [Security](security.md): what the server exposes, where keys live, what agents can run, and what opening an untrusted repository allows.
- [Troubleshooting](troubleshooting.md): `razor doctor`, restarting the server, and the common errors with what to do about each.
- [Uninstall](uninstall.md): removing the background service, the program and your data.

## Working on Razor

- [Contributing](contributing.md): the spec, the rules, the line budgets, the tests, and how to send a change.
- [The flows and the tasks](flows-and-tasks.md): what the end-to-end tests cover.
- [Design pages](design/): each module's public surface and the reasoning behind it, written before its code.
