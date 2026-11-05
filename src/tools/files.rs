//! `read`, `write`, `edit` and `skill`.

use super::{Error, Tools, io, load, other};
use crate::protocol::{EntryId, IMAGE_BYTES, SPILL, image_type};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

/// A line `read` shows is cut at this many characters.
const LINE: usize = 2_000;

#[derive(Deserialize)]
pub(super) struct Read {
    path: String,
    offset: Option<u64>,
    limit: Option<u64>,
}

#[derive(Deserialize)]
pub(super) struct Write {
    path: String,
    content: String,
}

#[derive(Deserialize)]
pub(super) struct Edit {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

#[derive(Deserialize)]
pub(super) struct Skill {
    name: String,
}

/// A window of lines, streamed: at most `limit` lines and `SPILL` characters, never fewer than
/// one line, and no more than `LINE` characters of any line held. A PNG, JPEG or WebP file is
/// the image instead: a copy of at most `IMAGE_BYTES`, named after the call, for the model.
pub(super) async fn read(tools: &Tools, at: EntryId, input: Read) -> Result<Value, Error> {
    let path = tools.path(&input.path);
    let first = input.offset.unwrap_or(1).max(1);
    // Empty lines still occupy memory: no window holds more than SPILL lines.
    let limit = input.limit.unwrap_or(2_000).clamp(1, SPILL as u64);
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| io(e, &input.path))?;
    let mut reader = BufReader::new(file);
    let head = reader.fill_buf().await.map_err(|e| io(e, &input.path))?;
    if let Some((media_type, extension)) = image_type(head) {
        let copy = tools.dir.join(format!("images/read-{}.{extension}", at.0));
        let copied = async {
            tokio::fs::create_dir_all(tools.dir.join("images")).await?;
            let mut file = tokio::fs::File::create(&copy).await?;
            tokio::io::copy(&mut reader.take(IMAGE_BYTES as u64 + 1), &mut file).await
        };
        let copied = copied
            .await
            .map_err(|e| other(e, "Could not copy the image."))?;
        if copied > IMAGE_BYTES as u64 {
            let _ = tokio::fs::remove_file(&copy).await;
            return Err(Error::Refused(format!(
                "{} is an image over 7.5 MB; make a smaller copy with a command and read that.",
                input.path
            )));
        }
        return Ok(json!({"path": path, "images": [{"path": copy, "media_type": media_type}]}));
    }
    let mut number = 1; // the line being read
    let mut line = Vec::new(); // its first bytes, at most four per character shown
    let mut begun = false; // a byte of it has been read
    let mut lines = Vec::new();
    let mut size = 0;
    let next = loop {
        let buffer = reader.fill_buf().await.map_err(|e| io(e, &input.path))?;
        if buffer.is_empty() && !begun {
            break None;
        }
        if lines.len() as u64 == limit {
            break Some(number);
        }
        // At end of input an unfinished line ends there.
        let (length, used, ended) = match memchr::memchr(b'\n', buffer) {
            Some(i) => (i, i + 1, true),
            None => (buffer.len(), buffer.len(), buffer.is_empty()),
        };
        if number >= first {
            let room = (LINE * 4).saturating_sub(line.len());
            line.extend_from_slice(&buffer[..length.min(room)]);
        }
        reader.consume(used);
        begun = true;
        if !ended {
            continue;
        }
        if number >= first {
            let bytes = line.strip_suffix(b"\r").unwrap_or(&line);
            let text: String = String::from_utf8_lossy(bytes).chars().take(LINE).collect();
            let count = text.chars().count();
            if !lines.is_empty() && size + count > SPILL {
                break Some(number);
            }
            size += count;
            lines.push(text);
        }
        number += 1;
        line.clear();
        begun = false;
    };
    if lines.is_empty() && first > 1 {
        return Err(Error::Refused(format!(
            "{} has {} lines; read from line {} or earlier.",
            input.path,
            number - 1,
            number - 1
        )));
    }
    Ok(json!({"path": path, "first": first, "lines": lines, "next": next}))
}

pub(super) async fn write(tools: &Tools, input: Write) -> Result<Value, Error> {
    let path = tools.path(&input.path);
    let created = !tokio::fs::try_exists(&path).await.unwrap_or(true);
    tools.save(&input.path, &input.content).await?;
    Ok(json!({"path": path, "created": created}))
}

/// Replaces a unique match, or every match with `replace_all`; otherwise changes nothing.
pub(super) async fn edit(tools: &Tools, input: Edit) -> Result<Value, Error> {
    if input.old_string.is_empty() {
        return Err(Error::Refused(
            "old_string is empty; give the exact text to replace.".into(),
        ));
    }
    let path = tools.path(&input.path);
    let text = load(&path, &input.path).await?;
    let count = text.matches(&input.old_string).count();
    if count == 0 {
        return Err(Error::Refused(format!(
            "{} does not contain old_string; read the file and copy the text exactly.",
            input.path
        )));
    }
    if count > 1 && !input.replace_all {
        let lines: Vec<String> = text
            .match_indices(&input.old_string)
            .take(10)
            .map(|(at, _)| {
                (memchr::memchr_iter(b'\n', &text.as_bytes()[..at]).count() + 1).to_string()
            })
            .collect();
        return Err(Error::Refused(format!(
            "old_string occurs {count} times in {} (lines {}); add context to make it unique, or set replace_all.",
            input.path,
            lines.join(", ")
        )));
    }
    // A successful match above guarantees the first match exists.
    let at = text.find(&input.old_string).unwrap_or(0);
    let line = memchr::memchr_iter(b'\n', &text.as_bytes()[..at]).count() + 1;
    let changed = text.replace(&input.old_string, &input.new_string);
    tokio::fs::write(&path, changed)
        .await
        .map_err(|e| io(e, &input.path))?;
    Ok(json!({"path": path, "replaced": count, "line": line}))
}

pub(super) async fn skill(tools: &Tools, at: EntryId, input: Skill) -> Result<Value, Error> {
    let dirs = tools.state().config.skills.clone();
    let (dir, text) = find_skill(dirs, &input.name).await?;
    let (output, path) = tools.spill(at, text).await?;
    Ok(json!({"name": input.name, "dir": dir, "output": output, "path": path}))
}

/// The heading of the startup page's skill list; `gpt.md` names the same two.
const SKILLS: &str =
    "## Skills\n### Available skills\nThe `skill` action reads one whole, by its name.\n";

/// The first `<name>/SKILL.md` in `dirs`, whole, with its directory.
pub(crate) async fn find_skill(dirs: Vec<PathBuf>, name: &str) -> Result<(PathBuf, String), Error> {
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\']) {
        return Err(Error::Refused(
            "A skill's name is a bare directory name, such as `writing`.".into(),
        ));
    }
    for dir in dirs {
        let (dir, file) = (dir.join(name), dir.join(name).join("SKILL.md"));
        if tokio::fs::try_exists(&file).await.unwrap_or(false) {
            return Ok((dir, load(&file, name).await?));
        }
    }
    Err(Error::Refused(format!("There is no skill named `{name}`.")))
}

impl Tools {
    /// The startup page's skill list: the first 100 of the described folders' skills with their
    /// descriptions. Lookup-only folders stay available through `skill`. None when empty.
    pub(crate) async fn skills(&self) -> Option<String> {
        let (dirs, described) = {
            let config = &self.state().config;
            (config.skills.clone(), config.described)
        };
        let listed = list_skills(dirs[..described].to_vec(), 100).await;
        let list: String = listed
            .iter()
            .map(|s| format!("- {}: {}\n", s.name, s.description))
            .collect();
        (!list.is_empty()).then(|| format!("{SKILLS}{list}"))
    }
}

/// Each `<name>/SKILL.md` that [`find_skill`] finds in `dirs`, by name; at most `cap`.
async fn skill_files(
    dirs: Vec<PathBuf>,
    cap: usize,
) -> std::collections::BTreeMap<String, PathBuf> {
    let mut names = std::collections::BTreeMap::new();
    for dir in dirs {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while names.len() < cap
            && let Ok(Some(entry)) = entries.next_entry().await
        {
            let file = entry.path().join("SKILL.md");
            if let Ok(name) = entry.file_name().into_string()
                && !name.starts_with('.')
                && !names.contains_key(&name)
                && tokio::fs::try_exists(&file).await.unwrap_or(false)
            {
                names.insert(name, file);
            }
        }
    }
    names
}

/// [`skill_files`] with the first 200 characters of each front matter's `description`, 4 KiB
/// read from each.
pub(crate) async fn list_skills(dirs: Vec<PathBuf>, cap: usize) -> Vec<crate::protocol::Skill> {
    let mut list = Vec::new();
    for (name, path) in skill_files(dirs, cap).await {
        let mut head = Vec::new();
        if let Ok(file) = tokio::fs::File::open(path).await
            && file.take(4096).read_to_end(&mut head).await.is_ok()
        {
            let description = description(&String::from_utf8_lossy(&head));
            list.push(crate::protocol::Skill { name, description });
        }
    }
    list
}

/// A front matter's `description`, plain, quoted or folded, on one line of at most 200
/// characters.
fn description(text: &str) -> String {
    let front = text
        .strip_prefix("---\n")
        .and_then(|t| t.split("\n---").next());
    let mut lines = front.unwrap_or("").lines();
    let first = lines.find_map(|line| line.strip_prefix("description:"));
    let text = match first.map(str::trim) {
        Some(">" | ">-" | "|" | "|-") => {
            let folded: Vec<_> = lines
                .take_while(|l| l.starts_with(' '))
                .map(str::trim)
                .collect();
            folded.join(" ")
        }
        other => other.unwrap_or("").trim_matches(['"', '\'']).to_string(),
    };
    let cut: String = text.chars().take(200).collect();
    if cut.len() < text.len() {
        cut + "…"
    } else {
        cut
    }
}
