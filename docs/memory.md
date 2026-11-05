# Memory

Each project has a memory that outlasts its sessions, so a new session starts knowing where the work stands and what you decided. Razor keeps it in plain Markdown files in its home, one file per memory, which you can read, edit and delete like any other file. It also keeps the project's tasks, the work still to do (see [Tasks](#tasks)).

## What a session starts with

Every session opens with its project's startup page, at most about 2,500 tokens. It holds the project's state note, which the last session wrote when it closed with `/done`: the current work and the decisions behind it, in at most 1,500 bytes. A project has one state note, and each new one replaces the last. Below it come your rulings, decisions you made in your own words, at most twenty lines: standing ones first, then those that expire, each newest first. Then a line for each fact and note, its one-line description, newest first, at most 24 and then a count of the rest; and last, the tasks the session holds, the open tasks due within a week, and how many are ready.

Nothing else is added to a session's context by itself. During the work, agents search memory, read a memory whole, and write new ones with their `memory` tool, whenever you say "remember this" or they find something worth keeping.

## What is kept

| Kind | What it is |
| --- | --- |
| state note | The project's current state, written at a session's close |
| ruling | A decision of yours, quoted from your own message |
| fact | Something true about the project or the world that later sessions should know, with a one-line description |
| note | Anything else worth keeping, with a one-line description |

A memory belongs to its project, or to every project when the agent writes it that way. It can carry an expiry date, after which it is moved into the retired folder; a one-time answer of yours, such as a yes to one request, is recorded that way. A fact that changes is written again, replacing the old one.

## How your decisions become rulings

A ruling can only come from you. When an agent records one, it names the message of yours, or your answer to its question, that the ruling comes from, and Razor writes the ruling from the record itself: the question the agent asked, your words exactly as you wrote them, and the agent's reading of them in a line. An agent cannot write a ruling in its own words, and a ruling never cites another agent.

So when you settle something ("yes, every merge waits for the tests"), the agent can make it a ruling, and every later session in that project starts with your sentence. To retire a ruling, tell an agent; it retires it with the `memory` tool.

Two kinds of session write less. A helper (a child) writes no memory at all: it tells its parent what is worth keeping. A session in an always-on agent's folder, its threads and projects included, writes rulings, facts and notes there, but not the state note, which is the agent's home session's.

## Where the files are

Memories for one project are in `~/.razor/memory/projects/`, followed by the project folder's own path; memories for every project are in `~/.razor/memory/all/`:

```text
~/.razor/memory/
  all/
    fact-20261101-203800-0a1b2c3d.md
  projects/home/you/code/app/
    state.md
    ruling-20261101-204512-9f3e11c0.md
    note-20261102-081500-77aa0b12.md
```

The project is the repository's main checkout, so a helper working in its own worktree shares its parent's memory; outside git, it is the folder the session started in. A memory's file name says its kind and when it was written, and its id is the name without `.md`. Each file starts with a short header (where the memory came from, any expiry date, and a fact's or note's description) between `---` lines, then the text:

```text
---
source: razor:20261101-203800-b659:l-12
---
the agent asked: "Should every merge wait for the tests?"

the human said (razor:20261101-203800-b659:l-12): "yes"

agent's reading: merges wait for the tests.
```

You can edit a file, delete it, or add your own; any other `.md` file is a note, and the next memory call sees the change. A link counts as the file or folder it points to, so a project's memory folder can be a link to a folder you keep elsewhere, such as in a repository of your own. The exception is `state.md`: the next session to close replaces it whole, so an edit there lasts only until then. If you move a project to another folder, move its memory folder and its [tasks folder](#tasks) to match. With `RAZOR_HOME` set, the memory folder is `$RAZOR_HOME/memory`.

## Retired and deleted memories

A retired memory is one no longer in use but kept on disk: when an agent replaces or retires a memory, Razor moves its file to the same place under `~/.razor/memory/retired/`, such as `retired/projects/home/you/code/app/ruling-20261101-204512-9f3e11c0.md`. Search, reading a memory and the startup page never look in that folder.

- Retired: out of use, still on disk. To bring one back, move its file back to the same place outside `retired/`.
- Deleted: gone for good, whether you delete the file yourself or, for an always-on agent's memory, a client deletes it through the server's API.

## How agents search memory

Search matches the query's words as whole words, ignoring case, in a memory's text (not its header, so not a fact's description). Results are ranked by how many of the query's words each memory holds; among memories that hold the same number, the newest comes first. Newest here and on the startup page means most recently changed on disk, so a file you edit moves to the top. A search returns 8 results unless the agent asks for up to 20. Search reads the files each time and holds nothing in memory between calls. It was measured with 2,000 memories in a project and 2,000 for every project; past that, each search takes longer, because it reads every file, but there is no limit.

## Several machines

Each Razor home keeps its own memory. Paired servers (Razor servers on two machines connected to each other) do not share it, and a helper started on another machine reads that machine's memory.

## Tasks

A project's tasks are the work still to do: each has a number, a title, a description and a priority (0, most urgent, to 4), and may have a parent task, tasks blocking it, a due date and a claim. Agents keep them with their `task` tool; anything owed, an agent's promises to itself included, is a task with a due date. You see a project's open tasks with `/tasks` in the terminal app.

- A **claim** says which session is on a task, and sets that session's headline to the task's title. It ends when the session closes with `/done` or `/quit`. A parent can start a helper on a task; when it settles the helper, the helper's report becomes a comment on the task, and merging a finished helper closes it. Helpers read tasks but do not write them.
- When a task closes, each session holding a task it blocked gets a note. The day after a task's due date, the session holding it, or else the session that added it, gets one reminder if that session is open; a new due date allows a new reminder.

Each project's tasks are one folder in `~/.razor/tasks/`: a folder for each part of the project's path (with `%` written `%25`), then a folder named `%`, so a nested project's tasks never sit inside its parent's. An open task is `<number>.md`; a closed one moves to `closed/`; comments are files under `comments/<number>/`:

```text
~/.razor/tasks/home/you/code/app/%/
  12.md
  closed/9.md
  comments/12/20261102-161200-0a1b2c3d.md
```

To share a project's tasks between your homes and machines, or with people working on the same repository on their own, name a remote in the project's private settings file, `.razor/config.local.toml` (only that file may set it): a remote of the checkout, or a git URL or path (a relative path is read from the checkout). A URL keeps the tasks where you put them when the checkout's remote later moves elsewhere, a public repository included; a name follows the remote wherever it goes.

```toml
[tasks]
remote = "ssh://git@git.example.com/you/app.git"
```

Anyone who can read the repository can fetch its tasks, so name a remote only for a repository no stranger can read.

The tasks folder then becomes a git checkout whose commits go only to the hidden reference `refs/razor/tasks` on that remote, never to a branch, so they start no CI run and never reach a pull request. Each change is applied to the remote's tasks as fetched within the last 30 seconds, committed and pushed before it answers, and if another machine pushed first Razor fetches and tries again, so a claim that lands is the only claim; a change is refused while the remote is out of reach, or while the project's settings do not read, and reading keeps working on what was last fetched. Edits you make by hand in the folder are committed and kept. A claim that could not be released while the remote was out of reach is released within a minute of its answering again. A project naming no remote is plain files and always writable. A session's startup page lists the tasks its home has fetched; in a shared project with no open task there, it says to run `task list`, since this home may not have fetched them yet.

