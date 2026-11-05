//! The transcript's rows: each mounted entry laid out at the screen's width, the live reply and
//! working line under them, counted in full but owning strings only for the viewport.
mod replies;

use self::replies::{card, input_rows, limit_words, reply, thought, unsaved};
use super::{Action, Row, Transcript};
use crate::protocol::{Body, End, Entry, EntryId, Input, MailKind, Origin, ReportStatus, Trigger};
use crate::terminal::{markdown, motion::progress, text::*, tools, tools::Next, view::PHONE};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use std::{collections::BTreeMap, ops::Range};

impl Transcript {
    pub(super) fn layout_rows(
        &self,
        columns: usize,
        model: &str,
        range: Range<usize>,
        cached: bool,
    ) -> (Rows, BTreeMap<EntryId, (usize, usize)>) {
        let start = self.entries.len().saturating_sub(self.mounted());
        let mut rows = Rows {
            range,
            look: u64::MAX,
            size: u64::MAX,
            ..Rows::default()
        };
        // Entries kept but not drawn: the line counts them, and a click draws one more window.
        if start > 0 {
            let back = self
                .entries
                .keys()
                .nth(start.saturating_sub(super::MOUNT))
                .copied();
            let rest = format!(
                " earlier item{} · scroll up or click to bring them back",
                plural(start)
            );
            let words = [
                ("  ▲ ".into(), GRAY),
                (start.to_string(), WHITE),
                (rest, GRAY),
            ];
            let words = words.map(|(w, ink)| Span::styled(w, Style::default().fg(ink)));
            rows.push(Row {
                action: back.map(Action::Jump),
                ..Row::plain(Line::from(words.to_vec()))
            });
        }
        let entries: Vec<_> = self.entries.values().skip(start).collect();
        let (presence, zone) = (self.presence, self.zone.as_ref());
        let places = tools::places(&entries, &self.jobs, presence, self.motion, zone);
        let (mut children, mut counts, mut closing) = (Vec::new(), BTreeMap::new(), None);
        // Inputs withdrawn before delivery draw nowhere and end nothing. Adjacent replies continue
        // the same reply, adjacent thinking summaries the same aside.
        let (mut follows, mut mused) = (false, false);
        for entry in entries.iter().filter(|e| !self.withdrawn.contains(&e.id)) {
            if matches!(
                entry.body,
                Body::Input(_) | Body::Text { .. } | Body::Thinking { .. } | Body::TurnEnd { .. }
            ) {
                self.child_rows(&mut children, closing.take(), columns, &mut rows);
            }
            let (first, place) = (rows.total, places.get(&entry.id));
            let count = self
                .counts
                .get(&entry.id)
                .map(|(_, count)| *count)
                .filter(|_| cached);
            match count.filter(|count| rows.range.end <= first || first + count <= rows.range.start)
            {
                Some(count) => rows.total += count,
                None => self.entry_rows(entry, place, columns, model, [follows, mused], &mut rows),
            }
            follows = matches!(entry.body, Body::Text { .. });
            mused = matches!(entry.body, Body::Thinking { .. });
            counts.insert(entry.id, (first, rows.total - first));
            if matches!(entry.body, Body::Call { .. }) {
                rows.mark(first, Action::Toggle(entry.id));
                if self.children.contains_key(&entry.id) {
                    children.push(entry.id);
                }
                if place.is_some_and(|p| p.closes && p.threads > 0) {
                    closing = place.map(|p| (*entry, p));
                }
            }
        }
        self.child_rows(&mut children, closing, columns, &mut rows);
        self.live_rows(columns, model, follows, &mut rows);
        (rows, counts)
    }

    // Cards of children launched since the last input, reply or turn end, at that point, each
    // under the row of threads it starts with, then the closing rule of the turn that launched
    // them once it has ended.
    fn child_rows(
        &self,
        children: &mut Vec<EntryId>,
        closing: Option<(&Entry, &tools::Place)>,
        columns: usize,
        rows: &mut Rows,
    ) {
        for id in children.drain(..) {
            if let Some((card, next, _)) = self.children.get(&id) {
                let first = rows.total;
                rows.extend(card.iter().enumerate().map(|(i, row)| Row {
                    action: (i > 0).then_some(Action::Toggle(id)),
                    ..row.clone()
                }));
                rows.moving(first, *next);
            }
        }
        if let Some((entry, place)) = closing {
            let (call, first) = (self.call(entry, Some(place)), rows.total);
            rows.extend(call.close(columns, self.tick, self.motion));
            rows.moving(first, call.next(columns, self.tick, self.motion));
        }
    }

    fn entry_rows(
        &self,
        e: &Entry,
        place: Option<&tools::Place>,
        columns: usize,
        model: &str,
        [follows, mused]: [bool; 2],
        rows: &mut Rows,
    ) {
        match &e.body {
            // A queued input waits in the band above the composer until a turn takes it.
            Body::Input(_) if self.queued.contains(&e.id) => {}
            // An answer is read back on its question's road.
            Body::Input(Input {
                from: Origin::Answer { ask },
                ..
            }) if self.entries.contains_key(ask) => {}
            Body::Input(input) => match self.report_line(input, columns) {
                Some(line) => {
                    rows.push(Row::plain(""));
                    rows.push(Row {
                        action: Some(Action::Toggle(e.id)),
                        ..Row::plain(line)
                    });
                }
                None => {
                    // A handed skill's text is worn off once a later rebuild pages it out.
                    let after = || self.entries.range(e.id..).map(|(_, e)| &e.body);
                    let worn = input.text.starts_with("<skill name=\"")
                        && after().any(|b| matches!(b, Body::Rebuild { .. }));
                    let worn = (worn && self.takes).then_some(e.id);
                    rows.push(Row::plain(""));
                    rows.extend(crate::terminal::audio::note(input, e.id, columns));
                    input_rows(input, model, columns, worn, rows)
                }
            },
            Body::Notice { text } => reply(text, columns, None, rows, None),
            Body::Text { text } => {
                let sign = (!follows).then_some((model, self.name.as_str()));
                let open = self.expanded.contains(&e.id);
                reply(text, columns, sign, rows, Some((e.id, open)));
            }
            Body::Thinking { text } => {
                let open = self.expanded.contains(&e.id);
                // Live while the model has written nothing after it: it is the newest entry
                // of the turn and no reply text is streaming.
                let newest = self.entries.keys().next_back() == Some(&e.id);
                let live = self.working && newest && self.partial.is_empty() && self.newest();
                let clock = self.motion.then_some((self.tick, e.at));
                thought(text, columns, (e.id, open), mused, (live, clock), rows);
            }
            Body::Call { .. } if self.deliveries.contains_key(&e.id) => {
                if let Some(d) = self.deliveries.get(&e.id) {
                    reply(&d.line, columns, Some((model, &self.name)), rows, None);
                    if let Some(voice) = &d.voice {
                        rows.push(crate::terminal::audio::spoken(voice, e.id, columns));
                    }
                }
            }
            Body::Call { name, .. }
                if tools::kind(name) == "skill" && !self.call(e, None).failed() =>
            {
                rows.push(Row::plain(""));
                let (line, next) =
                    tools::skill::line(&self.call(e, None), columns, self.tick, self.motion);
                let first = rows.total;
                rows.push(line);
                rows.moving(first, next);
            }
            Body::Call { .. } => {
                if let Some(asked) = self.asked(e) {
                    rows.push(Row::plain(""));
                    let name = crate::terminal::text::model(model);
                    let first = rows.total;
                    rows.extend(super::question::rows(
                        &asked,
                        &name,
                        columns,
                        self.tick,
                        self.motion,
                    ));
                    return rows
                        .moving(first, super::question::next(&asked, self.tick, self.motion));
                }
                let (call, first) = (self.call(e, place), rows.total);
                rows.extend(call.rows(columns, self.tick, self.motion));
                rows.moving(first, call.next(columns, self.tick, self.motion));
            }
            Body::Rebuild { trigger, note, .. } => self.rebuild(e, *trigger, note, columns, rows),
            Body::TurnEnd { end, inputs, .. } => {
                if let Some(words) = self.unsaved.get(&e.id) {
                    reply(words, columns, Some((model, &self.name)), rows, None);
                    unsaved(columns, rows);
                }
                match (end, self.limited.filter(|l| l.0 == e.id)) {
                    (End::Error(_), Some((_, at, minute))) => {
                        let words = limit_words(at, minute * 60_000, self.zone.as_ref());
                        let ink = Style::default().fg(WHITE);
                        let parts = words.into_iter().map(|w| vec![Span::styled(w, ink)]);
                        card(parts.collect(), (AMBER, AMBER_BG), columns, rows);
                    }
                    (End::Error(error), None) => {
                        let head = Style::default().fg(RED).add_modifier(Modifier::BOLD);
                        let words = [Span::styled("ERROR  ", head), Span::styled(error, WHITE)];
                        card(vec![words.to_vec()], (RED, RED_BG), columns, rows);
                    }
                    // A call's closing line says it; a turn stopped before any call says it alone,
                    // unless it answered nothing (`/quit` on an idle session).
                    (End::Stopped, _) if !inputs.is_empty() => {
                        let turn = self.entries.range(..e.id).rev().map(|(_, e)| &e.body);
                        let mut turn = turn.take_while(|b| !matches!(b, Body::TurnEnd { .. }));
                        if !turn.any(|b| matches!(b, Body::Call { .. })) {
                            let ink = Style::default().fg(GRAY).add_modifier(Modifier::BOLD);
                            rows.push(Row::plain(""));
                            rows.push(Row::plain(Line::styled("  stopped", ink)));
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    // Every row is there from the start, so the rebuild card asks only for look frames.
    fn rebuild(&self, e: &Entry, trigger: Trigger, note: &str, columns: usize, rows: &mut Rows) {
        let p = self.presence;
        let at = if p.focused && p.missed(e.at) {
            p.back
        } else {
            e.at
        };
        // With motion off every part is drawn, as if long after `at`.
        let now = if self.motion { self.tick } else { u64::MAX };
        let k = |delay: u64, ms| progress(now, at + delay, ms);
        let st = |fg| Style::default().fg(fg);
        let ink = |ink: Color, k: f64| st(mix(GLASS, ink, k));
        let (first, inner, stacked) = (rows.total, columns.saturating_sub(6), columns < 78);
        let title = format!(" REBUILD · {} ", super::super::status::how(trigger));
        let title = [title.as_str(), ""][usize::from(width(&title) + 6 > columns)];
        let rule = "═".repeat(columns.saturating_sub(width(title) + 5));
        let top = format!("  ╔═{title}{rule}╗");
        let drawn = (top.chars().count() as f64 * k(0, 300)).round() as usize;
        rows.push(Row::plain(""));
        let top: String = top.chars().take(drawn).collect();
        rows.push(Row::plain(Line::styled(top, ink(GREEN, 1.))).frame());
        // A piece wraps beside its head, which is `lead` columns wide on every row.
        let mut n = 0;
        let mut piece = |rows: &mut Rows, head: &str, lead: usize, spans: &[Span<'_>]| {
            let from = n;
            n += rows.wrap(spans, inner.saturating_sub(lead), |i, l| {
                let fade = k(300 + 60 * (from + i).min(9) as u64, 150);
                let pad = " ".repeat(inner.saturating_sub(lead + l.width()));
                let head = format!("{:lead$}", if i == 0 { head } else { "" });
                let mut spans = vec![
                    Span::styled("  ║ ", ink(GREEN, fade)),
                    Span::styled(head, ink(GRAY, fade)),
                ];
                let words = l.spans.into_iter();
                spans.extend(
                    words.map(|s| Span::styled(s.content, ink(s.style.fg.unwrap_or(DATA), fade))),
                );
                spans.push(Span::styled(format!("{pad} ║"), ink(GREEN, fade)));
                Row {
                    lead: (4 + lead) as u16,
                    ..Row::plain(Line::from(spans)).boxed()
                }
            });
        };
        let why = match trigger {
            Trigger::Threshold => "automatic, at the context threshold",
            Trigger::Overflow => "the host refused an oversized context",
            Trigger::Handover => "a handover was asked for",
            Trigger::Idle => {
                "idle for 50 minutes with a large context, before its prompt cache expired"
            }
            Trigger::Restart => "the earlier conversation could not be continued",
            Trigger::Change => "the model or profile changed",
            Trigger::Night => "the resident's night began",
        };
        // How full: what the turn end before it measured, of the window.
        let mut ends = self
            .entries
            .range(..e.id)
            .rev()
            .filter_map(|(_, e)| match e.body {
                Body::TurnEnd { context, .. } => Some(context),
                _ => None,
            });
        let full = ends.next().flatten().zip(self.window.filter(|w| *w > 0));
        let when = clock(e.at, self.zone.as_ref());
        let fill = full.map(|(c, w)| format!("{}% full", c * 100 / w));
        let facts = [
            (when.as_str(), WHITE),
            ([why, ""][usize::from(stacked)], GRAY),
            (fill.as_deref().unwrap_or(""), AMBER),
        ];
        let (mut line, dot) = (Vec::new(), [" · ", "  ·  "][usize::from(!stacked)]);
        for (i, (text, fg)) in facts.into_iter().filter(|f| !f.0.is_empty()).enumerate() {
            line.extend((i > 0).then(|| Span::styled(dot, st(GRAY))));
            line.push(Span::styled(text, st(fg)));
        }
        // The tokens behind the percentage only when the whole line fits.
        let tokens = full.map_or(String::new(), |(c, w)| {
            format!(" ({} of {})", kilo(c), kilo(w))
        });
        if line.iter().map(Span::width).sum::<usize>() + width(&tokens) <= inner {
            line.push(Span::styled(tokens, st(GRAY)));
        }
        let (text, fg) = [(note, CYAN), ("none written", GRAY_DIM)][usize::from(note.is_empty())];
        let (head, lead) = [("ITS NOTE", 14), ("", 2)][usize::from(stacked)];
        if stacked {
            piece(rows, "", 0, &[Span::styled(why, st(GRAY))]);
        }
        piece(rows, "", 0, &line);
        if stacked {
            piece(rows, "", 0, &[Span::styled("ITS NOTE", st(GRAY))]);
        }
        piece(rows, head, lead, &[Span::styled(text, st(fg))]);
        let last = 300 + 60 * n.saturating_sub(1).min(9) as u64;
        let rule = "═".repeat(columns.saturating_sub(4));
        let bottom = Line::styled(format!("  ╚{rule}╝"), ink(GREEN, k(last, 150)));
        rows.push(Row::plain(bottom).frame());
        let busy = now < at + last + 150;
        rows.moving(first, Next::at(if busy { now + 83 } else { u64::MAX }));
    }

    fn live_rows(&self, columns: usize, model: &str, follows: bool, rows: &mut Rows) {
        // Newer entries not loaded: the line says how many, and a click goes to the latest.
        if !self.newest() {
            let last = self.entries.last_key_value().map_or(0, |(id, _)| id.0);
            let newer = self.cursor.0.saturating_sub(last) as usize;
            let words = format!(
                "  ▼ {newer} newer entr{} · click to jump to the latest",
                if newer == 1 { "y" } else { "ies" }
            );
            let words = Line::styled(words, Style::default().fg(GRAY));
            return rows.push(Row {
                action: Some(Action::Latest),
                ..Row::plain(words)
            });
        }
        if !self.partial.is_empty() {
            let sign = (!follows).then_some((model, self.name.as_str()));
            reply(&self.partial, columns, sign, rows, None);
        }
        if self.working {
            rows.push(Row::plain(""));
            let (first, step) = (rows.total, self.tick - self.tick % 480 + 480);
            let dots = ".".repeat((self.tick / 480 % 4) as usize);
            let green = Style::default().fg(GREEN);
            // Narrow columns and splits say the turn's clock; only the addressed main session's
            // line says how to stop it.
            let since = (self.entries.values().rev())
                .take_while(|e| !matches!(e.body, Body::TurnEnd { .. }))
                .filter(|e| matches!(e.body, Body::Input(_)) && !self.queued.contains(&e.id))
                .last();
            let clock = timer(self.tick.saturating_sub(since.map_or(self.tick, |e| e.at)));
            let keys = match (columns + 2 < usize::from(PHONE), !self.stops.is_empty()) {
                (false, true) => format!("  {} stops", self.stops),
                (false, false) => format!("  this turn {clock}"),
                (true, true) => format!(" {clock} · {} stops", self.stops),
                (true, false) => format!(" {clock}"),
            };
            rows.push(Row::plain(Line::from(vec![
                Span::styled("  ● WORKING", green.add_modifier(Modifier::BOLD)),
                Span::styled(format!("{dots:3}"), green),
                Span::styled(keys, Style::default().fg(GRAY)),
            ])));
            let turn = self.entries.values().rev().take_while(|e| {
                let input = matches!(e.body, Body::Input(_)) && !self.withdrawn.contains(&e.id);
                let open = !input || self.queued.contains(&e.id);
                !matches!(e.body, Body::TurnEnd { .. }) && open
            });
            let rebuilt = turn
                .clone()
                .find(|e| matches!(e.body, Body::Rebuild { .. }));
            // What it is doing: writing, or its newest call's kind and target.
            let call = turn.filter_map(|e| match &e.body {
                Body::Call { name, input, .. } => Some((tools::label(name, input), e.at)),
                _ => None,
            });
            let activity = self
                .delta_at
                .map(|at| ("writing".to_owned(), at))
                .or_else(|| call.into_iter().next());
            if let Some((label, at)) = activity {
                let label = clip(
                    &label,
                    if columns + 2 < usize::from(PHONE) {
                        24
                    } else {
                        70
                    },
                );
                let age = self.tick.saturating_sub(at);
                let ago = if age < 60_000 {
                    format!("{}s", age / 1000)
                } else {
                    timer(age)
                };
                rows.push(Row::plain(Line::styled(
                    format!("    {label} · {ago} ago"),
                    Style::default().fg(GRAY),
                )));
                rows.moving(first, Next::at(at + (age / 1000 + 1) * 1000));
            }
            // A rebuild inside the running turn: how long since.
            if let Some(e) = rebuilt {
                let words = format!(
                    "    since the rebuild {}",
                    timer(self.tick - e.at.min(self.tick))
                );
                rows.push(Row::plain(Line::styled(words, Style::default().fg(GRAY))));
            }
            rows.moving(first, Next::at(step));
        }
        rows.push(Row::plain(""));
    }

    // A child's report, or mail with a sender, as one line: the child's name or else the sender
    // as its mail names it, how it finished or what the mail does, and its first line with
    // content. `None` for every other input.
    fn report_line(&self, input: &Input, columns: usize) -> Option<Line<'static>> {
        let (child, word, tone) = match &input.from {
            Origin::Report { child, status } => match status {
                ReportStatus::Finished => (child, "finished", GREEN),
                ReportStatus::Partial => (child, "partial", AMBER),
                ReportStatus::Blocked => (child, "blocked", AMBER),
            },
            Origin::Mail { id, kind, .. } => match kind {
                MailKind::Note => (&id.0, "says", GRAY),
                MailKind::Question => (&id.0, "asks", GRAY),
                MailKind::Answer { .. } => (&id.0, "answers", GRAY),
                MailKind::Mention => (&id.0, "mentions you", GRAY),
            },
            _ => return None,
        };
        let name = match (self.names.get(&child.on(&self.server)), &input.from) {
            (Some((name, _)), _) => name.into(),
            (None, Origin::Mail { from, .. }) if !from.is_empty() => screen_safe(from),
            (None, Origin::Mail { .. }) => return None,
            _ => "child".into(),
        };
        let first = input.text.lines().map(str::trim).find(|l| !l.is_empty());
        let room = columns.saturating_sub(width(&name) + width(word) + 6);
        let words = clip(&screen_safe(first.unwrap_or_default()), room);
        let ink = |c| Style::default().fg(c);
        let bold = ink(rgb(0xd6a0ff)).add_modifier(Modifier::BOLD);
        Some(Line::from(vec![
            Span::raw("  "),
            Span::styled(name.into_owned(), bold),
            Span::styled(format!(" {word}"), ink(tone)),
            Span::styled(" · ", ink(GRAY)),
            Span::styled(words, ink(DATA)),
        ]))
    }

    /// The opened report or mail: its one line as the header, which closes it, then its whole
    /// text as Markdown from `scroll`, kept within the text.
    pub(super) fn read(&self, b: &mut Buffer, r: Rect, input: &Input, scroll: &mut usize) {
        let head = self.report_line(input, r.width.saturating_sub(2) as usize);
        line(b, r, r.y, &head.unwrap_or_default(), style(DATA, GLASS));
        let body = Rect::new(
            r.x + 4,
            r.y + 1,
            r.width.saturating_sub(6),
            r.height.saturating_sub(1),
        );
        let (columns, height) = (body.width as usize, body.height as usize);
        let total = markdown::render(&input.text, columns, false, None, 0..0).total;
        *scroll = (*scroll).min(total.saturating_sub(height));
        let shown = markdown::render(&input.text, columns, false, None, *scroll..*scroll + height);
        for (i, words) in (shown.first..).zip(&shown.lines) {
            let y = body.y + (i - *scroll) as u16;
            line(b, body, y, &words.words, style(DATA, GLASS));
        }
        let x = r.right().saturating_sub(1);
        scrollbar(b, x, body.y, height, total, *scroll);
    }
}

// Counts every row but keeps only those in `range`, with the earliest instant a kept row looks
// different by itself and the earliest any row's count changes.
#[derive(Default)]
pub(super) struct Rows {
    pub(super) total: usize,
    range: Range<usize>,
    pub(super) rows: Vec<Row>,
    pub(super) look: u64,
    pub(super) size: u64,
}
impl Rows {
    // Folds the next changes of the rows pushed since `first`.
    fn moving(&mut self, first: usize, next: Next) {
        self.size = self.size.min(next.size);
        if first < self.range.end && self.total > self.range.start {
            self.look = self.look.min(next.look);
        }
    }
    // The part of the range still ahead, relative to the next row.
    fn requested(&self) -> Range<usize> {
        self.range.start.saturating_sub(self.total)..self.range.end.saturating_sub(self.total)
    }
    fn push(&mut self, row: Row) {
        if self.range.contains(&self.total) {
            self.rows.push(row);
        }
        self.total += 1;
    }
    fn extend(&mut self, rows: impl IntoIterator<Item = Row>) {
        rows.into_iter().for_each(|row| self.push(row));
    }
    fn markdown(
        &mut self,
        reply: markdown::Reply,
        offset: usize,
        mut row: impl FnMut(usize, Line<'static>, markdown::Kind) -> Row,
    ) {
        for (i, mut line) in reply.lines.into_iter().enumerate() {
            let shift = |l: markdown::Link| markdown::Link {
                columns: l.columns.start + offset..l.columns.end + offset,
                url: l.url,
            };
            let links = line.links.into_iter().map(shift).collect();
            line.kind.shift(offset as u16);
            self.rows.push(Row {
                links,
                ..row(reply.first + i, line.words, line.kind)
            });
        }
        self.total += reply.total;
    }
    // Wraps `spans` into rows, each made by `row` from its line number; returns how many.
    fn wrap(
        &mut self,
        spans: &[Span<'_>],
        columns: usize,
        mut row: impl FnMut(usize, Line<'static>) -> Row,
    ) -> usize {
        let first = self.total;
        visit_wrap(spans, columns, |parts, _, last| {
            if !last || !parts.is_empty() {
                if self.range.contains(&self.total) {
                    self.rows.push(row(self.total - first, to_line(parts)));
                }
                self.total += 1;
            }
            true
        });
        self.total - first
    }
    fn mark(&mut self, first: usize, action: Action) {
        let at = first.saturating_sub(self.range.start).min(self.rows.len());
        let rows = self.rows[at..].iter_mut();
        let rows = rows.filter(|row| row.kind != markdown::Kind::Frame);
        rows.for_each(|row| _ = row.action.get_or_insert(action));
    }
}
