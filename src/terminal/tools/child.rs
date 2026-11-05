//! The child card under the call that launched it: a pin while it starts, the card unrolling,
//! its title, context and a window into its session, then a fold into one row when it ends.
//! Times come from the record; the edge is `rim.rs`.
mod activity;

use super::{
    Next, launch,
    rim::{Lights, Pulse, RIM, Rim},
    slots,
};
use crate::{
    protocol::{AgentView, Body, Entry, EntryId, ReportStatus, State},
    terminal::{
        App, cards,
        motion::{ease_in_out, ease_out, played, progress},
        text::*,
        topbar::reset,
        transcript::{Row, Transcript},
        view::PHONE,
    },
};
use ratatui::{
    style::{Color, Modifier, Stylize},
    text::{Line, Span},
};

/// The card under the row of threads it starts with, and its next look and row-count deadlines.
/// Only the viewport asks for look changes. With motion off the same record times give the
/// settled card without reading `lights`.
pub(in crate::terminal) fn rows(
    app: &App,
    agent: &AgentView,
    launched: &Entry,
    columns: usize,
    now: u64,
    lights: &mut Lights,
) -> (Vec<Row>, Next) {
    let t = app.transcripts.get(&agent.session);
    let parent = agent.parent.as_ref().and_then(|id| app.transcripts.get(id));
    // The children its turn launched before it and after: its thread's slot beside the line.
    let (before, after) = parent.map_or((0, 1), |p| {
        let ties = |(_, e): &(&EntryId, &Entry)| launch(e) && !p.call(e, None).failed();
        let turn = |(_, e): &(&EntryId, &Entry)| !matches!(e.body, Body::TurnEnd { .. });
        let before = p
            .entries
            .range(..launched.id)
            .rev()
            .take_while(turn)
            .filter(ties);
        let after = p.entries.range(launched.id..).take_while(turn).filter(ties);
        (before.count(), after.count())
    });
    let count = slots(before + after, columns);
    let slot = (before + 1).min(count - 1);
    let ended = ended(app, agent);
    let motion = crate::terminal::motion::on(app);
    let rebuilt = agent.rebuilt.and_then(|at| reset(app, at, now));
    // The turn's line beside the card is live until the turn that launched it ends.
    let turn = |(_, e): (&EntryId, &Entry)| matches!(e.body, Body::TurnEnd { .. });
    let line = match parent.is_some_and(|p| p.entries.range(launched.id..).any(turn)) {
        true => rgb(0x41494f),
        false => rgb(0x22773c),
    };
    // Its own children, oldest launch first, with how each ended.
    let mut subs: Vec<_> = (app.agents.values())
        .filter(|a| a.parent.as_ref() == Some(&agent.session))
        .map(|a| (a, self::ended(app, a)))
        .collect();
    subs.sort_by_key(|(a, _)| a.launched);
    let card = Card {
        subs,
        line,
        agent,
        t,
        launched,
        columns,
        slot,
        pipes: count,
        now,
        motion,
        ended,
        rebuilt,
    };
    let gap = pipes(count, slot, |i, _| {
        ("│ ", if i == 0 { line } else { rgb(0xaf77da) })
    });
    let (mut rows, next) = card.rows(lights);
    rows.insert(0, Row::plain(Line::from(gap)));
    (rows, next)
}

/// When `agent` ended, by the record, and whether it failed: its report's entry in its parent's
/// transcript, else its last activity; a stuck child has failed, and one closed with no report
/// (stopped, or closed by its parent's finish) ended without failing.
pub(in crate::terminal) fn ended(app: &App, agent: &AgentView) -> Option<(u64, bool)> {
    let parent = agent.parent.as_ref().and_then(|id| app.transcripts.get(id));
    let report = agent.report.as_ref().map(|r| {
        let at = parent.and_then(|t| t.entries.get(&r.entry));
        (
            at.map_or(agent.last_activity, |e| e.at),
            r.status != ReportStatus::Finished,
        )
    });
    report.or(match agent.state {
        State::Stuck => Some((agent.last_activity, true)),
        State::Done => Some((agent.last_activity, false)),
        _ => None,
    })
}
struct Card<'a> {
    agent: &'a AgentView,
    t: Option<&'a Transcript>,
    launched: &'a Entry,
    columns: usize,
    slot: usize,
    pipes: usize,
    now: u64,
    motion: bool,
    ended: Option<(u64, bool)>,
    /// When its session last rebuilt, as the local time of day.
    rebuilt: Option<String>,
    /// The turn's line beside it.
    line: Color,
    /// Its own children and how each ended, hung from its foot.
    subs: Vec<Sub<'a>>,
}
/// A child of the card's child, and when and whether it failed if it has ended.
type Sub<'a> = (&'a AgentView, Option<(u64, bool)>);
impl Card<'_> {
    fn rows(self, lights: &mut Lights) -> (Vec<Row>, Next) {
        let (agent, t, launched) = (self.agent, self.t, self.launched);
        let (now, motion, ended) = (self.now, self.motion, self.ended);
        let since = ended.map_or(0, |(at, _)| now.saturating_sub(at));
        let fail = ended.is_some_and(|(_, fail)| fail);
        let mut next = Next::NEVER;
        if let Some(rows) = self.short(&mut next) {
            return (rows, next);
        }
        let opening = motion && ended.is_none();
        let open = ease_out(played(opening, now, launched.at + 2000, 420));
        let mut out = self.card(ended.map_or(now, |e| e.0));
        let inside = out.len().saturating_sub(2);
        let roll = if ended.is_some() && !fail && motion {
            ease_in_out(progress(now, ended.map_or(now, |e| e.0) + 420, 580))
        } else {
            0.
        };
        let keep =
            (inside as f64 * ((open - 0.25) / 0.75).clamp(0., 1.) * (1. - roll)).round() as usize;
        let foot = out.pop();
        out.truncate(1 + keep);
        if open >= 0.25 {
            out.extend(foot);
        }
        // A child that is not working rests: its light stops where it is, and its card asks only
        // for the wall's next second, which every resting card's clocks share.
        let live = motion && ended.is_none() && agent.state == State::Working;
        let pulse = if ended.is_some() {
            Pulse::default()
        } else {
            lights.pulse(agent, t, now, live)
        };
        let rim = self.rim(pulse, open);
        rim.paint(&mut out);
        if fail {
            self.failure(&mut out, since, rim.fray);
        }
        if open >= 1. && roll == 0. {
            self.hung(&mut out);
        }
        if ended.is_none() {
            next.look = now + if live { 83 } else { 1000 - now % 1000 };
        }
        if motion && (open < 1. || ended.is_some() && since < if fail { 2007 } else { 1500 }) {
            next.look = now + 83;
        }
        next.size = resize(launched.at, ended, now, motion, inside);
        (out, next)
    }
    fn rim(&self, pulse: Pulse, open: f64) -> Rim<'_> {
        let (now, motion, ended) = (self.now, self.motion, self.ended);
        let fail = ended.is_some_and(|e| e.1);
        let (fray, flash) = super::rim::ending(ended, now, motion);
        let pull = if motion && ended.is_some() && !fail {
            progress(now, ended.map_or(now, |e| e.0) + 1000, 500)
        } else {
            0.
        };
        Rim {
            left: self.slot.min(self.pipes.saturating_sub(1)) * 2,
            columns: self.columns,
            now,
            motion: motion && (ended.is_some() || self.agent.state == State::Working),
            t: self.t,
            open,
            pulse,
            fray,
            flash,
            pull,
        }
    }
    // The card's rows; its age, heartbeat and window ages count the wall's whole seconds.
    fn card(&self, now: u64) -> Vec<Row> {
        let agent = self.agent;
        let (columns, pipes) = (self.columns, self.pipes);
        let wide = columns >= usize::from(PHONE);
        let n = columns.saturating_sub(pipes * 2 + 1);
        let (left, right) = self.title(n, wide, now);
        let (beat, tone) = self.heartbeat(now);
        let mut rows = vec![self.edge(true, left, span(right, GRAY))];
        let vitals = self.context(n, wide, Span::styled(beat.clone(), style(tone, RECESS)));
        rows.push(self.interior(vitals, RECESS));
        let count = WINDOW[usize::from(!wide)];
        let room = n.saturating_sub(if wide { 8 } else { 1 });
        let window = self.window(count, room);
        let from = window.len().saturating_sub(count);
        for i in 0..count {
            let mut spans = vec![Span::raw(if wide { " " } else { "" })];
            if let Some((at, line)) = window.get(from + i) {
                spans.extend(line.spans.clone());
                if let Some(at) = at.as_ref().filter(|_| wide) {
                    let used: usize = spans.iter().map(|s| width(&s.content)).sum();
                    spans.push(Span::raw(" ".repeat(n.saturating_sub(used + 5))));
                    spans.push(Span::styled(
                        format!("{:>4} ", age(secs(now, *at))),
                        style(rgb(0x3d464f), WELL),
                    ));
                }
            }
            rows.push(self.interior(Line::from(spans), WELL));
        }
        // The launch keeps its kind even when the preview no longer includes Start.
        let kind = match &self.launched.body {
            Body::Call { input, .. } => input["kind"].as_str().unwrap_or("worker"),
            _ => "child",
        };
        // The labels fit beside the rule that carries the drops of the children hung from it.
        let drops = self.inset().saturating_sub(3);
        let (label, right) = if wide {
            let hint = ["press to open", ""][usize::from(width(kind) + drops + 26 > n)];
            (kind.to_owned(), span(hint, GRAY_DIM))
        } else {
            let free = n.saturating_sub(width(&beat) + 10 + drops);
            let (_, model, effort) = shed(free, "", &model(&agent.model), effort(agent.effort));
            (joined(&[&model, &effort]), span(beat, tone))
        };
        let label = vec![span(label, GRAY)];
        rows.push(self.edge(false, label, right));
        rows
    }
    // The top edge's words: the name and, wide, the model, effort and brief as room allows; at
    // the right, wide, the call count, and the card's age.
    fn title(&self, n: usize, wide: bool, now: u64) -> (Vec<Span<'static>>, String) {
        let agent = self.agent;
        let age = timer(secs(now, self.launched.at) * 1000);
        let name = Span::styled(clip(&agent.name, n / 2), bold(rgb(0xd6a0ff), GLASS));
        if !wide {
            return (vec![name], age);
        }
        let calls = self.t.map(|t| {
            t.entries
                .values()
                .filter(|e| matches!(e.body, Body::Call { .. }))
                .count()
        });
        let right = match calls {
            Some(n) => format!("{n} call{} · {age}", plural(n)),
            None => age,
        };
        let brief = match &self.launched.body {
            Body::Call { input, .. } => input["brief"].as_str().unwrap_or(""),
            _ => "",
        };
        // Reserve 24 edge cells for stitches.
        let room = n.saturating_sub(width(&name.content) + width(&right) + 30);
        let (task, model, effort) = shed(
            room,
            brief.lines().next().unwrap_or(""),
            &model(&agent.model),
            effort(agent.effort),
        );
        let mut left = vec![name];
        for (words, ink) in [
            (joined(&[&model, &effort]), GRAY),
            (task, mix(DATA, GRAY, 0.4)),
        ] {
            if !words.is_empty() {
                left.extend([
                    Span::raw(" "),
                    span("─", RIM),
                    Span::raw(" "),
                    span(words, ink),
                ]);
            }
        }
        (left, right)
    }
    // An edge: the pipes and this card's corner, its words, the rule and its right-hand words.
    // The bottom edge sets its words three cells further in.
    fn edge(&self, top: bool, left: Vec<Span<'static>>, right: Span<'static>) -> Row {
        let (count, slot) = (self.pipes, self.slot);
        let n = self.columns.saturating_sub(count * 2 + 1);
        let mark = if top { "├─" } else { "╰─" };
        let mut spans = pipes(count, slot, |i, anchor| match i {
            _ if i < anchor => ("│ ", if i == 0 { self.line } else { rgb(0xaf77da) }),
            _ if i == anchor => (mark, if i == 0 { self.line } else { RIM }),
            _ => ("┼─", RIM),
        });
        // The foot's rule carries a `┬` for each child hung from it, then its words.
        let (subs, step) = self.hanging();
        let drop = format!("┬{}", "─".repeat(step - 1));
        let inset = match (top, subs.len()) {
            (true, _) => String::new(),
            (false, 0) => "───".into(),
            (false, m) => format!("{}┬───", drop.repeat(m - 1)),
        };
        let used: usize = left.iter().map(|s| width(&s.content)).sum::<usize>()
            + width(&inset)
            + width(&right.content);
        let rule = "─".repeat(n.saturating_sub(used + 5));
        spans.push(span(inset, RIM));
        spans.push(Span::raw(" "));
        spans.extend(left);
        spans.extend([
            Span::raw(" "),
            span(rule, RIM),
            Span::raw(" "),
            right,
            Span::raw(" "),
            span(if top { "─╮" } else { "─╯" }, RIM),
        ]);
        Row::plain(Line::from(spans)).frame()
    }
    fn interior(&self, line: Line<'static>, bg: Color) -> Row {
        let (count, slot) = (self.pipes, self.slot);
        let n = self.columns.saturating_sub(count * 2 + 1);

        let used: usize = line.spans.iter().map(|s| width(&s.content)).sum();
        let mut spans = pipes(count, slot, |i, anchor| match i {
            0 => ("│ ", self.line),
            _ if i == anchor => ("│ ", RIM),
            _ => ("│ ", rgb(0xaf77da)),
        });
        spans.extend(
            line.spans
                .into_iter()
                .map(|s| Span::styled(s.content, style(DATA, bg).patch(s.style))),
        );
        spans.push(Span::styled(
            " ".repeat(n.saturating_sub(used)),
            style(DATA, bg),
        ));
        spans.push(span("│", RIM));
        Row {
            lead: (count * 2) as u16,
            ..Row::plain(Line::from(spans)).boxed()
        }
    }
    fn context(&self, n: usize, wide: bool, beat: Span<'static>) -> Line<'static> {
        let agent = self.agent;

        let number = cards::number(agent, false);
        let label = if wide { "context " } else { "ctx " };
        let rebuilt = match &self.rebuilt {
            Some(at) if wide => format!(" · rebuilt {at}"),
            _ => String::new(),
        };
        let side = width(&beat.content) + width(&rebuilt);
        let cells = match wide {
            true => 40.min(n.saturating_sub(label.len() + side + 10)),
            false => n.saturating_sub(label.len() + number.width() + 2),
        };
        // The card's bar and number, on this row's ground.
        let mut spans = vec![Span::styled(label, style(GRAY, RECESS))];
        spans.extend(
            cards::bar(agent, cells, false)
                .into_iter()
                .map(|s| s.bg(RECESS)),
        );
        spans.extend([Span::styled(" ", style(GRAY, RECESS)), number.bg(RECESS)]);
        spans.push(Span::styled(rebuilt, style(GRAY, RECESS)));
        if wide {
            let used: usize = spans.iter().map(|s| width(&s.content)).sum();
            spans.push(Span::styled(
                " ".repeat(n.saturating_sub(used + width(&beat.content) + 1)),
                style(DATA, RECESS),
            ));
            spans.push(beat);
        }
        Line::from(spans)
    }
    fn pin(&self) -> Row {
        let agent = self.agent;
        let (columns, count, slot) = (self.columns, self.pipes, self.slot);

        let mut spans = pipes(count, slot, |i, anchor| {
            // The child's thread falls from its pin row in green.
            match i {
                _ if i == anchor => ("╽ ", GREEN),
                0 => ("│ ", self.line),
                _ => ("│ ", rgb(0x41494f)),
            }
        });
        // As a call row: `agent`, the name, ` › ` what it is doing (or `starting`), then its
        // context as the card draws it, and how long it has run.
        let phone = columns < usize::from(PHONE);
        let (tab, cells, tail) = if phone {
            (" agent  ", 5, 3)
        } else {
            ("   agent   ", 10, 11)
        };
        let number = cards::number(agent, false);
        let took = match phone {
            true => String::new(),
            false => dur(self.now.saturating_sub(self.launched.at)),
        };
        let at = columns.saturating_sub(tail + 1 + number.width() + cells);
        let used = 2 * count + width(tab) + width(&agent.name) + 3;
        let doing = Some(agent.headline.as_str())
            .filter(|h| !h.is_empty())
            .unwrap_or("starting");
        let doing = clip(doing, at.saturating_sub(used + 1));
        let gap = " ".repeat(at.saturating_sub(used + width(&doing)));
        spans.extend([
            Span::styled(tab, bold(DATA, GLASS)),
            Span::styled(agent.name.clone(), bold(rgb(0xd6a0ff), GLASS)),
            span(" › ", GRAY),
            span(doing, DATA),
            span(gap, GRAY),
        ]);
        spans.extend(cards::bar(agent, cells, false));
        spans.extend([span(" ", GRAY), number]);
        spans.push(span(format!("{took:>w$}", w = tail - 1), GRAY));
        Row::plain(Line::from(spans))
    }
    fn fold(&self, since: u64, fail: bool) -> Row {
        let agent = self.agent;
        let (columns, count, slot) = (self.columns, self.pipes, self.slot);
        let motion = self.motion;

        let slide = if motion && !fail {
            ((since as f64 - 1500.) / 600.).clamp(0., 1.)
        } else {
            1.
        };
        let mark = if fail { "✗ " } else { "◉ " };
        let tone = if fail {
            AMBER
        } else {
            mix(rgb(0xecc5ff), rgb(0xd6a0ff), (slide * 2.).min(1.))
        };
        let settled = ((since as f64 - 1500.) / (760. / 1.5)).clamp(0., 1.);
        let thread = mix(
            rgb(0x22773c),
            rgb(0x41494f),
            if motion { settled } else { 1. },
        );
        // A failed child ties back to the turn's line dashed: `├┄✗`.
        let mut spans = pipes(count, slot, |i, anchor| match i {
            _ if i == anchor => (mark, tone),
            0 if fail => ("├┄", AMBER),
            _ if fail && i < anchor => ("┄┄", AMBER),
            0 => ("├─", self.line),
            _ if i < anchor => ("──", thread),
            _ => ("│ ", thread),
        });
        // The folded row: the name, the result squeezed to one line, a dotted leader and the
        // card's time in the duration column every call row's time takes, sliding in from the
        // right as the card rolls up.
        let (name, ink, gap) = if fail {
            (format!(" {} ", agent.name), style(GLASS, AMBER), 1)
        } else {
            (agent.name.clone(), style(rgb(0xd6a0ff), GLASS), 2)
        };
        let took = self
            .ended
            .map_or(0, |(at, _)| at.saturating_sub(self.launched.at));
        let tail = format!(" {}", dur(took));
        // One closed with no report was stopped: `⊘ stopped` in grey, as its hung row says.
        let stopped = !fail && agent.report.is_none();
        let (check, tone) = match (fail, stopped) {
            (true, _) => ("", AMBER),
            (_, true) => ("⊘ ", GRAY),
            _ => ("✓ ", DATA),
        };
        let room = columns.saturating_sub(count * 2);
        let free = room.saturating_sub(width(&name) + gap + width(&tail) + width(check) + 1);
        let words = agent.report.as_ref().map_or("", |r| r.text.as_str());
        let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
        let said = match (fail, stopped) {
            (true, _) if words.is_empty() => "failed",
            (_, true) => "stopped",
            _ => &words,
        };
        let said = clip(said, free.max(4));
        let lead = (free + 1).saturating_sub(width(&said)).max(1);
        let leader: String = (0..lead)
            .map(|k| if k % 2 == 0 { ' ' } else { '·' })
            .collect();
        let body = width(check) + width(&said) + lead + width(&tail);
        let offset = ((1. - ease_out(slide)) * body.min(18) as f64).round() as usize;
        spans.extend([
            Span::styled(name, ink.add_modifier(Modifier::BOLD)),
            Span::raw(" ".repeat(gap + offset)),
            span(check, [GREEN, GRAY][usize::from(stopped)]),
            span(said, tone),
            span(leader, mix(GLASS, GRAY_DIM, 0.9)),
            span(tail, GRAY),
        ]);
        Row::plain(Line::from(spans))
    }
    // Its own children hang from its foot, the newest that fit (six wide, four on the phone):
    // `┬` drops on the foot's rule and a row each under the card, the newest first, `╰╴name ·
    // model  doing  24s`, `✓` or `✗` once ended, the drops of those hung before it running past,
    // and `+N` for the ones not shown. The drops' columns, counted from the card's pipes.
    fn hanging(&self) -> (&[Sub<'_>], usize) {
        let wide = usize::from(self.columns >= usize::from(PHONE));
        let step = [5, 10][wide];
        // As many drops as leave the foot its heartbeat and its corner.
        let room = self.columns.saturating_sub(2 * self.pipes + 24);
        let fits = (room / step + 1).clamp(1, [4, 6][wide]);
        (&self.subs[self.subs.len().saturating_sub(fits)..], step)
    }
    // The foot's rule before its words: three cells, or the drops and three after the last.
    fn inset(&self) -> usize {
        let (subs, step) = self.hanging();
        match subs.len() {
            0 => 3,
            m => step * (m - 1) + 4,
        }
    }
    fn hung(&self, out: &mut Vec<Row>) {
        let ((subs, step), n) = (self.hanging(), self.columns);
        let tone = |e: Option<(u64, bool)>| match e {
            None => mix(GREEN, GLASS, 0.25),
            Some((_, fail)) => [GRAY_DIM, AMBER][usize::from(fail)],
        };
        for i in (0..subs.len()).rev() {
            let (sub, ended) = subs[i];
            let mut row = pipes(self.pipes, self.slot, |k, anchor| match k {
                _ if k == anchor => ("  ", GLASS),
                0 => ("│ ", self.line),
                _ => ("│ ", rgb(0xaf77da)),
            });
            let mut push = |text: String, ink, bold: bool| {
                let look = style(ink, GLASS);
                row.push(Span::styled(
                    text,
                    [look, look.add_modifier(Modifier::BOLD)][usize::from(bold)],
                ));
            };
            for &(_, e) in &subs[..i] {
                push(format!("│{}", " ".repeat(step - 1)), tone(e), false);
            }
            let start = self
                .t
                .and_then(|t| t.entries.get(&sub.launched?))
                .map(|e| e.at);
            let ran = start.map(|s| ended.map_or(self.now, |e| e.0).saturating_sub(s));
            let clock = ran.filter(|_| step == 10).map_or_else(String::new, timer);
            let report = sub.report.as_ref().and_then(|r| r.text.lines().next());
            let headline = Some(sub.headline.as_str()).filter(|h| !h.is_empty());
            let (mark, doing, ink) = match ended {
                None => ("", headline.unwrap_or("working"), mix(GRAY, GLASS, 0.1)),
                Some((_, false)) if report.is_none() => ("⊘ ", "stopped", GRAY),
                Some((_, false)) => ("✓ ", report.unwrap_or("finished"), GRAY),
                Some((_, true)) => ("✗ ", report.unwrap_or("stuck"), AMBER),
            };
            let model = match step {
                10 => format!(" · {}", model(&sub.model)),
                _ => String::new(),
            };
            let hidden = self.subs.len() - subs.len();
            let more = if i == 0 && hidden > 0 {
                format!(" +{hidden}")
            } else {
                String::new()
            };
            let used = 2 * self.pipes + step * i + 2 + width(&sub.name) + width(&model) + 2;
            let room = n.saturating_sub(used + width(mark) + width(&more) + width(&clock) + 2);
            push("╰╴".into(), tone(ended), false);
            push(sub.name.clone(), mix(rgb(0xd6a0ff), WHITE, 0.15), true);
            push(format!("{model}  "), GRAY, false);
            push(
                mark.into(),
                match mark {
                    "✗ " => AMBER,
                    "⊘ " => GRAY,
                    _ => GREEN,
                },
                false,
            );
            let doing = clip(doing, room);
            let gap = room.saturating_sub(width(&doing));
            push(doing, ink, false);
            push(more, GRAY, false);
            push(format!("{}{clock}", " ".repeat(gap + 1)), GRAY_DIM, false);
            out.push(Row::plain(Line::from(row)));
        }
    }
    fn short(&self, next: &mut Next) -> Option<Vec<Row>> {
        let (ended, now, motion) = (self.ended, self.now, self.motion);
        let elapsed = now.saturating_sub(self.launched.at);
        let since = ended.map_or(0, |e| now.saturating_sub(e.0));

        // The pin shows for two seconds, motion on or off.
        if ended.is_none() && elapsed < 2000 {
            next.look = self.launched.at + 2000;
            next.size = next.look;
            return Some(vec![self.pin()]);
        }
        if ended.is_some_and(|(_, fail)| !fail) && (!motion || since >= 1500) {
            if motion && since < 2100 {
                next.look = now + 83;
            }
            return Some(vec![self.fold(since, false)]);
        }
        None
    }
    fn failure(&self, out: &mut Vec<Row>, since: u64, fray: f64) {
        let (columns, pipes) = (self.columns, self.pipes);

        if fray > 0.4 {
            let n = columns.saturating_sub(pipes * 2 + 1);
            let a = (n as f64 * 0.31).round() as usize;
            let b = (n as f64 * 0.72).round() as usize;
            let text = format!(
                "{}╵{}╵",
                " ".repeat(pipes * 2 + a),
                " ".repeat(b.saturating_sub(a + 1))
            );
            out.push(Row::plain(Line::styled(
                text,
                style(mix(GLASS, AMBER, fray), GLASS),
            )));
        }
        out.push(self.fold(since, true));
    }
}

const WINDOW: [usize; 2] = [6, 5];
const WELL: Color = rgb(0x070a0e);
// The pipes left of a card: the first in the rail's grey, those of earlier launches lit, this
// card's anchor, and later launches crossing it.
fn pipes(
    pipes: usize,
    slot: usize,
    at: impl Fn(usize, usize) -> (&'static str, Color),
) -> Vec<Span<'static>> {
    let anchor = slot.min(pipes.saturating_sub(1));
    (0..pipes)
        .map(|i| at(i, anchor))
        .map(|(s, tone)| span(s, tone))
        .collect()
}

// The wall's whole seconds from `at` to `now`, so every clock on screen turns on the same second.
fn secs(now: u64, at: u64) -> u64 {
    (now / 1000).saturating_sub(at / 1000)
}

// The brief, model and effort while they fit `room` (each with its three cells of
// separator), else the brief cut, else model and effort, else the model alone, else nothing.
fn shed(room: usize, task: &str, model: &str, effort: &str) -> (String, String, String) {
    let part = |s: &str| if s.is_empty() { 0 } else { width(s) + 3 };
    let (m, e) = (part(model), part(effort));
    if part(task) + m + e <= room {
        (task.into(), model.into(), effort.into())
    } else if room.saturating_sub(m + e + 3) >= 8 {
        (clip(task, room - m - e - 3), model.into(), effort.into())
    } else if m + e <= room {
        (String::new(), model.into(), effort.into())
    } else if m <= room {
        (String::new(), model.into(), String::new())
    } else {
        Default::default()
    }
}

// The words kept, as the card says a model and its effort: `Opus 5.5 high`.
fn joined(words: &[&str]) -> String {
    let words: Vec<_> = words.iter().copied().filter(|w| !w.is_empty()).collect();
    words.join(" ")
}

fn age(s: u64) -> String {
    match s {
        0 => "now".into(),
        1..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        _ => format!("{}h", s / 3600),
    }
}
// The row count is monotone within each opening/rolling window. Find its first change, not
// another light frame: look-only frames must never recount the mounted transcript.
fn resize(launched: u64, ended: Option<(u64, bool)>, now: u64, motion: bool, inside: usize) -> u64 {
    if !motion {
        return u64::MAX;
    }
    let Some((at, fail)) = ended else {
        if now < launched + 2000 {
            return launched + 2000;
        }
        return super::change(now, launched + 2420, |t| {
            let open = ease_out(progress(t, launched + 2000, 420));
            usize::from(open >= 0.25)
                + (inside as f64 * ((open - 0.25) / 0.75).clamp(0., 1.)).round() as usize
        });
    };
    if fail {
        return if now < at + 561 { at + 561 } else { u64::MAX };
    }
    if now < at + 1000 {
        return super::change(now, at + 1000, |t| {
            (inside as f64 * (1. - ease_in_out(progress(t, at + 420, 580)))).round() as usize
        })
        .min(at + 1500);
    }
    if now < at + 1500 { at + 1500 } else { u64::MAX }
}
