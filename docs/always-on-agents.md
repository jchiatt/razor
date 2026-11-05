# Always-on agents and schedules

An always-on agent, which Razor calls a resident, is an agent defined in your settings that runs as part of the server: it has its own folder, its own identity record and memory, and it wakes at the times you give it. Use one for work that should happen without you starting it: a morning briefing, a weekly report, a folder it checks every few hours.

## Set one up

1. Make its folder. The resident works there, and its memory belongs to that folder's project.

   ```sh
   mkdir -p ~/scout
   ```

2. Define it in `~/.razor/config.toml`. Its name, here `scout`, is up to 32 lowercase letters, digits and hyphens, starting with a letter. This one sends you a note at eight on weekdays:

   ```toml
   [residents.scout]
   cwd = "~/scout"

   [residents.scout.schedules.morning]
   at = ["08:00"]
   days = ["mon", "tue", "wed", "thu", "fri"]
   text = "Find the three news stories from the last day that matter most to my work."
   deliver = "notify"
   ```

3. Keep the server running when you are not at the terminal:

   ```sh
   razor install
   ```

   This installs a service: a background program, a systemd user unit on Linux or a launchd agent on macOS, that runs the same server `razor serve` runs and restarts it within a minute of a failure. On macOS it starts when you log in. On Linux it also starts at login, and at boot once you run `loginctl enable-linger`. A server already running for this home is stopped first, and the service starts a fresh one. The service is named after its home's folder without the leading dot: `razor` for `~/.razor`, `razor-work` for a `RAZOR_HOME` of `~/.razor-work`, so each home can have its own; `razor install` refuses to replace a service of the same name that runs another home.

   On macOS the service starts Razor.app first, a small program installed once in `~/.local/share/razor/Razor.app`, which hands over to the server. macOS counts what the server and its agents do as Razor's, so a permission you give Razor holds across updates. To let agents see the screen and use the mouse and keyboard, turn on Razor in System Settings, Privacy & Security, under Accessibility (Device Control and Data Access on macOS 27) and under Screen & System Audio Recording; if Razor is not in a list, click +, press Command-Shift-G, type `~/.local/share/razor/Razor.app` and choose it. Then run `razor restart`. `razor doctor` says whether the running server has both. Every agent on that Mac, in every Razor home, then has them ([Security](security.md#screen-and-control-on-macos)).

While the server runs, a new resident starts within a minute of saving the file, since Razor rereads the settings at least once a minute. Its name is its session's id, so you talk to it directly:

```sh
razor --resume scout
```

## What it is made of

**Its home session** has the resident's name as its id. Its heartbeat, its schedules and its night all run there, one input at a time.

**Its identity record** is `~/.razor/sessions/scout/identity.toml`, made from a template at its first start: its display name, role, values, relationships and ways of working. Every conversation it has starts with this record, and the resident keeps it current itself with its file tools; you can edit it too.

**Its startup list.** A resident starts with your `startup` list unless it has its own. Give it a persona by naming a file:

```toml
[residents.scout]
cwd = "~/scout"
startup = ["~/scout/persona.md", "razor", "about", "project"]
```

**A heartbeat** is an input that wakes it at set times. One that arrives while the resident is busy is skipped, and one missed while the server was down is not made up.

```toml
[residents.scout.heartbeat]
at = ["09:00", "15:00"]
text = "Check ~/scout/inbox and tell me what needs me."
```

**A schedule** is a named job that comes due at set times. One that comes due while the resident is busy waits for the turn to end. One that came due while the server was down runs once at the next start, as long as it has run at least once before. Each schedule can name its own model and effort, so a cheap job can run on a cheap model.

**Fresh runs.** With `fresh = true`, each run of a schedule gets a thread of its own, a new session with a fresh context, which closes when the run ends. Use it for jobs that should not crowd the home session's context. A fresh run's thread stays out of the plain session list; `GET /v1/sessions?resident=<name>` lists the resident's threads with its fresh runs, each marked with its `job`.

**Projects.** Each folder in the resident's `projects` folder (`~/scout/projects` here; hidden folders and links excepted) is a project, shown by the first line of its `.razor/name`, or by the folder's own name without one. You make and rename projects through the API (or the phone client), and start a thread in one by naming it as `resident_project`; renaming changes only the shown name, so the folder, and the project's id, never move. A folder the resident makes there is a project too.

**Its night** comes at 03:00 unless `night` says otherwise. The home session first gets one input asking it to record what the day taught it, tidy its memory and tasks and rewrite the project's state note; the input lists the threads and fresh runs active in the last 24 hours, each with the first line of its last reply, so the resident keeps one picture of its own work. When that turn ends, the home session's context is rebuilt from its record. A night missed while the server was down is not made up.

**Its memory** is its folder's project memory ([Memory](memory.md)). The home session writes the project's state note; threads keep rulings, facts and notes, and when you close a thread with `/done` it mails the home session what to keep instead of writing a state note.

## Hearing from it

`deliver = "notify"` tells a schedule's turn to end by sending you a note, and `deliver = "ask"` to end by asking you a question. Notes go to your inbox in the terminal app and through the `notify` program; questions wait in the question band and go through `ask` ([Configuration](configuration.md#notifications)). If a scheduled run fails or is stopped, Razor tells you through `notify` too.

Any session, a resident included, that has had unanswered input for 15 minutes (`[stuck] after`) with no command running, no question open and no helper working shows as stuck; open it and see, or stop it.

## Controlling it from a script

Threads, running a schedule now, and stopping a resident go through the server's API. These lines use the `razor_api` shell function from [Using Razor from scripts](scripting.md#the-local-api), which sends each request to the server with your local key:

```sh
razor_api http://localhost/v1/residents                                  # residents, timetables and recent runs
razor_api -X POST http://localhost/v1/residents/scout/run/morning        # run a schedule now
razor_api -X POST -H 'content-type: application/json' -d '{"resident": "scout"}' http://localhost/v1/sessions   # start a thread
razor_api -X POST http://localhost/v1/residents/scout/stop               # stop it; start does the reverse
razor_api http://localhost/v1/residents/scout/projects                   # its projects, newest activity first
razor_api -X POST -H 'content-type: application/json' -d '{"name": "Garden"}' http://localhost/v1/residents/scout/projects   # make a project
razor_api -X PATCH -H 'content-type: application/json' -d '{"name": "Vegetables"}' http://localhost/v1/residents/scout/projects/<id>   # rename one
razor_api 'http://localhost/v1/sessions?resident=scout'                  # its threads and fresh runs
razor_api http://localhost/v1/residents/scout/memory                     # its memories
razor_api 'http://localhost/v1/search?q=garden+plan'                     # search past conversations
```

Starting a thread prints its session id; open it with `razor --resume <id>`. A stopped resident stays stopped until you start it again or the server restarts; starting it runs each schedule it missed while stopped, once. A project's name is one line of at most 80 characters.

## Every setting

| Setting | Default | What it does |
| --- | --- | --- |
| `cwd` | required | The resident's folder, fixed when its home session is made. |
| `startup` | your `startup` list | Its own startup list. |
| `model`, `effort` | your `model` and `effort` | The home session's model and effort, read only when that session is first made. |
| `night` | `"03:00"` | When its nightly rebuild comes, in local time. |
| `heartbeat.at` | required for a heartbeat | The times of day it wakes, `HH:MM`. |
| `heartbeat.days` | every day | The weekdays it runs on: `mon` to `sun`. |
| `heartbeat.text` | required for a heartbeat | The input it receives. |
| `heartbeat.model`, `heartbeat.effort` | the session's | The model and effort for that turn only. |
| `schedules.<job>.at`, `.days`, `.text`, `.model`, `.effort` | as for the heartbeat | The same, for one named schedule. At most 64 schedules are read. |
| `schedules.<job>.deliver` | `"none"` | `"notify"` ends the run with a note to you; `"ask"` with a question. |
| `schedules.<job>.fresh` | `false` | Runs each time in a thread of its own that closes when the run ends. |
| `sky` | none | A JSON file, relative to `cwd`, that a separately distributed phone client draws from. |
