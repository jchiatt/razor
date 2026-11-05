//! Best-effort checks of command text, not a shell parser or a sandbox.

use super::Error;
use crate::{
    protocol::{JobId, Millis},
    shell::Shell,
};
use std::{
    collections::BTreeSet, io::ErrorKind, iter::Peekable, mem, path::Path, str::Chars,
    time::UNIX_EPOCH,
};

/// Resolves observed literal pattern stops to owned job handles; checks every deletion first.
/// Says whether the command runs `post`, the jobs its stops name, and whether each stop is a
/// bare `pkill` a shell function of that name replaces: none is wrapped or named by its path.
pub(super) async fn command(
    text: &str,
    cwd: &Path,
    since: Millis,
    delete: bool,
    shell: &Shell,
) -> Result<(bool, Vec<JobId>, bool), Error> {
    let mut lexer = Lexer {
        chars: text.chars().peekable(),
        piped: false,
        heredocs: Vec::new(),
    };
    let (mut pgrep, mut post, mut jobs, mut bare) = (false, false, BTreeSet::new(), true);
    while let Some((words, piped)) = lexer.command() {
        pgrep &= piped;
        let mut wrapped = false;
        let start = words.iter().position(|word| {
            let text = word.text.as_str();
            let wrapper = [
                "sudo", "env", "command", "exec", "nohup", "nice", "timeout", "time",
            ]
            .contains(&last(text));
            let assignment = text.split_once('=').is_some_and(|(name, _)| {
                name.starts_with(|c: char| c == '_' || c.is_ascii_alphabetic())
                    && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
            });
            let skip = wrapper
                || assignment
                || [
                    "!", "{", "if", "then", "else", "elif", "do", "while", "until",
                ]
                .contains(&text)
                || (wrapped && text.starts_with(|c: char| c == '-' || c.is_ascii_digit()));
            wrapped |= wrapper;
            !skip
        });
        let Some(start) = start else { continue };
        let name = last(&words[start].text);
        let args = &words[start + 1..];
        let pattern = match name {
            "pkill" | "killall" => true,
            "kill" => args
                .iter()
                .any(|w| w.text == "-1" || w.text.contains("pgrep") || w.text.contains("pidof")),
            "xargs" => pgrep && args.iter().any(|w| last(&w.text) == "kill"),
            _ => false,
        };
        if pattern {
            let refused = || {
                Error::Refused(format!(
                    "`{name}` here would kill processes chosen by a pattern, or every process; kill a job by its handle, or a process by an id you know."
                ))
            };
            let query = stop_pattern(name, args).ok_or_else(refused)?;
            let found = shell.matching(&query).await?;
            if found.is_empty() {
                return Err(refused());
            }
            jobs.extend(found);
            bare &= !wrapped && words[start].text == name;
        }
        pgrep |= matches!(name, "pgrep" | "pidof");
        post |= name == "post";
        if delete {
            for (target, literal) in targets(name, args) {
                deletable(cwd, target, literal, name, since).await?;
            }
        }
    }
    Ok((post, jobs.into_iter().collect(), bare))
}

/// The termination forms observed in sessions; every other pattern command keeps its refusal.
fn stop_pattern(name: &str, args: &[Word]) -> Option<Vec<String>> {
    if name != "pkill" || args.iter().any(|w| w.expands) {
        return None;
    }
    let (pattern, flags) = args.split_last()?;
    let escaped = flags.last().is_some_and(|w| w.text == "--");
    if (pattern.text.starts_with('-') && !escaped)
        || !pattern.text.chars().any(char::is_alphanumeric)
    {
        return None;
    }
    let mut query = Vec::new();
    for flag in flags {
        match flag.text.as_str() {
            "-f" | "-x" => query.push(flag.text.clone()),
            "-INT" | "-TERM" | "-KILL" | "-9" | "--" => {}
            _ => return None,
        }
    }
    query.extend(["--".into(), pattern.text.clone()]);
    Some(query)
}

pub(super) async fn deletable(
    cwd: &Path,
    target: &str,
    literal: bool,
    name: &str,
    since: Millis,
) -> Result<(), Error> {
    let reason = if !literal {
        Some("a path the command's text does not name literally")
    } else {
        let born = match tokio::fs::symlink_metadata(cwd.join(target)).await {
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
            Ok(metadata) => metadata.created().ok(),
            Err(_) => None,
        };
        match born.and_then(|t| t.duration_since(UNIX_EPOCH).ok()) {
            Some(born) if born.as_millis() >= u128::from(since) => None,
            Some(_) => Some("which existed before this session started"),
            None => Some("and the filesystem does not say when it was made"),
        }
    };
    match reason {
        Some(reason) => Err(Error::Refused(format!(
            "`{name}` would delete {target}, {reason}; ask the person to delete it."
        ))),
        None => Ok(()),
    }
}

fn targets<'a>(name: &str, args: &'a [Word]) -> Vec<(&'a str, bool)> {
    let named = |args: &'a [Word]| {
        let mut options = true;
        args.iter()
            .filter(|w| {
                if options && w.text == "--" {
                    options = false;
                    return false;
                }
                !options || !w.text.starts_with('-')
            })
            .map(|w| (w.text.as_str(), !w.expands))
            .collect::<Vec<_>>()
    };
    let here = |paths: Vec<(&'a str, bool)>| {
        if paths.is_empty() {
            vec![(".", true)]
        } else {
            paths
        }
    };
    match name {
        "rm" | "rmdir" | "unlink" | "shred" => named(args),
        "find"
            if args.iter().any(|w| w.text == "-delete")
                || args.windows(2).any(|w| {
                    matches!(w[0].text.as_str(), "-exec" | "-execdir") && last(&w[1].text) == "rm"
                }) =>
        {
            here(
                args.iter()
                    .take_while(|w| !w.text.starts_with(['-', '!', '(']))
                    .map(|w| (w.text.as_str(), !w.expands))
                    .collect(),
            )
        }
        "git" => match args.iter().position(|w| !w.text.starts_with('-')) {
            Some(i) if args[i].text == "clean" => {
                let rest = &args[i + 1..];
                if rest
                    .iter()
                    .any(|w| matches!(w.text.as_str(), "-n" | "--dry-run"))
                {
                    Vec::new()
                } else {
                    here(named(rest))
                }
            }
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn last(text: &str) -> &str {
    text.rsplit('/').next().unwrap_or(text)
}

#[derive(Default)]
struct Word {
    text: String,
    expands: bool,
}

/// Retains one command, not a character copy and every command in the input.
struct Lexer<'a> {
    chars: Peekable<Chars<'a>>,
    piped: bool,
    heredocs: Vec<(String, bool)>,
}

impl Lexer<'_> {
    fn command(&mut self) -> Option<(Vec<Word>, bool)> {
        let mut words = Vec::new();
        let mut word = Word::default();
        let (mut open, mut target, mut delimiter) = (false, false, None);
        loop {
            let c = self.chars.next();
            let boundary = c.is_none() || matches!(c, Some('\n' | ';' | '(' | ')' | '&' | '|'));
            if boundary || matches!(c, Some(' ' | '\t' | '<' | '>')) {
                if mem::take(&mut open) {
                    let word = mem::take(&mut word);
                    if let Some(tabs) = delimiter.take() {
                        self.heredocs.push((word.text, tabs));
                    } else if !mem::take(&mut target) {
                        words.push(word);
                    }
                }
                if boundary {
                    let piped = self.piped;
                    self.piped = c == Some('|');
                    if matches!(c, Some('&' | '|')) && self.chars.peek().copied() == c {
                        self.chars.next();
                        self.piped = false;
                    }
                    target = false;
                    delimiter = None;
                    if c == Some('\n') {
                        self.skip_heredocs();
                    }
                    if !words.is_empty() {
                        return Some((words, piped));
                    }
                    c?;
                } else if let Some(c @ ('<' | '>')) = c {
                    let mut operator = String::from(c);
                    while let Some(&c @ ('<' | '>' | '&' | '|' | '-')) = self.chars.peek() {
                        operator.push(c);
                        self.chars.next();
                    }
                    if operator.starts_with("<<") && !operator.starts_with("<<<") {
                        delimiter = Some(operator.ends_with('-'));
                    } else if !operator.ends_with('-') {
                        target = true;
                    }
                }
                continue;
            }
            match c? {
                '#' if !open => {
                    while self.chars.peek().is_some_and(|c| *c != '\n') {
                        self.chars.next();
                    }
                }
                quote @ ('\'' | '"') => {
                    open = true;
                    self.quoted(quote, &mut word);
                }
                '\\' => match self.chars.next() {
                    Some('\n') | None => {}
                    Some(c) => {
                        open = true;
                        word.text.push(c);
                    }
                },
                c @ ('$' | '`') => {
                    open = true;
                    self.substitution(c, &mut word);
                }
                c => {
                    word.expands |= matches!(c, '*' | '?' | '[' | '{') || (c == '~' && !open);
                    open = true;
                    word.text.push(c);
                }
            }
        }
    }

    fn quoted(&mut self, quote: char, word: &mut Word) {
        while let Some(c) = self.chars.next() {
            if c == quote {
                break;
            }
            match c {
                '\\' if quote == '"' => match self.chars.next() {
                    Some(c @ ('$' | '`' | '"' | '\\')) => word.text.push(c),
                    Some('\n') | None => {}
                    Some(c) => {
                        word.text.push('\\');
                        word.text.push(c);
                    }
                },
                '$' | '`' if quote == '"' => self.substitution(c, word),
                c => word.text.push(c),
            }
        }
    }

    fn substitution(&mut self, first: char, word: &mut Word) {
        word.expands = true;
        word.text.push(first);
        let (open, close) = match (first, self.chars.peek()) {
            ('`', _) => ('`', '`'),
            ('$', Some('(')) => ('(', ')'),
            ('$', Some('{')) => ('{', '}'),
            _ => return,
        };
        if first == '$' {
            self.chars.next();
            word.text.push(open);
        }
        let mut depth = 1;
        while let Some(c) = self.chars.next() {
            word.text.push(c);
            if c == '\\' {
                if let Some(c) = self.chars.next() {
                    word.text.push(c);
                }
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            } else if c == open {
                depth += 1;
            } else if matches!(c, '\'' | '"') && open != '`' {
                for inner in self.chars.by_ref() {
                    word.text.push(inner);
                    if inner == c {
                        break;
                    }
                }
            }
        }
    }

    fn skip_heredocs(&mut self) {
        for (delimiter, tabs) in mem::take(&mut self.heredocs) {
            loop {
                // Compare as a stream: a heredoc body line need not fit in memory.
                let mut matched = 0;
                let mut equal = true;
                let mut prefix = tabs;
                let mut ended = true;
                for c in self.chars.by_ref() {
                    if c == '\n' {
                        ended = false;
                        break;
                    }
                    if prefix && c == '\t' {
                        continue;
                    }
                    prefix = false;
                    if delimiter[matched..].starts_with(c) {
                        matched += c.len_utf8();
                    } else {
                        equal = false;
                    }
                }
                if (equal && matched == delimiter.len()) || ended {
                    break;
                }
            }
        }
    }
}
