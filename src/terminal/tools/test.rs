//! Test counts come from a runner's summary, never from a shell's exit status.
use crate::terminal::text::plural;

/// The newest summary's counts; the runner's explicit total wins, otherwise passed + failed.
#[derive(Clone, Copy, Debug)]
pub(super) struct Run {
    pub(super) runner: &'static str,
    pub(super) passed: u64,
    pub(super) failed: u64,
    pub(super) total: u64,
}
impl Run {
    /// The row's headline figure: the passes, then any failures (`410✓ 2✗`).
    pub(super) fn figure(&self) -> String {
        match (self.passed, self.failed) {
            (n, 0) => format!("{n}✓"),
            (0, n) => format!("{n}✗"),
            (pass, fail) => format!("{pass}✓ {fail}✗"),
        }
    }
    /// The card's foot label, with the command's runner and the known test total.
    pub(super) fn label(&self) -> String {
        format!(
            "{} · {} test{}",
            self.runner,
            self.total,
            plural(self.total)
        )
    }
}

/// Recognizes test commands at shell command boundaries, including common runner wrappers.
pub(in crate::terminal) fn runner(command: &str) -> Option<&'static str> {
    command.split([';', '&', '|', '\n']).find_map(|part| {
        let mut words = part.split_whitespace().peekable();
        while words.peek().is_some_and(|s| s.contains('=')) {
            words.next();
        }
        fn name(s: &str) -> &str {
            s.trim_matches(['\'', '"']).rsplit('/').next().unwrap_or(s)
        }
        let mut first = name(words.next()?);
        if first == "env" {
            while words.peek().is_some_and(|s| s.contains('=')) {
                words.next();
            }
            first = name(words.next()?);
        }
        let wrapper = match first {
            "uv" | "poetry" => Some("run"),
            "python" | "python3" => Some("-m"),
            _ => None,
        };
        if let Some(argument) = wrapper {
            if words.next()? != argument {
                return None;
            }
            first = name(words.next()?);
        }
        match first {
            "pytest" => Some("pytest"),
            "vitest" => Some("vitest"),
            "jest" => Some("jest"),
            "cargo" => match words.next()? {
                "test" => Some("cargo test"),
                "nextest" => Some("cargo nextest"),
                _ => None,
            },
            "go" if words.next()? == "test" => Some("go test"),
            "npm" | "pnpm" | "yarn" | "bun" | "npx" => {
                let mut next = words.next()?;
                if matches!(next, "exec" | "run" | "x" | "--yes") {
                    next = words.next()?;
                }
                match next {
                    "vitest" => Some("vitest"),
                    "jest" => Some("jest"),
                    "pytest" => Some("pytest"),
                    "test" => Some(match first {
                        "npm" => "npm test",
                        "pnpm" => "pnpm test",
                        "yarn" => "yarn test",
                        _ => "bun test",
                    }),
                    _ => None,
                }
            }
            _ => None,
        }
    })
}

// ANSI decoration is discarded before recognizing anchored summary words.
fn plain(line: &str) -> String {
    let mut chars = line.chars();
    let mut text = String::new();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.next() == Some('[') {
            for ch in chars.by_ref() {
                if ('@'..='~').contains(&ch) {
                    break;
                }
            }
        } else {
            text.push(ch);
        }
    }
    text
}

/// Reads the newest summary in shell result JSON, bounded text, or a running job's tail.
/// Go's non-verbose `ok` has no test count and returns none; verbose top-level verdicts count.
pub(super) fn read(command: &str, output: &str) -> Option<Run> {
    let runner = runner(command)?;
    if output.trim_start().starts_with('{')
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(output)
        && let Some(text) = value["output"].as_str()
    {
        return read(command, text);
    }
    for line in output.lines().rev() {
        let text = plain(line);
        let text = text.trim().trim_matches('=').trim();
        let first = text.split_whitespace().next().unwrap_or("");
        if !(text.starts_with("test result:")
            || text.starts_with("Tests ")
            || text.starts_with("Tests:")
            || text.starts_with("Summary ")
            || first.chars().all(|c| c.is_ascii_digit())
            || ["✓", "✔", "✗", "✘", "×"].contains(&first))
        {
            continue;
        }
        let get = |word: &str| {
            text.match_indices(word).find_map(|(at, _)| {
                let before = text[..at].trim_end();
                before
                    .rsplit_once(|c: char| !c.is_ascii_digit())
                    .map_or(before, |(_, n)| n)
                    .parse::<u64>()
                    .ok()
            })
        };
        let (passed, failed) = (get(" passed"), get(" failed"));
        if passed.is_none() && failed.is_none() {
            continue;
        }
        let (passed, failed) = (passed.unwrap_or(0), failed.unwrap_or(0));
        let bracketed = text
            .strip_prefix("Tests")
            .and_then(|s| s.rsplit_once('('))
            .and_then(|(_, s)| s.split_once(')'))
            .and_then(|(n, _)| n.parse().ok());
        let total = get(" total")
            .or(bracketed)
            .unwrap_or_else(|| passed.saturating_add(failed));
        return Some(Run {
            runner,
            passed,
            failed,
            total,
        });
    }
    if runner != "go test" {
        return None;
    }
    let mut counts = [0; 3];
    for line in output.lines() {
        if let Some(i) = ["--- PASS:", "--- FAIL:", "--- SKIP:"]
            .iter()
            .position(|p| line.starts_with(p))
        {
            counts[i] += 1;
        }
    }
    let [passed, failed, ignored] = counts;
    (passed + failed + ignored > 0).then_some(Run {
        runner,
        passed,
        failed,
        total: passed + failed,
    })
}
