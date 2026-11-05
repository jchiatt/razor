//! A project's tasks: one Markdown file per open task, `<n>.md`, in the
//! project's folder, moved to `closed/<n>.md` by one rename when it closes, and a file per
//! comment in `comments/<n>/`. Whether a task is open is where its file is; everything else is
//! derived when read. Nothing is held between calls, and everything here runs on a blocking
//! thread, under memory's lock for a change.

use super::files::{self, fit};
use super::{Answer, Error, Note, TEXT, fill, other, refuse};
use serde::Deserialize;
use serde_json::json;
use std::cmp::Reverse;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// The most rows a list answers, and comments a get does.
const ROWS: usize = 50;
const COMMENTS: usize = 20;
/// The startup page's tasks: at most this many lines, in this many bytes.
const PAGE: (usize, usize) = (10, 1200);

/// The fields a `task` write sets, each absent to leave it as it is.
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct Edit {
    pub(super) id: Option<u32>,
    title: Option<String>,
    text: Option<String>,
    priority: Option<u8>,
    /// 0 clears it.
    parent: Option<u32>,
    blocked_by: Option<Vec<u32>>,
    /// An empty one clears it.
    due: Option<String>,
    pub(super) claim: Option<bool>,
    /// A child's write may release only its own claim. Never the model's to set.
    #[serde(skip)]
    pub(super) own: bool,
    pub(super) comment: Option<String>,
    pub(super) close: Option<String>,
    /// The due date a reminder went out for, so it goes out once. Never the model's to set.
    #[serde(skip)]
    pub(super) notified: Option<String>,
    /// A session whose claim this claim may take over: the parent starting its child on the
    /// task. Never the model's to set.
    #[serde(skip)]
    pub(super) from: Option<String>,
    /// The header the task must still have, as read when the server decided on the change: a
    /// release when a session closes or is settled, or a reminder. Never the model's to set.
    #[serde(skip)]
    pub(super) seen: Option<Vec<(String, String)>>,
}

/// What a `task list` call keeps: the tasks `filter` takes (ready, the default; open; or closed),
/// only those naming `parent` as theirs when it is given, whose title and description hold every
/// word of `query`.
#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct Listing {
    pub(super) filter: Option<String>,
    pub(super) query: String,
    pub(super) parent: Option<u32>,
}

/// A task as its file holds it: the header's lines in order, then its description. A closed
/// task read only for its number has neither.
#[derive(Clone)]
pub(super) struct Task {
    pub(super) id: u32,
    open: bool,
    pub(super) head: Vec<(String, String)>,
    text: String,
}

impl Task {
    fn read(id: u32, open: bool, file: &str) -> Task {
        let head = files::head(file).map(|(k, v)| (k.into(), v.into()));
        let (head, text) = (head.collect(), files::split(file).1.into());
        Task {
            id,
            open,
            head,
            text,
        }
    }

    pub(super) fn get(&self, key: &str) -> Option<&str> {
        let line = self.head.iter().find(|(k, _)| k == key);
        line.map(|(_, value)| value.as_str())
    }

    /// Sets header line `key` in its place, or removes it for `None` or an empty value.
    fn set(&mut self, key: &str, value: Option<String>) {
        let at = self.head.iter().position(|(k, _)| k == key);
        match (at, value.filter(|v| !v.is_empty())) {
            (Some(at), Some(value)) => self.head[at].1 = value,
            (None, Some(value)) => self.head.push((key.into(), value)),
            (Some(at), None) => drop(self.head.remove(at)),
            (None, None) => {}
        }
    }

    pub(super) fn blockers(&self) -> impl Iterator<Item = u32> + '_ {
        let line = self.get("blocked-by").unwrap_or_default();
        line.split_whitespace().filter_map(|n| n.parse().ok())
    }

    fn priority(&self) -> u8 {
        let priority = self.get("priority").and_then(|p| p.parse().ok());
        priority.unwrap_or(2)
    }

    /// The session holding it, when its claim was written by server `server`.
    pub(super) fn holder(&self, server: &str) -> Option<&str> {
        let claim = self.get("claim").map(ids);
        claim.filter(|ids| ids.1 == server).map(|ids| ids.0)
    }
}

/// A claim's or a creator's session and server, its first two words.
pub(super) fn ids(line: &str) -> (&str, &str) {
    let mut words = line.split(' ');
    let mut next = || words.next().unwrap_or_default();
    (next(), next())
}

/// A claim's or a creator's words for a person: the name and machine after the ids.
pub(super) fn shown(line: &str) -> &str {
    line.splitn(3, ' ').nth(2).unwrap_or(line)
}

/// The tasks in `dir`, by number: each open task's file, and each closed one's when `closed` is
/// set, else only its number.
pub(super) fn scan(dir: &Path, closed: bool) -> Result<Vec<Task>, Error> {
    let mut all = Vec::new();
    for (folder, open) in [(dir.to_owned(), true), (dir.join("closed"), false)] {
        let entries = match fs::read_dir(&folder) {
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            entries => entries.map_err(other("reading task files"))?,
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let id = name.to_str().and_then(|n| n.strip_suffix(".md"));
            let id = id.and_then(|n| n.parse().ok());
            // A folder, a pipe, or a file removed since the listing is no task.
            let file = match open || closed {
                true => files::read(&entry.path(), "").map(|found| found.file),
                false => Ok(String::new()),
            };
            if let (Some(id), Ok(file)) = (id, file) {
                all.push(Task::read(id, open, &file));
            }
        }
    }
    all.sort_by_key(|t| t.id);
    Ok(all)
}

/// Whether task `n` of `all`, in number order, is open.
pub(super) fn open(all: &[Task], n: u32) -> bool {
    let at = all.binary_search_by_key(&n, |t| t.id);
    at.is_ok_and(|at| all[at].open)
}

/// Each task's state, in `all`'s order: closed; blocked while a task it names in `blocked-by` is
/// open; in progress while claimed; open while an open task names it as its parent; else ready.
fn states(all: &[Task]) -> Vec<&'static str> {
    let parents: Vec<_> = all
        .iter()
        .filter_map(|t| t.get("parent").filter(|_| t.open))
        .collect();
    let state = |t: &Task| match () {
        _ if !t.open => "closed",
        _ if t.blockers().any(|n| open(all, n)) => "blocked",
        _ if t.get("claim").is_some() => "in progress",
        _ if parents.contains(&t.id.to_string().as_str()) => "open",
        _ => "ready",
    };
    all.iter().map(state).collect()
}

/// The `list` answer: the tasks `l` keeps, words matched as memory search matches them: ready and
/// open by priority, due date and number, closed newest first.
pub(super) fn list(dir: &Path, l: &Listing) -> Answer {
    let filter = l.filter.as_deref().unwrap_or("ready");
    if !matches!(filter, "ready" | "open" | "closed") {
        return Err(refuse(&TEXT.filter));
    }
    let all = scan(dir, filter == "closed")?;
    let parent = l.parent.map(|p| p.to_string());
    let taken = |(t, state): &(&Task, &str)| {
        let text = format!("{} {}", t.get("title").unwrap_or_default(), t.text);
        let (held, wanted) = files::held(&text, &l.query);
        let part = parent.as_deref().is_none_or(|p| t.get("parent") == Some(p));
        part && held == wanted && (filter == "open" && t.open || *state == filter)
    };
    let mut rows: Vec<_> = all.iter().zip(states(&all)).filter(taken).collect();
    match filter {
        "closed" => {
            rows.sort_by_key(|&(t, _)| Reverse((t.get("closed").and_then(|c| c.get(..10)), t.id)))
        }
        _ => rows.sort_by_key(|&(t, _)| (t.priority(), t.get("due").unwrap_or("~"), t.id)),
    }
    let row = |(t, state): &(&Task, &str)| {
        let parent = t.get("parent").and_then(|p| p.parse::<u32>().ok());
        json!({"id": t.id, "title": t.get("title"), "priority": t.priority(), "state": state,
            "claim": t.get("claim").map(shown), "due": t.get("due"), "parent": parent,
            "blocked_by": t.blockers().collect::<Vec<_>>(), "closed": t.get("closed")})
    };
    let more = rows.len().saturating_sub(ROWS);
    Ok(json!({"tasks": rows.iter().take(ROWS).map(row).collect::<Vec<_>>(), "more": more}))
}

/// The `get` answer: task `id`'s file as it is written, open or closed, and its newest
/// `COMMENTS` comments, oldest first.
pub(super) fn get(dir: &Path, id: u32) -> Answer {
    let name = format!("{id}.md");
    let found = files::read(&dir.join(&name), "");
    let found = found.or_else(|_| files::read(&dir.join("closed").join(&name), ""));
    let file = found.map_err(|_| missing(id))?.file;
    let folder = dir.join("comments").join(id.to_string());
    let names = fs::read_dir(&folder).into_iter().flatten().flatten();
    let names = names.filter_map(|e| e.file_name().into_string().ok());
    let mut names: Vec<_> = names.filter(|n| n.ends_with(".md")).collect();
    names.sort();
    let more = names.len().saturating_sub(COMMENTS);
    let comment = |name: &String| {
        let said = Task::read(0, true, &files::read(&folder.join(name), "").ok()?.file);
        let (by, at) = (said.get("by").map(shown), name.get(..15));
        Some(json!({"by": by, "at": at, "text": said.text}))
    };
    let comments: Vec<_> = names[more..].iter().filter_map(comment).collect();
    Ok(json!({"file": file, "comments": comments, "more": more}))
}

/// Applies `e`, written by `who` in a claim's form (the session's id, its server's id, then its
/// name and machine as a person reads them), to the tasks in `dir` on server `server`, once all
/// of it is checked: an open task's number or none to add one, its fields within their bounds,
/// every number it names a task's, a claim only of a task unclaimed, already the writer's or
/// `from`'s, a header still `seen`, and the whole file within what a read holds. Every file is
/// replaced by one rename, the comment first and the close last. The answer is the task's
/// number and title, and `claimed` for a claim.
pub(super) fn apply(dir: &Path, server: &str, who: &str, e: &Edit) -> Answer {
    let (all, t) = (scan(dir, false)?, &*TEXT);
    let mut task = match e.id.map(|id| (id, all.binary_search_by_key(&id, |t| t.id))) {
        None => Task::read(all.last().map_or(1, |t| t.id + 1), true, ""),
        Some((_, Ok(at))) if all[at].open => all[at].clone(),
        Some((id, Ok(_))) => return Err(refuse(&fill(&t.closed, &[("id", &id)]))),
        Some((id, Err(_))) => return Err(missing(id)),
    };
    if e.seen.as_ref().is_some_and(|seen| *seen != task.head) {
        return Err(refuse(&fill(&t.changed, &[("id", &task.id)])));
    }
    let line = |s: &Option<String>, most| {
        let fits =
            |s: &String| !s.trim().is_empty() && s.len() <= most && !s.contains(['\n', '\r']);
        s.as_ref().is_none_or(fits)
    };
    let long = |s: &Option<String>| s.as_ref().is_some_and(|s| s.len() > 16 << 10);
    let blank = e.comment.as_ref().is_some_and(|c| c.trim().is_empty());
    let fits = (e.title.is_some() || e.id.is_some()) && line(&e.title, 200) && line(&e.close, 500);
    if !fits || long(&e.text) || long(&e.comment) || blank || e.priority.is_some_and(|p| p > 4) {
        return Err(refuse(&t.bounds));
    }
    let date = |d: &str| d.parse::<jiff::civil::Date>().map_err(|_| refuse(&t.due));
    let due = match e.due.as_deref() {
        Some("") => Some(String::new()),
        Some(d) => Some(date(d)?.to_string()),
        None => None,
    };
    let mut named = e.blocked_by.clone().unwrap_or_default();
    named.extend(e.parent.filter(|&n| n > 0));
    let unknown = |n: &u32| all.binary_search_by_key(n, |t| t.id).is_err();
    if let Some(n) = named.into_iter().find(unknown) {
        return Err(missing(n));
    }
    let mine = |held: &&str| {
        let from = e.from.as_deref().is_some_and(|f| ids(held) == (f, server));
        ids(held) == ids(who) || from
    };
    let claim = e.claim.filter(|claim| *claim || e.own);
    if let (Some(_), Some(held)) = (claim, task.get("claim").filter(|h| !mine(h))) {
        let with: [(_, &dyn std::fmt::Display); 2] = [("id", &task.id), ("holder", &shown(held))];
        return Err(refuse(&fill(&t.claimed, &with)));
    }
    let join = |ns: &Vec<u32>| ns.iter().map(u32::to_string).collect::<Vec<_>>().join(" ");
    let closed = |why: &String| Some(format!("{} {why}", jiff::Zoned::now().date()));
    for (key, value) in [
        ("by", e.id.is_none().then(|| Some(who.to_owned()))),
        ("title", e.title.clone().map(Some)),
        ("priority", e.priority.map(|p| Some(p.to_string()))),
        ("parent", e.parent.map(|n| (n > 0).then(|| n.to_string()))),
        ("blocked-by", e.blocked_by.as_ref().map(|n| Some(join(n)))),
        ("due", due.map(Some)),
        ("claim", e.claim.map(|claim| claim.then(|| who.to_owned()))),
        ("notified", e.notified.clone().map(Some)),
        ("closed", e.close.as_ref().map(closed)),
        ("claim", e.close.as_ref().map(|_| None)),
    ] {
        if let Some(value) = value {
            task.set(key, value);
        }
    }
    let line = |(k, v): &(String, String)| format!("{k}: {v}\n");
    let head: String = task.head.iter().map(line).collect();
    let (name, max) = (format!("{}.md", task.id), files::FILE);
    let file = format!("---\n{head}---\n{}", e.text.as_ref().unwrap_or(&task.text));
    // A file longer than a read holds was read cut short, so only a new description replaces it.
    let cut = e.text.is_none() && fs::metadata(dir.join(&name)).is_ok_and(|m| m.len() > max);
    if cut || file.len() as u64 > max {
        return Err(refuse(&t.bounds));
    }
    if let Some(comment) = &e.comment {
        let folder = dir.join("comments").join(task.id.to_string());
        let said = format!("---\nby: {who}\n---\n{comment}");
        files::put(&folder, &format!("{}.md", files::stamp()), &said)?;
    }
    files::put(dir, &name, &file)?;
    if e.close.is_some() {
        let closed = dir.join("closed");
        let moved = fs::create_dir_all(&closed)
            .and_then(|_| fs::rename(dir.join(&name), closed.join(&name)));
        moved.map_err(other("closing a task"))?;
    }
    let claimed = (e.claim == Some(true)).then_some(true);
    Ok(json!({"id": task.id, "title": task.get("title"), "claimed": claimed}))
}

fn missing(id: u32) -> Error {
    refuse(&fill(&TEXT.no_task, &[("id", &id)]))
}

/// The notes owed once tasks open in `before` have closed: each session of `server` holding a
/// task one of them blocked hears which closed and why, and whether its task is now ready.
pub(super) fn closes(dir: &Path, server: &str, before: &[Task]) -> Result<Vec<Note>, Error> {
    let (all, mut notes) = (scan(dir, false)?, Vec::new());
    for gone in before.iter().filter(|t| t.open && !open(&all, t.id)) {
        let found = files::read(&dir.join("closed").join(format!("{}.md", gone.id)), "");
        let Ok(found) = found.map(|found| Task::read(gone.id, false, &found.file)) else {
            continue;
        };
        let why = found.get("closed").unwrap_or_default();
        let why = why.split_once(' ').map_or(why, |w| w.1);
        let why = why.trim_end_matches('.');
        let title = found.get("title").unwrap_or_default();
        let closed: [(_, &dyn std::fmt::Display); 3] =
            [("id", &gone.id), ("title", &title), ("why", &why)];
        for t in all.iter().filter(|t| t.blockers().any(|n| n == gone.id)) {
            let Some(holder) = t.holder(server) else {
                continue;
            };
            let waits = t.blockers().filter(|&n| open(&all, n));
            let waits = waits.map(|n| n.to_string()).collect::<Vec<_>>().join(", ");
            let said = [&TEXT.ready, &TEXT.waits][usize::from(!waits.is_empty())];
            let said = fill(&fill(said, &closed), &[("mine", &t.id), ("waits", &waits)]);
            notes.push((holder.to_owned(), said));
        }
    }
    Ok(notes)
}

/// The page's tasks part for `session` of `server`: how many are ready, then the open tasks it
/// holds and those due within a week, overdue ones included; when none is open, nothing, or for
/// a `shared` project, that `task list` shows them: this home may not have fetched them yet.
pub(super) fn page(dir: &Path, session: &str, server: &str, shared: bool) -> Result<String, Error> {
    let all = scan(dir, false)?;
    if !all.iter().any(|t| t.open) {
        let said = if shared { &*TEXT.shared } else { "" };
        return Ok(said.into());
    }
    let ready = states(&all).iter().filter(|s| **s == "ready").count();
    let week = jiff::Span::new().days(7);
    let soon = jiff::Zoned::now().date().saturating_add(week).to_string();
    let line = |t: &Task| {
        let due = t.get("due");
        let said = match due {
            _ if t.holder(server) == Some(session) => &TEXT.held,
            Some(due) if due <= soon.as_str() => &TEXT.owed,
            _ => return None,
        };
        let due = due.map(|d| fill(&TEXT.due_on, &[("date", &d)]));
        let (due, title) = (due.unwrap_or_default(), t.get("title").unwrap_or_default());
        let said = fill(said, &[("id", &t.id), ("due", &due), ("title", &title)]);
        Some(said)
    };
    let lines = fit(all.iter().filter_map(line), PAGE).0;
    let heading = fill(&TEXT.tasks, &[("ready", &ready)]);
    Ok(format!("{heading}\n{lines}").trim_end().into())
}

/// The folder of the project kept for `root`, an absolute path, within the tasks folder: a
/// folder for each part of its path, `%` written `%25`, then `%`, which no part can be, so no
/// name is too long and no project's folder is inside another's.
pub(super) fn folder(root: &Path) -> PathBuf {
    let part = |p: &std::ffi::OsStr| p.to_string_lossy().replace('%', "%25");
    root.iter().skip(1).map(part).chain(["%".into()]).collect()
}

/// The project of each folder `folder` made within the tasks folder `dir`.
pub(super) fn projects(dir: &Path) -> Vec<PathBuf> {
    let (mut dirs, mut found) = (vec![dir.to_owned()], Vec::new());
    while let Some(at) = dirs.pop() {
        for entry in fs::read_dir(&at).into_iter().flatten().flatten() {
            if entry.file_name() == "%" {
                let parts = at.strip_prefix(dir).unwrap_or(&at).iter();
                let parts = parts.map(|p| p.to_string_lossy().replace("%25", "%"));
                found.push(Path::new("/").join(parts.collect::<PathBuf>()));
            } else if entry.file_type().is_ok_and(|t| t.is_dir()) {
                dirs.push(entry.path());
            }
        }
    }
    found
}
