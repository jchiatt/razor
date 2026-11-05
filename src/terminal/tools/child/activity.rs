//! The child’s recent calls, replies and heartbeat in its card.
use super::*;
use crate::protocol::Outcome;
use crate::terminal::tools::{Call, glance, kind, row};

impl Card<'_> {
    // The window, oldest first, each line with the record time its age counts from (none on a
    // line that continues another): the child's newest calls with their figures, and under the
    // newest shell call its output's last non-empty line, from its live tail while it runs or its
    // output lives in the job's file; each finished reply's first two rows; the reply being
    // written as its last two rows; then what it asks and how it ended.
    pub(super) fn window(&self, count: usize, room: usize) -> Vec<(Option<u64>, Line<'static>)> {
        let (agent, t, now) = (self.agent, self.t, self.now);
        let live = self.motion && agent.state == State::Working;
        let dots = ["·  ", "·· ", "···"][if live { (now / 300 % 3) as usize } else { 2 }];
        let mut end = Vec::new();
        if let Some(t) = t.filter(|t| agent.state == State::Working && !t.partial.is_empty()) {
            end = prose(&t.partial, room.saturating_sub(3), None, Some(dots));
        }
        for ask in agent.asks.iter().rev().take(count).rev() {
            let at = t
                .and_then(|t| t.entries.get(&ask.id.1))
                .map_or(agent.last_activity, |e| e.at);
            let words = clip(&ask.text, room.saturating_sub(28));
            let words = format!("? waiting on this session · {words}");
            end.push((Some(at), Line::from(Span::styled(words, style(CYAN, WELL)))));
        }
        if let Some(report) = &agent.report {
            let (mark, word, tone) = match report.status {
                ReportStatus::Finished => ("✓", "done", GREEN),
                ReportStatus::Partial => ("◐", "partial", AMBER),
                ReportStatus::Blocked => ("!", "blocked", AMBER),
            };
            let text = clip(&report.text, room.saturating_sub(3 + word.len()));
            let line = vec![
                Span::styled(format!("{mark} "), style(tone, WELL)),
                Span::styled(word, bold(tone, WELL)),
                Span::styled(format!(" {text}"), style(TARGET, WELL)),
            ];
            end.push((Some(agent.last_activity), Line::from(line)));
        }
        let mut window = Vec::new();
        let mut shell = true;
        for e in t.iter().flat_map(|t| t.entries.values().rev()) {
            if window.len() + end.len() >= count {
                break;
            }
            let mut lines = match (&e.body, t) {
                (Body::Call { name, .. }, Some(t)) if kind(name) != "ask" => {
                    let call = t.call(e, None);
                    let mut lines = vec![(Some(e.at), called(&call, room, dots, now))];
                    if shell && kind(name) == "shell" {
                        shell = false;
                        let seen = |l: &&str| !l.trim().is_empty();
                        let tail = call.tail.and_then(|(text, _)| text.lines().rfind(seen));
                        let last = call.lines().rfind(seen).filter(|_| !call.spooled());
                        let last = last.or(tail);
                        let words = clip(last.unwrap_or("").trim_end(), room.saturating_sub(4));
                        let out = format!("    {words}");
                        let out = Line::from(Span::styled(out, style(OUT, WELL)));
                        lines.extend(last.map(|_| (None, out)));
                    }
                    lines
                }
                (Body::Text { text }, _) if !text.trim().is_empty() => {
                    prose(text, room.saturating_sub(2), Some(e.at), None)
                }
                _ => continue,
            };
            lines.append(&mut window);
            window = lines;
        }
        window.append(&mut end);
        if window.is_empty() {
            let words = match (agent.state, t) {
                (State::Working, _) => "· starting",
                (_, None) => "loading its session",
                _ => "nothing read from its session",
            };
            window.push((None, Line::from(Span::styled(words, style(GRAY, WELL)))));
        }
        window
    }
    // The heartbeat's words and ink: how an ended child came out and how long it took; else what
    // its call in flight does (a wait waits, a launch starts) in a softer green, or how long it
    // has been quiet, amber from a minute.
    pub(super) fn heartbeat(&self, now: u64) -> (String, Color) {
        let (agent, t) = (self.agent, self.t);
        if let Some((at, fail)) = self.ended {
            let took = timer(at.saturating_sub(self.launched.at));
            return match fail {
                true => (format!("✗ failed · {took}"), AMBER),
                false => (format!("✓ {took}"), GREEN),
            };
        }
        let first = t.and_then(|t| {
            let calls = agent.calls.iter().filter_map(|id| t.entries.get(id));
            calls.min_by_key(|e| e.at)
        });
        let since = t.and_then(|t| t.running_since()).or(first.map(|e| e.at));
        let what = match first.map(|e| &e.body) {
            Some(Body::Call { name, input, .. }) if kind(name) == "agent" => {
                match input["action"].as_str() {
                    Some("wait") => "waiting",
                    Some("start") => "starting",
                    _ => "running",
                }
            }
            _ => "running",
        };
        let (soft, quiet) = (mix(GREEN, GRAY, 0.35), secs(now, agent.last_activity));
        // Three calls or more in the last eight seconds: busy.
        let recent = t.map_or(0, |t| {
            let calls = t.entries.values().rev().take_while(|e| e.at + 8000 > now);
            calls
                .filter(|e| matches!(e.body, Body::Call { .. }))
                .count()
        });
        match since {
            _ if !agent.asks.is_empty() => ("? asking".into(), CYAN),
            _ if recent >= 3 && self.columns < usize::from(PHONE) => {
                (format!("{recent} in 8s"), GREEN)
            }
            _ if recent >= 3 => (format!("busy · {recent} calls in 8s"), GREEN),
            Some(at) => (format!("{what} {}", timer(secs(now, at) * 1000)), soft),
            None if !agent.calls.is_empty() => (what.into(), soft),
            None if agent.state == State::Working => ("working".into(), soft),
            None if quiet >= 60 => (format!("quiet {}", timer(quiet * 1000)), AMBER),
            None => (
                format!("quiet {}", timer(quiet * 1000)),
                mix(GRAY, DATA, 0.4),
            ),
        }
    }
}

// The window's inks: a call's mark and figure, its target, its output, and the child's words.
const QUIET: Color = rgb(0x6b767f);
const TARGET: Color = rgb(0x606a72);
const OUT: Color = rgb(0x58626b);
const PROSE: Color = rgb(0x8f9aa3);

// A call in the window: its mark and target, and at the right the dots while it runs, then its
// figure in their place.
fn called(call: &Call<'_>, room: usize, dots: &str, now: u64) -> Line<'static> {
    let (mark, target) = glance(call.entry).unwrap_or(("·", String::new()));
    let outcome = call.outcome();
    let (mark, tone) = match (mark, outcome) {
        (_, Some(Outcome::Error)) => ("✗", AMBER),
        (_, Some(Outcome::Unknown)) => ("?", RED),
        ("±", _) => (mark, rgb(0xda4fbd)),
        ("$", _) => (mark, rgb(0x35c45e)),
        ("✦", _) => (mark, rgb(0xd6a0ff)),
        _ => (mark, QUIET),
    };
    // Running dots, then the result two cells after the target; a plain success says nothing.
    let (figure, ink) = match outcome {
        None => (dots.to_owned(), GREEN),
        Some(_) => (
            row::figure(call, now),
            [rgb(0x545f68), tone][usize::from(call.failed())],
        ),
    };
    let figure = if figure == "ok" {
        String::new()
    } else {
        figure
    };
    let target = clip(&target, room.saturating_sub(width(&figure) + 4));
    let gap = " ".repeat(room.saturating_sub(4 + width(&target) + width(&figure)));
    Line::from(vec![
        Span::styled(mark, style(tone, WELL)),
        // An agent call's target in the agent's colour.
        Span::styled(
            format!(" {target}  "),
            style([TARGET, rgb(0xd6a0ff)][usize::from(mark == "✦")], WELL),
        ),
        Span::styled(figure, style(ink, WELL)),
        Span::styled(gap, style(ink, WELL)),
    ])
}

// Two rows of the child's words at `room` cells, `›` before the first, which alone carries `at`:
// a finished reply's first two, the second cut with `…` when more follows; or, given `dots`, the
// reply as it is written, its last two with the dots at its end and `…` in front when cut. That
// wraps from its last paragraph, or past 2 KiB from a KiB step, so its rows stay put as words
// arrive.
fn prose(
    text: &str,
    room: usize,
    at: Option<u64>,
    dots: Option<&'static str>,
) -> Vec<(Option<u64>, Line<'static>)> {
    let text = text.trim_end();
    let (mut rows, mut from) = (Vec::new(), 0);
    let paragraph = text.rfind('\n').map_or(0, |i| i + 1);
    if dots.is_some() {
        from =
            text.ceil_char_boundary(paragraph.max(text.len().saturating_sub(2048) / 1024 * 1024));
    }
    let words = text[from..]
        .split_whitespace()
        .skip(usize::from(from > paragraph));
    let words = words.take(if dots.is_some() { usize::MAX } else { 2 * room });
    let words: Vec<_> = words.chain(dots.map(|_| "···")).collect();
    visit_wrap(&[Span::raw(words.join(" "))], room, |parts, _, _| {
        rows.push(parts.iter().map(|p| p.text).collect::<String>());
        dots.is_some() || rows.len() < 3
    });
    if dots.is_some() {
        let cut = from > 0 || rows.len() > 2;
        rows.drain(..rows.len().saturating_sub(2));
        if cut {
            rows[0].insert(0, '…');
        }
    } else if let [_, second, third] = &rows[..] {
        rows[1] = clip(&format!("{second} {third}"), room);
        rows.truncate(2);
    }
    let last = rows.len().saturating_sub(1);
    let rows = rows.iter().enumerate().map(|(i, row)| {
        let dots = dots.filter(|_| i == last);
        let words = row.strip_suffix("···").filter(|_| dots.is_some());
        let spans = vec![
            Span::styled(if i == 0 { "› " } else { "  " }, style(rgb(0x9e6cc5), WELL)),
            Span::styled(words.unwrap_or(row).to_owned(), style(PROSE, WELL)),
            Span::styled(dots.unwrap_or(""), style(GREEN, WELL)),
        ];
        (at.filter(|_| i == 0), Line::from(spans))
    });
    rows.collect()
}
