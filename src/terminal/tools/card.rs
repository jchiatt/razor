//! Cards open, light their edges, and pull back into rows; no animation state is retained.
use super::*;
use crate::terminal::motion::{ease_in_out, ease_out, played, progress};
use unicode_segmentation::UnicodeSegmentation;

struct Shape {
    open: f64,
    edge: f64,
    tone: Color,
    live: bool,
}

pub(super) fn earned(call: &Call<'_>) -> bool {
    call.asking()
        || call
            .ended()
            .is_some_and(|end| end.saturating_sub(call.entry.at) >= 2000)
            && has_body(call)
}

fn has_body(call: &Call<'_>) -> bool {
    let (kind, input) = call.input();
    matches!(kind, "shell" | "web") || row::waits(kind, input)
}

fn shape(call: &Call<'_>, now: u64, motion: bool) -> Option<Shape> {
    let age = now.saturating_sub(call.entry.at);
    let since = call.since(now);
    let tone = if call.asking() {
        rgb(0x40b2c6)
    } else {
        rgb(0x40a163)
    };
    if call.failed() {
        let rest = if call.outcome() == Some(Outcome::Unknown) {
            rgb(0xeb392f)
        } else {
            rgb(0xe19e17)
        };
        let since = since.unwrap_or(0);
        let k = call
            .place
            .fixed
            .map(|at| ease_in_out(played(motion, now, at, 620)));
        if k == Some(1.) {
            return None;
        }
        let open = if motion && !earned(call) {
            ease_out(since as f64 / 342.)
        } else {
            1.
        };
        let flash = if motion && !call.waiting(now) {
            (1. - since as f64 / 228.).max(0.)
        } else {
            0.
        };
        return Some(Shape {
            open: k.map_or(open, |k| 1. - k),
            edge: k.map_or(open, |k| 1. - 0.8 * k),
            tone: k.map_or_else(
                || mix(rest, WHITE, 0.7 * flash),
                |k| mix(rest, rgb(0xc18816), k),
            ),
            live: false,
        });
    }
    if let Some(since) = since {
        if !motion || !earned(call) || since >= 560 {
            return None;
        }
        let k = ease_in_out((since as f64 - 160.) / 400.);
        return Some(Shape {
            open: 1. - k,
            edge: 1. - 0.8 * k,
            tone: match call.waiting(now) {
                true => tone,
                false => mix(tone, WHITE, 0.8 * (1. - since as f64 / 160.).max(0.)),
            },
            live: false,
        });
    }
    if !call.asking() && (age < 2000 || !has_body(call)) {
        return None;
    }
    let open = if !motion {
        1.
    } else if call.asking() {
        ease_out(progress(now, call.entry.at, 380))
    } else {
        ease_out(progress(now, call.entry.at.saturating_add(2000), 420))
    };
    Some(Shape {
        open,
        edge: open,
        tone,
        live: motion && !call.asking(),
    })
}

pub(super) fn rows(call: &Call<'_>, columns: usize, now: u64, motion: bool) -> Vec<Row> {
    let Some(s) = shape(call, now, motion).filter(|s| s.open > 0.) else {
        let mut rows = vec![Row::plain(row::draw(call, columns, now, motion))];
        // A mended failure keeps its first failure line under its target.
        let mended = call.failed() && call.place.fixed.is_some();
        if let Some(line) = failure(call).first().filter(|_| mended) {
            let words = clip(
                &screen_safe(line),
                columns.saturating_sub(indent(columns) + 1),
            );
            let spans = [" ".repeat(indent(columns)), words].map(|w| span(w, GRAY));
            rows.push(Row::plain(Line::from(spans.to_vec())));
        }
        return rows;
    };
    let face = face(call, columns, now);
    let n = shown(face.body.len() + 2, s.open);
    let perimeter = (2 * columns + 2 * n).max(1) as f64;
    let light = |i: usize| {
        let k = (i as f64 - now as f64 / 1000. * 26.).rem_euclid(perimeter);
        let d = k.min(perimeter - k) / 9.;
        let tone = if s.live && d < 1. {
            mix(s.tone, rgb(0xbdeece), (1. - d).powi(2))
        } else {
            s.tone
        };
        mix(GLASS, tone, s.edge)
    };
    let mut rows = vec![edge(
        head(call, columns, now, &face),
        &light,
        0,
        false,
        s.open,
    )];
    let pad = |n: usize| Span::styled(" ".repeat(n), style(DATA, face.ground));
    for i in 0..n {
        let line = i.checked_sub(1).and_then(|i| face.body.get(i));
        let used = line.map_or(0, |spans| spans.iter().map(|s| width(&s.content)).sum());
        let mut spans = vec![
            span("│", light(2 * columns + 2 * n - 1 - i)),
            Span::raw(" "),
            pad(indent(columns) - 2),
        ];
        spans.extend(line.into_iter().flatten().cloned());
        spans.push(pad(columns.saturating_sub(indent(columns) + 1 + used)));
        spans.push(span("│", light(columns + i)));
        // The air rows above and below the words hold none of their own.
        let (row, air) = (Row::plain(Line::from(spans)), line.is_none());
        rows.push(if air { row.frame() } else { row.boxed() });
    }
    if n > 0 {
        rows.push(edge(
            foot(columns, &face.foot),
            &light,
            columns + n,
            true,
            1.,
        ));
    }
    // A copy leaves out the edges and the air; a click anywhere on the card opens or folds it.
    for row in &mut rows {
        row.action = Some(crate::terminal::transcript::Action::Toggle(call.entry.id));
    }
    rows
}

// Where a card's body text starts, counted from its left edge.
fn indent(columns: usize) -> usize {
    if columns < usize::from(PHONE) { 10 } else { 13 }
}

#[derive(Default)]
struct Face {
    figure: String,
    run: Option<test::Run>,
    ground: Color,
    body: Vec<Vec<Span<'static>>>,
    foot: String,
}

fn face(call: &Call<'_>, columns: usize, now: u64) -> Face {
    let (kind, input) = call.input();
    let room = columns.saturating_sub(indent(columns) + 1);
    let (ink, ground) = match call.outcome() {
        Some(Outcome::Success) => (DATA, RECESS),
        Some(Outcome::Unknown) => (RED, RED_BG),
        Some(Outcome::Error) => (AMBER, AMBER_BG),
        None => (GRAY, RECESS),
    };
    let mark = if call.failed() { ink } else { GREEN };
    let said = |text: &str| {
        vec![
            Span::styled("› ", style(mark, ground)),
            Span::styled(clip(text, room.saturating_sub(3)), style(ink, ground)),
        ]
    };
    let plain = |text: &str| vec![Span::styled(clip(text, room), style(ink, ground))];
    let command = || super::command(kind, input).unwrap_or("").to_owned();
    let mut face = Face {
        ground,
        ..Face::default()
    };
    if call.failed() {
        // A job's last line amber and the one before it grey; any other's first line amber and
        // the next grey, then what the edit looked for or the address that failed.
        let lines = failure(call);
        let lead = if call.job.is_some() {
            lines.len().saturating_sub(1)
        } else {
            0
        };
        face.body = (lines.into_iter().enumerate())
            .map(|(i, l)| {
                let mut words = said(&screen_safe(l));
                if i != lead {
                    words[1].style.fg = Some(GRAY);
                }
                words
            })
            .collect();
        face.body.extend(sought(call, room, ground));
        face.foot = command();
        return face;
    }
    if call.outcome().is_some() {
        face.run = call.run();
        face.figure = face
            .run
            .map_or_else(|| row::figure(call, now), |run| run.figure());
    }
    if let Some(run) = face.run {
        let room = room.saturating_sub(1);
        let wide = columns >= usize::from(PHONE);
        let dashes = if wide {
            room.saturating_sub(30).clamp(6, 52)
        } else {
            room
        };
        let count = [
            Span::styled((run.passed + run.failed).to_string(), style(DATA, ground)),
            Span::styled(format!(" of {}", run.total), style(GRAY, ground)),
        ];
        let mut gauge = vec![Span::styled(
            "╌".repeat(dashes),
            style(rgb(0x384149), ground),
        )];
        if wide {
            gauge.push(Span::styled("   ", style(DATA, ground)));
            gauge.extend(count);
            face.body.push(gauge);
        } else {
            face.body.extend([gauge, count.into()]);
        }
        face.foot = run.label();
    } else if call.asking() {
        face.body.push(plain(
            input["text"].as_str().unwrap_or("waiting for an answer"),
        ));
    } else if row::waits("agent", input) && call.input().0 == "agent" {
        // A wait's live body: what it waits for, then its limit filling, `━━╸╌╌ 0:42 of 10m`.
        let limit = input["timeout_ms"].as_u64().unwrap_or(600_000).max(1);
        let waited = call
            .ended()
            .unwrap_or(now)
            .saturating_sub(call.entry.at)
            .min(limit);
        // The time fits first; the bar takes what is left, and no bar under six cells.
        let of = match format!(" {} of {}", timer(waited), timer(limit)) {
            of if 7 + width(&of) <= room => of,
            _ => format!(" {}", timer(waited)),
        };
        let cells = room.saturating_sub(8 + width(&of)).min(40);
        let cells = if cells < 6 { 0 } else { cells };
        let done = (waited as f64 / limit as f64 * cells as f64).round() as usize;
        let tip = if done < cells { "╸" } else { "" };
        let rest = cells.saturating_sub(done + tip.len().min(1));
        let what = clip("the next input", room.saturating_sub(8));
        let parts = [("for    ".to_string(), GRAY), (what, DATA)];
        face.body.push(
            parts
                .map(|(w, ink)| Span::styled(w, style(ink, ground)))
                .to_vec(),
        );
        face.body.push(vec![
            Span::styled("limit  ", style(GRAY, ground)),
            Span::styled("━".repeat(done) + tip, style(GREEN, ground)),
            Span::styled("╌".repeat(rest), style(GRAY_DIM, ground)),
            Span::styled(of, style(GRAY, ground)),
        ]);
    } else {
        // The newest three lines, `› ` green and data on the newest and dim and grey before it,
        // kept while a finished card pulls in; with none yet, how long it has run.
        let done = call.outcome().is_some();
        let tail = call.tail.filter(|(text, _)| !text.is_empty() && !done);
        let lines: Vec<_> =
            tail.map_or_else(|| call.lines().collect(), |(t, _)| t.lines().collect());
        let last = &lines[lines.len().saturating_sub(3)..];
        for (i, l) in last.iter().enumerate() {
            let (mark, ink) = if i + 1 == last.len() {
                (GREEN, DATA)
            } else {
                (GRAY_DIM, GRAY)
            };
            face.body.push(vec![
                Span::styled("› ", style(mark, ground)),
                Span::styled(
                    clip(&screen_safe(l), room.saturating_sub(3)),
                    style(ink, ground),
                ),
            ]);
        }
        // Keep the command available when its description takes the head's place.
        let job = call.job.map(|(id, ..)| format!("job {}", id.0));
        let said = input.get("description").and(super::command(kind, input));
        let cwd = input["cwd"].as_str().filter(|_| kind == "shell");
        let facts: Vec<_> = [said, job.as_deref(), cwd].into_iter().flatten().collect();
        face.foot = if done { command() } else { facts.join(" · ") };
        if done {
            face.body
                .extend(last.is_empty().then(|| plain("no output")));
        } else if last.is_empty() {
            let age = now.saturating_sub(call.job.map_or(call.entry.at, |j| j.1.start));
            let clock = if age < 1000 {
                "starting… · 0:00".into()
            } else {
                format!("running · {}", timer(age))
            };
            face.body.extend([plain(&clock), plain("no output yet")]);
        }
    }
    face
}

// A failure's lines: its job's last two with content, as the end recorded them (none in an older
// record), else its result's first two.
fn failure<'a>(call: &'a Call<'_>) -> Vec<&'a str> {
    match call.job {
        Some((_, job)) => job.last.iter().map(String::as_str).collect(),
        None => (call.lines().filter(|l| !l.trim().is_empty()).take(2)).collect(),
    }
}

// A failed edit's `looked for <old words>`, a failed web call's `✗ <domain>`.
fn sought(call: &Call<'_>, room: usize, ground: Color) -> Option<Vec<Span<'static>>> {
    let (kind, input) = call.input();
    let first = |k: &str| input[k].as_str().and_then(|s| s.lines().next());
    let room = room.saturating_sub(15);
    let (said, what, ink) = match kind {
        "edit" => ("  looked for ", first("old_string")?, RED),
        "web" => (
            "  ✗ ",
            first("url")?.split("//").nth(1)?.split('/').next()?,
            GRAY,
        ),
        _ => return None,
    };
    let words = [(said, GRAY), (&clip(what, room), ink)];
    Some(
        words
            .map(|(w, ink)| Span::styled(w.to_owned(), style(ink, ground)))
            .to_vec(),
    )
}

// The kind's tab: `├─── shell` and `├──── test` wide, `├─ shell` on the phone; a failure's tab is
// solid in its ink, with how its job ended at the right.
fn head(call: &Call<'_>, columns: usize, now: u64, face: &Face) -> Line<'static> {
    let (kind, input) = call.input();
    let phone = columns < usize::from(PHONE);
    let (kind, target) = match face.run {
        Some(run) => {
            let command = super::command(kind, input).unwrap_or("");
            let bare = command.strip_prefix(run.runner).filter(|_| phone);
            ("test", bare.unwrap_or(command).trim_start().to_owned())
        }
        None => (row::verb(kind), target(kind, input)),
    };
    if call.failed() {
        let (mark, ink, tone) = match call.outcome() {
            Some(Outcome::Unknown) => ("?", RED, rgb(0xeb392f)),
            _ => ("✗", AMBER, rgb(0xe19e17)),
        };
        let figure = format!("{mark} {}", row::failed(call));
        let (tab, tail) = if phone {
            (format!(" {kind:>5} "), 3)
        } else {
            (format!("  {kind:>6} "), 11)
        };
        let figure_at = columns.saturating_sub(tail + width(&figure));
        let right = columns.saturating_sub(figure_at + width(&figure) + 2);
        let start = 2 + width(&tab) + if phone { 1 } else { 2 };
        let target = clip(
            &target,
            figure_at.saturating_sub(start + if phone { 2 } else { 3 }),
        );
        let end = start + width(&target);
        let rule = |n: usize| span("─".repeat(n), tone);
        return Line::from(vec![
            Span::styled(mark, bold(ink, GLASS)),
            span("─", tone),
            Span::styled(tab, bold(GLASS, ink)),
            Span::raw(if phone { " " } else { "  " }),
            span(target, rgb(0xc849ae)),
            Span::raw(" "),
            rule(figure_at.saturating_sub(end + 2)),
            Span::raw(" "),
            Span::styled(figure, bold(ink, GLASS)),
            Span::raw(" "),
            rule(right),
            span("╮", tone),
        ]);
    }
    let mark = if call.asking() { "◇" } else { "├" };
    let dashes = (if phone { 6 } else { 8 } as usize)
        .saturating_sub(width(kind))
        .max(1);
    let prefix = format!(
        "{mark}{} {kind}{}",
        "─".repeat(dashes),
        if phone { "  " } else { "   " }
    );
    let duration = dur(call.ended().unwrap_or(now).saturating_sub(call.entry.at));
    let figure = &face.figure;
    let figures = match (phone, figure.is_empty()) {
        (true, _) => figure.clone(),
        (false, true) => duration.clone(),
        (false, false) => format!("{figure} ── {duration}"),
    };
    let room = columns
        .saturating_sub(width(&prefix) + width(&figures) + if figures.is_empty() { 2 } else { 5 });
    let target = clip(&target, room);
    let rule = "─".repeat(room.saturating_sub(width(&target)));
    let mut spans = vec![
        Span::styled(prefix, bold(WHITE, GLASS)),
        span(target, MAGENTA),
        span(format!(" {rule}"), GRAY_DIM),
    ];
    if !figures.is_empty() {
        spans.push(Span::raw(" "));
        if !figure.is_empty() {
            spans.push(span(figure.clone(), GREEN));
        }
        if !phone {
            spans.push(Span::raw(if figure.is_empty() { "" } else { " ── " }));
            spans.push(span(duration, GRAY));
        }
        spans.push(Span::raw(" ─"));
    }
    spans.push(Span::raw("╮"));
    Line::from(spans)
}

fn foot(columns: usize, label: &str) -> Line<'static> {
    let (prefix, label) = if label.is_empty() {
        ("├".to_owned(), String::new())
    } else {
        let prefix = format!("├{} ", "─".repeat(indent(columns) - 2));
        let label = clip(label, columns.saturating_sub(width(&prefix) + 4));
        (prefix, format!("{label} "))
    };
    let rule = "─".repeat(columns.saturating_sub(width(&prefix) + width(&label) + 1));
    Line::from(vec![
        Span::raw(prefix),
        span(label, GRAY),
        Span::raw(format!("{rule}╯")),
    ])
}

// A card's top or bottom edge, lit by `light` and drawn as far as `open` reaches.
fn edge(
    line: Line<'static>,
    light: &impl Fn(usize) -> Color,
    offset: usize,
    back: bool,
    open: f64,
) -> Row {
    let columns = line.width();
    let reach = (open / 0.35).clamp(0., 1.) * columns as f64;
    let mut x = 0;
    let mut spans = Vec::new();
    for part in line.spans {
        for glyph in part.content.graphemes(true) {
            let edge = matches!(glyph, "─" | "╮" | "╯" | "├");
            let ink = if edge {
                light(offset + if back { columns.saturating_sub(x) } else { x })
            } else {
                part.style.fg.unwrap_or(DATA)
            };
            let glyph = if edge && x as f64 > reach { " " } else { glyph };
            let style = if edge {
                part.style.remove_modifier(Modifier::BOLD)
            } else {
                part.style
            };
            spans.push(Span::styled(glyph.to_owned(), style.fg(ink)));
            x += width(glyph);
        }
    }
    Row::plain(Line::from(spans)).frame()
}

fn shown(n: usize, open: f64) -> usize {
    (n as f64 * ((open - 0.25) / 0.75).clamp(0., 1.)).round() as usize
}

// The card's rows at `now` with `body` lines: its head alone as a row, or head, body and foot.
fn height(call: &Call<'_>, body: usize, now: u64, motion: bool) -> usize {
    let Some(s) = shape(call, now, motion).filter(|s| s.open > 0.) else {
        return 1;
    };
    let n = shown(body + 2, s.open);
    1 + n + usize::from(n > 0)
}

pub(super) fn next(call: &Call<'_>, columns: usize, now: u64, motion: bool) -> Next {
    let shape = shape(call, now, motion);
    let mut next = Next::at(if shape.is_none() {
        row::next(call, now, motion)
    } else {
        u64::MAX
    });
    if motion
        && shape.is_some()
        && (call.outcome().is_none()
            || call
                .since(now)
                .is_some_and(|t| t < if call.failed() { 342 } else { 560 })
            || call
                .place
                .fixed
                .is_some_and(|at| now >= at && now.saturating_sub(at) < 620))
    {
        next.look = now.saturating_add(83);
    }
    let earn = call.entry.at.saturating_add(2000);
    let end = call.place.beat.or_else(|| call.ended());
    let phases = [
        (end.is_none() && !call.asking()).then_some(earn),
        (end.is_none() && !call.asking()).then_some(earn.saturating_add(420)),
        (end.is_none() && call.asking()).then_some(call.entry.at.saturating_add(380)),
        end.filter(|_| call.failed())
            .map(|at| at.saturating_add(342)),
        end.filter(|_| !call.failed() && earned(call))
            .map(|at| at.saturating_add(160)),
        end.filter(|_| !call.failed() && earned(call))
            .map(|at| at.saturating_add(560)),
        call.place.fixed,
        call.place.fixed.map(|at| at.saturating_add(620)),
    ];
    let mut phases = phases
        .into_iter()
        .flatten()
        .filter(|at| *at > now)
        .peekable();
    // The face is read once: its line count does not change before the record does.
    let body = phases.peek().map(|_| face(call, columns, now).body.len());
    let current = body.map(|body| height(call, body, now, motion));
    if let (Some(body), Some(current)) = (body, current)
        && let Some(until) = phases
            .filter(|at| height(call, body, *at, motion) != current)
            .min()
    {
        next.size = change(now, until, |t| height(call, body, t, motion));
    }
    if shape.as_ref().is_some_and(|s| s.live) {
        next.look = next.look.min(now.saturating_add(83));
    }
    // A running card's clocks: the head's tenths (drawn only wide), the body's running time and
    // the quiet seconds since the last output.
    if call.outcome().is_none() && shape.is_some() {
        let tick =
            |from: u64, step: u64| now.saturating_add(step - now.saturating_sub(from) % step);
        if columns >= usize::from(PHONE) {
            next.look = next.look.min(tick(call.entry.at.saturating_sub(50), 100));
        }
        next.look = next
            .look
            .min(tick(call.job.map_or(call.entry.at, |j| j.1.start), 1000));
        if let Some((_, at)) = call.tail.filter(|(text, _)| !text.is_empty()) {
            next.look = next.look.min(tick(*at, 1000));
        }
    }
    next
}
