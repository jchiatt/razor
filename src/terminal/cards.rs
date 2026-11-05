//! A child's card: three rows in the status pane (who and how long, what it is doing, its context
//! and vitals), two in the split's head; its descendants one line each under it. Drawn from the
//! child's row and the newest page the terminal read of its session.
use super::{App, text::*, tools};
use crate::protocol::{AgentView, Body, ReportStatus, SessionId, State};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier},
    text::{Line, Span},
};

/// Columns before a card's words: the selection mark, the glyph or rule, a space.
const LEAD: usize = 3;
/// The narrowest bar the vitals keep their model beside, and the widest.
const BAR: (usize, usize) = (8, 16);
/// The number beside the bar (`100%`), right-aligned in its column.
const NUMBER: usize = 4;
/// Narrower than this a card says its route short (`sol hi`).
const SHORT: usize = 45;
/// An ended card, or an idle child's, leaves the main list five minutes after its last activity; a working
/// one that writes nothing for 30 seconds is quiet, for five minutes a caution.
const KEEP_MS: u64 = 300_000;
const QUIET_MS: u64 = 30_000;
const STALE_MS: u64 = 300_000;
/// A working child's turning glyph, each frame held 180 ms.
pub const PULSE: (u64, [&str; 4]) = (180, ["◐", "◓", "◑", "◒"]);

/// A card's height. The place drawing it picks the tallest its list fits.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Size {
    /// Identity, what it is doing, context and vitals.
    Full,
    /// Identity with the context number, what it is doing.
    Compact,
    /// The glyph, the name, what it is doing when six columns are spare, the clock.
    Mini,
}
impl Size {
    /// Compact while `count` cards fit `rows` at two rows each, else Mini.
    pub fn fit(count: usize, rows: usize) -> Self {
        match count * 2 <= rows {
            true => Size::Compact,
            false => Size::Mini,
        }
    }
    pub fn rows(self) -> usize {
        match self {
            Size::Full => 3,
            Size::Compact => 2,
            Size::Mini => 1,
        }
    }
}

/// What a list of cards says beside each one, decided once for the list so no card names its
/// model where its neighbour does not: the model, and the bar's width down the list.
#[derive(Clone, Copy)]
pub struct Detail {
    model: bool,
    bar: usize,
}

enum Doing {
    Call(&'static str, String, String),
    Words(String, Color),
    Done(ReportStatus, String),
}

/// One child as its card says it.
pub struct Card<'a> {
    pub agent: &'a AgentView,
    pub glyph: &'static str,
    pub word: &'static str,
    pub tone: Color,
    /// Its work is over: the card is drawn quieter.
    over: bool,
    silent: bool,
    pub clock: String,
    /// How long it has run, when its start is read.
    pub ran: Option<u64>,
    pub location: String,
    doing: Doing,
}

/// The reason a top-level session needs the person after an interruption.
fn interrupted(a: &AgentView) -> Option<&str> {
    a.error
        .as_deref()
        .filter(|_| a.interrupted && a.parent.is_none())
}

impl<'a> Card<'a> {
    pub fn of(app: &App, agent: &'a AgentView, now: u64) -> Self {
        let silent = super::status::server(app, &agent.session).and_then(|s| s.silent);
        // With motion off a working card shows its phase glyph; else the pulse asks for its step.
        let pulse = || {
            if super::motion::on(app) && silent.is_none() {
                app.next.every(now, PULSE.0);
                PULSE.1[(now / PULSE.0 % 4) as usize]
            } else {
                "●"
            }
        };
        let interrupted = interrupted(agent);
        let (glyph, word, tone) = match (agent.state, &agent.report) {
            (_, _) if !agent.asks.is_empty() => ("?", "asking", CYAN),
            (_, _) if interrupted.is_some() => ("?", "interrupted", CYAN),
            (State::Working, _) => (pulse(), "working", GREEN),
            (State::NeedsYou, _) => ("○", "idle", GRAY),
            (State::Stuck, _) => ("!", "stuck", RED),
            (State::Done, Some(_)) if agent.branch.is_some() && agent.settled.is_none() => {
                ("◆", "awaiting merge", CYAN)
            }
            (State::Done, Some(_)) => ("✓", "finished", GRAY),
            (State::Done, None) if agent.parent.is_none() => ("⊘", "closed", GRAY),
            (State::Done, None) => ("⊘", "stopped", GRAY),
        };
        let idle = now.saturating_sub(agent.last_activity);
        let prompt = agent.asks.first().map(|a| format!("? {}", a.text));
        let prompt = prompt.or_else(|| interrupted.map(str::to_owned));
        let doing = match (agent.state, &agent.report, prompt) {
            (State::NeedsYou, _, Some(text)) => Doing::Words(text, CYAN),
            (State::NeedsYou, ..) => Doing::Words(
                match (agent.last_reply.as_str(), agent.headline.as_str()) {
                    ("", "") => "No reply yet",
                    ("", headline) => headline,
                    (reply, _) => reply,
                }
                .into(),
                GRAY,
            ),
            (State::Stuck, ..) => Doing::Words("the supervisor marked it stuck".into(), RED),
            (State::Done, Some(report), _) => Doing::Done(report.status, report.text.clone()),
            (State::Done, ..) if agent.parent.is_none() => Doing::Words("closed".into(), GRAY),
            (State::Done, ..) => Doing::Words("stopped by its parent".into(), GRAY),
            (State::Working, ..) if agent.closing => {
                Doing::Words("closing after this turn".into(), CYAN)
            }
            (State::Working, ..) if silent.is_some() => Doing::Words("last: working".into(), GRAY),
            (State::Working, ..) => call(app, agent, now).unwrap_or_else(|| match idle {
                STALE_MS.. => Doing::Words(format!("quiet · no output for {}", timer(idle)), AMBER),
                QUIET_MS.. => Doing::Words(format!("quiet for {}", timer(idle)), GRAY),
                _ => Doing::Words("thinking".into(), GRAY),
            }),
        };
        let over = agent.state == State::Done && glyph != "◆";
        let glyph = if silent.is_some() { "⊘" } else { glyph };
        let ran = lifetime(app, agent, silent.unwrap_or(now));
        Card {
            agent,
            glyph,
            word,
            tone,
            over,
            silent: silent.is_some(),
            clock: ran.map_or("?".into(), timer),
            ran,
            location: String::new(),
            doing,
        }
    }

    /// Whether the main list keeps it rather than the archive line; an idle session waits for
    /// the person, so only an idle child counts as ended.
    pub fn kept(&self, now: u64) -> bool {
        let ended = self.over || self.word == "idle" && self.agent.parent.is_some();
        let settled = self.agent.settled.is_some();
        !ended || !settled && now.saturating_sub(self.agent.last_activity) < KEEP_MS
    }

    /// The child's model and effort as the card says them, `Opus 5.5 high`, or `Opus hi` short.
    fn model(&self, short: bool) -> String {
        let (name, level) = (model(&self.agent.model), effort(self.agent.effort));
        if !short {
            return format!("{name} {level}");
        }
        let version = |w: &&str| w.chars().all(|c| c.is_ascii_digit() || c == '.');
        let mut words = name.split_whitespace();
        let first = words.next().unwrap_or_default();
        let word = words.rev().find(|w| !version(w)).unwrap_or(first);
        let level = match level {
            "high" => "hi",
            "xhigh" => "xh",
            "medium" => "med",
            other => other,
        };
        format!("{word} {level}")
    }

    /// The card's rows, each `width` columns with the selection mark's column first.
    pub fn rows(
        &self,
        size: Size,
        width: usize,
        detail: Detail,
        look: (bool, bool),
    ) -> Vec<Line<'static>> {
        let (selected, open) = look;
        // Under 12 columns a card is its name alone, clipped.
        if width < 12 {
            return vec![Line::styled(
                clip(&self.agent.name, width),
                style(WHITE, GLASS),
            )];
        }
        let room = width.saturating_sub(LEAD);
        let rule = match self.over {
            true => GRAY_DIM,
            false => mix(GLASS, self.tone, 0.55),
        };
        let count = size.rows();
        (0..count)
            .map(|i| {
                let mark = if i == 0 && selected { "❯" } else { " " };
                let edge = match i {
                    0 => Span::styled(self.glyph, style(self.tone, GLASS)),
                    _ if i + 1 == count => Span::styled("╰", style(rule, GLASS)),
                    _ => Span::styled("│", style(rule, GLASS)),
                };
                let mut row = vec![bold(mark, CYAN), edge, span(" ", GRAY)];
                row.extend(match i {
                    0 => self.identity(size, room, (detail, width < SHORT), open),
                    1 => self.doing(room),
                    _ => self.vitals(room, detail, width < SHORT),
                });
                pad(&mut row, width);
                if self.silent {
                    dim(&mut row);
                }
                Line::from(row)
            })
            .collect()
    }

    // The name and clock stay; location takes the model/state's space on a root card.
    fn identity(
        &self,
        size: Size,
        room: usize,
        say: (Detail, bool),
        open: bool,
    ) -> Vec<Span<'static>> {
        let (detail, short) = say;
        let clock = width(&self.clock);
        let location = width(&self.location);
        let compact = size == Size::Compact || size == Size::Mini && location > 0;
        let context = usize::from(compact && fill(self.agent).is_some()) * 6;
        let reserve = clock + location + context + 2 + usize::from(location > 0);
        let name = clip(&self.agent.name, room.saturating_sub(reserve).max(1));
        let mid = room.saturating_sub(width(&name) + clock + 2);
        let ink = match (open, self.over) {
            (true, _) => CYAN,
            (_, true) => GRAY,
            _ => WHITE,
        };
        let mut row = vec![if self.over {
            span(name, ink)
        } else {
            bold(name, ink)
        }];
        if location > 0 {
            row.extend([span("  ", GRAY), span(self.location.clone(), GRAY)]);
        }
        if size == Size::Mini && location == 0 && mid >= 6 {
            row.push(span("  ", GRAY));
            row.extend(match &self.doing {
                Doing::Call(mark, target, _) => vec![
                    bold(*mark, GREEN),
                    span(" ", GRAY),
                    span(clip(target, mid.saturating_sub(4)), MAGENTA),
                ],
                _ => self.doing(mid.saturating_sub(2)),
            });
        }
        let model =
            (location == 0 && size == Size::Compact && detail.model).then(|| self.model(short));
        let words = width(" · ") + width(self.word);
        if let Some(m) = model.filter(|m| width(m) + width(" · ") + words <= mid) {
            row.extend([span(" · ", GRAY_DIM), span(m, GRAY)]);
        }
        if location == 0 && size != Size::Mini && words <= mid {
            row.extend([span(" · ", GRAY_DIM), span(self.word, self.tone)]);
        }
        if compact && fill(self.agent).is_some() {
            let number = number(self.agent, false);
            let at = room.saturating_sub(clock + 2 + number.width());
            if used(&row) + 2 <= at {
                pad(&mut row, at);
                row.push(number);
            }
        }
        pad(&mut row, room.saturating_sub(clock));
        let clock = if self.over { GRAY_DIM } else { GRAY };
        row.push(span(self.clock.clone(), clock));
        row
    }

    fn doing(&self, room: usize) -> Vec<Span<'static>> {
        match &self.doing {
            Doing::Call(mark, target, clock) => {
                let target_room = room.saturating_sub(width(mark) + 3 + width(clock));
                let mut row = vec![bold(*mark, GREEN), span(" ", GRAY)];
                row.push(span(clip(target, target_room), MAGENTA));
                pad(&mut row, room.saturating_sub(width(clock)));
                row.push(span(clock.clone(), GRAY));
                row
            }
            Doing::Words(text, ink) => vec![span(clip(text, room), *ink)],
            Doing::Done(status, text) => {
                let (lead, ink) = match status {
                    ReportStatus::Finished => ("✓ done", GREEN),
                    ReportStatus::Partial => ("◐ partial", AMBER),
                    ReportStatus::Blocked => ("! blocked", AMBER),
                };
                let text = text.lines().next().unwrap_or_default();
                let mut row = vec![span(lead, ink)];
                if !text.is_empty() {
                    let text = clip(text, room.saturating_sub(width(lead) + 3));
                    row.extend([span(" · ", GRAY_DIM), span(text, GRAY)]);
                }
                row
            }
        }
    }

    // The model where the list has room for it.
    fn right(&self, detail: Detail, short: bool) -> Vec<Span<'static>> {
        let ink = if self.over { GRAY_DIM } else { GRAY };
        let model = detail.model.then(|| span(self.model(short), ink));
        model.into_iter().collect()
    }

    fn right_needs(&self, detail: Detail, short: bool) -> usize {
        let right = used(&self.right(detail, short));
        if right > 0 { right + 2 } else { 0 }
    }

    // The context as a bar and its number, then at the right the model.
    fn vitals(&self, room: usize, detail: Detail, short: bool) -> Vec<Span<'static>> {
        let right = self.right(detail, short);
        let free = room.saturating_sub(1 + NUMBER + self.right_needs(detail, short));
        let cells = if detail.bar > 0 {
            detail.bar.min(free)
        } else {
            free.min(BAR.1)
        };
        let (mut row, number) = (
            bar(self.agent, cells, self.over),
            number(self.agent, self.over),
        );
        row.push(span(
            " ".repeat((1 + NUMBER).saturating_sub(number.width())),
            GRAY,
        ));
        row.push(number);
        if !right.is_empty() {
            pad(&mut row, room.saturating_sub(used(&right)));
            row.extend(right);
        }
        clip_spans(row, room)
    }

    /// One descendant's line under a card: its connectors, its name, then what is left of its
    /// model, state and clock; the clock goes first, then the state, and the name stays.
    pub fn line(&self, prefix: String, room: usize, selected: bool) -> Line<'static> {
        let prefix = match selected {
            true => prefix.replacen(' ', "❯", 1),
            false => prefix,
        };
        let (long, short) = (self.model(false), self.model(true));
        let mut tiers = vec![
            (&short, true, true),
            (&short, true, false),
            (&short, false, false),
        ];
        if room >= SHORT {
            tiers.insert(0, (&long, true, true));
        }
        let need = |(m, s, c): (&String, bool, bool)| {
            let state = if s { 3 + width(self.word) } else { 0 };
            3 + width(m) + state + if c { 3 + width(&self.clock) } else { 0 }
        };
        let (head, name) = (width(&prefix), width(&self.agent.name));
        let pick = tiers
            .iter()
            .copied()
            .find(|t| head + name + need(*t) <= room);
        let (model, state, clock) = pick.unwrap_or(tiers[tiers.len() - 1]);
        let shown = clip(
            &self.agent.name,
            room.saturating_sub(head + need((model, state, clock)))
                .max(6.min(name)),
        );
        let name_ink = if self.over { GRAY } else { WHITE };
        let mut row = vec![
            span(prefix, GRAY_DIM),
            Span::styled(shown, style(name_ink, GLASS)),
        ];
        if selected {
            row[1].style = row[1].style.add_modifier(Modifier::BOLD);
        }
        row.extend([span(" · ", GRAY_DIM), span(model.clone(), GRAY)]);
        if state {
            row.push(span(format!(" · {}", self.word), self.tone));
        }
        if clock {
            row.push(span(format!(" · {}", self.clock), self.tone));
        }
        let mut row = clip_spans(row, room);
        if self.silent {
            dim(&mut row);
        }
        Line::from(row)
    }
}

/// What a list of cards `width` columns wide at `size` says beside each. Full: the model when
/// every card has a bar of at least eight cells beside it, and one bar width down the list.
/// Compact: the model when every card has its name, model, state and clock with five columns
/// spare. Mini: neither.
pub fn detail(cards: &[&Card], columns: usize, size: Size) -> Detail {
    let (room, short) = (columns.saturating_sub(LEAD), columns < SHORT);
    if size != Size::Full {
        let needs = |c: &&Card| {
            width(&c.agent.name) + width(&c.model(short)) + width(c.word) + width(&c.clock) + 8
        };
        return Detail {
            model: size == Size::Compact && cards.iter().all(|c| needs(c) + 5 <= room),
            bar: 0,
        };
    }
    let bar = |model: bool| {
        let each = cards.iter().map(|c| {
            room.saturating_sub(1 + NUMBER + c.right_needs(Detail { model, bar: 0 }, short))
        });
        each.min().unwrap_or(0).min(BAR.1)
    };
    match bar(true) {
        n if n >= BAR.0 || short && n > 0 => Detail {
            model: true,
            bar: n,
        },
        _ => Detail {
            model: false,
            bar: bar(false),
        },
    }
}

/// `parent`'s own children, oldest launch first.
pub fn children<'a>(app: &'a App, parent: &SessionId) -> Vec<&'a AgentView> {
    let mut all: Vec<_> = (app.agents.values())
        .filter(|a| a.parent.as_ref() == Some(parent))
        .collect();
    all.sort_by(|a, b| (a.launched, &a.name).cmp(&(b.launched, &b.name)));
    all
}

/// The descendants of `root` at every depth, depth first, oldest launch first; ended ones fold
/// into a count unless `open`, except one that something kept hangs from.
pub fn descendants<'a>(
    app: &'a App,
    root: &SessionId,
    open: bool,
    now: u64,
) -> (Vec<(usize, Card<'a>)>, usize) {
    let mut all = Vec::new();
    let mut stack = vec![(root.clone(), 0)];
    while let Some((id, depth)) = stack.pop() {
        if depth > 0
            && let Some(agent) = app.agents.get(&id)
        {
            all.push((depth, Card::of(app, agent, now)));
        }
        let under = children(app, &id);
        // A record that names its own descendant as its parent cannot loop the walk.
        stack.extend(
            under
                .into_iter()
                .rev()
                .filter(|_| depth < 32)
                .map(|a| (a.session.clone(), depth + 1)),
        );
    }
    let mut keep = vec![false; all.len()];
    let mut below: Option<usize> = None;
    for (i, (depth, card)) in all.iter().enumerate().rev() {
        if below.is_some_and(|b| *depth < b) || card.kept(now) {
            (keep[i], below) = (true, Some(*depth));
        }
    }
    let ended = keep.iter().filter(|k| !**k).count();
    let shown = all
        .into_iter()
        .zip(keep)
        .filter(|(_, k)| open || *k)
        .map(|(c, _)| c);
    (shown.collect(), ended)
}

/// The prefixes that connect each descendant line to the ones above it.
pub fn connectors(depths: &[usize], folded: bool, room: usize) -> Vec<String> {
    let mut lasts: Vec<bool> = Vec::new();
    let mut out = Vec::new();
    for (at, depth) in depths.iter().enumerate() {
        let last = match depths[at + 1..].iter().find(|d| *d <= depth) {
            Some(next) => next < depth,
            None => !(folded && *depth == 1),
        };
        lasts.truncate(depth - 1);
        let levels = lasts.len().min(room.saturating_sub(10) / 2);
        let mut prefix: String = lasts[lasts.len() - levels..]
            .iter()
            .map(|l| if *l { "  " } else { " │" })
            .collect();
        prefix.push_str(if last { " └ " } else { " ├ " });
        out.push(prefix);
        lasts.push(last);
    }
    out
}

/// `▸ 40 finished, 12 stopped`: the archived children as one count line.
pub fn archive_words(cards: &[Card], open: bool) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for card in cards {
        match counts.iter_mut().find(|(word, _)| *word == card.word) {
            Some((_, n)) => *n += 1,
            None => counts.push((card.word, 1)),
        }
    }
    counts.sort_by_key(|(word, _)| {
        ["finished", "stopped", "closed"]
            .iter()
            .position(|w| w == word)
            .unwrap_or(2)
    });
    let counts: Vec<_> = counts
        .iter()
        .map(|(word, n)| format!("{n} {word}"))
        .collect();
    format!("{} {}", if open { '▾' } else { '▸' }, counts.join(", "))
}

/// Draws `card` at `size` into `r` as `detail` decided for its list: the split's head, and each
/// of its children.
pub fn draw(app: &App, b: &mut Buffer, r: Rect, card: &Card, size: Size, detail: Detail) {
    let agent = card.agent;
    let open = app.focus.as_ref() == Some(&agent.session);
    let selected = app.selected_child() == Some(&agent.session);
    let rows = card.rows(size, r.width as usize, detail, (selected, open));
    for (y, words) in (r.y..r.bottom()).zip(rows) {
        line(b, r, y, &words, style(DATA, GLASS));
    }
}

// The newest call still out, from what is read of its session.
fn call(app: &App, agent: &AgentView, now: u64) -> Option<Doing> {
    let t = app.transcripts.get(&agent.session)?;
    let entry = agent
        .calls
        .iter()
        .filter_map(|id| t.entries.get(id))
        .max_by_key(|e| e.at)?;
    let (mark, target) = tools::glance(entry)?;
    Some(Doing::Call(
        mark,
        target,
        timer(now.saturating_sub(entry.at)),
    ))
}

// From the child's start, or its launch in the parent's transcript, to now or its last
// activity; none when neither is read.
fn lifetime(app: &App, agent: &AgentView, now: u64) -> Option<u64> {
    let started = app.transcripts.get(&agent.session).and_then(|t| {
        let mut entries = t.entries.values();
        entries
            .find(|e| matches!(e.body, Body::Start { .. }))
            .map(|e| e.at)
    });
    let launched = || {
        let t = app.transcripts.get(agent.parent.as_ref()?)?;
        Some(t.entries.get(&agent.launched?)?.at)
    };
    let until = match agent.state {
        State::Done => agent.last_activity,
        _ => now,
    };
    let born = || {
        // A session's id begins with its start in UTC: `20261103-152518`.
        let native = agent.session.as_str().get(..15)?;
        let at = jiff::civil::DateTime::strptime("%Y%m%d-%H%M%S", native).ok()?;
        let at = at.to_zoned(jiff::tz::TimeZone::UTC).ok()?.timestamp();
        u64::try_from(at.as_millisecond()).ok()
    };
    started
        .or_else(launched)
        .or_else(born)
        .map(|at| until.saturating_sub(at))
}

// A child's context as a share of its window, held to one; none while unmeasured.
fn fill(agent: &AgentView) -> Option<f64> {
    let (context, window) = agent.context.zip(agent.window)?;
    (window > 0).then(|| (context as f64 / window as f64).min(1.))
}

/// A child's context number as every card reads it: rounded, never `0%` with anything in it,
/// grey (quieter once its work is `over`), amber and bold from 70%, red from the rebuild line;
/// `?` while unmeasured.
pub fn number(agent: &AgentView, over: bool) -> Span<'static> {
    let Some(f) = fill(agent) else {
        return span("?", GRAY_DIM);
    };
    let text = format!("{}%", ((f * 100.).round() as u32).max(u32::from(f > 0.)));
    let ink = match tone(f, agent.threshold) {
        GRAY if over => GRAY_DIM,
        ink => ink,
    };
    if f >= 0.7 {
        bold(text, ink)
    } else {
        span(text, ink)
    }
}

pub(super) fn tone(f: f64, threshold: f64) -> Color {
    match f {
        _ if f >= threshold => RED,
        _ if f >= 0.7 => AMBER,
        _ => GRAY,
    }
}

/// A child's context bar `cells` wide as every card draws it: half cells over a dashed track,
/// any fill at least one, with ticks at 70% and the rebuild line from eight cells; a dotted
/// track while unmeasured.
pub fn bar(agent: &AgentView, cells: usize, over: bool) -> Vec<Span<'static>> {
    let Some(f) = fill(agent) else {
        return vec![span("┄".repeat(cells), GRAY_DIM)];
    };
    let threshold = agent.threshold;
    let ink = match (tone(f, threshold), over) {
        (GRAY, true) => GRAY_DIM,
        (GRAY, false) => mix(GRAY, DATA, 0.55),
        (ink, _) => ink,
    };
    let halves = ((f * cells as f64 * 2.).round() as usize).max(usize::from(f > 0.));
    let track = mix(GRAY_DIM, GLASS, 0.35);
    let mut out: Vec<_> = (0..cells)
        .map(|k| match halves.saturating_sub(2 * k) {
            0 => span("╌", track),
            1 => span("╸", ink),
            _ => span("━", ink),
        })
        .collect();
    if cells >= BAR.0 {
        for (at, line) in [(0.7, AMBER), (threshold, RED)] {
            let i = ((at * cells as f64).round() as usize).min(cells - 1);
            let lit = if f >= at {
                line
            } else {
                mix(GLASS, line, 0.55)
            };
            out[i] = span("╏", lit);
        }
    }
    out
}

fn dim(row: &mut [Span]) {
    for s in row {
        s.style.fg = Some(mix(s.style.fg.unwrap_or(GRAY), GLASS, 0.55));
    }
}

fn bold(text: impl Into<std::borrow::Cow<'static, str>>, ink: Color) -> Span<'static> {
    Span::styled(text, style(ink, GLASS).add_modifier(Modifier::BOLD))
}

fn used(row: &[Span]) -> usize {
    row.iter().map(|s| width(&s.content)).sum()
}

fn pad(row: &mut Vec<Span<'static>>, to: usize) {
    let have = used(row);
    if have < to {
        row.push(span(" ".repeat(to - have), GRAY));
    }
}
