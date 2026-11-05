//! The status pane and the `/status` card: the next input's model, alerts, usage, the addressed
//! session's children as cards with their descendants, the inbox, and its contexts.

use super::{
    App,
    cards::{self, Card, Size},
    text::*,
    view::{Action, Hit},
};
use crate::protocol::{AgentView, Body, Delivery, Server, SessionId, State, Trigger};
use crate::terminal::bindings;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier},
    text::{Line, Span},
};

/// The oldest actual question across homes, with other questions and unread deliveries counted.
pub fn chip(app: &App, room: usize) -> (&'static str, String, Color) {
    let asks = app
        .agents
        .values()
        .flat_map(|a| a.asks.iter().map(move |q| (a, q)));
    let count = asks.clone().count();
    let unread = app
        .delivered
        .iter()
        .filter(|d| !d.read && !d.archived)
        .count();
    let tail = [
        (count > 1).then(|| format!(" +{}", count - 1)),
        (unread > 0).then(|| format!(" · {unread} unread")),
    ]
    .into_iter()
    .flatten()
    .collect::<String>();
    if let Some((agent, _)) = asks.min_by_key(|(_, q)| (q.at, &q.id)) {
        let available = room.saturating_sub(width(&tail));
        let place = location(
            app,
            &agent.session,
            available.saturating_sub(width(&agent.name) + 4),
        );
        let name = clip(&agent.name, available.saturating_sub(width(&place) + 4));
        return ("?", format!("{name} on {place}{tail}"), CYAN);
    }
    if unread > 0 {
        return ("●", format!("{unread} unread"), CYAN);
    }
    ("◧", "status".into(), GRAY)
}

/// What a press on the chip opens: the pane at its inbox while a delivery is unread, else the pane.
pub fn press(app: &App) -> Action {
    match app.delivered.iter().any(|d| !d.read && !d.archived) {
        true => Action::Inbox(None),
        false => Action::Status,
    }
}

/// The descriptor owning an outer session route; no peer connections live in the terminal.
pub fn server<'a>(app: &'a App, id: &SessionId) -> Option<&'a Server> {
    let name = id.as_str().split_once('@').map_or("", |(_, name)| name);
    app.servers.iter().find(|s| s.name == name)
}

/// The server owning `id` while it does not answer.
fn silent_server<'a>(app: &'a App, id: &SessionId) -> Option<&'a Server> {
    server(app, id).filter(|s| s.silent.is_some())
}

pub(super) fn location(app: &App, id: &SessionId, room: usize) -> String {
    let Some(s) = server(app, id) else {
        return id
            .as_str()
            .split_once('@')
            .map_or("local", |(_, name)| name)
            .into();
    };
    let words = if s.realm.is_empty() {
        s.machine.clone()
    } else {
        format!("{} · {}", s.machine, s.realm)
    };
    if width(&words) > room {
        words.replace(" · ", "·")
    } else {
        words
    }
}

/// One list row, its press, and a press for two cells from a column: a parent's fold mark.
/// Child cards start in their mark column.
pub(super) struct Row(
    pub Line<'static>,
    pub Option<Action>,
    pub Vec<(u16, Action)>,
);

struct Status<'a> {
    app: &'a App,
    width: usize,
    now: u64,
    rows: Vec<Row>,
}

impl Status<'_> {
    fn push(&mut self, line: impl Into<Line<'static>>) {
        self.rows.push(Row(line.into(), None, Vec::new()));
    }

    fn action(&mut self, words: impl Into<Line<'static>>, action: Action) {
        self.rows.push(Row(words.into(), Some(action), Vec::new()));
    }

    fn text(&mut self, text: &str, ink: Color) {
        self.push(vec![span(clip(text, self.width), ink)]);
    }

    // A blank row, then `── LABEL ──────── hint`.
    fn rule(&mut self, label: &str, hint: Option<&str>) {
        self.push(Vec::new());
        self.push(rule(label, hint, self.width));
    }

    fn wrapped(&mut self, text: &str, ink: Color) {
        let words = vec![Span::styled(clip(text, 1024), style(ink, GLASS))];
        let lines = wrap(words, self.width);
        lines.into_iter().for_each(|l| self.push(l));
    }

    fn model(&mut self) {
        let (name, level) = self.app.choice();
        let level = level.map_or("effort pending", effort);
        // A model staged for the next input reads `running → staged` until that input goes.
        let row = self.app.row();
        let running = row.filter(|a| !a.model.is_empty()).map(|a| model(&a.model));
        let name = match running.filter(|r| *r != model(name)) {
            Some(running) => format!("{running} → {}", model(name)),
            None => model(name),
        };
        let choice = clip(&format!("{name} · {level}"), self.width.saturating_sub(8));
        let choice = Span::styled(choice, bold(CYAN, GLASS));
        self.push(Vec::new());
        self.push(vec![span("model   ", GRAY), choice]);
    }

    // A configuration error, the context past its rebuild line, reached limits, then each child
    // that may be stuck.
    fn alerts(&mut self) {
        let app = self.app;
        let agent = app.row();
        let mine = |a: &&AgentView| app.focus.is_some() && a.parent == app.focus;
        let stuck: Vec<_> = (app.agents.values().filter(mine))
            .filter(|a| a.state == State::Stuck)
            .collect();
        let windows = agent.map_or(&[][..], |a| a.usage.as_slice());
        let past = agent.and_then(|a| {
            let pct = a.context? * 100 / a.window.filter(|w| *w > 0)?;
            let line = (a.threshold * 100.).round() as u64;
            (pct >= line).then(|| format!("Context {pct}%, past the automatic rebuild at {line}%."))
        });
        let rejected = windows.iter().any(|w| w.rejected);
        if app.config.is_none() && past.is_none() && stuck.is_empty() && !rejected {
            return;
        }
        self.rule("ALERTS", None);
        for words in [app.config.as_ref(), past.as_ref()].into_iter().flatten() {
            self.wrapped(words, RED);
        }
        for window in windows.iter().filter(|w| w.rejected) {
            let reset = window
                .resets_at
                .and_then(|at| super::topbar::reset(app, at, self.now));
            let reset = reset.map_or("reset time not reported".into(), |at| {
                format!("resets {at}")
            });
            let name = &window.name;
            let why: String = window.reason.iter().map(|r| r.clone() + " · ").collect();
            self.wrapped(&format!("{name} limit reached · {why}{reset}"), RED);
        }
        for child in stuck {
            self.text(&format!("{} may be stuck.", child.name), AMBER);
        }
    }

    fn usage(&mut self) {
        self.rule("USAGE", None);
        let agent = self.app.row();
        let windows = agent.map_or(&[][..], |a| a.usage.as_slice());
        // A plan without a 5-hour limit (Claude Max since 31 October) never reports one, so its
        // row shows only when measured; 7-day always does.
        for name in ["5-hour", "7-day"] {
            let window = windows.iter().find(|w| w.name == name);
            if window.is_some() || name == "7-day" {
                self.window(name, window);
            }
        }
        for window in windows
            .iter()
            .filter(|w| w.name != "5-hour" && w.name != "7-day")
        {
            self.window(&window.name, Some(window));
        }
        let cache = agent
            .and_then(|a| a.cache)
            .map(|c| format!("{:.0}% reused", c * 100.0));
        let cache = cache.map_or(span("—", GRAY_DIM), |c| span(c, WHITE));
        self.push(vec![span("cache ", GRAY), cache]);
    }

    fn window(&mut self, name: &str, window: Option<&crate::protocol::UsageWindow>) {
        let label = if width(name) > 7 {
            self.wrapped(name, GRAY);
            "        ".into()
        } else {
            format!("{name}{}", " ".repeat(8 - width(name)))
        };
        let stale = window.is_some_and(|w| w.resets_at.is_some_and(|at| at <= self.now));
        let pct = window
            .filter(|_| !stale)
            .and_then(|w| w.utilization)
            .filter(|p| p.is_finite() && (0.0..=100.0).contains(p));
        match pct {
            Some(pct) => {
                let cells = self.width.saturating_sub(15);
                let lit = (pct / 100. * cells as f64).round() as usize;
                let ink = if pct >= 100. || window.is_some_and(|w| w.rejected) {
                    RED
                } else if pct >= 80. {
                    AMBER
                } else {
                    GREEN
                };
                self.push(vec![
                    span(label, GRAY),
                    span("━".repeat(lit), ink),
                    span("─".repeat(cells - lit), GRAY_DIM),
                    span(format!("{:>5}", format!("{pct:.0}%")), WHITE),
                ]);
            }
            None => self.text(
                &format!(
                    "{label}{}",
                    if stale {
                        "awaiting a reading"
                    } else {
                        "not measured yet"
                    }
                ),
                GRAY_DIM,
            ),
        }
        let reset = window.and_then(|w| super::topbar::reset(self.app, w.resets_at?, self.now));
        let reset = reset.map_or(span("—", GRAY_DIM), |at| span(at, WHITE));
        self.push(vec![span("resets  ", GRAY), reset]);
        if stale {
            self.text("reset time passed", GRAY_DIM);
        }
        if let Some(at) = window.and_then(|w| w.resets_at).filter(|at| *at > self.now) {
            self.app.next.at(at);
        }
    }

    // Kept children as cards, each with its descendants; the archived ones behind one line.
    fn children(&mut self) {
        let (app, now) = (self.app, self.now);
        let mine = app
            .focus
            .as_ref()
            .map_or(Vec::new(), |id| cards::children(app, id));
        let cards: Vec<_> = mine.into_iter().map(|a| Card::of(app, a, now)).collect();
        let hint = bindings::text(app, "<targets.next> or <children.next> to pick");
        self.rule("CHILDREN", (!cards.is_empty()).then_some(hint.as_str()));
        let (kept, mut archived): (Vec<_>, Vec<_>) = cards.into_iter().partition(|c| c.kept(now));
        // The ended ones most recently stopped first.
        archived.sort_by_key(|c| std::cmp::Reverse(c.agent.last_activity));
        if kept.is_empty() {
            self.text("none out", GRAY_DIM);
        }
        let all: Vec<_> = kept.iter().chain(&archived).collect();
        let detail = cards::detail(&all, self.width + 1, Size::Full);
        for c in &kept {
            self.card(c, detail, Size::Full);
        }
        if !archived.is_empty() {
            self.action(
                span(cards::archive_words(&archived, self.app.archive), GRAY),
                Action::Archive,
            );
        }
        for c in archived.iter().filter(|_| self.app.archive) {
            self.card(c, detail, Size::Full);
        }
    }

    fn card(&mut self, card: &Card, detail: cards::Detail, size: Size) {
        let id = &card.agent.session;
        let selected = self.app.selected_child() == Some(id);
        for line in card.rows(size, self.width + 1, detail, (selected, false)) {
            self.action(line, Action::Child(id.clone()));
        }
        if card.agent.parent.is_none() {
            return;
        }
        let (under, ended) = cards::descendants(self.app, id, self.app.archive, self.now);
        let depths: Vec<_> = under.iter().map(|(depth, _)| *depth).collect();
        let prefixes = cards::connectors(&depths, ended > 0, self.width + 1);
        for ((_, card), prefix) in under.iter().zip(prefixes) {
            let selected = self.app.selected_child() == Some(&card.agent.session);
            let line = card.line(prefix, self.width + 1, selected);
            self.action(line, Action::Child(card.agent.session.clone()));
        }
        if ended > 0 {
            let mark = if self.app.archive { '▾' } else { '▸' };
            let words = clip(&format!("└ {mark} +{ended} ended"), self.width + 1);
            self.action(span(words, GRAY), Action::Archive);
        }
    }

    fn deliveries(&mut self) {
        let (app, columns) = (self.app, self.width.saturating_sub(3));
        let delivered = app.delivered.iter().filter(|d| !d.archived);
        if delivered.clone().next().is_none() {
            return;
        }
        let unread = delivered.clone().filter(|d| !d.read).count();
        self.rule(&format!("INBOX · {unread} unread"), Some("▎ this session"));
        let fresh = delivered
            .clone()
            .next_back()
            .is_some_and(|d| self.now.saturating_sub(d.at) < 60_000);
        app.next.every(self.now, if fresh { 1000 } else { 60_000 });
        // Folded, the newest four, so the pane fits without scrolling; opened, pages of 15.
        let total = delivered.clone().count();
        let (offset, shown) = app
            .inbox
            .map_or((0, 4), |at| (at.min((total - 1) / 15 * 15), 15));
        if app.inbox.is_some() {
            let words = ["↑ newer", "▴ fewer"][usize::from(offset == 0)];
            self.action(span(words, GRAY), Action::Inbox(offset.checked_sub(15)));
        }
        for d in delivered.clone().rev().skip(offset).take(shown) {
            let action =
                Action::Delivery(d.session.clone(), d.entry, d.url.as_deref().map(Into::into));
            let bar = [" ", "▎"][usize::from(app.root.as_ref() == Some(&d.session))];
            // An item is two rows: the bar, a lead, words, and a note at the right edge.
            let row = |lead: Span<'static>, words: Vec<Span<'static>>, note: String| {
                let used = words.iter().map(|s| width(&s.content)).sum::<usize>();
                let gap = " ".repeat(columns.saturating_sub(used + width(&note)));
                let mut row = vec![span(bar, VIOLET), lead];
                row.extend(words);
                row.extend([span(gap, GRAY), span(note, GRAY)]);
                row
            };
            let age = age(self.now.saturating_sub(d.at));
            let subject = clip(&d.line, columns.saturating_sub(width(&age) + 1));
            let ink = [WHITE, GRAY][usize::from(d.read)];
            let face = [Modifier::BOLD, Modifier::empty()][usize::from(d.read)];
            let lead = span(if d.read { "  " } else { "● " }, CYAN);
            let subject = Span::styled(subject, style(ink, GLASS).add_modifier(face));
            self.action(row(lead, vec![subject], age), action.clone());
            let kind = kind(d);
            let place = location(
                app,
                &d.session,
                columns.saturating_sub(width(&d.name) + width(kind) + 4),
            );
            let silent = silent_server(app, &d.session).is_some();
            let who = app
                .agents
                .get(&d.session)
                .map_or(d.name.as_str(), |a| &a.name);
            let suffix = format!(" · {}{place}", if silent { "⊘ " } else { "" });
            let name = clip(
                who,
                columns.saturating_sub(width(&suffix) + width(kind) + 1),
            );
            let words = vec![
                span(name, ink),
                span(suffix, if silent { AMBER } else { GRAY }),
            ];
            self.action(row(span("  ", GRAY), words, kind.into()), action);
            if let Some(voice) = &d.voice {
                let wave = super::audio::spoken(voice, d.entry, self.width);
                self.action(wave.words, Action::Play(d.session.clone(), d.entry));
            }
        }
        let earlier = total.saturating_sub(offset + shown);
        if earlier > 0 {
            let (words, ink, to) = match app.inbox {
                None => (format!("▾ {earlier} more"), GRAY, 0),
                Some(_) => (format!("+{earlier} earlier"), GRAY_DIM, offset + shown),
            };
            self.action(span(words, ink), Action::Inbox(Some(to)));
        }
    }

    // Each context of the retained history: how long it lasted and how it began.
    fn contexts(&mut self) {
        let Some(t) = self.app.transcript() else {
            self.rule("CONTEXTS", None);
            return self.text("not attached", GRAY_DIM);
        };
        let first = t.entries.first_key_value();
        let known = first.is_some_and(|(_, e)| matches!(e.body, Body::Start { .. }));
        let starts: Vec<_> = t
            .entries
            .values()
            .filter_map(|e| match &e.body {
                Body::Start { .. } => Some((e.at, "started")),
                Body::Rebuild { trigger, .. } => Some((e.at, how(*trigger))),
                _ => None,
            })
            .collect();
        // A history read from its start counts its rebuilds in the head.
        match known {
            true => {
                let n = starts.len().saturating_sub(1);
                let label = format!("CONTEXTS · {n} rebuild{} so far", plural(n));
                self.rule(&label, None);
            }
            false => {
                self.rule("CONTEXTS", None);
                self.text("earlier contexts before this history", GRAY_DIM);
            }
        }
        let agent = self.app.row();
        let reading = agent.and_then(|a| Some(a.context? * 100 / a.window.filter(|w| *w > 0)?));
        for (n, (at, how)) in starts.iter().enumerate() {
            let next = starts.get(n + 1).map(|(at, _)| *at);
            let ms = next.unwrap_or(self.now).saturating_sub(*at);
            let number = if known {
                format!("#{}  ", n + 1)
            } else {
                "#?  ".into()
            };
            let (ink, ms_ink, how) = match next {
                Some(_) => (GRAY, GRAY, how.to_string()),
                None => (
                    GREEN,
                    WHITE,
                    reading.map_or("this one".into(), |p| format!("this one · {p}%")),
                ),
            };
            let ms = format!("{:11}", elapsed(ms));
            let how = clip(&how, self.width.saturating_sub(15).max(6));
            self.push(vec![span(number, ink), span(ms, ms_ink), span(how, ink)]);
        }
        if starts.is_empty() {
            self.text("current context start not in this history", GRAY_DIM);
        }
    }
}

fn age(ms: u64) -> String {
    match ms / 1000 {
        seconds @ 0..60 => format!("{seconds}s"),
        seconds @ 60..3600 => format!("{}m", seconds / 60),
        seconds => format!("{}h", seconds / 3600),
    }
}

/// The person's short name for the reason a context was rebuilt.
pub(super) fn how(trigger: Trigger) -> &'static str {
    match trigger {
        Trigger::Threshold | Trigger::Overflow => "automatic",
        Trigger::Handover => "handover",
        Trigger::Idle => "idle for 50 minutes",
        Trigger::Restart => "recovery",
        Trigger::Change => "model change",
        Trigger::Night => "nightly",
    }
}

// `74 s`, `5 min`, `2 h 4 min`.
pub fn elapsed(ms: u64) -> String {
    let s = (ms + 500) / 1000;
    match s / 60 {
        0 => format!("{s} s"),
        m @ 1..60 => format!("{m} min"),
        m => format!("{} h {} min", m / 60, m % 60),
    }
}

/// The `/status` card's rows: the pane's sections with the addressed session's own facts.
pub fn rows(app: &App, width: usize, now: u64) -> Vec<Line<'static>> {
    let mut status = Status {
        app,
        width,
        now,
        rows: Vec::new(),
    };
    status.model();
    if let Some(agent) = app.row() {
        status.text(&format!("session {}", agent.session), GRAY);
        status.text(&format!("state {}", Card::of(app, agent, now).word), WHITE);
        let reading = agent.context.zip(agent.window).filter(|(_, w)| *w > 0);
        let reading = reading.map_or("not measured".into(), |(c, w)| {
            format!(
                "{} of {} · {}%",
                kilo(c),
                kilo(w),
                c.saturating_mul(100) / w
            )
        });
        status.text(&format!("context {reading}"), GRAY);
    }
    let entries = app.transcript().map(|t| t.entries.values().rev());
    let usage = entries.and_then(|mut entries| {
        entries.find_map(|e| match &e.body {
            Body::TurnEnd { usage, .. } => Some(usage),
            _ => None,
        })
    });
    if let Some(u) = usage {
        let (input, output) = (kilo(u.input), kilo(u.output));
        status.text(&format!("last turn {input} in · {output} out"), GRAY);
    }
    status.alerts();
    status.usage();
    status.children();
    status.deliveries();
    status.contexts();
    status.rows.into_iter().map(|Row(line, ..)| line).collect()
}

/// The pane: its tab, the scrolled rows with their presses, and a scrollbar while they overflow.
/// Its foot is where the stage will stand.
pub fn draw(app: &mut App, b: &mut Buffer, r: Rect, now: u64, hits: &mut Vec<Hit>) {
    app.status_area = Some(r);
    fill(b, r, style(WHITE, GLASS));
    if r.width < 6 || r.height < 2 {
        return;
    }
    for y in r.y..r.bottom() {
        put(b, r.x, y, "│", 1, style(BEZEL_HI, GLASS));
    }
    put(b, r.x + 2, r.y, "STATUS", r.width - 2, bold(WHITE, GLASS));
    hits.push((
        Rect::new(r.x + 1, r.y, 8.min(r.width - 1), 1),
        Action::Status,
    ));
    let foot = super::stage::draw(app, b, r, now);
    let mut status = Status {
        app,
        width: r.width.saturating_sub(4) as usize,
        now,
        rows: Vec::new(),
    };
    status.model();
    status.alerts();
    status.usage();
    status.children();
    let inbox = status.rows.len();
    status.deliveries();
    status.contexts();
    let rows = status.rows;
    let (visible, total) = (
        r.height.saturating_sub(1 + foot + u16::from(foot > 0)) as usize,
        rows.len(),
    );
    if std::mem::take(&mut app.to_inbox) {
        app.status_scroll = inbox;
    }
    app.status_scroll = app.status_scroll.min(total.saturating_sub(visible));
    for (y, Row(words, action, fold)) in
        (r.y + 1..).zip(rows.into_iter().skip(app.status_scroll).take(visible))
    {
        let x = r.x + 2 - u16::from(matches!(action, Some(Action::Child(_))));
        let row = Rect::new(x, y, r.right().saturating_sub(x + 1), 1);
        line(b, row, y, &words, style(WHITE, GLASS));
        let fold = fold
            .into_iter()
            .map(|(x, a)| (Rect::new(row.x + x, y, 2, 1), a));
        hits.extend(action.map(|a| (row, a)).into_iter().chain(fold));
    }
    scrollbar(b, r.right() - 1, r.y + 1, visible, total, app.status_scroll);
}

// What a delivery is, by its file's or its address's extension, else by what it carries.
fn kind(d: &Delivery) -> &'static str {
    let extension = d
        .file
        .as_deref()
        .and_then(std::path::Path::extension)
        .or_else(|| std::path::Path::new(d.url.as_deref()?.split(['?', '#']).next()?).extension())
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "html" | "htm" => "page",
        "md" | "markdown" => "markdown",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "avif" => "image",
        _ if d.url.is_some() => "link",
        _ if d.voice.is_some() => "voice note",
        _ if d.file.is_none() => "note",
        _ => "file",
    }
}
