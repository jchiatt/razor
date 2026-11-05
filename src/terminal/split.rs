//! The split while a child is addressed: the main session paused on the left from 80 columns,
//! and on the right the child's ancestry, identity, transcript, children and compact composer.
//! With motion on, a clicked card grows into the split, and shrinks back to it after.
use super::motion::{ease_out, progress};
use super::tools::rim::Rim;
use super::view::{self, Action, Hit, PHONE, Question};
use super::{App, cards, gauge, picker, text::*, tools};
use crate::protocol::{SessionId, State};
use crate::terminal::bindings;
use ratatui::{buffer::Buffer, layout::Rect};

/// The frame moving between a child's card and the split: whose, since when, and the card's
/// edges it opens from as the frame clicked on drew them, or none while it closes onto the card
/// where each frame draws it. Every edge between is derived per frame.
pub struct Move {
    child: SessionId,
    at: u64,
    card: Option<[i32; 4]>,
}
impl Move {
    // How open the frame is at `now`, 0 on the card and 1 on the split, eased out each way: an
    // opening over 420 ms, a closing over 280; none once it has landed.
    fn open(&self, now: u64) -> Option<f64> {
        let opening = self.card.is_some();
        let ms = [280, 420][usize::from(opening)];
        let eased = ease_out(progress(now, self.at, ms));
        (now < self.at + ms).then_some(if opening { eased } else { 1. - eased })
    }
}

/// Notes the addressed session changing from `old`: a child opened from the main view grows
/// from its card as the last frame drew it, and the split closing shrinks it back; a change
/// mid-move reverses from where the frame is. Motion off, or no card drawn, moves nothing.
pub fn start(app: &mut App, old: Option<SessionId>, now: u64) -> Option<()> {
    let (opening, was) = (old == app.root, app.moving.take());
    let child = if opening { app.focus.clone() } else { old };
    let child = child.filter(|_| super::motion::on(app) && app.split() == opening)?;
    // An opening needs its card on screen.
    let card = card(app, &app.hits, &child).filter(|_| opening);
    (card.is_some() == opening).then_some(())?;
    // On from the same openness, the other way.
    let open = was.filter(|m| m.child == child).and_then(|m| m.open(now));
    let open = open.unwrap_or(f64::from(u8::from(!opening)));
    let (part, ms) = match opening {
        true => (1. - (1. - open).cbrt(), 420.),
        false => (1. - open.cbrt(), 280.),
    };
    let at = now - (part * ms) as u64;
    app.moving = Some(Move { child, at, card });
    None
}

/// After the frame is drawn, while the split closes: over it, the child's transcript in a frame
/// shrinking from the right half to the child's card where this frame draws it. Some while it
/// moves, so the cursor hides; nothing under it takes a click.
pub fn closing(app: &mut App, b: &mut Buffer, now: u64, hits: &mut Vec<Hit>) -> Option<()> {
    let child = app.moving.as_ref().filter(|_| !app.split())?.child.clone();
    let inside = frame(app, b, b.area, now, hits)?;
    view::transcript(app, b, inside, &child, now, hits);
    hits.retain(|(r, _)| !r.intersects(inside));
    Some(())
}

/// Draws the split; while its frame opens, the right half is laid out inside the frame over the
/// paused main, and while the frame is under 12 rows tall its inside is the child's transcript.
/// Nothing under a moving frame takes a click, and the cursor hides. The divider comes in as the
/// frame's left edge.
pub fn draw(
    app: &mut App,
    b: &mut Buffer,
    area: Rect,
    question: Question,
    now: u64,
    hits: &mut Vec<Hit>,
) -> Option<(u16, u16)> {
    let focus = app.focus.clone()?;
    let wide = area.width >= 80;
    if wide || app.moving.is_some() {
        let mut left = area;
        left.width /= 1 + u16::from(wide);
        main(app, b, left, now, hits);
    }
    let moving = frame(app, b, area, now, &[]);
    let right = moving.unwrap_or(half(area));
    if let Some(inside) = moving.filter(|r| r.height < 12) {
        view::transcript(app, b, inside, &focus, now, hits);
        hits.retain(|(r, _)| !r.intersects(inside));
        return None;
    }
    let phone = right.width < PHONE;
    let card = match phone {
        true => app.composer.phone_rows(right.width, right.height),
        false => app.composer.rows(right.width, right.height),
    };
    let stack = view::band(app, &question, right.width, right.height, now);
    let stack = stack.min(right.height.saturating_sub(card));
    let rest = right.height.saturating_sub(card + stack);
    let header = match right.height {
        ..10 => 1,
        10..16 => 3,
        _ => 5,
    }
    .min(rest);
    let children = app
        .agents
        .values()
        .filter(|a| a.parent.as_ref() == Some(&focus));
    let band = match children.count() as u16 {
        0 => 0,
        n => (1 + n.min(1024) * 2).min(right.height / 3),
    };
    let band = band.min(rest.saturating_sub(header + 3));
    let at = |y, height| Rect::new(right.x, y, right.width, height);
    let transcript = at(right.y + header, rest.saturating_sub(header + band));
    identity(app, b, at(right.y, header), &focus, now, hits);
    view::transcript(app, b, transcript, &focus, now, hits);
    family(app, b, at(transcript.bottom(), band), &focus, now, hits);
    let stack = at(right.y + rest, stack);
    view::stack(app, b, stack, question, now, hits);
    let editor = at(stack.bottom(), card);
    picker::draw(
        app,
        b,
        at(transcript.y, editor.y.saturating_sub(transcript.y)),
    );
    let cursor = match phone {
        true => app.composer.phone(app, b, editor, hits),
        false => app.composer.draw(app, b, editor, now, hits),
    };
    if let Some(inside) = moving {
        hits.retain(|(r, _)| !r.intersects(inside));
        return None;
    }
    if wide {
        divider(app, b, area, right.x - 1, &focus, now);
    }
    cursor
}

// The split's right half: right of the main's half and the divider from 80 columns, else all.
fn half(area: Rect) -> Rect {
    let x = area.x + [0, area.width / 2 + 1][usize::from(area.width >= 80)];
    Rect::new(x, area.y, area.right() - x, area.height)
}

// The moving frame at `now`, between its card (an opening's own, else where `hits` put it) and
// the split's right half, whose edges lie one cell off the screen and on the divider: its edge
// in the child's light, still while it moves, around a cleared inside, which it returns. None,
// and the move is over, once it lands or its card is gone.
fn frame(app: &mut App, b: &mut Buffer, area: Rect, now: u64, hits: &[Hit]) -> Option<Rect> {
    let m = app.moving.as_ref()?;
    let child = m.child.clone();
    let Some((open, card)) = m.open(now).zip(m.card.or_else(|| card(app, hits, &child))) else {
        app.moving = None;
        return None;
    };
    let h = half(area);
    let [x, y, right, foot] = [h.x, h.y, h.right(), h.bottom()].map(i32::from);
    let to = [x - 1, y - 1, right, foot];
    let [l, t, r, bottom]: [i32; 4] =
        std::array::from_fn(|i| card[i] + (f64::from(to[i] - card[i]) * open).round() as i32);
    let ink = style(light(app, &child, now, false).colour(0., 1., 0.), GLASS);
    let mut edge = |x: i32, y: i32, g: &str| {
        if !within(area, [x, y, x + 1, y + 1]).is_empty() {
            put(b, x as u16, y as u16, g, 1, ink);
        }
    };
    for x in l..=r {
        let i = usize::from(x == r) + 2 * usize::from(x != l && x != r);
        edge(x, t, ["╭", "╮", "─"][i]);
        edge(x, bottom, ["╰", "╯", "─"][i]);
    }
    for y in t + 1..bottom {
        edge(l, y, "│");
        edge(r, y, "│");
    }
    let name = app.agents.get(&child).map_or("", |a| a.name.as_str());
    let title = within(area, [l + 2, t, r - 1, t + 1]);
    if title.height > 0 {
        let bold = bold(rgb(0xd6a0ff), GLASS);
        let words = format!(" {name} ");
        put(b, title.x, title.y, &words, title.width, bold);
    }
    app.next.at(now);
    let inside = within(area, [l + 1, t + 1, r, bottom]);
    fill(b, inside, style(DATA, GLASS));
    Some(inside)
}

// The divider in the child's light: lapping it while the child works, breathing while it
// thinks, still while it waits; on its end, by the record, a green flash once on a good one, or
// frayed amber on a failure, then at rest.
fn divider(app: &mut App, b: &mut Buffer, area: Rect, x: u16, id: &SessionId, now: u64) {
    let rim = light(app, id, now, true);
    let len = f64::from(area.height);
    for y in area.y..area.bottom() {
        let cell = rim.cell("│", f64::from(y - area.y), len, len / 2.);
        let ink = mix(cell.style.fg.unwrap_or(VIOLET), GREEN, rim.flash);
        put(b, x, y, &cell.content, 1, style(ink, GLASS));
    }
}

// The child's light as its card draws it, lapping only with `lap`, and its end's flash or fray;
// it asks for frames while it moves.
fn light(app: &mut App, id: &SessionId, now: u64, lap: bool) -> Rim<'static> {
    let Some(a) = app.agents.get(id) else {
        return Rim::default();
    };
    let (ended, motion) = (tools::child::ended(app, a), super::motion::on(app));
    let live = motion && ended.is_none() && a.state == State::Working;
    let mut rim = Rim::default();
    (rim.motion, rim.open) = (live && lap, 1.);
    rim.pulse = app.lights.pulse(a, app.transcripts.get(id), now, live);
    (rim.fray, rim.flash) = tools::rim::ending(ended, now, motion);
    if live || ended.is_some_and(|(at, _)| motion && now < at + 1400) {
        app.next.at(now + 83);
    }
    rim
}

// The part of `[left, top, right, bottom)` on `area`.
fn within(area: Rect, [left, top, right, bottom]: [i32; 4]) -> Rect {
    let x = |v: i32| v.clamp(i32::from(area.x), i32::from(area.right())) as u16;
    let y = |v: i32| v.clamp(i32::from(area.y), i32::from(area.bottom())) as u16;
    Rect::new(x(left), y(top), x(right) - x(left), y(bottom) - y(top))
}

// Where the last frame drew `child`'s card, as outer edges: down the tallest run of its hits, and
// across its cached top edge from its corner to its `╮`.
fn card(app: &App, hits: &[Hit], child: &SessionId) -> Option<[i32; 4]> {
    let agent = app.agents.get(child)?;
    let t = app.transcripts.get(agent.parent.as_ref()?)?;
    let rows = &t.children.get(&agent.launched?)?.0;
    let top = rows.get(1)?.words.to_string();
    let mine = |a: &Action| matches!(a, Action::Child(id) if id == child);
    let mut runs: Vec<Rect> = Vec::new();
    for (r, _) in hits.iter().filter(|(_, a)| mine(a)) {
        match runs.last_mut() {
            Some(run) if run.x == r.x && run.bottom() == r.y => *run = run.union(*r),
            _ => runs.push(*r),
        }
    }
    let r = runs.into_iter().max_by_key(|r| r.height)?;
    let [x, y, bottom] = [r.x, r.y, r.bottom()].map(i32::from);
    let corner = width(&top[..top.find('├')?]) as i32;
    (r.height >= 3).then_some([x + corner, y, x + width(&top) as i32 - 1, bottom - 1])
}

// The main session, paused: its bar, its tape from 80 columns (else a one-row gauge), its
// transcript and the way back.
fn main(app: &mut App, b: &mut Buffer, r: Rect, now: u64, hits: &mut Vec<Hit>) {
    let Some(id) = app.root.clone() else { return };
    let agent = app.agents.get(&id);
    super::topbar::draw(
        app,
        b,
        Rect::new(r.x, r.y, r.width, r.height.min(4)),
        agent,
        false,
        now,
    );
    if r.height < 7 {
        return;
    }
    let tape = if r.width >= 80 {
        gauge::columns(app)
    } else {
        0
    };
    let body = Rect::new(r.x + tape, r.y + 4, r.width - tape, r.height - 4);
    match tape {
        0 => gauge::horizontal(app, b, Rect { height: 1, ..body }, Some(&id), now),
        _ => gauge::draw(
            app,
            b,
            Rect::new(r.x, body.y, tape, body.height),
            Some(&id),
            now,
        ),
    }
    let top = u16::from(tape == 0);
    let transcript = Rect::new(body.x, body.y + top, body.width, body.height - top - 1);
    view::transcript(app, b, transcript, &id, now, hits);
    let footer = Rect::new(body.x, r.bottom() - 1, body.width, 1);
    fill(b, footer, style(GRAY_DIM, RECESS));
    // Questions waiting on the main side are counted, in cyan.
    let root = app.root.as_ref().and_then(|root| app.agents.get(root));
    let asks = root.map_or(0, |a| a.asks.len());
    let more = if asks > 0 {
        format!("{asks} for you · ")
    } else {
        String::new()
    };
    let (room, ink) = (
        footer.width.saturating_sub(2),
        [GRAY, CYAN][usize::from(asks > 0)],
    );
    let words = bindings::text(
        app,
        &format!("main · paused · {more}<composer.back> to go back"),
    );
    put(b, footer.x + 2, footer.y, &words, room, style(ink, RECESS));
    hits.push((footer, Action::Back));
}

// The ancestry from the main session, each ancestor a click target, and the way back.
fn ancestry(app: &App, b: &mut Buffer, r: Rect, id: &SessionId, hits: &mut Vec<Hit>) {
    // Ancestry comes from the capped status snapshot, never a navigation stack.
    let mut chain = vec![id];
    let parent = |id: &SessionId| app.agents.get(id)?.parent.as_ref();
    while let Some(parent) = chain.last().and_then(|id| parent(id)) {
        if chain.contains(&parent) || chain.len() >= app.agents.len() {
            break;
        }
        chain.push(parent);
        if app.root.as_ref() == Some(parent) {
            break;
        }
    }
    let (mut x, right) = (r.x, r.right());
    for (i, session) in chain.into_iter().rev().enumerate() {
        let name = app.agents.get(session).map(|a| a.name.as_str());
        let name = name.filter(|n| !n.is_empty()).unwrap_or(session.as_str());
        let words = match (session == id, i) {
            (true, _) => bold(CYAN, GLASS),
            (false, 0) => style(GRAY, GLASS),
            (false, _) => style(DATA, GLASS),
        };
        let (lead, ink) = if i == 0 {
            ("‹ ", GRAY)
        } else {
            (" › ", GRAY_DIM)
        };
        let start = put(b, x, r.y, lead, right.saturating_sub(x), style(ink, GLASS));
        x = put(b, start, r.y, name, right.saturating_sub(start), words);
        if session != id && x > start {
            hits.push((
                Rect::new(start, r.y, x - start, 1),
                Action::Child(session.clone()),
            ));
        }
    }
}

// The ancestry, then the child's identity, model, context and changed files.
fn identity(app: &App, b: &mut Buffer, r: Rect, id: &SessionId, now: u64, hits: &mut Vec<Hit>) {
    if r.height == 0 || r.width < 8 {
        return;
    }
    ancestry(app, b, r, id, hits);
    let Some(agent) = app.agents.get(id) else {
        return;
    };
    let head = cards::Card::of(app, agent, now);
    if r.height > 2 {
        let area = Rect::new(r.x, r.y + 1, r.width, 2);
        let detail = cards::detail(&[&head], r.width as usize, cards::Size::Compact);
        cards::draw(app, b, area, &head, cards::Size::Compact, detail);
    }
    if r.height > 3
        && let Some((_, branch, _)) = app.folder.as_ref().filter(|f| &f.0 == id)
    {
        let line = format!("{} · {branch}", agent.project);
        put(b, r.x, r.y + 3, &line, r.width, style(GRAY, GLASS));
    }
    // The context in its zone's colour, the cache reused and the files changed, each only when
    // it has something to say.
    if r.height > 4 {
        let files = &agent.changed_files;
        let named = files.first().and_then(|f| f.file_name()).map(|name| {
            let (s, more) = [("", ""), ("s", " …")][usize::from(files.len() > 1)];
            format!("{} file{s} ({}{more})", files.len(), name.to_string_lossy())
        });
        let cache = agent.cache.filter(|c| *c > 0.);
        let facts = [
            Some(vec![span("ctx ", GRAY), cards::number(agent, false)]),
            cache.map(|c| vec![span(format!("cache {:.0}%", c * 100.), GRAY)]),
            named.map(|files| vec![span(files, GRAY)]),
        ];
        let (mut x, y, room) = (r.x, r.y + 4, |x: u16| r.right().saturating_sub(x));
        for (i, words) in facts.into_iter().flatten().enumerate() {
            let lead = if i > 0 { " · " } else { "" };
            x = put(b, x, y, lead, room(x), style(GRAY_DIM, GLASS));
            for s in words {
                x = put(b, x, y, &s.content, room(x), s.style);
            }
        }
    }
}

// The addressed child's kept children, two rows each or one when crowded, under a count of all
// of them and of those working and waiting for you; `+N more` for the rest.
fn family(app: &App, b: &mut Buffer, r: Rect, id: &SessionId, now: u64, hits: &mut Vec<Hit>) {
    if r.height < 3 {
        return;
    }
    let mine = |a: &&crate::protocol::AgentView| a.parent.as_ref() == Some(id);
    let all = (app.agents.values().filter(mine)).map(|a| cards::Card::of(app, a, now));
    let (kept, ended): (Vec<_>, Vec<_>) = all.partition(|c| c.kept(now));
    let total = kept.len() + ended.len();
    let working = kept.iter().filter(|c| c.agent.state == State::Working);
    let waiting = kept.iter().filter(|c| !c.agent.asks.is_empty());
    let mut head = format!("── {total} child{}", if total == 1 { "" } else { "ren" });
    for (n, words) in [(working.count(), "working"), (waiting.count(), "for you")] {
        if n > 0 {
            head.push_str(&format!(" · {n} {words}"));
        }
    }
    let (gray, dim) = (style(GRAY, GLASS), style(GRAY_DIM, GLASS));
    let x = put(b, r.x, r.y, &format!("{head} "), r.width, gray);
    let room = r.right().saturating_sub(x);
    put(b, x, r.y, &"─".repeat(room.into()), room, dim);
    let size = cards::Size::fit(kept.len(), r.height as usize - 1);
    let tall = size.rows() as u16;
    let detail = cards::detail(&kept.iter().collect::<Vec<_>>(), r.width as usize, size);
    // Every kept card when they and nothing else fit; else as many as leave a row for the count.
    let room = r.height - 1;
    let shown = match kept.len() as u16 * tall <= room && ended.is_empty() {
        true => kept.len(),
        false => kept.len().min(usize::from((room - 1) / tall)),
    };
    for (y, card) in (0..).map(|i| r.y + 1 + i * tall).zip(&kept[..shown]) {
        let area = Rect::new(r.x, y, r.width, tall);
        cards::draw(app, b, area, card, size, detail);
        hits.push((area, Action::Child(card.agent.session.clone())));
    }
    let (hidden, y) = (total - shown, r.y + 1 + shown as u16 * tall);
    if hidden > 0 {
        let words = format!("+{hidden} more");
        put(b, r.x + 2, y, &words, r.width.saturating_sub(2), dim);
    }
}
