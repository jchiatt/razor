//! Inputs, replies and notices styled for the transcript’s viewport.
use super::*;
use crate::terminal::motion::FRAME;
use std::f64::consts::TAU;
use unicode_segmentation::UnicodeSegmentation;

// `[Image #N]` in an input's words drawn as its chip, white on the chip ground.
fn chips(spans: Vec<Span<'static>>) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    for s in spans {
        let mut rest: &str = &s.content;
        while let Some(at) = rest.find("[Image #")
            && let Some(end) = rest[at..].find(']').map(|e| at + e + 1)
        {
            out.push(Span::styled(rest[..at].to_owned(), s.style));
            out.push(Span::styled(
                rest[at..end].to_owned(),
                s.style.fg(WHITE).bg(CHIP),
            ));
            rest = &rest[end..];
        }
        out.push(Span::styled(rest.to_owned(), s.style));
    }
    out
}

// The last row of a reply the turn never recorded: `UNSAVED`, then the longest note that fits.
pub(super) fn unsaved(columns: usize, rows: &mut Rows) {
    let notes = [
        "the turn ended before this reply was recorded; the record has no copy of it",
        "the turn ended before it was recorded; the record has no copy",
        "not in the record",
    ];
    let room = columns.saturating_sub(13);
    let note = notes.into_iter().find(|n| width(n) <= room).unwrap_or("");
    let mark = bold(AMBER, AGENT_BG);
    let line = Line::from(vec![
        Span::styled("  ▌ ", style(VIOLET, AGENT_BG)),
        Span::styled("UNSAVED  ", mark),
        Span::styled(note, style(GRAY, AGENT_BG)),
    ]);
    rows.push(Row::on(line, AGENT_BG));
}

pub(super) fn limit_words(
    at: Option<u64>,
    now: u64,
    zone: Option<&jiff::tz::TimeZone>,
) -> Vec<String> {
    let lead = "You've hit your usage limit. Sorry about that.".to_owned();
    let Some((reset, today)) = at.and_then(|at| Some((zoned(at, zone)?, zoned(now, zone)?))) else {
        return vec![lead, "The reset time wasn't given.".into()];
    };
    let day = match reset.date() == today.date() {
        true => String::new(),
        false => reset.strftime("%A ").to_string(),
    };
    let when = format!("{day}at {}", reset.strftime("%-I:%M%P"));
    let Some(ahead) = at.and_then(|at| at.checked_sub(now)).filter(|ms| *ms > 0) else {
        return vec![lead, format!("It reset {when}, so you can go on.")];
    };
    let (minutes, hour) = (ahead.div_ceil(60_000), 3_600_000);
    let until = match (minutes / 60, minutes % 60) {
        _ if ahead < 60_000 => "under a minute".into(),
        _ if ahead >= 36 * hour => format!("{} days", (ahead + 12 * hour) / (24 * hour)),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    };
    vec![lead, format!("It resets {when}, in {until}.")]
}

// A railed card: a blank row, then each part wrapped behind `  │ ` in its tone on its ground.
pub(super) fn card(
    parts: Vec<Vec<Span>>,
    (tone, ground): (Color, Color),
    columns: usize,
    rows: &mut Rows,
) {
    rows.push(Row::plain(""));
    for part in parts {
        rows.wrap(&part, columns.saturating_sub(4), |_, l| {
            let bar = vec![Span::styled("  │ ", Style::default().fg(tone))];
            let row = Row::on(Line::from([bar, l.spans].concat()), ground);
            Row { lead: 4, ..row }
        });
    }
}

pub(super) fn input_rows(
    input: &Input,
    model: &str,
    columns: usize,
    worn: Option<EntryId>,
    rows: &mut Rows,
) {
    let human = matches!(input.from, Origin::Person | Origin::Answer { .. });
    // `YOU` bold in cyan on its band; any other sender's label plain grey.
    let (bg, ink) = if human { (YOU_BG, CYAN) } else { (GLASS, GRAY) };
    let weight = [Modifier::empty(), Modifier::BOLD][usize::from(human)];
    let mut content = input.text.as_str();
    // A handed-over skill is its gift line; the band holds only the person's words.
    if let Some((line, words)) = tools::skill::handed(content, model, columns).filter(|_| human) {
        rows.push(line);
        if let Some(entry) = worn {
            rows.push(Row {
                action: Some(Action::HandBack(entry)),
                ..Row::plain(Line::from(span("     🎁 hand it back", CYAN)))
            });
        }
        if words.is_empty() && input.images.is_empty() {
            return;
        }
        content = words;
    }
    // A job's end notice in Claude Code's `<task-notification>` shape reads as its summary.
    let summary = content
        .strip_prefix("<task-notification>")
        .and_then(|rest| {
            let start = rest.find("<summary>")? + "<summary>".len();
            Some(&rest[start..start + rest[start..].find("</summary>")?])
        });
    let mut content = summary.filter(|_| !human).unwrap_or(content).to_owned();
    // Sent images show as their chips; the pictures stay in the session's directory.
    for n in 1..=input.images.len() {
        let marker = format!("[Image #{n}]");
        if !content.contains(&marker) {
            content.push_str(&format!(" {marker}"));
        }
    }
    let rendered = markdown::render(
        &content,
        columns.saturating_sub(8),
        human,
        None,
        rows.requested(),
    );
    // Who it is from, within five letters so the words keep their column.
    let label = match &input.from {
        // A voice note's own rows above say `YOU`.
        _ if input.voice.is_some() => " ".repeat(6),
        _ if human => " YOU  ".into(),
        Origin::Mail { .. } => "MAIL  ".into(),
        Origin::Heartbeat | Origin::Schedule { .. } => "TIMER ".into(),
        Origin::Program { name } => format!("{:<5} ", clip(&screen_safe(name).to_uppercase(), 5)),
        _ => "RAZOR ".into(),
    };
    rows.markdown(rendered, 8, |i, line, kind| {
        let label = if i == 0 { label.clone() } else { " ".repeat(6) };
        let label = Span::styled(label, style(ink, bg).add_modifier(weight));
        Row {
            lead: 8,
            kind,
            ..Row::on(
                Line::from([vec![Span::raw("  "), label], chips(line.spans)].concat()),
                bg,
            )
        }
    });
}

// A thinking summary: the model's running account between its calls, an aside quieter than a
// reply: gray italic on the glass beside a dim dotted rail, under no cap, and every line of the
// summary a row of its own (GPT returns one title per line). Past four rows it shows three and a
// line that opens it, so the calls around it stay in view; a click on an open one folds it.
// While `live` (the newest entry of a working turn) its first row's rail cell spins, the rail
// breathes on 2.6 s, and a cursor follows the last letter when the whole thought is shown; the
// row draws it past its words, so a copy never takes it. The last letters arrive violet-white
// and each cools to gray over 0.9 s from when it was recorded. `clock` is `(now, recorded)`
// with motion on; without it the cursor stands still and nothing else moves.
pub(super) fn thought(
    text: &str,
    columns: usize,
    (id, open): (EntryId, bool),
    mused: bool,
    (live, clock): (bool, Option<(u64, u64)>),
    rows: &mut Rows,
) {
    let first = rows.total;
    let bar = ["  ┊ ", "  ┊"][usize::from(columns + 2 < usize::from(PHONE))];
    let (lead, ink) = (width(bar), Style::default().fg(GRAY));
    let step = FRAME.as_millis() as u64;
    let now = clock.map(|(now, _)| now).filter(|_| live);
    let breath = now.map_or(0., |t| 0.5 - 0.5 * (t as f64 / 2600. * TAU).cos());
    let rest = mix(mix(VIOLET, GLASS, 0.3), VIOLET_HI, 0.55 * breath);
    let rail = Span::styled(bar, Style::default().fg(rest));
    let spin = now.map(|t| {
        let frame = ["⠃", "⠉", "⠘", "⠰", "⢠", "⣀", "⡄", "⠆"][(t / step % 8) as usize];
        Span::styled(bar.replace('┊', frame), Style::default().fg(VIOLET_HI))
    });
    let age = clock.and_then(|(now, at)| now.checked_sub(at));
    let cool = age.map_or(1., |a| (a as f64 / 900.).min(1.));
    if !mused {
        rows.push(Row::plain(""));
    }
    let text = text.trim_end().replace('\n', "  \n");
    let room = columns.saturating_sub(lead);
    let mut said = markdown::render(&text, room, false, None, rows.requested());
    let action = (said.total > 4).then_some(Action::Toggle(id));
    let fold = action.filter(|_| !open).map(|_| said.total - 3);
    said.total = fold.map_or(said.total, |_| 3);
    said.lines.truncate(said.total.saturating_sub(said.first));
    let (total, italic) = (said.total, ink.add_modifier(Modifier::ITALIC));
    let quiet = |s: Span<'static>| Span::styled(s.content, s.style.patch(italic));
    rows.markdown(said, lead, |i, l, kind| {
        let mut words: Vec<_> = l.spans.into_iter().map(quiet).collect();
        // Only the whole thought has a last letter to warm and a cursor to follow it.
        let last = fold.is_none() && i + 1 == total;
        warm(&mut words, if last { cool } else { 1. });
        let bar = spin.as_ref().filter(|_| i == 0).unwrap_or(&rail).clone();
        Row {
            action,
            lead: lead as u16,
            kind,
            cursor: last && live && now.is_none_or(|t| t / 500 % 2 == 0),
            ..Row::plain(Line::from([vec![bar], words].concat()))
        }
    });
    if let Some(more) = fold {
        let words = format!("▸ {more} more line{} · click to open", plural(more));
        let words = Line::from(vec![rail, Span::styled(clip(&words, room), ink)]);
        rows.push(Row {
            action,
            ..Row::plain(words).frame()
        });
    }
    if let Some((now, _)) = clock.filter(|_| live || cool < 1.) {
        rows.moving(first, Next::at(now + step));
    }
}

// The last 14 letters of a row, violet-white through violet to the row's gray: a letter `d` from
// the end is `d / 14 + cool` of the way there, `cool` being the thought's age over 0.9 s. So the
// last letter cools too, and the warm stretch shortens to nothing as the thought ages.
fn warm(words: &mut Vec<Span<'static>>, cool: f64) {
    let n = (14. * (1. - cool)).ceil() as usize;
    let mut tail = Vec::with_capacity(n);
    while tail.len() < n
        && let Some(span) = words.pop()
    {
        let at = span
            .content
            .grapheme_indices(true)
            .rev()
            .nth(n - tail.len() - 1);
        let at = at.map_or(0, |(at, _)| at);
        if at > 0 {
            words.push(Span::styled(span.content[..at].to_owned(), span.style));
        }
        for g in span.content[at..].graphemes(true).rev() {
            let k = tail.len() as f64 / 14. + cool;
            let ink = mix(mix(VIOLET_HI, VIOLET, 2. * k), GRAY, 2. * k - 1.);
            tail.push(Span::styled(g.to_owned(), span.style.fg(ink)));
        }
    }
    words.extend(tail.into_iter().rev());
}

// A reply under its name line after a blank row; without `sign`, a text block continuing the
// reply above, after a blank row of its ground.
pub(super) fn reply(
    text: &str,
    columns: usize,
    sign: Option<(&str, &str)>,
    rows: &mut Rows,
    entry: Option<(EntryId, bool)>,
) {
    let rail = match columns + 2 < usize::from(PHONE) {
        true => "  ▌",
        false => "  ▌ ",
    };
    let lead = width(rail);
    let rail = Span::styled(rail, style(VIOLET, AGENT_BG));
    match sign {
        None => rows.push(Row::on(Line::from(rail.clone()), AGENT_BG)),
        Some((model, name)) => {
            let room = columns.saturating_sub(lead);
            let model = clip(&crate::terminal::text::model(model), room);
            let left = room.saturating_sub(width(&model) + 3);
            let mut cap = vec![rail.clone(), Span::styled(model, bold(VIOLET, AGENT_BG))];
            if left >= 2 && !name.is_empty() {
                let name = format!(" ❖ {}", clip(&screen_safe(name), left));
                cap.push(Span::styled(name, style(VIOLET, AGENT_BG)));
            }
            rows.push(Row::plain(""));
            rows.push(Row {
                action: entry.map(|(id, _)| Action::Pin(id)),
                ..Row::on(Line::from(cap), AGENT_BG).frame()
            });
        }
    }
    let open = entry.map(|(_, open)| open);
    let rendered = markdown::render(
        text,
        columns.saturating_sub(lead),
        false,
        open,
        rows.requested(),
    );
    let (fold, copy) = (rendered.fold, rendered.copy);
    rows.markdown(rendered, lead, |i, l, kind| {
        let action = entry.and_then(|(id, _)| match Some(i) {
            at if at == fold => Some(Action::Toggle(id)),
            at if at == copy => Some(Action::Copy(id)),
            _ => None,
        });
        // The copy line sits under the block, on the glass with no rail.
        let (bar, bg) = match Some(i) == copy {
            true => (Span::raw(" ".repeat(lead)), GLASS),
            false => (rail.clone(), AGENT_BG),
        };
        let quote = l.spans.first().is_some_and(|s| s.content == "│ ");
        let words = l.spans.into_iter();
        let words = words.map(|s| Span::styled(s.content, style(DATA, bg).patch(s.style)));
        let words = Line::from([bar].into_iter().chain(words).collect::<Vec<_>>());
        // A copy leaves out the fold and copy lines and a quote's bar.
        let kind = action.map_or(kind, |_| markdown::Kind::Frame);
        Row {
            action,
            lead: (lead + 2 * usize::from(quote)) as u16,
            kind,
            ..Row::on(words, bg)
        }
    });
}
