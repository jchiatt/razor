//! The composer's full card: its border label with who speaks, the agent's mood and the next
//! input's choice, the words, and the dock of chips and the send button.
use super::super::view::{self, Action, Hit};
use super::super::{App, audio, bindings, status, text::*};
use super::{Composer, chosen, emoji_chip, speaks_as};
use crate::protocol::{Body, State, UsageWindow};
use ratatui::{buffer::Buffer, layout::Rect, style::Color};

impl Composer {
    /// The full card's height for its width: three to ten rows of words and its frame.
    pub fn rows(&self, width: u16, height: u16) -> u16 {
        let cap = (height / 4).clamp(3, 10) as usize;
        self.height(width.saturating_sub(8) as usize, cap).max(3) as u16 + 3
    }

    /// The full card: its border label, the words or a hint, and the dock of chips and buttons.
    pub fn draw(
        &self,
        app: &App,
        b: &mut Buffer,
        r: Rect,
        now: u64,
        hits: &mut Vec<Hit>,
    ) -> Option<(u16, u16)> {
        fill(b, r, style(DATA, RECESS));
        if r.width < 5 || r.height < 4 {
            return None;
        }
        let (x, right, bottom) = (r.x + 1, r.right() - 2, r.bottom() - 1);
        let inner = right - x - 1;
        let agent = app.row();
        let working = agent.is_some_and(|a| a.state == State::Working);
        put(
            b,
            x,
            bottom,
            &format!("╰{}╯", "─".repeat(inner as usize)),
            r.width - 2,
            style(CYAN, RECESS),
        );
        for y in r.y + 1..bottom {
            fill(b, Rect::new(x + 1, y, inner, 1), style(WHITE, BEZEL));
            put(b, x, y, "│", 1, style(CYAN, BEZEL));
            put(b, right, y, "│", 1, style(CYAN, BEZEL));
        }
        let dock = Rect::new(x, bottom - 1, right - x + 1, 1);
        self.dock(app, b, dock, working, hits);
        let words = Rect::new(
            x + 4,
            r.y + 1,
            inner.saturating_sub(4),
            bottom - 1 - r.y - 1,
        );
        let cursor = self.content(b, words, app);
        // After the words, so the edge counts the rows this frame scrolled off.
        let above = self.window.borrow().as_ref().map_or(0, |w| w.first);
        // A click on the status summary opens the pane, as Ctrl-T does.
        if let Some(tail) = header(app, b, r, now, above) {
            hits.push((tail, Action::Status));
        }
        put(
            b,
            x + 2,
            r.y + 1,
            if above > 0 { "↑ " } else { "❯ " },
            2,
            style(CYAN, BEZEL),
        );
        cursor
    }

    // The chips: emoji, image, the next input's choice (a click opens the picker), the status
    // pane's doorway while the pane is not docked, and the send button. The row keeps the longest
    // form that fits beside Send and the status chip: Emoji and Image with their words or as
    // icons, then the choice with its effort, without it, or not at all.
    fn dock(&self, app: &App, b: &mut Buffer, r: Rect, working: bool, hits: &mut Vec<Hit>) {
        let (x, y, inner) = (r.x, r.y, r.width - 2);
        let (model, effort) = chosen(app);
        // A split's dock keeps Emoji, Image and Send: no model or status chip.
        let choice = match app.split_view() {
            true => Default::default(),
            false => [
                String::new(),
                format!(" ◇ {model} "),
                format!(" ◇ {model} · {effort} "),
            ],
        };
        fill(b, Rect::new(x + 1, y, inner, 1), style(DATA, RECESS));
        let send = if working { " Queue ⏎ " } else { " Send ⏎ " };
        let at = (r.right() - 1).saturating_sub(width(send) as u16 + 1);
        let status = !view::docked(app) && !app.split_view();
        let chip = status.then(|| width(&status::chip(app, 40).1) + 5);
        // The Image chip counts the images waiting to go, as ` ⎘ Image · 2 `.
        let waiting = app.composer.active_images().count();
        let count = match waiting {
            0 => String::new(),
            n => format!("· {n} "),
        };
        let need = |words: bool, n: usize| {
            [8, 23 + width(&count)][usize::from(words)] + width(&choice[n]) + 1
        };
        let room = usize::from(at.saturating_sub(x + 2)).saturating_sub(chip.unwrap_or_default());
        let mut forms = [(true, 2), (true, 1), (false, 2), (false, 1)].into_iter();
        let (words, n) = forms
            .find(|&(w, n)| need(w, n) <= room)
            .unwrap_or((false, 0));
        let image = if words {
            format!(" ⎘ Image {count}")
        } else {
            " ⎘ ".into()
        };
        let emoji = ["  ☺  Emoji  ", " ☺ "][usize::from(!words)];
        let mut end = x + 1;
        let voice = ["", " Voice "][usize::from(audio::available(app))];
        let chips = [
            (voice, style(CYAN, BEZEL_HI), Action::Record),
            (emoji, emoji_chip(app), Action::Emoji),
            (image.as_str(), style(DATA, BEZEL_HI), Action::Image),
            (&choice[n], style(CYAN, BEZEL_HI), Action::Model),
        ];
        for (words, look, action) in chips.into_iter().filter(|c| !c.0.is_empty()) {
            let start = end + 1;
            end = put(b, start, y, words, at.saturating_sub(start), look);
            hits.push((Rect::new(start, y, end.saturating_sub(start), 1), action));
        }
        let look = if self.text.trim().is_empty() {
            style(GRAY, BEZEL_HI)
        } else {
            style(WHITE, BUTTON)
        };
        put(b, at, y, send, width(send) as u16, look);
        hits.push((Rect::new(at, y, width(send) as u16, 1), Action::Send));
        if status {
            // The chip grows left toward the model chip, never over it.
            let room = at.saturating_sub(end + 1).min(42).saturating_sub(2);
            let (mark, words, tone) = status::chip(app, room.into());
            let w = width(&words) as u16 + 4;
            let chip = Rect::new(at.saturating_sub(w + 1), y, w, 1);
            let ink = style([WHITE, GRAY][usize::from(tone == GRAY)], RECESS);
            put(b, chip.x, y, &format!(" {mark} {words} "), w, ink);
            put(b, chip.x + 1, y, mark, 1, style(tone, RECESS));
            hits.push((chip, status::press(app)));
        }
    }
}

// With the draft empty and the pane closed, the edge says what the pane would: the usage
// windows measured, the children out, and the key that opens it.
fn summary(app: &App, now: u64) -> String {
    let windows = app.row().map_or(&[][..], |a| a.usage.as_slice());
    let live = |w: &&UsageWindow| w.resets_at.is_none_or(|at| at > now);
    let mut parts: Vec<_> = (["5-hour", "7-day"].into_iter())
        .filter_map(|n| {
            let w = windows.iter().filter(live).find(|w| w.name == n)?;
            Some(format!("{n} {:.0}%", w.utilization?))
        })
        .collect();
    let out = (app.agents.values())
        .filter(|a| a.parent.is_some() && a.parent == app.focus && a.settled.is_none())
        .count();
    if out > 0 {
        parts.push(format!(
            "{out} child{} out",
            if out == 1 { "" } else { "ren" }
        ));
    }
    parts.push(bindings::hint(app, "global.status"));
    parts.join(" · ")
}

// A split's card speaks to its child: `╭─ YOU · talking to heron · as you ── Esc back ─╮`.
fn speaker(app: &App, b: &mut Buffer, r: Rect) {
    let agent = app.row();
    let name = agent.map_or("you", |a| a.name.as_str());
    let (x, right) = (r.x + 1, r.right() - 2);
    let inner = usize::from(right - x - 1);
    let words = format!(" · talking to {name} · {} ", speaks_as(agent).0);
    let words = clip(&words, inner.saturating_sub(16));
    let rule = "─".repeat(inner.saturating_sub(width(&words) + 16));
    let back = bindings::text(app, "<composer.back> back");
    let border = format!("╭─ YOU{words}{rule} {back} ─╮");
    put(b, x, r.y, &border, r.width - 2, style(CYAN, RECESS));
    let at = put(b, x + 2, r.y, " YOU", 4, bold(CYAN, RECESS));
    put(
        b,
        at,
        r.y,
        &words,
        width(&words) as u16,
        bold(AMBER, RECESS),
    );
    let w = width(&back) as u16;
    put(
        b,
        right.saturating_sub(w + 2),
        r.y,
        &back,
        w,
        style(GRAY_DIM, RECESS),
    );
}

// The border's right end, its forms longest first and the last empty: the rows scrolled off and
// the counts, else the status summary while the draft is empty and the pane closed; `flag`, a
// caution the status chip names, leads each.
fn ends(
    app: &App,
    inner: usize,
    above: usize,
    pane: bool,
    flag: Option<String>,
    now: u64,
) -> Vec<String> {
    let text = &app.composer.text;
    let lines = app.composer.wrapped(inner.saturating_sub(4));
    let counts = format!(
        "{lines} line{} · {} ch",
        plural(lines),
        text.chars().count()
    );
    let key = || bindings::hint(app, "global.status");
    let mut ends = match (above, text.is_empty(), pane, flag) {
        (1.., ..) => vec![
            format!("↑ {above} more above · {counts}"),
            format!("↑ {above} more above"),
        ],
        (_, false, _, Some(flag)) => vec![format!("{counts} · {flag}"), flag, counts],
        (_, false, ..) => vec![counts],
        (_, true, false, Some(flag)) => {
            let rest = summary(app, now);
            vec![
                format!("{flag} · {rest}"),
                format!("{flag} · {}", key()),
                flag,
            ]
        }
        (_, true, false, None) => vec![summary(app, now), key()],
        _ => Vec::new(),
    };
    ends.push(String::new());
    ends
}

// The border's label: who speaks, to whom, the agent's mood, and the next input's choice; at its
// right end one of `ends`. The end keeps its longest form that fits, the mood its short form when
// that keeps a longer end, and the choice the longest of model, effort and name, model and
// effort, or model that leaves three rule columns.
fn header(app: &App, b: &mut Buffer, r: Rect, now: u64, above: usize) -> Option<Rect> {
    if app.split_view() {
        speaker(app, b, r);
        return None;
    }
    let agent = app.row();
    let (model, effort) = chosen(app);
    let name = agent.map_or("you", |a| a.name.as_str());
    let (x, right) = (r.x + 1, r.right() - 2);
    let inner = usize::from(right - x - 1);
    let pane = app.pane_open.unwrap_or(app.columns >= 140);
    // A caution the status chip names leads the summary in its colour, even while typing.
    let caution = match status::chip(app, 40) {
        ("⚠", words, tone) if !pane => Some((format!("⚠ {words}"), tone)),
        _ => None,
    };
    let flag = caution.as_ref().map(|c| c.0.clone());
    let ends = ends(app, inner, above, pane, flag, now);
    let used = |mood: &str| width(" YOU") + width(mood) + 4;
    let end = |mood: &str| {
        let room = inner.saturating_sub(used(mood) + 3);
        (ends.iter()).position(|t| t.is_empty() || width(t) + 4 <= room)
    };
    let (moods, tone) = mood(app, now);
    let roomy = inner >= 62 && end(&moods[0]) <= end(&moods[1]);
    let mood = &moods[usize::from(!roomy)];
    let tail = match &ends[end(mood).unwrap_or_default()] {
        t if t.is_empty() => String::new(),
        t => format!(" {t} "),
    };
    let spare = inner.saturating_sub(used(mood) + width(&tail) + 2 + 3);
    let choice = [model.as_str(), effort.as_str(), name];
    let fit = |n: usize| choice[..n].iter().map(|w| width(w) + 3).sum::<usize>() <= spare;
    let shown = (0..=3).rev().find(|&n| fit(n)).unwrap_or_default();
    let chosen: String = choice[..shown].iter().map(|w| format!(" · {w}")).collect();
    let label = format!(" YOU · {mood}{chosen} ");
    let (edge, join) = match tail.is_empty() {
        true => (1, ""),
        false => (width(&tail) + 2, "─"),
    };
    let rule = "─".repeat(inner.saturating_sub(width(&label) + edge));
    let label = clip(&label, inner.saturating_sub(1));
    let border = format!("╭─{label}{rule}{tail}{join}╮");
    put(b, x, r.y, &border, r.width - 2, style(CYAN, RECESS));
    let at = right.saturating_sub(width(&tail) as u16 + 1);
    put(b, at, r.y, &tail, width(&tail) as u16, style(GRAY, RECESS));
    let flagged = caution.filter(|_| above == 0);
    if let Some((flag, tone)) = &flagged
        && let Some(i) = tail.find(flag.as_str())
    {
        let x = at + width(&tail[..i]) as u16;
        put(b, x, r.y, flag, width(flag) as u16, style(*tone, RECESS));
    }
    let typed = !app.composer.text.is_empty();
    let summary = !tail.is_empty() && above == 0 && (!typed || flagged.is_some()) && !pane;
    let summary = summary.then(|| Rect::new(at, r.y, width(&tail) as u16, 1));
    let mut at = put(b, x + 2, r.y, " YOU", inner as u16, bold(CYAN, RECESS));
    let looks = [
        bold(CYAN, RECESS),
        style(CYAN, RECESS),
        style(VIOLET, RECESS),
    ];
    let words = [(mood.as_str(), style(tone, RECESS))];
    let words = words
        .into_iter()
        .chain(choice.into_iter().zip(looks).take(shown));
    let dot = style(GRAY, RECESS);
    for (words, look) in words {
        at = put(b, at, r.y, " · ", right.saturating_sub(at), dot);
        at = put(b, at, r.y, words, right.saturating_sub(at), look);
    }
    summary
}

// The agent's mood, long and short: attaching until the stream first syncs, a waiting question,
// working with the turn's clock and its queued inputs, a main session closed by `/done` or `/quit`,
// or idle. A turn runs from the first input after the last turn end, or since the row turned busy.
fn mood(app: &App, now: u64) -> ([String; 2], Color) {
    let working = app.row().is_some_and(|a| a.state == State::Working);
    if app.focus.is_some() && !app.transcript().is_some_and(|t| t.live) {
        return (["◦ attaching…"; 2].map(String::from), GRAY);
    } else if app.question().is_some() {
        let more = match app.asks().len() {
            1 => String::new(),
            n => format!(" ({n})"),
        };
        let keys = app
            .visible_options
            .iter()
            .filter(|n| **n < 9)
            .map(|n| bindings::hint(app, &format!("question.{}", n + 1)))
            .collect::<Vec<_>>()
            .join("/");
        let keys = if keys.is_empty() { "/answer" } else { &keys };
        let long = format!("? a question waits{more} · {keys} answers it");
        return ([long, format!("? question open{more}")], CYAN);
    } else if working && app.row().is_some_and(|a| a.closing) {
        return (["● closing after this turn"; 2].map(String::from), CYAN);
    } else if !working {
        let agent = app.row();
        let words = match agent.is_some_and(|a| a.parent.is_none() && a.state == State::Done) {
            true => ["◦ closed · a message reopens it", "◦ closed"],
            false => ["◦ agent idle", "◦ idle"],
        };
        return (words.map(String::from), GRAY);
    }
    let t = app.transcript();
    let entries = t.into_iter().flat_map(|t| t.entries.values().rev());
    let turn = entries.take_while(|e| !matches!(e.body, Body::TurnEnd { .. }));
    let inputs = turn.filter(|e| matches!(e.body, Body::Input(_)));
    let busy = inputs.last().map(|e| e.at).or(app.busy_at);
    let clock = timer(now.saturating_sub(busy.unwrap_or(now)));
    let queued = match t.map_or(0, |t| t.queued.difference(&t.taking).count()) {
        0 => String::new(),
        n => format!(" · {n} queued"),
    };
    let words = ["● agent working", "● working"].map(|w| format!("{w} {clock}{queued}"));
    (words, GREEN)
}
