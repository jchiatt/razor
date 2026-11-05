//! The children strip on the bezel, on the phone and over a split: the main session's children
//! counted in one line, and opened, one line each in the columns they share.
use super::cards::Card;
use super::view::{Action, Hit, PHONE};
use super::{App, text::*};
use crate::protocol::{AgentView, State};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Color,
    text::{Line, Span},
};

/// The main session's own children, oldest launch first.
fn fleet(app: &App) -> Vec<&AgentView> {
    (app.root.as_ref()).map_or(Vec::new(), |root| super::cards::children(app, root))
}

/// The main session's children as cards: those kept, then the ended ones.
fn cards(app: &App, now: u64) -> (Vec<Card<'_>>, Vec<Card<'_>>) {
    let all = fleet(app).into_iter().map(|a| Card::of(app, a, now));
    all.partition(|c| c.kept(now))
}

/// Rows the children strip wants: none without children of the main session, or off a phone
/// outside a split; one folded; open, one more per kept child, one for the ended ones' line and
/// one for each of them while the archive is open, six at most.
pub fn rows(app: &App, now: u64) -> u16 {
    if fleet(app).is_empty() || app.columns >= PHONE && !app.split_view() {
        return 0;
    }
    if !app.strip {
        return 1;
    }
    let (kept, ended) = cards(app, now);
    let archive = if app.archive { ended.len() } else { 0 };
    1 + (kept.len() + usize::from(!ended.is_empty()) + archive).min(6) as u16
}

/// Draws the strip on the bezel: `▸` and the fullest wording that fits; open, `▾` and a line for
/// each child. The head opens or folds the list; a child's line opens its split.
pub fn draw(app: &App, b: &mut Buffer, r: Rect, now: u64, hits: &mut Vec<Hit>) {
    if r.height == 0 {
        return;
    }
    fill(b, r, style(DATA, BEZEL));
    let mark = if app.strip { " ▾ " } else { " ▸ " };
    let x = put(b, r.x, r.y, mark, 3, style(CYAN, BEZEL));
    let glyphs = glyphs(app, now);
    let used: usize = glyphs.iter().map(|s| width(&s.content)).sum();
    let words = words(app, usize::from(r.width).saturating_sub(4 + used));
    let words = Line::from([glyphs, words].concat());
    line(
        b,
        Rect::new(x, r.y, r.width.saturating_sub(3), 1),
        r.y,
        &words,
        style(DATA, BEZEL),
    );
    hits.push((Rect::new(r.x, r.y, r.width, 1), Action::Strip));
    if !app.strip {
        return;
    }
    // The kept children, then the ended ones folded behind one line, `▸ 2 finished`, which opens
    // and folds them with the pane's archive.
    let (kept, ended) = cards(app, now);
    let open: Vec<&Card> = kept
        .iter()
        .chain(ended.iter().filter(|_| app.archive))
        .collect();
    let words = lines(&open, r.width.saturating_sub(3) as usize).into_iter();
    let mut rows: Vec<_> = (words.zip(&open))
        .map(|(words, c)| (words, Action::Child(c.agent.session.clone()), Some(*c)))
        .collect();
    if !ended.is_empty() {
        let words = format!(" {}", super::cards::archive_words(&ended, app.archive));
        let words = Line::from(Span::styled(words, style(GRAY, BEZEL)));
        rows.insert(kept.len(), (words, Action::Archive, None));
    }
    for (y, (words, action, card)) in (r.y + 1..r.bottom()).zip(rows) {
        let area = Rect::new(r.x, y, r.width, 1);
        line(b, area, y, &words, style(DATA, BEZEL));
        hits.push((area, action));
        // Its clock turns each second.
        if let Some(ran) = card.and_then(|c| c.ran.filter(|_| c.agent.state != State::Done)) {
            app.next.at(now + 1000 - ran % 1000);
        }
    }
}

// Folded, one glyph per child in launch order as its card draws it, eight at most, then `+N`.
fn glyphs(app: &App, now: u64) -> Vec<Span<'static>> {
    let all = if app.strip { Vec::new() } else { fleet(app) };
    let cards = all.iter().take(8).map(|a| Card::of(app, a, now));
    let mut glyphs: Vec<_> = cards
        .map(|c| Span::styled(c.glyph, style(c.tone, BEZEL)))
        .collect();
    let more = all.len().checked_sub(8).filter(|n| *n > 0);
    glyphs.extend(more.map(|n| Span::styled(format!("+{n}"), style(GRAY, BEZEL))));
    glyphs.extend((!all.is_empty()).then(|| Span::raw(" ")));
    glyphs
}

// Each child's line in the columns the lines share (Loom's fleet strip): its glyph, name, the
// fullest form of its model and effort that every line fits, its state, how long it has run and
// its context.
fn lines(cards: &[&Card], room: usize) -> Vec<Line<'static>> {
    // Format each model once; fitting its three forms only measures the retained strings.
    let forms: Vec<_> = cards
        .iter()
        .map(|c| {
            let (name, level) = (model(&c.agent.model), effort(c.agent.effort));
            let family = name.split(' ').next().unwrap_or_default().to_owned();
            [
                format!("{name} {level}"),
                format!("{family} {level}"),
                family,
            ]
        })
        .collect();
    let widest_model = |level: usize| forms.iter().map(|f| width(&f[level])).max().unwrap_or(0);
    let tokens =
        |c: &Card| (c.agent.context).map_or("no usage".into(), |t| format!("{} tok", kilo(t)));
    let widest = |of: &dyn Fn(&Card) -> String| cards.iter().map(|c| width(&of(c))).max();
    let widest = |of: &dyn Fn(&Card) -> String| widest(of).unwrap_or(0);
    let id = widest(&|c| c.agent.name.clone()).min(10);
    let (word, used) = (widest(&|c| c.word.into()), widest(&tokens));
    let took = widest(&|c| c.clock.clone());
    let room = room.saturating_sub(id + word + took + used + 4).max(4);
    let level = (0..2)
        .find(|&level| widest_model(level) <= room)
        .unwrap_or(2);
    let model = room.min(widest_model(level));
    let pad = |text: &str, w: usize, ink| {
        let text = clip(text, w);
        let fill = " ".repeat(w.saturating_sub(width(&text)));
        Span::styled(format!("{text}{fill}"), style(ink, BEZEL))
    };
    let right = |text: String, w: usize| {
        let fill = " ".repeat(w.saturating_sub(width(&text)));
        Span::styled(format!("{fill}{text}"), style(WHITE, BEZEL))
    };
    let gap = || Span::styled(" ", style(GRAY, BEZEL));
    cards
        .iter()
        .zip(&forms)
        .map(|(c, forms)| {
            Line::from(vec![
                Span::styled(format!(" {} ", c.glyph), style(c.tone, BEZEL)),
                pad(&c.agent.name, id, WHITE),
                gap(),
                pad(&forms[level], model, WHITE),
                gap(),
                pad(c.word, word, c.tone),
                gap(),
                right(c.clock.clone(), took),
                gap(),
                right(tokens(c), used),
            ])
        })
        .collect()
}

/// The strip's words for the main session's children within `room` columns, fullest first:
/// `main agent waiting on 2 of 3 agents · 1 done` while it waits on them, then shorter; else
/// `3 agents · 1 working · 1 done`, then `3 · …`; the counts in their states' colours.
fn words(app: &App, room: usize) -> Vec<Span<'static>> {
    let mut counts: [(usize, &str, Color); 5] = [
        (0, "working", GREEN),
        (0, "idle", CYAN),
        (0, "stuck", AMBER),
        (0, "done", WHITE),
        (0, "stopped", AMBER),
    ];
    let all = fleet(app);
    for agent in &all {
        let group = match (agent.state, &agent.report) {
            (State::Working, _) => 0,
            (State::NeedsYou, _) if !agent.asks.is_empty() => 0,
            (State::NeedsYou, _) => 1,
            (State::Stuck, _) => 2,
            (State::Done, Some(_)) => 3,
            (State::Done, None) => 4,
        };
        counts[group].0 += 1;
    }
    let (total, working) = (all.len(), counts[0].0);
    let busy = app.root.as_ref().and_then(|id| app.agents.get(id));
    let waiting = working > 0 && busy.is_none_or(|a| a.state != State::Working);
    let noun = if total == 1 { "agent" } else { "agents" };
    let leads = match waiting {
        true => vec![
            format!("main agent waiting on {working} of {total} {noun}"),
            format!("waiting on {working} of {total} {noun}"),
            format!("waiting on {working} of {total}"),
        ],
        false => vec![format!("{total} {noun}"), format!("{total}")],
    };
    let shown = counts.iter().filter(|(n, ..)| *n > 0);
    let shown: Vec<_> = shown
        .filter(|(_, word, _)| !waiting || *word != "working")
        .collect();
    let words = |lead: &str, sep: &'static str| {
        let mut row = vec![Span::styled(lead.to_owned(), style(WHITE, BEZEL))];
        for (n, word, tone) in &shown {
            row.push(Span::styled(sep, style(GRAY, BEZEL)));
            row.push(Span::styled(format!("{n} {word}"), style(*tone, BEZEL)));
        }
        row
    };
    for sep in [" · ", " "] {
        for lead in &leads {
            let row = words(lead, sep);
            if row.iter().map(|s| width(&s.content)).sum::<usize>() <= room {
                return row;
            }
        }
    }
    let last: String = words(&leads[leads.len() - 1], " ")
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    vec![Span::styled(clip(&last, room), style(WHITE, BEZEL))]
}
