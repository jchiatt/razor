//! The recap: back at a session ten minutes or more after anyone last looked at it or the person
//! last wrote in it, with entries after the last one seen, one request for what happened; shown
//! above the composer until a key.
use super::{App, motion, text::*, view};
use crate::protocol::now;
use ratatui::{buffer::Buffer, layout::Rect, text::Span};
use reqwest::Method;

/// Trey, 2026-11-04: "if I haven't interacted w/ a thread in 10 minutes or longer, not 5".
const AWAY: u64 = 10 * 60_000;

/// The person leaves the addressed session, switching away or losing focus: its last entry shown
/// is kept on its transcript and reported seen, the server keeping it on the session's status row
/// for every device; the recap, and a return not yet asked about, go.
pub fn leave(app: &mut App) {
    (app.recap, app.away) = (None, None);
    let (id, focused) = (app.focus.clone(), app.presence.focused);
    let Some((id, t)) = id.zip(app.transcript_mut()).filter(|_| focused) else {
        return;
    };
    let (entry, _) = *t.seen.insert((t.cursor, now()));
    app.spawn(|api| async move {
        let path = format!("v1/sessions/{id}/seen?entry={}", entry.0);
        let _ = api.json(Method::PUT, &path, None).await;
        |_: &mut App| {}
    });
}

/// The person is back at the addressed session: if no one has looked at it, nor has he written
/// in it, for ten minutes, `check` asks for its recap from the last entry seen, or the last this
/// terminal showed as he left it when the server lost that at a restart.
pub fn back(app: &mut App) {
    let t = app.transcript();
    let mine = t.and_then(|t| t.seen);
    app.away = app.row().and_then(|row| {
        let since = (row.looked_at.max(row.person_input_at)).max(mine.map(|m| m.1))?;
        let after = row.seen.or(mine.map(|m| m.0)).or(t.map(|t| t.cursor))?;
        (now() >= since + AWAY).then(|| (row.session.clone(), after, since))
    });
}

/// Asks once per return, when the session's transcript holds entries after the one `back`
/// chose; its newest page landing, or focus coming back, runs it. The words land only while that
/// return lasts: no leave or key has put its recap away since.
pub fn check(app: &mut App) {
    let Some((id, after, since)) = app.away.take_if(|a| app.focus.as_ref() == Some(&a.0)) else {
        return;
    };
    let moved = app.transcripts.get(&id).is_some_and(|t| t.cursor > after);
    if !moved || app.pending >= 8 {
        return;
    }
    app.recap = Some((id.clone(), since, String::new(), 0));
    app.spawn(|api| async move {
        let path = format!("v1/sessions/{id}/recap?after={}", after.0);
        let read = api.json(Method::POST, &path, None).await;
        let text = read.ok().and_then(|v| Some(v["text"].as_str()?.to_owned()));
        let text = text.map(|t| t.chars().filter(|c| !c.is_control()).take(400).collect());
        move |app: &mut App| {
            if matches!(&app.recap, Some((s, at, ..)) if (s, *at) == (&id, since)) {
                app.recap = text.map(|text| (id, since, text, now()));
            }
        }
    });
}

/// The recap's rows at `width` while the person is here and its session is addressed: ` ↺ away
/// 14 min · ` and the words, wrapped.
pub fn lines(app: &App, width: u16) -> Vec<ratatui::text::Line<'static>> {
    let shown = app
        .recap
        .as_ref()
        .filter(|r| app.presence.focused && app.focus.as_ref() == Some(&r.0) && !r.2.is_empty());
    let Some((_, since, text, landed)) = shown else {
        return Vec::new();
    };
    let spans = vec![
        Span::styled(" ↺ ", style(GRAY, RECESS)),
        Span::styled(
            format!("away {} min", (landed - since) / 60_000),
            style(CYAN, RECESS),
        ),
        Span::styled(format!(" · {text}"), style(WHITE, RECESS)),
    ];
    wrap(spans, width.saturating_sub(4) as usize)
}

/// Draws the recap's rows on the recess, the words after the first row under the glyph's
/// words, fading in from the glass over 300 ms from when it landed.
pub fn draw(app: &App, b: &mut Buffer, r: Rect, now: u64) {
    for (y, row) in (r.y..r.bottom()).zip(lines(app, r.width)) {
        let x = r.x + if y == r.y { 0 } else { 3 };
        fill(b, Rect::new(r.x, y, r.width, 1), style(DATA, RECESS));
        line(b, Rect { x, ..r }, y, &row, style(DATA, RECESS));
    }
    let start = app.recap.as_ref().map_or(0, |r| r.3);
    let k = 1. - motion::ease_out(motion::progress(now, start, 300));
    if motion::on(app) && k > 0. && r.height > 0 {
        view::tint(b, r, GLASS, GLASS, k);
        app.next.at(now);
    }
}
