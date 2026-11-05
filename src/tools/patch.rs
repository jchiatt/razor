//! Codex's patch grammar, parsed before any write and applied one file at a time: adapted from
//! Codex's apply-patch crate under the Apache License 2.0, as NOTICE records.

use super::{Error, Tools, guard, io, load};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
pub(super) struct Patch {
    patch: String,
}

enum Hunk<'a> {
    Add {
        path: &'a str,
        lines: Vec<&'a str>,
    },
    Delete {
        path: &'a str,
    },
    Update {
        path: &'a str,
        to: Option<&'a str>,
        chunks: Vec<Chunk<'a>>,
    },
}

#[derive(Default)]
struct Chunk<'a> {
    context: Option<&'a str>,
    old: Vec<&'a str>,
    new: Vec<&'a str>,
    /// Context lines retain the file's text and endings after a loose match.
    kept: Vec<(usize, usize)>,
    end: bool,
}

pub(super) async fn apply(tools: &Tools, input: Patch) -> Result<Value, Error> {
    let hunks = parse(&input.patch)?;
    if tools.state().config.delete_guard {
        for hunk in &hunks {
            if let Hunk::Delete { path } = hunk {
                guard::deletable(&tools.cwd, path, true, "apply_patch", tools.since).await?;
            }
        }
    }
    let (mut added, mut modified, mut deleted) = (Vec::new(), Vec::new(), Vec::new());
    for hunk in hunks {
        if let Err(error) = write(tools, &hunk).await {
            let written = added
                .iter()
                .chain(&modified)
                .chain(&deleted)
                .copied()
                .collect::<Vec<_>>();
            return Err(if written.is_empty() {
                error
            } else {
                Error::Refused(format!(
                    "After writing {}, the patch stopped: {error}",
                    written.join(", ")
                ))
            });
        }
        match hunk {
            Hunk::Add { path, .. } => added.push(path),
            Hunk::Delete { path } => deleted.push(path),
            Hunk::Update { path, to, .. } => modified.push(to.unwrap_or(path)),
        }
    }
    Ok(json!({"added": added, "modified": modified, "deleted": deleted}))
}

async fn write(tools: &Tools, hunk: &Hunk<'_>) -> Result<(), Error> {
    match hunk {
        Hunk::Add { path, lines } => {
            let text: String = lines.iter().flat_map(|line| [*line, "\n"]).collect();
            tools.save(path, &text).await
        }
        Hunk::Delete { path } => remove(tools, path).await,
        Hunk::Update { path, to, chunks } => {
            let text = replace(&load(&tools.path(path), path).await?, chunks, path)?;
            tools.save(to.unwrap_or(path), &text).await?;
            match to {
                Some(_) => remove(tools, path).await,
                None => Ok(()),
            }
        }
    }
}

async fn remove(tools: &Tools, shown: &str) -> Result<(), Error> {
    tokio::fs::remove_file(tools.path(shown))
        .await
        .map_err(|e| io(e, shown))
}

fn replace(text: &str, chunks: &[Chunk<'_>], shown: &str) -> Result<String, Error> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let ending = if text.find('\n').is_some_and(|at| text[..at].ends_with('\r')) {
        "\r\n"
    } else {
        "\n"
    };
    let mut replacements = Vec::new();
    let mut at = 0;
    for chunk in chunks {
        if let Some(context) = chunk.context {
            at = seek(&lines, &[context], at, false).ok_or_else(|| {
                Error::Refused(format!(
                    "Could not find the context '{context}' in {shown}."
                ))
            })? + 1;
        }
        if chunk.old.is_empty() {
            replacements.push((lines.len(), 0, &chunk.new[..]));
            continue;
        }
        let (mut old, mut new) = (&chunk.old[..], &chunk.new[..]);
        let mut found = seek(&lines, old, at, chunk.end);
        if found.is_none() && old.last() == Some(&"") {
            old = &old[..old.len() - 1];
            if new.last() == Some(&"") {
                new = &new[..new.len() - 1];
            }
            found = seek(&lines, old, at, chunk.end);
        }
        let found = found.ok_or_else(|| {
            Error::Refused(format!(
                "Could not find the expected lines in {shown}:\n{}",
                chunk.old.join("\n")
            ))
        })?;
        let (mut o, mut n) = (0, 0);
        for &(kept_old, kept_new) in &chunk.kept {
            if kept_old >= old.len() || kept_new >= new.len() {
                break;
            }
            if o != kept_old || n != kept_new {
                replacements.push((found + o, kept_old - o, &new[n..kept_new]));
            }
            (o, n) = (kept_old + 1, kept_new + 1);
        }
        if o != old.len() || n != new.len() {
            replacements.push((found + o, old.len() - o, &new[n..]));
        }
        at = found + old.len();
    }
    replacements.sort_by_key(|r| r.0);
    // The last, empty replacement copies the lines after every other.
    replacements.push((lines.len(), 0, &[]));
    let mut out = String::with_capacity(text.len());
    let mut source = lines.into_iter();
    let mut index = 0;
    for (start, length, new) in replacements {
        for line in source.by_ref().take(start.saturating_sub(index)) {
            out.push_str(line);
            if !line.ends_with('\n') {
                out.push_str(ending);
            }
        }
        source.by_ref().take(length).for_each(drop);
        for line in new {
            out.push_str(line);
            out.push_str(ending);
        }
        index = start + length;
    }
    Ok(out)
}

fn seek(lines: &[&str], pattern: &[&str], start: usize, end: bool) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }
    let last = lines.len().checked_sub(pattern.len())?;
    let from = if end { last.max(start) } else { start };
    let passes: [fn(&str) -> &str; 3] = [|line| line, str::trim_end, str::trim];
    passes.iter().find_map(|pass| {
        (from..=last).find(|&i| {
            pattern.iter().zip(&lines[i..]).all(|(p, l)| {
                let body = l
                    .strip_suffix('\n')
                    .map_or(*l, |b| b.strip_suffix('\r').unwrap_or(b));
                pass(body) == pass(p)
            })
        })
    })
}

fn parse(patch: &str) -> Result<Vec<Hunk<'_>>, Error> {
    let lines: Vec<&str> = patch.trim().lines().collect();
    match lines.as_slice() {
        [first, .., last]
            if first.trim() == "*** Begin Patch" && last.trim() == "*** End Patch" => {}
        [first, ..] if first.trim() == "*** Begin Patch" => {
            return Err(Error::Refused(
                "The last line of the patch must be '*** End Patch'.".into(),
            ));
        }
        _ => {
            return Err(Error::Refused(
                "The first line of the patch must be '*** Begin Patch'.".into(),
            ));
        }
    }
    let mut hunks = Vec::new();
    for (index, &line) in lines[1..lines.len() - 1].iter().enumerate() {
        let number = index + 2;
        let text = if matches!(hunks.last(), Some(Hunk::Update { .. })) {
            line.trim_end()
        } else {
            line.trim()
        };
        let header = if let Some(path) = text.strip_prefix("*** Add File: ") {
            Some(Hunk::Add {
                path,
                lines: Vec::new(),
            })
        } else if let Some(path) = text.strip_prefix("*** Delete File: ") {
            Some(Hunk::Delete { path })
        } else {
            text.strip_prefix("*** Update File: ")
                .map(|path| Hunk::Update {
                    path,
                    to: None,
                    chunks: Vec::new(),
                })
        };
        if let Some(header) = header {
            finished(hunks.last())?;
            hunks.push(header);
        } else {
            match hunks.last_mut() {
                Some(Hunk::Add { lines, .. }) if line.starts_with('+') => lines.push(&line[1..]),
                Some(Hunk::Update { to, chunks, .. }) => change(line, number, to, chunks)?,
                _ => {
                    return Err(Error::Refused(format!(
                        "Line {number}, '{}', is not a valid hunk header: use '*** Add File: ', '*** Delete File: ' or '*** Update File: '.",
                        line.trim()
                    )));
                }
            }
        }
    }
    finished(hunks.last())?;
    if hunks.is_empty() {
        return Err(Error::Refused("The patch changes no files.".into()));
    }
    Ok(hunks)
}

fn finished(hunk: Option<&Hunk<'_>>) -> Result<(), Error> {
    if let Some(Hunk::Update { path, chunks, .. }) = hunk {
        if chunks.is_empty() {
            return Err(Error::Refused(format!("The update of {path} is empty.")));
        }
        if chunks
            .last()
            .is_some_and(|c| c.old.is_empty() && c.new.is_empty())
        {
            return Err(Error::Refused(format!(
                "An update hunk of {path} has no lines."
            )));
        }
    }
    Ok(())
}

fn change<'a>(
    line: &'a str,
    number: usize,
    to: &mut Option<&'a str>,
    chunks: &mut Vec<Chunk<'a>>,
) -> Result<(), Error> {
    let text = line.trim_end();
    let marker = text == "@@" || text.starts_with("@@ ");
    if chunks.last().is_some_and(|c| c.end) {
        if text.is_empty() {
            return Ok(());
        }
        if !marker {
            return Err(Error::Refused(format!(
                "Line {number}: after '*** End of File' a hunk starts with '@@', not '{line}'."
            )));
        }
    }
    if chunks.is_empty()
        && to.is_none()
        && let Some(path) = text.strip_prefix("*** Move to: ")
    {
        *to = Some(path);
        return Ok(());
    }
    let empty = chunks
        .last()
        .is_some_and(|c| c.old.is_empty() && c.new.is_empty());
    let unexpected = || {
        Error::Refused(format!(
            "Line {number}, '{line}', is unexpected in an update hunk: every line starts with ' ', '+' or '-', and a new hunk with '@@'."
        ))
    };
    if marker {
        if empty {
            return Err(unexpected());
        }
        chunks.push(Chunk {
            context: text.strip_prefix("@@ "),
            ..Chunk::default()
        });
    } else if text == "*** End of File" {
        if empty {
            return Err(Error::Refused(format!(
                "Line {number}: the hunk has no lines."
            )));
        }
        if let Some(chunk) = chunks.last_mut() {
            chunk.end = true;
        }
    } else {
        let (sign, rest) = match line.chars().next() {
            None => (' ', ""),
            Some(sign @ (' ' | '+' | '-')) => (sign, &line[1..]),
            _ => return Err(unexpected()),
        };
        if chunks.is_empty() {
            chunks.push(Chunk::default());
        }
        let chunk = chunks.last_mut().ok_or_else(unexpected)?;
        if sign == ' ' {
            chunk.kept.push((chunk.old.len(), chunk.new.len()));
        }
        if sign != '+' {
            chunk.old.push(rest);
        }
        if sign != '-' {
            chunk.new.push(rest);
        }
    }
    Ok(())
}
