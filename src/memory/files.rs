//! Razor's own memory (docs/design/memory.md): one Markdown file per memory in the server's
//! home, in `memory/all/` for every project and in `memory/projects/<the project's path>/` for
//! one, and a retired memory at the same place under `memory/retired/`. A file's name says its
//! kind, the project's state note is `state.md`, and a header between `---` lines holds its
//! source, any expiry and a fact's or note's description. Nothing is held between calls, and
//! everything here runs on a blocking thread. Task files share the reader, header and writer.

use super::{Answer, Error, Kind, TEXT, Write, other, refuse, words};
use serde_json::{Value, json};
use std::collections::hash_map::RandomState;
use std::fs::{self, File};
use std::hash::{BuildHasher, Hasher};
use std::io::{ErrorKind, Read, Write as _};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

/// The most of one file a read holds: a written memory's 16 KiB of text and its header.
pub(super) const FILE: u64 = 17 * 1024;
/// The most memories one read keeps: the page's newest rulings, or the person's list.
pub(super) const KEEP: usize = 64;
/// The startup page's rulings, and its list of what is known: at most this many lines, in this
/// many bytes (about 1,000 and 600 tokens).
const RULINGS: (usize, usize) = (20, 4000);
const KNOWN: (usize, usize) = (24, 2400);

/// The folders of one project's memories and of every project's, each pair in that order.
pub(super) struct Folders {
    current: [PathBuf; 2],
    retired: [PathBuf; 2],
}

/// One memory or task file as a read found it: at most `FILE` bytes, and when it last changed.
pub(super) struct Found {
    id: String,
    at: SystemTime,
    pub(super) file: String,
}

/// The memory folders of the project kept for `root` and of every project: its path's
/// components under `projects/`.
pub(super) fn folders(dir: &Path, root: &Path) -> Folders {
    let normal = |c: &Component| matches!(c, Component::Normal(_));
    let project = root.components().filter(normal).collect::<PathBuf>();
    let both = |base: PathBuf| [base.join("projects").join(&project), base.join("all")];
    let (current, retired) = (both(dir.into()), both(dir.join("retired")));
    Folders { current, retired }
}

/// The page's memory part: the state note, the rulings (standing ones first, then those with an
/// expiry, each newest first) and the descriptions of facts and notes, newest first, then how
/// many more there are.
pub(super) fn page(folders: &Folders) -> Result<(Option<String>, String, String), Error> {
    let today = jiff::Zoned::now().date().to_string();
    let state = read(&folders.current[0].join("state.md"), "state").ok();
    let state = state.filter(|f| !expired(&f.file, &today));
    let standing = |f: &Found| Some(usize::from(header(&f.file, "expires").is_none()));
    let rulings = best(folders, KEEP, |id| kind(id) == "ruling", standing)?.0;
    let line = |f: &Found| {
        let until = header(&f.file, "expires").map(|d| TEXT.until.replace("{date}", d));
        let said = words(split(&f.file).1);
        format!("- {}{said} ({})", until.unwrap_or_default(), f.id)
    };
    let rulings = fit(rulings.iter().map(line), RULINGS).0;
    let state = state.map(|f| split(&f.file).1.to_owned());
    Ok((state, rulings, known(folders, KNOWN)?))
}

/// The descriptions of facts and notes, newest first, in at most `most` lines and bytes, then a
/// line counting the rest: the page's list, and the night's when longer.
pub(super) fn known(folders: &Folders, most: (usize, usize)) -> Result<String, Error> {
    let pick = |id: &str| matches!(kind(id), "fact" | "note");
    let (found, more) = best(folders, most.0, pick, |_| Some(0))?;
    let line = |f: &Found| {
        let first = split(&f.file).1.lines().find(|l| !l.trim().is_empty());
        let first = first.map_or("", |l| &l[..l.floor_char_boundary(200)]);
        let said = header(&f.file, "description").unwrap_or(first);
        format!("- {said} ({})", f.id)
    };
    let (mut lines, left) = fit(found.iter().map(line), most);
    if more + left > 0 {
        lines += &format!("\n{}", TEXT.more.replace("{n}", &(more + left).to_string()));
    }
    Ok(lines)
}

/// The memories whose text holds the most of `query`'s words, matched whole in lowercase,
/// newest first among equals; every memory when it has no words.
pub(super) fn search(folders: &Folders, query: &str, limit: usize) -> Answer {
    let rank = |f: &Found| {
        let (held, wanted) = held(split(&f.file).1, query);
        (held > 0 || wanted == 0).then_some(held)
    };
    let (found, more) = best(folders, limit, |_| true, rank)?;
    Ok(hits(&found, more))
}

/// How many of `query`'s words `text` holds, matched whole in lowercase, and how many it has.
pub(super) fn held(text: &str, query: &str) -> (usize, usize) {
    let (text, query) = (text.to_lowercase(), query.to_lowercase());
    let wanted: Vec<&str> = query.split(apart).filter(|w| !w.is_empty()).collect();
    let holds = |w: &&&str| text.split(apart).any(|t| t == **w);
    (wanted.iter().filter(holds).count(), wanted.len())
}

/// Memory `id`: its kind, and its file as it is written, header and all.
pub(super) fn get(folders: &Folders, id: &str) -> Answer {
    let (_, _, found) = find(folders, id)?;
    Ok(json!({"id": id, "kind": kind(id), "file": found.file}))
}

/// Writes `w` as a new file, or over `state.md` for a state note, after retiring the memory it
/// replaces: of writers replacing one memory, the one whose rename succeeds writes, and the rest
/// are refused. A crash between leaves the old words retired.
pub(super) fn write(folders: &Folders, w: Write) -> Answer {
    if let Some(old) = &w.replaces {
        retire(folders, old, false)?;
    }
    let tail = stamp();
    let id = match w.kind {
        Kind::Ruling => format!("ruling-{tail}"),
        Kind::Fact => format!("fact-{tail}"),
        Kind::Note => format!("note-{tail}"),
        Kind::State => "state".into(),
    };
    let line = |key, value: Option<String>| value.map(|v| format!("{key}: {v}\n"));
    let tail = line("expires", w.expires).unwrap_or_default();
    let tail = tail + &line("description", w.description).unwrap_or_default();
    let head = format!("---\nsource: {}\n{tail}---\n", w.source);
    let folder = &folders.current[usize::from(w.all)];
    put(folder, &format!("{id}.md"), &(head + &w.text))?;
    Ok(json!({"id": id, "replaced": w.replaces}))
}

/// Moves memory `id` into the retired tree, where search, get and the page no longer see it;
/// with `forget`, the person's word, deletes it instead, words and all. It is one rename or
/// removal, so of calls that retire one memory at once, one succeeds and the others are refused.
pub(super) fn retire(folders: &Folders, id: &str, forget: bool) -> Answer {
    let (path, to, _) = find(folders, id)?;
    let done = match forget {
        true => fs::remove_file(&path),
        false => {
            fs::create_dir_all(to.parent().unwrap_or(&to)).and_then(|_| fs::rename(&path, &to))
        }
    };
    done.map_err(|e| match e.kind() {
        ErrorKind::NotFound => refuse(&TEXT.gone),
        _ => other("retiring a memory file")(e),
    })?;
    Ok(json!({ "retired": id }))
}

/// Writes `text` as `name` in `folder`, making it: a temporary dotfile, `sync_all`, then one
/// rename, so a reader or a crash sees the old file or the new one, never half of one.
pub(super) fn put(folder: &Path, name: &str, text: &str) -> Result<(), Error> {
    let temp = folder.join(format!(".{}.tmp", stamp()));
    let put = || {
        fs::create_dir_all(folder)?;
        let mut out = File::create_new(&temp)?;
        out.write_all(text.as_bytes())?;
        out.sync_all()?;
        fs::rename(&temp, folder.join(name))
    };
    put().map_err(other("writing a memory or task file"))
}

/// The time to the second and eight random hex digits: a new file's name, unique in practice.
pub(super) fn stamp() -> String {
    let random = RandomState::new().build_hasher().finish() as u32;
    let time = jiff::Timestamp::now().strftime("%Y%m%d-%H%M%S");
    format!("{time}-{random:08x}")
}

/// The `keep` memories in `folders` whose ids `pick` takes and that `rank` ranks, higher first
/// and then newest, read one file at a time; and how many more it ranked. A memory past its
/// `expires` date is moved into the retired tree as the read passes it.
fn best(
    folders: &Folders,
    keep: usize,
    pick: impl Fn(&str) -> bool,
    rank: impl Fn(&Found) -> Option<usize>,
) -> Result<(Vec<Found>, usize), Error> {
    let today = jiff::Zoned::now().date().to_string();
    let (mut kept, mut ranked) = (Vec::<(usize, Found)>::new(), 0);
    for (folder, retired) in folders.current.iter().zip(&folders.retired) {
        let entries = match fs::read_dir(folder) {
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            entries => entries.map_err(other("reading memory files"))?,
        };
        for entry in entries.flatten() {
            let name = entry.file_name().into_string().unwrap_or_default();
            let id = name.strip_suffix(".md").filter(|id| pick(id));
            // A folder, a pipe, or a file removed since the listing is no memory.
            let Some(found) = id.and_then(|id| read(&entry.path(), id).ok()) else {
                continue;
            };
            if expired(&found.file, &today) {
                let moved = fs::create_dir_all(retired)
                    .and_then(|_| fs::rename(entry.path(), retired.join(&name)));
                moved.unwrap_or_else(|e| eprintln!("razor: retiring expired memory {name}: {e}"));
                continue;
            }
            let Some(rank) = rank(&found) else {
                continue;
            };
            ranked += 1;
            let at = kept.partition_point(|(r, f)| (*r, f.at, &f.id) > (rank, found.at, &found.id));
            kept.insert(at, (rank, found));
            kept.truncate(keep);
        }
    }
    let more = ranked - kept.len();
    Ok((kept.into_iter().map(|(_, found)| found).collect(), more))
}

/// The lines that fit, in order: at most `most.0` of them in `most.1` bytes, passing over a line
/// that does not fit rather than cutting it; and how many were passed over.
pub(super) fn fit(lines: impl Iterator<Item = String>, most: (usize, usize)) -> (String, usize) {
    let (mut kept, mut room, mut left) = (Vec::new(), most.1, 0);
    for line in lines {
        if kept.len() < most.0 && line.len() < room {
            room -= line.len() + 1;
            kept.push(line);
        } else {
            left += 1;
        }
    }
    (kept.join("\n"), left)
}

/// Memory `id`'s path among the current memories in `folders`, its place in the retired tree
/// and its file, or the refusal that says there is none; an id holding `/` could name a file
/// outside them, so it names none.
fn find(folders: &Folders, id: &str) -> Result<(PathBuf, PathBuf, Found), Error> {
    let name = format!("{id}.md");
    let inside = folders.current.iter().zip(&folders.retired);
    for (folder, retired) in inside.filter(|_| !id.contains('/')) {
        if let Ok(found) = read(&folder.join(&name), id) {
            return Ok((folder.join(&name), retired.join(&name), found));
        }
    }
    Err(refuse(&TEXT.no_memory.replace("{id}", id)))
}

/// The file at `path`, if it is a regular file: opened without waiting on a pipe, so a pipe is
/// no memory and no task.
pub(super) fn read(path: &Path, id: &str) -> std::io::Result<Found> {
    let file = File::options()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(ErrorKind::InvalidInput.into());
    }
    let at = meta.modified()?;
    let mut bytes = Vec::new();
    file.take(FILE).read_to_end(&mut bytes)?;
    let (id, file) = (id.into(), String::from_utf8_lossy(&bytes).into_owned());
    Ok(Found { id, at, file })
}

/// A file's header and its text, lines ending in LF or CRLF; a file without a header is all
/// text.
pub(super) fn split(file: &str) -> (&str, &str) {
    let rest = file
        .strip_prefix("---\n")
        .or_else(|| file.strip_prefix("---\r\n"));
    rest.and_then(|r| {
        r.split_once("\n---\n")
            .or_else(|| r.split_once("\n---\r\n"))
    })
    .unwrap_or(("", file))
}

/// The header lines of a file, each a key and its value.
pub(super) fn head(file: &str) -> impl Iterator<Item = (&str, &str)> {
    let pairs = split(file).0.lines().filter_map(|l| l.split_once(": "));
    pairs.map(|(key, value)| (key, value.trim_end()))
}

/// The value of header line `key` in `file`.
fn header<'a>(file: &'a str, key: &str) -> Option<&'a str> {
    head(file).find(|(k, _)| *k == key).map(|(_, value)| value)
}

/// Whether the memory in `file` is past its `expires` date, `today` being a date like it.
fn expired(file: &str, today: &str) -> bool {
    header(file, "expires").is_some_and(|date| date < today)
}

/// A memory's kind, from its id: `ruling-…`, `fact-…`, `state`; any other is a note.
fn kind(id: &str) -> &str {
    match id.split_once('-').map_or(id, |(kind, _)| kind) {
        kind @ ("ruling" | "fact" | "state") => kind,
        _ => "note",
    }
}

/// Memories as search and the person's list show them: by a description, else a ruling by its
/// page line and any other by its text on one line, each at most 400 bytes.
fn hits(found: &[Found], omitted: usize) -> Value {
    let hit = |f: &Found| {
        let text = || words(split(&f.file).1);
        let summary = header(&f.file, "description").map_or_else(text, str::to_owned);
        let at = jiff::Timestamp::try_from(f.at).map(|t| t.strftime("%Y-%m-%dT%H:%M:%SZ"));
        let at = at.map(|at| at.to_string()).unwrap_or_default();
        let summary = &summary[..summary.floor_char_boundary(400)];
        json!({"id": f.id, "kind": kind(&f.id), "summary": summary, "updated_at": at})
    };
    json!({"hits": found.iter().map(hit).collect::<Vec<_>>(), "omitted_count": omitted})
}

/// Whether `c` separates words: anything but a letter or a digit.
fn apart(c: char) -> bool {
    !c.is_alphanumeric()
}
