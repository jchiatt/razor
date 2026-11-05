//! What a conversation opens with besides its tools: the startup list read into the system
//! prompt, the session's facts for its first input, and `startup.md`, which shows both.

use super::Profile;
use crate::record::Summary;
use serde_json::Value;
use std::{
    collections::HashSet,
    io::Read,
    path::{Path, PathBuf},
    sync::LazyLock,
    time::Duration,
};

/// The editable part of `razor`, which the person may copy and name in its place.
const RAZOR: &str = include_str!("data/razor.md");
/// The most one listed or included file may hold, and the whole list.
const FILE: u64 = 64 << 10;
const LIST: usize = 1 << 20;
/// The most `.md` files a folder entry gives, in name order.
const FOLDER: usize = 64;

static TEXT: LazyLock<Value> =
    LazyLock::new(|| super::shipped(include_str!("data/startup.json")).unwrap_or_default());

fn text(key: &str) -> &'static str {
    TEXT[key].as_str().unwrap_or_default()
}

/// What a conversation opens with besides its tools.
pub(crate) struct Opening {
    /// Each entry's text in the list's order, joined by one blank line, with nothing of Razor's
    /// between entries; it holds the profile's route part exactly once.
    pub(crate) prompt: String,
    /// The head of the first input: the session's facts, a resident's identity record, and one
    /// line per entry left out.
    facts: String,
}

/// Reads the startup list for one opening. An entry is `razor` (the editable part, then the
/// profile's route part), `project` (in each folder from `root` down to `cwd`, its `AGENTS.md`
/// then its `CLAUDE.md`, each under a line naming its path), `about` (the person's `about.md`
/// in Razor's `home`, left out without a word when there is none), a file, or a folder (its
/// first 64 `.md` files by name). A line that is only `@<path>` (relative to its file, or `~/`)
/// is replaced by that file's text; one naming no file stays as written. Each file and word is
/// used once; a file is at most 64 KiB and the whole at most 1 MiB; what cannot be used is left
/// out and named in `facts`, never fatal. A list without `razor` gets the route part after its
/// last entry. Reads run on a thread of their own; git has two seconds to name the branch, and
/// no branch is said when it cannot. The facts name the session by the name it answers to
/// (`Summary::name`).
pub(crate) async fn opening(
    profile: &Profile,
    list: &[String],
    root: Option<&Path>,
    s: &Summary,
    identity: Option<String>,
    home: &Path,
) -> Opening {
    let cwd = &s.cwd;
    let (list, route, folder) = (list.to_vec(), profile.route.clone(), cwd.to_path_buf());
    let (root, about) = (root.unwrap_or(cwd).to_path_buf(), home.join("about.md"));
    let read = tokio::task::spawn_blocking(move || read(&list, route, &root, &folder, &about));
    let (read, branch) = tokio::join!(read, branch(cwd));
    let (prompt, skipped) = read.unwrap_or_else(|e| (profile.route.clone(), vec![e.to_string()]));
    let now = jiff::Zoned::now();
    let fields = [
        ("{name}", s.name.clone()),
        ("{session}", s.session.as_str().to_owned()),
        ("{date}", now.strftime("%A %Y-%m-%d").to_string()),
        ("{time}", now.strftime("%H:%M %Z (%Q, UTC%:z)").to_string()),
        ("{machine}", machine()),
        ("{folder}", cwd.display().to_string()),
    ];
    let mut facts = (fields.iter()).fold(text("facts").into(), |t: String, (k, v)| t.replace(k, v));
    facts.extend(branch.map(|branch| text("branch").replace("{branch}", &branch)));
    facts.extend(skipped);
    facts.extend(identity.map(|identity| format!("\n\n{identity}")));
    Opening { prompt, facts }
}

impl Opening {
    /// The first input's prefix: the facts, the memory page, the skills, then the MCP servers,
    /// each part present at most once and separated by a blank line.
    pub(crate) fn page(&self, memory: String, skills: Option<String>, mcp: &str) -> String {
        let parts = [
            Some(&*self.facts),
            Some(&*memory),
            skills.as_deref(),
            Some(mcp),
        ];
        let parts: Vec<_> = parts
            .into_iter()
            .flatten()
            .filter(|p| !p.is_empty())
            .collect();
        parts.join("\n\n")
    }
}

/// Replaces `dir/startup.md` with what an opening sent ahead of the first input: the system
/// prompt, the tool declarations and the first input's prefix, each under a heading with its
/// size. Written to a sibling and renamed, so a reader never sees half a file; a failure is
/// logged and never stops the opening.
pub(crate) async fn save(dir: &Path, prompt: &str, tools: &[Value], page: &str) {
    let sent = serde_json::to_string(tools).map_or(0, |json| json.len());
    let tools = serde_json::to_string_pretty(tools).unwrap_or_default();
    let tools = format!("```json\n{tools}\n```");
    let mut file = text("title").to_owned();
    for (key, part, size) in [
        ("prompt", prompt, prompt.len()),
        ("tools", &tools, sent),
        ("page", page, page.len()),
    ] {
        let heading = text(key).replace("{size}", &size.to_string());
        file += &format!("\n\n{heading}\n\n{part}");
    }
    let (path, part) = (dir.join("startup.md"), dir.join("startup.md.part"));
    let written = async {
        tokio::fs::write(&part, file + "\n").await?;
        tokio::fs::rename(&part, &path).await
    };
    if let Err(e) = written.await {
        eprintln!("razor: could not write {}: {e}", path.display());
    }
}

/// The prompt, and a line for each file left out.
fn read(
    list: &[String],
    route: String,
    root: &Path,
    cwd: &Path,
    about: &Path,
) -> (String, Vec<String>) {
    let mut reader = Reader::default();
    let mut parts = Vec::new();
    for entry in list {
        let word = matches!(entry.as_str(), "razor" | "project" | "about");
        if word && !reader.seen.insert(PathBuf::from(entry)) {
            continue;
        }
        match entry.as_str() {
            "razor" => parts.push(format!("{}\n\n{route}", RAZOR.trim_end())),
            "project" => {
                for file in project(root, cwd) {
                    let label = text("project").replace("{path}", &file.display().to_string());
                    parts.extend(reader.file(&file).map(|t| format!("{label}\n\n{t}")));
                }
            }
            "about" if about.is_file() => parts.extend(reader.file(about)),
            "about" => {}
            path => match std::fs::read_dir(path) {
                Ok(folder) => {
                    let mut files: Vec<_> = (folder.flatten().map(|f| f.path()))
                        .filter(|p| p.extension().is_some_and(|e| e == "md") && p.is_file())
                        .collect();
                    files.sort();
                    for file in files.iter().take(FOLDER) {
                        parts.extend(reader.file(file));
                    }
                }
                Err(_) => parts.extend(reader.file(Path::new(path))),
            },
        }
    }
    if !reader.seen.contains(Path::new("razor")) {
        parts.push(route);
    }
    (parts.join("\n\n"), reader.skipped)
}

#[derive(Default)]
struct Reader {
    /// Every file read, and the words used.
    seen: HashSet<PathBuf>,
    size: usize,
    skipped: Vec<String>,
}

impl Reader {
    /// A file's text with each `@` line replaced by the text of the file it names, trimmed at
    /// its end, and so on down; None when it was used before or is left out. One walk with a
    /// stack of the files open, so a long chain of includes costs memory, never call depth.
    fn file(&mut self, path: &Path) -> Option<String> {
        let mut out = String::new();
        // Each open file, the lines it has left, and where its text starts in `out`.
        let mut open = vec![(path.to_path_buf(), self.lines(path)?, 0)];
        while let Some((from, lines, start)) = open.last_mut() {
            let start = *start;
            match lines.next().map(|line| (include(&line, from), line)) {
                Some((None, line)) => out.push_str(&line),
                Some((Some(file), _)) => {
                    if let Some(lines) = self.lines(&file) {
                        open.push((file, lines, out.len()));
                        continue;
                    }
                }
                None => {
                    out.truncate(start + out[start..].trim_end().len());
                    open.pop();
                    if open.is_empty() {
                        break;
                    }
                }
            }
            out.push('\n');
        }
        Some(out)
    }

    /// A file's lines, counted against the limits; None when it was used before or is left
    /// out, which `skipped` names.
    fn lines(&mut self, path: &Path) -> Option<std::vec::IntoIter<String>> {
        let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if !self.seen.insert(key) {
            return None;
        }
        let mut body = String::new();
        let read =
            std::fs::File::open(path).and_then(|f| f.take(FILE + 1).read_to_string(&mut body));
        let why = match read {
            Err(e) => Some(e.to_string()),
            Ok(n) if n as u64 > FILE => Some(text("large").to_owned()),
            Ok(n) if self.size + n > LIST => Some(text("full").to_owned()),
            Ok(n) => {
                self.size += n;
                None
            }
        };
        if let Some(why) = why {
            let line = text("skipped").replace("{path}", &path.display().to_string());
            self.skipped.push(line.replace("{why}", &why));
            return None;
        }
        let lines: Vec<_> = body.trim_end().lines().map(str::to_owned).collect();
        Some(lines.into_iter())
    }
}

/// The file a line that is only `@<path>` names, when there is one.
fn include(line: &str, from: &Path) -> Option<PathBuf> {
    let name = line
        .strip_prefix('@')
        .filter(|n| !n.is_empty() && !n.contains(char::is_whitespace))?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let path = match (name.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => from.parent().unwrap_or(Path::new("/")).join(name),
    };
    path.is_file().then_some(path)
}

/// In each folder from `root` down to `cwd`, its `AGENTS.md` then its `CLAUDE.md`, where they
/// exist; `cwd` alone when it is not under `root`.
fn project(root: &Path, cwd: &Path) -> Vec<PathBuf> {
    let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let depth = cwd
        .strip_prefix(root)
        .map_or(0, |below| below.components().count());
    let mut folders: Vec<_> = cwd.ancestors().take(depth + 1).collect();
    folders.reverse();
    let files = folders
        .into_iter()
        .flat_map(|f| ["AGENTS.md", "CLAUDE.md"].map(|n| f.join(n)));
    files.filter(|file| file.is_file()).collect()
}

/// The branch `cwd` is on, from git within two seconds.
async fn branch(cwd: &Path) -> Option<String> {
    let mut git = tokio::process::Command::new("git");
    git.args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .current_dir(cwd);
    let git = git
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(Duration::from_secs(2), git)
        .await
        .ok()?
        .ok()?;
    let branch = String::from_utf8(output.stdout).ok()?;
    (output.status.success() && !branch.trim().is_empty()).then(|| branch.trim().to_owned())
}

/// This machine's name; empty when the system cannot say, since the buffer then stays zeroed.
pub(crate) fn machine() -> String {
    let mut name = [0u8; 256];
    // SAFETY: gethostname writes at most the given length into the buffer it is handed.
    unsafe { libc::gethostname(name.as_mut_ptr().cast(), name.len()) };
    let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
    String::from_utf8_lossy(&name[..end]).into_owned()
}
