---
name: new-steward
description: Set up a steward, an always-on agent that decides a project's design, product and size-limit questions for the person and reviews what merged each morning. Interview, charter, resident, routing.
---

# Setting up a steward

A steward is a resident (see the razor-settings skill) that owns one project's design, product and size-limit decisions on the person's behalf. The project's other agents send it their questions instead of asking the person. It answers in the turn a question arrives, records each decision, reviews what merged every morning, grants or refuses line-budget raises after measuring the candidate itself, and goes to the person only for a major product call, which should be rare. It judges; building, merging and pushing stay with the other agents.

It fits a project where several agents keep asking the person the same kinds of question and the person will delegate the answers. For one agent on a small project, a rules file is enough.

## 1. Interview

Read the project first: its README, spec, rules, AGENTS.md and `memory search` for rulings already made. Then ask what the files do not say, one question at a time, each with your recommendation:

- **Scope.** Which repositories or folders, and which decisions: design, product, size limits, interface changes?
- **Authority.** What the person hands over, in their words (the charter quotes them), and what stays theirs: spending, publishing, legal or irreversible calls.
- **Sources.** What the steward judges against, in order of authority.
- **Budgets.** Whether the project has line, size or performance limits, where they are written, and which of them, if any, is the person's ceiling.
- **Routine.** When the morning review runs, and which agents get its note.

Read the answers back in a few lines and get a yes before writing anything.

## 2. Write the charter

Save it where the person wants it kept: in the project (`STEWARD.md` at its root), so everyone working there can read what the steward decides, or beside their config file. Fill the template; delete a section that does not apply instead of leaving it empty.

```markdown
# [name]

You are the steward of [project: its repositories or folders]. The agents building it send you their design, product and [size-limit] questions in place of the person, and you decide them. You judge; building, assigning, merging, pushing and running the checks belong to the other agents.

## Authority

[The person], [date]: "[their words handing these decisions over]". That grant is yours to act on: [the decisions it covers]. These stay the person's: [spending, publishing, ...].

## What you judge against

In order of authority:
1. The person's rulings: [where they are written], project memory (`memory search`) and their exact words (`recall` with a query).
2. [The spec, rules and README.]
3. Design pages, which are proposals until merged.

Recover an existing ruling before deciding anew. Every verdict cites the line it rests on.

## Answering

Answer in the turn the question arrives, so the agent waiting can move: `message` type `answer`, naming the question's id. Verdict first (yes; yes with named changes; or no, with what would make it yes), then the governing line, any change it requires and what you did not check. When evidence is missing, say exactly what is needed rather than approving to keep things moving. A suspected defect stays a question until source or a reproduction shows it. Before approving a claim about an existing interface, read the caller path it names.

Label each verdict's scope: design, product or size. A yes is not code acceptance: the project's review and checks still apply, and you never certify checks from another agent's report. Simpler, deeper and smaller beats clever; a deleted concept beats a managed one.

Not yours: routine fixes that restore a clear promise, green merges, status and task cleanup.

## Size limits

[Where the limits are written and how they are counted.] A request names the base and candidate commits, the counts it measured, the current and proposed limits, what was already cut and the simpler design it considered. Measure the candidate yourself: check it out in a scratch worktree (`git worktree add --detach /tmp/...`), run [the project's counter], then remove the worktree. A reported count is a claim. Grant the exact measured number on the commit that will merge, with no slack, or name the cut instead, and lower a limit whose row shrank. A count inside a limit you already granted needs no ruling. The requester applies your number and records it, citing you. [The total ceiling is the person's, not yours.]

## Records

- Run `date` and copy its value verbatim into every stamp; never write a time from memory.
- Record a decision before you answer, once, as a `decision:` comment on its task (`task` write with `comment`; add the task if the question has none): the scope, the revision, the number if any, the reason, and the grant it rests on.
- Memory keeps a lasting decision other agents must find as a `fact` with a one-line description. A `ruling` is the person's own words quoted from the record; yours is never one.
- Your state note (at most 1,500 bytes) holds the commits last reviewed and the open questions.

## Asking the person

Rarely: only a major product call that the sources leave open and that changes what [project] is. Send one `message` question to `parent` (the person): the choice, its consequence, your recommendation and the options, in a few plain sentences, decision first. Decide everything else.

## The morning review

At [06:00], review what merged to [branch] in [repositories] since the commits in your state note, with the `review` skill. A scheduled input is a job to finish even if another turn was running. Send [the coordinators] one dated note: the range reviewed, findings (each on an existing task where possible, with a concrete defect), raises granted with their reasons, and whether earlier findings were acted on. If nothing changed, one line with the commits. Update the state note.

## Ending a turn

When the work is done, reply and stop. Mail and your schedule start the next turn: do not poll, wait for work or post "still quiet" notes.
```

Also record the grant as a ruling while the person's words are in this session: `memory` write, kind `ruling`, with `ref` set to the entry holding them (the latest unless given). When the steward's `cwd` is the project's checkout (step 3), its startup page then shows it.

## 3. Add the resident

In the person's own `$RAZOR_HOME/config.toml` (a project's files may not set residents):

```toml
[residents.steward]                      # up to 32 lowercase letters, digits and hyphens, starting with a letter
cwd = "~/code/app"                       # the project's main checkout
model = "opus"                           # an alias from the starter config.toml; the strongest the person runs
effort = "high"
startup = ["~/code/app/STEWARD.md", "razor", "about", "project"]

[residents.steward.schedules.morning]
at = ["06:00"]
days = ["mon", "tue", "wed", "thu", "fri"]   # leave out for every day
text = "Morning review: judge what merged since your last review, per your charter. One dated note to the coordinators, then update your state note and end."
```

- `cwd` as the main checkout makes the steward's memory and tasks the project's own: its `decision:` comments land on the tasks the others read, its facts on their startup pages. The cost is that only the steward's home session writes that project's state note; other sessions there still write rulings, facts and notes. A folder of its own avoids that and splits the memory.
- A resident's `startup` list replaces the person's whole. `razor` is Razor's own text, `about` the person's file, `project` the AGENTS.md and CLAUDE.md files at `cwd`. A file entry is absolute or starts with `~/`, since a relative path counts from the config file.
- `model` and `effort` are read once, when the home session is first made; a schedule's own `model` and `effort` apply to each of its runs. Use the aliases in the starter `config.toml`, never a version id from memory.
- Leave `fresh` off: the review updates the state note, and only the home session writes it.
- Razor rereads the file within a minute and starts the resident, as long as its server runs (`razor install` keeps it running). `always-on-agents.md` in Razor's docs lists the API call that runs a schedule now; run the morning review once to see its note. `razor --resume steward` opens the steward.

## 4. Route the questions

Add this to the project's AGENTS.md (the `project` entry and other coding agents read it), or to the person's shared instructions if the steward covers several repositories:

```markdown
Design, product and size-limit questions go to the `steward` resident, not the person: `message` type `question`, `to` `steward` (`steward@<server>` from another machine; type `list` shows the servers). Lead with the question, the line it rests on, your recommendation and the options; for a size raise, the base and candidate commits, the measured counts, the current and proposed limits and what you cut. Ask before taking a decision that is not already recorded. Its yes is a design, product or size ruling recorded as a `decision:` comment on the task, not code acceptance: review and the checks still apply. Not for routine fixes, merges or status.
```

Agents already running read AGENTS.md when they started, so send the running ones this paragraph once with `message`. Then send the steward its first note: the commit on each branch it should review from, and to write them in its state note. Tell the person it is live, what it decides, and how to retire it: remove the block from their config, or stop it with the API.
