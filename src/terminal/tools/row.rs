//! A call's row: the beat, the leader, and the figures, from record times alone.
use super::*;
use crate::terminal::motion::{ease_in_out, ease_out, played, progress};
use unicode_segmentation::UnicodeSegmentation;

pub(super) struct Look {
    pub(super) running: bool,
    pub(super) arrive: f64,
    pub(super) flash: f64,
    pub(super) calm: f64,
    settle: f64,
    /// With motion off, the finish's still acknowledgement: mark and figure white for 400 ms.
    ack: bool,
    /// The turn's rail at this instant, which a finished tick calms into.
    rail: Color,
}

pub(super) fn look(call: &Call<'_>, now: u64, motion: bool) -> Look {
    let running = call.outcome().is_none();
    let delay = if super::card::earned(call) { 900 } else { 0 };
    let settle = call.since(now).map_or(0., |t| progress(t, delay, 760));
    let s = if motion { settle } else { 1. };
    let since = call.since(now).filter(|_| !call.waiting(now));
    Look {
        ack: !motion && since.is_some_and(|t| t < 400),
        running,
        arrive: played(motion, now, call.entry.at, 380),
        flash: if running || s >= 0.34 || call.waiting(now) {
            0.
        } else {
            (1. - s / 0.34).powi(2)
        },
        calm: ease_out(s),
        settle: s,
        rail: super::turn::tone(call, now, motion),
    }
}

pub(super) fn mark(call: &Call<'_>, lk: &Look) -> (&'static str, Color) {
    if call.asking() {
        return if lk.running {
            ("◇", CYAN)
        } else {
            ("◆", rgb(0x2b90a1))
        };
    }
    match call.outcome() {
        Some(Outcome::Error) => (
            "✗",
            if call.place.fixed.is_some() {
                rgb(0xcd9116)
            } else {
                AMBER
            },
        ),
        Some(Outcome::Unknown) => ("?", RED),
        None => ("┼", GREEN),
        _ if lk.flash > 0. => ("┼", mix(lk.rail, WHITE, lk.flash)),
        _ => ("┼", mix(GREEN, lk.rail, lk.calm)),
    }
}

pub(super) fn draw(call: &Call<'_>, columns: usize, now: u64, motion: bool) -> Line<'static> {
    let lk = look(call, now, motion);
    let (kind, input) = call.input();
    let phone = columns < usize::from(PHONE);
    if kind == "read"
        && !phone
        && !lk.running
        && !call.failed()
        && let Some(mut line) = call.read_row(columns)
    {
        if lk.ack {
            let figure = line.spans.len() - 1;
            for i in [0, figure] {
                line.spans[i].style.fg = Some(WHITE);
            }
        }
        duration(&mut line, call, columns, now);
        return entrance(line, lk.arrive);
    }
    // A finished test run reads as a test of the runner's command, the runner dropped on the phone.
    let run = call.run().filter(|_| kind == "shell");
    let command = super::command(kind, input).unwrap_or("");
    let bare = run.and_then(|r| command.strip_prefix(r.runner).filter(|_| phone));
    // A launch's row, and another call about a child that went through, name the child and its
    // brief or what was done, and its model as the figure.
    let tie = call.place.tie.is_some();
    let child = call.child.filter(|_| tie || !call.failed());
    let (kind, target) = match (run, child) {
        (Some(_), _) => ("test", bare.unwrap_or(command).trim_start().to_owned()),
        // An agent wait waits for the session's next input: a report, mail or the person.
        _ if waits(kind, input) => ("wait", "the next input".to_owned()),
        (None, Some((name, _))) if !tie => (kind, name.clone()),
        (None, _) => (kind, target(kind, input)),
    };
    let figure = match child {
        Some((_, model)) => model.clone(),
        None if tie => String::new(),
        None => figure(call, now),
    };
    // The phone keeps three columns right of the figure and two before it; wide rows keep the
    // duration's eleven and three.
    let (tail, gap) = if phone { (3, 2) } else { (11, 3) };
    let figure_at = columns.saturating_sub(tail + width(&figure));
    let verb = verb(kind);
    let prefix = if phone {
        format!(" {verb:>6}  ")
    } else {
        format!("   {verb:>6}   ")
    };
    let start = 1 + width(&prefix);
    let pair = (!phone && kind == "edit" && !call.failed())
        .then(|| edit_pair(input))
        .flatten();
    let facts = pair
        .as_ref()
        .map_or(0, |(old, new)| width(old) + width(new) + 5);
    let room = figure_at.saturating_sub(start + gap);
    let pair = pair.filter(|_| room >= facts + 12);
    let text = clip(
        &target,
        room.saturating_sub(if pair.is_some() { facts } else { 0 }),
    );
    let (mark, tone) = mark(call, &lk);
    let tone = if lk.ack { WHITE } else { tone };
    let verb = if lk.running {
        WHITE
    } else {
        mix(DATA, WHITE, lk.flash)
    };
    let ink = if lk.running {
        MAGENTA
    } else {
        mix(MAGENTA, rgb(0xc849ae), lk.calm)
    };
    let rest = rgb(0xc2cad0);
    let fig = if tie || child.is_some() {
        GRAY
    } else if lk.running {
        GREEN
    } else if lk.ack {
        WHITE
    } else if lk.flash > 0. {
        mix(rest, WHITE, 1.4 * lk.flash)
    } else {
        mix(WHITE, rest, lk.calm)
    };
    // A failure a later call mended keeps its kind as an amber tab.
    let tab = match call.failed() && call.place.fixed.is_some() {
        true => style(AMBER, AMBER_BG),
        false => style(verb, GLASS),
    };
    let mut spans = vec![
        span(mark, tone),
        Span::styled(prefix, tab.add_modifier(Modifier::BOLD)),
        span(text, ink),
    ];
    if tie || child.is_some() {
        let left = room.saturating_sub(width(&spans[2].content) + 1);
        spans[2].style = bold(rgb(0xd6a0ff), GLASS);
        let brief = input["brief"].as_str().and_then(|b| b.lines().next());
        let done = || input["decision"].as_str().or(input["action"].as_str());
        let brief = brief.or_else(|| done().filter(|_| !tie));
        if let Some(brief) = brief.filter(|b| left > 3 && !b.is_empty()) {
            spans.push(span(format!(" {}", clip(brief, left)), GRAY));
        }
    }
    let end = start
        + spans[2..].iter().map(|s| width(&s.content)).sum::<usize>()
        + if pair.is_some() { facts } else { 0 };
    if let Some((old, new)) = pair {
        spans.extend([
            span(format!("  {old}"), mix(RED, GLASS, 0.25 * lk.calm)),
            span(" → ", GRAY),
            span(new, mix(GREEN, GLASS, 0.25 * lk.calm)),
        ]);
    }
    // A test's gauge cell before its figure: dashed, since no runner here reports its files.
    let gauge = (run.is_some() && !phone)
        .then(|| columns.saturating_sub(36).max(end + 2))
        .filter(|at| at + 11 < figure_at);
    let dots_to = gauge.unwrap_or(figure_at);
    spans.extend(leader(call, &lk, end, dots_to, now, motion));
    if let Some(at) = gauge {
        let rest = " ".repeat(figure_at - at - 10);
        spans.extend([span("╌".repeat(10), GRAY_DIM), Span::raw(rest)]);
    }
    // An edit's figure: its added lines green and its removed red, bold, calming with the row; a
    // test's failures amber and bold after its passes.
    let edited = matches!(kind, "edit" | "apply_patch") && !lk.running && !call.failed();
    match (
        figure.split_once(" −").filter(|_| edited),
        figure.split_once(" "),
    ) {
        (Some((added, removed)), _) => spans.extend(
            [(added.to_owned(), GREEN), (format!(" −{removed}"), RED)].map(|(words, ink)| {
                Span::styled(words, bold(mix(ink, GLASS, 0.3 * lk.calm), GLASS))
            }),
        ),
        (_, Some((passed, failed))) if run.is_some() => spans.extend([
            span(format!("{passed} "), fig),
            Span::styled(failed.to_owned(), bold(AMBER, GLASS)),
        ]),
        _ => spans.push(span(figure, fig)),
    }
    let mut line = Line::from(spans);
    if !phone && !tie {
        duration(&mut line, call, columns, now);
    }
    entrance(line, lk.arrive)
}

// The duration column: the call's run time, live while it runs and kept once it has settled.
fn elapsed(call: &Call<'_>, now: u64) -> Option<String> {
    let (kind, input) = call.input();
    if matches!(kind, "deliver" | "ask" | "message") {
        return None;
    }
    // A wait's column holds its limit: `of 10m`.
    if waits(kind, input) {
        let s = input["timeout_ms"].as_u64().unwrap_or(600_000) / 1000;
        let limit = match s {
            ..120 => format!("{s}s"),
            _ => format!("{}m", (s + 30) / 60),
        };
        return Some(format!("of {limit}"));
    }
    let ms = call.ended().unwrap_or(now).saturating_sub(call.entry.at);
    Some(dur(ms))
}

fn duration(line: &mut Line<'static>, call: &Call<'_>, columns: usize, now: u64) {
    if let Some(duration) = elapsed(call, now) {
        let gap = columns.saturating_sub(3 + width(&duration) + line.width());
        line.spans
            .push(span(format!("{}{duration}", " ".repeat(gap)), GRAY));
    }
}

// A code search's hits and the files they are in, from Claude Code's Grep or Glob result: `Found
// 3 files` and its list, `Found 12 total occurrences across 3 files.`, or `path:12:words` lines,
// one file's lines together; `≥` when the result was cut short.
fn search_figure(call: &Call<'_>) -> Option<String> {
    let lines: Vec<&str> = (call.lines()).filter(|l| !l.trim().is_empty()).collect();
    let partial = ["", "≥"][usize::from(lines.iter().any(|l| l.starts_with("[Showing results")))];
    let number = |l: &str, at: usize| l.split(' ').nth(at)?.parse::<usize>().ok();
    let total = lines
        .iter()
        .find(|l| l.starts_with("Found ") && l.contains(" occurrences"));
    let (hits, files) = match (total, *lines.first()?) {
        (Some(l), _) => (number(l, 1)?, number(l, 5)?),
        (_, l) if l.starts_with("No ") => (0, 0),
        (_, l) if l.starts_with("Found ") => (number(l, 1)?, number(l, 1)?),
        _ => {
            let found = lines.iter().filter(|l| !l.starts_with('['));
            let mut paths: Vec<_> = found.map(|l| l.split(':').next()).collect();
            let hits = paths.len();
            paths.dedup();
            (hits, paths.len())
        }
    };
    Some(format!("{partial}{hits} in {files} file{}", plural(files)))
}

pub(super) fn verb(kind: &str) -> &str {
    if kind == "message" { "mail" } else { kind }
}

// What a failed call is known by, in its row, its card and its opened tag alike: `unknown`,
// `exit N` or `signal N`, its kind's own word (`no file`, `bounced`), else `failed`.
pub(super) fn failed(call: &Call<'_>) -> String {
    let (kind, error) = (call.input().0, call.raw());
    let has = |words: &[&str]| words.iter().any(|w| error.contains(w));
    let word = match (call.outcome(), call.end(), kind) {
        (Some(Outcome::Unknown), ..) => "unknown",
        (_, Some(JobEnd::Exit(n)), _) => return format!("exit {n}"),
        (_, Some(JobEnd::Signal(n)), _) => return format!("signal {n}"),
        (.., "message") => "bounced",
        (.., "read") if has(&["does not exist", "No such file"]) => "no file",
        (.., "edit") if has(&["does not contain", "not found in file"]) => "no match",
        (.., "write") if has(&["has not been read", "modified since"]) => "stale",
        (.., "search") if has(&["regex parse error"]) => "bad regex",
        _ => "failed",
    };
    word.into()
}

pub(super) fn waits(kind: &str, input: &Value) -> bool {
    kind == "agent" && input["action"] == "wait"
}

pub(super) fn figure(call: &Call<'_>, now: u64) -> String {
    let (kind, input) = call.input();
    // A wait's figure is how long it has waited, or that it timed out.
    if waits(kind, input) && !call.failed() {
        return match call.raw() {
            "null" => "timed out".into(),
            _ => timer(call.ended().unwrap_or(now).saturating_sub(call.entry.at)),
        };
    }
    if call.outcome().is_none() {
        if kind == "shell" {
            return String::new();
        }
        return ["·  ", "·· ", "···"][(now / 300 % 3) as usize].into();
    }
    if call.failed() {
        return failed(call);
    }
    match kind {
        "shell" if let Some(run) = call.run() => run.figure(),
        "shell" => {
            let count = if call.spooled() {
                call.job.and_then(|(_, job)| job.total)
            } else {
                Some(call.count())
            };
            count.map_or("output saved".into(), |n| format!("{n} line{}", plural(n)))
        }
        "message" => "sent".into(),
        "deliver" => match input["path"].as_str() {
            Some(path) => std::path::Path::new(path)
                .file_name()
                .map_or("file".into(), |n| n.to_string_lossy().into()),
            None => "link".into(),
        },
        // A write that made its file, as Razor's result or Claude Code's says: `+N new`.
        "write" => {
            let n = input["content"].as_str().unwrap_or("").lines().count();
            let raw = call.raw();
            match raw.contains(r#""created":true"#) || raw.starts_with("File created") {
                true => format!("+{n} new"),
                false => format!("{n} line{}", plural(n)),
            }
        }
        "read" => match call
            .decoded
            .map(|r| (r.first, r.count, r.images.is_empty()))
        {
            Some((_, 0, true)) => "0 lines".into(),
            Some((Some(first), count, _)) => {
                format!(
                    "{first}–{}",
                    first.saturating_add(count as u64).saturating_sub(1)
                )
            }
            _ => "ok".into(),
        },
        "edit" | "apply_patch" => super::diff::counts(call)
            .map(|(added, removed)| format!("+{added} −{removed}"))
            .unwrap_or_else(|| "ok".into()),
        "search" => search_figure(call).unwrap_or_else(|| "ok".into()),
        // A fetch is one page; a search or answer counts the addresses its result lists.
        "web" if input["url"].is_string() => "1 page".into(),
        "web" => {
            let address = |l: &&str| l.starts_with("http") && !l.contains(' ');
            match call.lines().filter(address).count() {
                0 => "ok".into(),
                n => format!("{n} result{}", plural(n)),
            }
        }
        _ => "ok".into(),
    }
}

fn edit_pair(input: &Value) -> Option<(String, String)> {
    let (old, new) = (input["old_string"].as_str()?, input["new_string"].as_str()?);
    let mut start = old
        .chars()
        .zip(new.chars())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum::<usize>();
    let mut end = old[start..]
        .chars()
        .rev()
        .zip(new[start..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum::<usize>();
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    while start > 0 && word(old[..start].chars().next_back()) && word(old[start..].chars().next()) {
        start -= old[..start].chars().next_back().map_or(0, char::len_utf8);
    }
    while end > 0
        && word(old[old.len() - end..].chars().next())
        && word(old[..old.len() - end].chars().next_back())
    {
        end -= old[old.len() - end..]
            .chars()
            .next()
            .map_or(0, char::len_utf8);
    }
    Some((
        clip(old[start..old.len() - end].trim(), 9),
        clip(new[start..new.len() - end].trim(), 20),
    ))
}

fn leader(
    call: &Call<'_>,
    lk: &Look,
    end: usize,
    to: usize,
    now: u64,
    motion: bool,
) -> Vec<Span<'static>> {
    if to <= end {
        return Vec::new();
    }
    let lo = end + 1;
    let hi = to.saturating_sub(2).max(lo);
    let phase = (now % 1700) as f64 / 1700.;
    let forward = phase < 0.5;
    let laid = call
        .since(now)
        .filter(|t| motion && *t < 900 && super::card::earned(call))
        .map(|t| progress(t, 560, 340));
    let k = laid.unwrap_or_else(|| ease_in_out(if forward { phase * 2. } else { 2. - phase * 2. }));
    let forward = laid.is_some() || forward;
    let head = lo + (k * hi.saturating_sub(lo) as f64).round() as usize;
    let running = lk.running || laid.is_some();
    let seed = call_seed(call);
    let late = call.place.cat && now.saturating_sub(call.entry.at) >= 2000;
    let cat =
        (seed.is_multiple_of(10_000) || late) && hi.saturating_sub(lo) + 1 >= 4 && !call.failed();
    let track = if laid.is_some() {
        rgb(0x1f6b37)
    } else {
        rgb(0x123921)
    };
    let mut spans = Vec::new();
    for i in end..to {
        let dot = i > end && i + 1 < to && i % 2 == 0;
        let rest = if lk.flash > 0. {
            mix(rgb(0x3d464f), WHITE, 1.2 * lk.flash)
        } else {
            mix(rgb(0x1f6b37), rgb(0x3d464f), lk.calm)
        };
        let behind = if forward {
            head as i64 - i as i64
        } else {
            i as i64 - head as i64
        };
        let tone = if !running {
            rest
        } else if (1..=4).contains(&behind) {
            mix(GREEN, track, behind as f64 / 5.)
        } else {
            rgb(0x123921)
        };
        let mut glyph =
            if dot && (!running || laid.is_some() && i < head || laid.is_none() && motion) {
                "·"
            } else {
                " "
            };
        let mut ink = tone;
        if running && motion && i == head {
            glyph = if forward { "▸" } else { "◂" };
            ink = GREEN;
        }
        let style = if matches!(glyph, "▸" | "◂") {
            bold(ink, GLASS)
        } else {
            style(ink, GLASS)
        };
        spans.push(Span::styled(glyph, style));
    }
    if cat {
        cat_leader(&mut spans, end..to, lk, laid, seed, (now, motion));
        return spans;
    }
    if running && !motion && hi.saturating_sub(lo) >= 7 {
        for (offset, c) in "running".chars().enumerate() {
            spans[lo - end + offset] = span(c.to_string(), GREEN);
        }
    }
    spans
}

fn cat_leader(
    spans: &mut [Span<'static>],
    range: std::ops::Range<usize>,
    lk: &Look,
    progress: Option<f64>,
    seed: u32,
    clock: (u64, bool),
) {
    let (now, motion) = clock;
    let a = range.start as i64 + 1;
    let b = range.end as i64 - 2;
    let (lo, hi) = (a + 2, b - 1);
    let running = lk.running || progress.is_some();
    let mut paint = |i: i64, glyph: &str, ink: Color| {
        if i >= range.start as i64
            && let Some(span) = spans.get_mut((i - range.start as i64) as usize)
        {
            *span = Span::styled(glyph.to_owned(), style(ink, GLASS));
        }
    };
    let h = if running {
        progress.map_or(lo + 1_i64.min(hi - lo), |p| {
            lo + (p * (hi - lo) as f64).round() as i64
        })
    } else {
        hi
    };
    for i in a..h - 2 {
        let age = h - 3 - i;
        let tangled = if running {
            age < 3
        } else {
            age < 3 && age >= (ease_out(lk.settle / 0.55) * 3.).round() as i64 && motion
        };
        if tangled {
            let fade = if running { age as f64 / 3. } else { 0.5 };
            paint(i, tangle(i, seed), mix(GREEN, rgb(0x1f6b37), fade));
        } else if i % 2 == 0 {
            paint(
                i,
                "·",
                if running {
                    rgb(0x123921)
                } else {
                    rgb(0x3d464f)
                },
            );
        }
    }
    let ink = if running {
        GREEN
    } else {
        mix(GREEN, GRAY, lk.calm)
    };
    for (i, glyph) in ["ᓚ", "ᘏ", "ᗢ"].into_iter().enumerate() {
        paint(h - 2 + i as i64, glyph, ink);
    }
    if !running {
        paint(b, "@", mix(GREEN, rgb(0x278d46), lk.calm));
        return;
    }
    for i in h + 2..=b {
        if i % 2 == 0 {
            paint(i, "·", rgb(0x123921));
        }
    }
    let period = if progress.is_some() { 1100 } else { 850 };
    let bat = motion && (now.saturating_add(u64::from(seed) % period)) % period < period * 3 / 10;
    if bat && h + 2 <= b {
        paint(h + 1, " ", GREEN);
        paint(h + 2, "@", GREEN);
    } else {
        paint(h + 1, if bat { "°" } else { "@" }, GREEN);
    }
    if !motion && h + 10 <= b + 1 {
        for (i, glyph) in "running".chars().enumerate() {
            paint(h + 3 + i as i64, &glyph.to_string(), GREEN);
        }
    }
}

fn tangle(i: i64, seed: u32) -> &'static str {
    let truncate = |x: f64| x.trunc().rem_euclid(4_294_967_296.) as u32;
    let x = truncate(i as f64 * 374_761_393. + f64::from(seed % 9973) * 668_265_263.);
    let x = truncate(f64::from(x ^ (x >> 13)) * 1_274_126_177.);
    let noise = (x ^ (x >> 16)) % 10_000;
    ["∽", "ℓ", "~", "ℓ", "∽", "~"][(noise as usize * 6 / 10_000).min(5)]
}

fn call_seed(call: &Call<'_>) -> u32 {
    let id = match &call.entry.body {
        Body::Call { call, .. } => call.0.as_str(),
        _ => "",
    };
    id.encode_utf16().fold(2_166_136_261_u32, |hash, unit| {
        (hash ^ u32::from(unit)).wrapping_mul(16_777_619)
    })
}

fn entrance(line: Line<'static>, arrive: f64) -> Line<'static> {
    if arrive >= 1. {
        return line;
    }
    // Only the cells from the target on arrive; the knot and the kind field stand whole.
    let text = line.spans.iter().take(2).map(Span::width).sum::<usize>() as f64;
    let span = (line.width() as f64 - text - 1.).max(1.);
    let mut x = 0.;
    let mut spans = Vec::new();
    for part in line.spans {
        for glyph in part.content.graphemes(true) {
            let k = ease_out(((arrive * 1.35 - (x - text) / span) / 0.3).clamp(0., 1.));
            let k = if x < text { 1. } else { k };
            let ink = mix(GLASS, part.style.fg.unwrap_or(DATA), k);
            spans.push(Span::styled(glyph.to_owned(), part.style.fg(ink)));
            x += width(glyph) as f64;
        }
    }
    Line::from(spans)
}

pub(super) fn next(call: &Call<'_>, now: u64, motion: bool) -> u64 {
    let mut at = u64::MAX;
    if call.outcome().is_none() {
        at = if call.input().0 == "shell" {
            u64::MAX
        } else {
            now.saturating_add(300 - now % 300)
        };
        if elapsed(call, now).is_some() {
            at =
                at.min(now.saturating_add(
                    100 - now.saturating_sub(call.entry.at).saturating_add(50) % 100,
                ));
        }
        if motion {
            at = at.min(now.saturating_add(83));
        }
    }
    let since = call.since(now);
    if motion
        && (now.saturating_sub(call.entry.at) < 380
            || since.is_some_and(|t| t < if super::card::earned(call) { 1660 } else { 760 }))
    {
        at = at.min(now.saturating_add(83));
    }
    if let Some(t) = since.filter(|t| *t < 1660) {
        at = at.min(now.saturating_add(1660 - t));
    }
    if let Some(t) = since.filter(|t| !motion && *t < 400 && !call.waiting(now)) {
        at = at.min(now.saturating_add(400 - t));
    }
    at
}
