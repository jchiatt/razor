//! Skill lines and their cards.
use super::*;
use ratatui::style::Stylize;

pub(super) const HI: Color = rgb(0xfff1c2);
pub(super) const DIM: Color = rgb(0x8a7a4a);

/// Display metadata decoded once with the result; at most 4,096 description characters, 1,024
/// path characters and two icon columns per retained result (the entry cap bounds the count).
pub(in crate::terminal) struct Info {
    pub(in crate::terminal) name: String,
    description: String,
    pub(super) icon: String,
    source: String,
    bytes: usize,
    complete: bool,
}
impl Info {
    /// Reads only a skill result's display fields; no filesystem access during a draw.
    pub(super) fn read(value: &Value) -> Option<Self> {
        let name = value["name"].as_str()?;
        let dir = value["dir"].as_str()?;
        let text = value["output"].as_str()?;
        let front = text
            .strip_prefix("---\n")
            .and_then(|s| s.split_once("\n---"))
            .filter(|_| value["path"].is_null());
        let description = front.map_or_else(String::new, |(s, _)| field(s, "description"));
        let bytes = front.map_or(text.len(), |(_, rest)| rest.trim_start_matches('\n').len());
        Some(Self {
            name: name.chars().take(256).collect(),
            description,
            icon: icon(front.map_or("", |(s, _)| s)),
            source: dir.chars().take(1024).collect(),
            bytes,
            complete: value["path"].is_null(),
        })
    }
}

fn field(front: &str, key: &str) -> String {
    let mut lines = front.lines();
    let value = lines.find_map(|l| l.strip_prefix(key).and_then(|s| s.strip_prefix(':')));
    let value = value.unwrap_or("").trim();
    let text = if matches!(value, ">" | ">-" | "|" | "|-") {
        lines
            .take_while(|l| l.starts_with([' ', '\t']))
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        value.trim_matches(['\'', '"']).into()
    };
    text.chars().take(4096).collect()
}

fn icon(front: &str) -> String {
    match field(front, "icon") {
        icon if icon.is_empty() => "🧩".into(),
        icon => clip(&icon, 2),
    }
}

pub(super) fn name<'a>(call: &Call<'a>) -> &'a str {
    match &call.entry.body {
        crate::protocol::Body::Call { input, .. } => input["name"].as_str().unwrap_or("skill"),
        _ => "skill",
    }
}

pub(super) fn info<'a>(call: &'a Call<'_>) -> Option<&'a Info> {
    call.decoded.and_then(|d| d.skill.as_ref())
}

/// Elapsed sequence time, or none when settled, including the source's one-second clock skew.
pub(super) fn burst(now: u64, start: u64, total: u64, motion: bool) -> Option<u64> {
    (motion && start <= now.saturating_add(1000) && now < start.saturating_add(total))
        .then(|| now.saturating_sub(start))
}

fn verb(name: &str) -> &'static str {
    let h = name
        .chars()
        .fold(7u32, |h, c| h.wrapping_mul(31).wrapping_add(c as u32));
    [
        "picked up",
        "unlocked",
        "slotted in",
        "tuned in to",
        "borrowed",
    ][h as usize % 5]
}

fn letters(name: &str, age: Option<u64>, wearing: bool, worn: bool) -> Vec<Span<'static>> {
    let n = name.chars().count() as f64;
    let at = age.unwrap_or(1100) as f64;
    let crossing = -2. + ((at - 300.) / 500.) * (n + 4.);
    name.chars()
        .enumerate()
        .map(|(i, ch)| {
            let color = if age.is_none() && worn {
                GRAY
            } else if !wearing && (300. ..800.).contains(&at) && (i as f64 - crossing).abs() <= 1. {
                HI
            } else if (wearing && at < 300.)
                || (!wearing && (at >= 800. || (i as f64) < crossing - 1. && at >= 300.))
            {
                LAMP
            } else {
                DIM
            };
            let span = span(ch.to_string(), color);
            if age.is_none() && worn {
                span
            } else {
                span.bold()
            }
        })
        .collect()
}

/// A single skill line and its next look change; fixed row count, settled with motion off.
pub(in crate::terminal) fn line(
    call: &Call<'_>,
    columns: usize,
    now: u64,
    motion: bool,
) -> (Row, Next) {
    let worn = call.worn;
    let name = name(call);
    let info = info(call);
    let arrival = burst(now, call.entry.at, 1100, motion);
    let wear = worn.and_then(|at| burst(now, at, 900, motion));
    let age = wear.or(arrival);
    let pending = call.result.is_none();
    let letters = letters(
        name,
        if pending { Some(0) } else { age },
        wear.is_some(),
        worn.is_some(),
    );
    let mut left = vec![
        span("  ", CYAN),
        span("⚡ ", DATA),
        span(format!("Claude {} ", verb(name)), DATA),
        span(format!("{} ", info.map_or("🧩", |i| i.icon.as_str())), DATA),
    ];
    left.extend(letters);
    if let Some(info) = info.filter(|i| !i.description.is_empty()) {
        left.extend([
            span(" — ", GRAY_DIM),
            span(sentence(&info.description), GRAY),
        ]);
    }
    let mut right = Vec::new();
    if arrival.is_some() || pending {
        let spark = match arrival.unwrap_or(0) {
            0..600 => "",
            600..700 | 1000.. => "·",
            700..800 | 900..1000 => "✦",
            _ => "✨",
        };
        right.push(span(
            format!("{spark}{}", " ".repeat(3usize.saturating_sub(width(spark)))),
            HI,
        ));
    }
    if worn.is_some() {
        right.push(span("worn off ", AMBER));
    } else if columns >= 60 {
        right.push(span(format!("{} ", clock(call.entry.at, call.zone)), GRAY));
    }
    right.push(span("▸", GRAY));
    let rw: usize = right.iter().map(|s| width(&s.content)).sum();
    let room = columns.saturating_sub(rw + 1);
    let mut left = clip_spans(left, room);
    let used: usize = left.iter().map(|s| width(&s.content)).sum();
    left.push(span(" ".repeat(columns.saturating_sub(used + rw)), DATA));
    left.extend(right);
    let next = if age.is_some() {
        Next::at(now.saturating_add(50))
    } else {
        Next::NEVER
    };
    (Row::plain(Line::from(clip_spans(left, columns))), next)
}

/// A skill the person handed over with `/name`, which the server records ahead of their words
/// as `<skill name="…">`, the `SKILL.md` and `</skill>`: its one gift line and the words after.
pub(in crate::terminal) fn handed<'a>(
    text: &'a str,
    model: &str,
    columns: usize,
) -> Option<(Row, &'a str)> {
    let (name, rest) = text.strip_prefix("<skill name=\"")?.split_once("\">\n")?;
    let (skill, words) = rest.split_once("\n</skill>")?;
    let front = skill
        .strip_prefix("---\n")
        .and_then(|s| s.split_once("\n---"));
    let front = front.map_or("", |(f, _)| f);
    let icon = icon(front);
    let mut left = vec![
        span("  🎁 ", DATA),
        span(
            format!("You handed {} {icon} ", crate::terminal::text::model(model)),
            DATA,
        ),
        span(name.to_owned(), LAMP).bold(),
    ];
    let why = sentence(&field(front, "description"));
    if !why.is_empty() {
        left.extend([span(" — ", GRAY_DIM), span(why, GRAY)]);
    }
    Some((
        Row::plain(Line::from(clip_spans(left, columns))),
        words.trim_start(),
    ))
}

fn when(call: &Call<'_>, at: u64, now: u64) -> String {
    let ago = now.saturating_sub(at);
    let seconds = ago.saturating_add(500) / 1000;
    let words = match ago {
        0..45000 => "just now".into(),
        _ if seconds < 60 => format!("{seconds} s ago"),
        _ if seconds < 3600 => format!("{} min ago", seconds / 60),
        _ => format!("{} h {} min ago", seconds / 3600, seconds / 60 % 60),
    };
    format!("{} · {words}", clock(at, call.zone))
}

/// The next instant the card's relative loaded time changes, including while idle.
pub(in crate::terminal) fn next(at: u64, now: u64) -> u64 {
    let age = now.saturating_sub(at);
    let step = if age < 59500 { 1000 } else { 60000 };
    at.saturating_add(if age < 45000 {
        45000
    } else {
        (age + 500) / step * step + step - 500
    })
}

/// The opened skill's card, including its source and the first rebuild that removed its text.
pub(super) fn card(call: &Call<'_>, columns: usize, now: u64) -> Vec<Row> {
    let worn = call.worn;
    let info = info(call);
    let inner = columns.saturating_sub(8);
    let (state, tone) = match (worn.is_some(), call.result.is_some()) {
        (true, _) => ("  worn off", AMBER),
        (_, true) => ("  in context", LAMP),
        _ => ("  loading", LAMP),
    };
    let mut lines = vec![Line::from(vec![
        span(format!("{} ", info.map_or("🧩", |i| i.icon.as_str())), DATA),
        span(name(call).to_owned(), WHITE).bold(),
        span(state, tone),
    ])];
    if let Some(info) = info {
        lines.extend(wrap(vec![span(info.description.clone(), DATA)], inner));
        lines.push(Line::raw(""));
        let least = if info.complete { "" } else { "at least " };
        let kb = info.bytes as f64 / 1024.;
        let tokens = (info.bytes as f64 / 400. + 0.5).floor() / 10.;
        for (label, value) in [
            // The skill's folder, cut in its middle so it never passes for a file.
            ("from", clip_middle(&info.source, inner.saturating_sub(9))),
            (
                "size",
                format!("{least}{kb:.1} KB · about {tokens:.1}k tokens"),
            ),
        ] {
            card_field(&mut lines, label, &value, inner, DATA);
        }
    }
    let loaded = when(call, call.entry.at, now);
    card_field(&mut lines, "loaded", &loaded, inner, DATA);
    card_field(&mut lines, "chosen", "⚡ Claude powered up", inner, DATA);
    if let Some(at) = worn {
        card_field(
            &mut lines,
            "state",
            &format!(
                "worn off at {} · its text was paged out by a rebuild. Claude can reload it.",
                clock(at, call.zone)
            ),
            inner,
            AMBER,
        );
        lines.push(Line::from(span("🎁 hand it back".to_string(), CYAN)));
    }
    let border = |ch: &str| {
        Row::plain(Line::from(vec![
            span("    ", DATA),
            Span::styled(ch.to_owned(), style(BEZEL_HI, RECESS)),
        ]))
    };
    let rule = "─".repeat(columns.saturating_sub(6));
    let mut rows = vec![border(&format!("┌{rule}┐"))];
    for line in lines {
        let mut spans = vec![span("    ", DATA), span("│ ", BEZEL_HI)];
        let words = clip_spans(line.spans, inner);
        let used: usize = words.iter().map(|s| width(&s.content)).sum();
        spans.extend(words);
        spans.push(span(
            format!("{} │", " ".repeat(inner.saturating_sub(used))),
            BEZEL_HI,
        ));
        for s in spans.iter_mut().skip(1) {
            s.style = s.style.bg(RECESS);
        }
        rows.push(Row::plain(Line::from(spans)));
    }
    rows.push(border(&format!("└{rule}┘")));
    rows
}

/// Draws a scrollable card viewport without reading the skill's file.
pub(in crate::terminal) fn detail(
    call: &Call<'_>,
    b: &mut ratatui::buffer::Buffer,
    r: ratatui::layout::Rect,
    scroll: &mut usize,
    now: u64,
) -> Option<u16> {
    if r.height == 0 {
        return None;
    }
    let rows = card(call, r.width as usize, now);
    *scroll = (*scroll).min(rows.len().saturating_sub(r.height as usize));
    // A worn-off card's last inner row hands the skill back; where it was drawn, if it was.
    let back = (rows.len() - 2)
        .checked_sub(*scroll)
        .filter(|_| call.worn.is_some());
    for (i, row) in rows
        .into_iter()
        .skip(*scroll)
        .take(r.height as usize)
        .enumerate()
    {
        crate::terminal::text::line(b, r, r.y + i as u16, &row.words, style(DATA, GLASS));
    }
    back.filter(|y| *y < usize::from(r.height))
        .map(|y| r.y + y as u16)
}

fn card_field(lines: &mut Vec<Line<'static>>, label: &str, value: &str, inner: usize, tone: Color) {
    for (i, line) in wrap(
        vec![span(value.to_owned(), tone)],
        inner.saturating_sub(9).max(1),
    )
    .into_iter()
    .enumerate()
    {
        let mut spans = vec![span(
            format!("{:<9}", if i == 0 { label } else { "" }),
            GRAY,
        )];
        spans.extend(line.spans);
        lines.push(Line::from(spans));
    }
}
