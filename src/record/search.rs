//! Past-session search: beside each top-level session's record, its
//! `search.txt` holds a line per entry anyone searches; a search reads each file from its end, a
//! window at a time, and keeps nothing between searches.

use super::Summary;
use crate::protocol::{
    Body, Effort, Entry, EntryId, Hit, ModelSettings, Origin, Search, SessionId, Speed,
};
use memchr::{memchr, memmem, memrchr, memrchr_iter};
use std::os::unix::fs::FileExt;
use std::{cmp::Reverse, fs, io::BufRead, io::Write, path::Path, path::PathBuf};

/// A line per entry, tab-separated: who it is from (empty on a name line), its id, its time,
/// its text folded, its text on one line; the first line, the Start's, then adds the session's
/// folder, resident and resident project. Derived from the record, so deleting it while the
/// server is stopped rebuilds it at the next start.
pub(super) const FILE: &str = "search.txt";

/// The most of a search file a search holds at once; a longer line is never found.
const WINDOW: usize = 1 << 20;

/// The kinds of entry a line comes from, as each record line's `kind` begins.
const KINDS: [&str; 5] = ["start\"", "name\"", "input\"", "text\"", "child\""];

/// The base letter of each of U+00E0 to U+017F, or `.` where the letter stays itself.
const LATIN: &[u8; 160] = b"aaaaaa.ceeeeiiii.nooooo.ouuuuy.yaaaaaaccccccccddddeeeeeeeeeegggggggghhhhiiiiiiiiii..jjkk.llllllllllnnnnnn...oooooo..rrrrrrssssssssttttttuuuuuuuuuuuuwwyyyzzzzzzs";

/// One character for each: lowercase, a Latin letter without its accent, and a space for
/// anything but a letter or digit, so a word begins at the start or after a space.
fn fold(text: &str) -> String {
    let fold = |c: char| {
        let c = c.to_lowercase().next().unwrap_or(c);
        match LATIN.get((c as usize).wrapping_sub(0xE0)) {
            _ if !c.is_alphanumeric() => ' ',
            Some(&base) if base != b'.' => char::from(base),
            _ => c,
        }
    };
    text.chars().map(fold).collect()
}

/// `entry`'s line once `s` has folded it: none in a child's or a fresh run's record, and none
/// for tool calls, thinking or Razor's own inputs.
pub(super) fn line(entry: &Entry, s: &Summary) -> Option<String> {
    if s.child.is_some() || s.thread.as_ref().is_some_and(|t| t.job.is_some()) {
        return None;
    }
    let flat = |text: &str| text.replace(['\t', '\n', '\r'], " ");
    let mut start = String::new();
    let (from, text) = match &entry.body {
        // The Start's own folder: a backfill's summary has none.
        Body::Start { cwd, thread, .. } => {
            let t = thread.as_deref();
            let resident = t.map_or("", |t| t.resident.as_str());
            let project = t.and_then(|t| t.resident_project.as_deref());
            let cwd = flat(&cwd.to_string_lossy());
            start = format!("\t{cwd}\t{resident}\t{}", project.unwrap_or_default());
            ("", &s.name)
        }
        Body::Name { .. } => ("", &s.name),
        Body::Text { text } => ("agent", text),
        Body::Input(input) => match &input.from {
            Origin::Person | Origin::Answer { .. } => ("person", &input.text),
            Origin::Mail { from, .. } if from.is_empty() => ("mail", &input.text),
            Origin::Mail { from, .. } => (from.as_str(), &input.text),
            Origin::Report { child, .. } => {
                let mut children = s.children.iter();
                let named = children.find(|c| c.session == *child);
                (named.map_or(child.as_str(), |c| &c.child.name), &input.text)
            }
            _ => return None,
        },
        _ => return None,
    };
    let (id, at, from, fold, text) = (entry.id.0, entry.at, flat(from), fold(text), flat(text));
    Some(format!("{from}\t{id}\t{at}\t{fold}\t{text}{start}\n"))
}

/// Writes the search file of each session that has none, from its record, before the server
/// starts any session, so nothing is appended meanwhile.
pub async fn backfill(home: &Path) {
    let home = home.to_owned();
    let build_all = move || {
        for (session, dir) in sessions(&home).filter(|(_, dir)| !dir.join(FILE).exists()) {
            let _ = build(&dir, &session);
        }
    };
    let _ = tokio::task::spawn_blocking(build_all).await;
}

/// Decodes only the kinds a line comes from, a child's or fresh run's record only to its Start,
/// and stops at the first line it cannot read.
fn build(dir: &Path, session: &SessionId) -> std::io::Result<()> {
    let record = fs::File::open(dir.join("record.jsonl"))?;
    let settings = ModelSettings {
        effort: Effort::Low,
        speed: Speed::Standard,
    };
    let mut s = Summary::new(session, Path::new(""), "", settings);
    let made = dir.join("search.txt.new");
    let mut file = std::io::BufWriter::new(fs::File::create(&made)?);
    for stored in std::io::BufReader::new(record)
        .lines()
        .map_while(Result::ok)
    {
        let kind = stored.split_once(r#""kind":""#).unwrap_or_default().1;
        if KINDS.iter().any(|k| kind.starts_with(k))
            && let Ok(entry) = serde_json::from_str::<Entry>(&stored)
        {
            s.fold(&entry);
            match line(&entry, &s) {
                Some(line) => file.write_all(line.as_bytes())?,
                None if matches!(entry.body, Body::Start { .. }) => break,
                None => {}
            }
        }
    }
    file.flush()?;
    fs::rename(made, dir.join(FILE))
}

/// The newest entry of each top-level session holding every word of `q` (or whose latest name
/// holds it), newest first, at most `limit`; with `q.session`, each such entry of that one.
/// None for a query of no words, more than 8, or over 200 bytes. On a blocking thread it reads
/// only search files, each from its end until it holds its newest hit.
pub(crate) async fn search(home: &Path, q: &Search, limit: usize) -> Option<Vec<Hit>> {
    let folded = fold(&q.q);
    let words: Vec<String> = folded.split_whitespace().map(str::to_owned).collect();
    if q.q.len() > 200 || !(1..=8).contains(&words.len()) {
        return None;
    }
    let (home, q) = (home.to_owned(), q.clone());
    let scan = move || scan(&home, &q, &words, limit);
    Some(tokio::task::spawn_blocking(scan).await.unwrap_or_default())
}

fn scan(home: &Path, q: &Search, words: &[String], limit: usize) -> Vec<Hit> {
    let asked = |s: &SessionId| q.session.as_ref().is_none_or(|q| q == s);
    let each = if q.session.is_some() { limit } else { 1 };
    let (mut hits, mut buf) = (Vec::<Hit>::new(), vec![0; WINDOW]);
    for (session, dir) in sessions(home).filter(|(s, _)| asked(s)) {
        let Ok(file) = fs::File::open(dir.join(FILE)) else {
            continue;
        };
        let len = file.metadata().map_or(0, |m| m.len());
        let head = &mut buf[..len.min(WINDOW as u64) as usize];
        let read = file.read_exact_at(head, 0).is_ok();
        let first = memchr(b'\n', head).filter(|_| read);
        let Some(start) = first.and_then(|n| parse(&head[..n], 0)) else {
            continue;
        };
        let mut facts = start.start.split(|b| *b == b'\t').map(lossy);
        let cwd = PathBuf::from(facts.next().unwrap_or_default());
        let resident = facts.next().and_then(|r| SessionId::try_from(r).ok());
        let resident_project = facts.next().filter(|p| !p.is_empty());
        if (q.resident.as_ref()).is_some_and(|r| resident.as_ref() != Some(r) && *r != session) {
            continue;
        }
        // The latest name: the last line that begins with a tab.
        let mut name = None;
        back(&file, len, &mut buf, |lines| {
            let starts = memrchr_iter(b'\n', lines).map(|n| n + 1).chain([0]);
            let at = { starts }.find(|&n| lines.get(n) == Some(&b'\t'));
            let line = at.and_then(|at| parse(lines, at));
            name = line.map(|l| (l.fold.to_vec(), lossy(l.text)));
            name.is_none()
        });
        let Some((name_fold, name)) = name else {
            continue;
        };
        // The entry must hold the words its name does not; lines holding the longest are read.
        let rest: Vec<&String> = words
            .iter()
            .filter(|w| begins(&name_fold, w).is_none())
            .collect();
        let longest = rest.iter().map(|w| w.as_bytes()).max_by_key(|w| w.len());
        let finder = memmem::FinderRev::new(longest.unwrap_or(b"\t"));
        let mut found = 0;
        back(&file, len, &mut buf, |lines| {
            let mut end = lines.len();
            while found < each
                && let Some(i) = finder.rfind(&lines[..end])
            {
                end = memrchr(b'\n', &lines[..i]).map_or(0, |n| n + 1);
                let holds = |l: &Line| rest.iter().all(|w| begins(l.fold, w).is_some());
                if let Some(line) = parse(lines, end).filter(|l| !l.from.is_empty() && holds(l)) {
                    found += 1;
                    hits.push(Hit {
                        session: session.clone(),
                        name: name.clone(),
                        cwd: cwd.clone(),
                        resident: resident.clone(),
                        resident_project: resident_project.clone(),
                        at: line.at,
                        entry: EntryId(line.entry),
                        from: lossy(line.from),
                        snippet: snippet(&line, words),
                    });
                }
            }
            found < each
        });
        hits.sort_unstable_by_key(|h| Reverse(h.at));
        hits.truncate(limit);
    }
    hits
}

/// Hands `each` the file's whole lines a window at a time, from its end, until it answers false
/// or the file has none left; a torn last line, or one longer than a window, is left out. A file
/// of `len` bytes that fits one window is already in it.
fn back(file: &fs::File, len: u64, window: &mut [u8], mut each: impl FnMut(&[u8]) -> bool) {
    let mut end = len;
    while end > 0 {
        let start = end.saturating_sub(WINDOW as u64);
        let buf = &mut window[..(end - start) as usize];
        if len > WINDOW as u64 && file.read_exact_at(buf, start).is_err() {
            return;
        }
        let tail = memrchr(b'\n', buf).map_or(0, |n| n + 1);
        let head = memchr(b'\n', &buf[..tail]).filter(|_| start > 0);
        let head = head.map_or(if start > 0 { tail } else { 0 }, |n| n + 1);
        if !each(&buf[head..tail]) {
            return;
        }
        // A window holding no line's start lies inside one line, which is skipped.
        end = start + if head < buf.len() { head as u64 } else { 0 };
    }
}

/// Each session's id and folder.
fn sessions(home: &Path) -> impl Iterator<Item = (SessionId, PathBuf)> {
    let dirs = fs::read_dir(home.join("sessions")).into_iter().flatten();
    let id = |d: &fs::DirEntry| SessionId::try_from(d.file_name().into_string().ok()?).ok();
    dirs.flatten()
        .filter_map(move |d| Some((id(&d)?, d.path())))
}

/// One line of a search file.
struct Line<'a> {
    from: &'a [u8],
    entry: u64,
    at: u64,
    fold: &'a [u8],
    text: &'a [u8],
    /// The Start line's folder, resident and resident project; empty on any other line.
    start: &'a [u8],
}

/// The line beginning at `at`, unless it is not a whole line of ours.
fn parse(buf: &[u8], at: usize) -> Option<Line<'_>> {
    let end = memchr(b'\n', &buf[at..]).map_or(buf.len(), |n| at + n);
    let mut fields = buf[at..end].splitn(6, |b| *b == b'\t');
    let number = |field: Option<&[u8]>| std::str::from_utf8(field?).ok()?.parse().ok();
    Some(Line {
        from: fields.next()?,
        entry: number(fields.next())?,
        at: number(fields.next())?,
        fold: fields.next()?,
        text: fields.next()?,
        start: fields.next().unwrap_or_default(),
    })
}

/// Where `word` first begins a word of the folded `text`.
fn begins(text: &[u8], word: &str) -> Option<usize> {
    memmem::find_iter(text, word.as_bytes()).find(|&i| i == 0 || text[i - 1] == b' ')
}

/// The line's text from shortly before the first word it holds, at most 200 characters: the
/// folded text has one character for each of the text's.
fn snippet(line: &Line, words: &[String]) -> String {
    let at = words.iter().filter_map(|w| begins(line.fold, w)).min();
    let before = lossy(&line.fold[..at.unwrap_or(0)]).chars().count();
    let skip = before.saturating_sub(40);
    lossy(line.text).chars().skip(skip).take(200).collect()
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
