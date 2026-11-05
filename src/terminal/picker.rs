//! Model, effort and speed staged together for the next input.
use super::{App, bindings, error, fail, text::*};
use crate::protocol::{ClientInput, Effort, Models, Speed};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::text::{Line, Span};
use ratatui::{buffer::Buffer, layout::Rect};
#[derive(Default)]
pub struct Picker {
    index: usize,
    chosen: Option<Effort>,
    speed: Speed,
    speed_row: bool,
    ready: bool,
    failed: bool,
    /// Choosing between keeping the conversation and a handover note, the latter highlighted.
    handover: Option<bool>,
}
/// Reads the model table of the addressed session's server, or of the server a new session
/// starts on, `ready` leaving out what it cannot run; an older read's answer is dropped.
pub fn request(app: &mut App, ready: bool) {
    app.models_at = app.models_at.wrapping_add(1);
    let version = app.models_at;
    // The addressed session's own id and server, or the server a new session starts on.
    let (session, server) = match &app.focus {
        Some(id) => id.as_str().split_once('@').unwrap_or((id.as_str(), "")),
        None => ("", app.server.as_str()),
    };
    let mut path = format!("v1/models?ready={ready}&server={server}");
    // Only the picker shows switches, which the session prices from its `startup.md`.
    if ready && !session.is_empty() {
        path += &format!("&session={session}");
    }
    app.spawn(|api| async move {
        let read = async {
            let value = api.json(reqwest::Method::GET, &path, None).await?;
            let mut models: Models = serde_json::from_value(value).map_err(fail(
                "The model list is invalid; reconnect to a matching server.",
            ))?;
            let ready = |m: &String| !models.unavailable.contains_key(m);
            models.models.retain(ready);
            let bytes: usize = models.models.iter().map(String::len).sum();
            match models.models.len() > 64 || bytes > super::api::BYTES {
                true => Err(error(
                    "The model list exceeds its display bound; use /model with a name instead.",
                )),
                false => Ok(models),
            }
        };
        let read = read.await;
        move |app: &mut App| read_models(app, version, read)
    });
}

fn read_models(app: &mut App, version: u64, read: Result<Models, super::Error>) {
    if version != app.models_at {
        return;
    }
    match read {
        Ok(models) => app.models = Some(models),
        Err(e) => {
            app.picker.iter_mut().for_each(|p| p.failed = true);
            return app.say(&e.to_string());
        }
    }
    let index = app.models.as_ref().map_or(0, |m| {
        m.models
            .iter()
            .position(|v| v == app.choice().0)
            .unwrap_or(m.models.len())
    });
    if let Some(p) = &mut app.picker {
        (p.index, p.ready) = (index, true);
    }
}
/// Reads choices afresh, dropping temporary provider access errors.
pub fn open(app: &mut App, speed_row: bool) {
    if app.sending.is_some() {
        return app.say("Input admission is pending; wait before choosing settings.");
    }
    app.picker = Some(Picker {
        speed_row,
        speed: app.speed(),
        ..Picker::default()
    });
    request(app, true);
}
/// Stages a model or effort, and whether a switch hands over, for the next input and says what
/// it will carry.
pub fn stage(
    app: &mut App,
    model: Option<String>,
    effort: Option<Effort>,
    speed: Option<Speed>,
    handover: Option<bool>,
) {
    let name = model.as_deref().unwrap_or(app.choice().0);
    let choice = choice(app, name, speed.unwrap_or(app.speed()));
    if let Some(reason) = choice.and_then(|c| c.reason.clone()) {
        return app.say(&reason);
    }
    let hint = choice
        .map_or("Provider pricing applies.", |c| c.hint.as_str())
        .to_owned();
    let pending = app.initial.get_or_insert_with(ClientInput::default);
    // A choice between keeping and a handover belongs to the model it was made for.
    pending.handover = handover.or(pending.handover.filter(|_| model.is_none()));
    pending.model = model.or(pending.model.take());
    pending.effort = effort.or(pending.effort);
    pending.speed = speed.or(pending.speed);
    let handover = pending.handover;
    let (model, effort) = app.choice();
    let switch = handover.zip(self::switch(app, model)).and_then(|(h, s)| {
        let ((words, cost), why) = (uncached(s, h)?, s.reason.as_deref().unwrap_or_default());
        let then = ["; ", ""][usize::from(why.is_empty())];
        Some(format!(" · {words}, {cost}{then}{why}"))
    });
    app.say(&format!(
        "Next input: {model} · {}{}{}. Applies to your next input. {hint}",
        effort.map_or("effort pending", super::text::effort),
        app.speed().suffix(),
        switch.unwrap_or_default()
    ));
}

/// What switching the addressed session to `model` reads uncached, as the server last said.
fn switch<'a>(app: &'a App, model: &str) -> Option<&'a crate::protocol::Switch> {
    app.models.as_ref()?.switch.get(model)
}

/// The words and uncached tokens of a switch keeping the conversation or, with `handover`,
/// starting from a handover note; `None` when the server offers no such switch.
fn uncached(s: &crate::protocol::Switch, handover: bool) -> Option<(&'static str, String)> {
    let tokens = if handover { s.handover? } else { s.keep? };
    let words = ["keep the conversation", "start from a handover note"][usize::from(handover)];
    let about = if handover { "about " } else { "" };
    let cost = format!("{about}{}k tokens uncached", (tokens + 500) / 1000);
    Some((words, cost))
}

/// The binding catalogue translates configurable picker actions to these native keys.
pub fn key(app: &mut App, key: KeyEvent) {
    if !key.modifiers.is_empty() {
        return;
    }
    if key.code == KeyCode::Esc {
        app.picker = None;
        return;
    }
    let Some(p) = app.picker.as_ref().filter(|p| p.ready) else {
        return;
    };
    let name = name(app, p).to_owned();
    let offered = levels(app, &name);
    let count = app.models.as_ref().map_or(0, |m| m.models.len());
    let baseline = app.choice().1;
    let Some(p) = &mut app.picker else {
        return;
    };
    // Choosing keep or a handover: Up and Down move between the two; `1` and `2` choose at once.
    let chose = match (p.handover, key.code) {
        (Some(h), KeyCode::Up | KeyCode::Down) => return p.handover = Some(!h),
        (Some(h), KeyCode::Enter) => Some(h),
        (Some(_), KeyCode::Char(c @ ('1' | '2'))) => Some(c == '2'),
        (Some(_), _) => return,
        (None, _) => None,
    };
    match key.code {
        KeyCode::Tab => p.speed_row = !p.speed_row,
        KeyCode::Up => p.index = p.index.saturating_sub(1),
        KeyCode::Down => p.index = (p.index + 1).min(count.saturating_sub(1)),
        KeyCode::Left | KeyCode::Right => {
            let left = key.code == KeyCode::Left;
            let at = if p.speed_row {
                p.speed as usize
            } else {
                (p.chosen.or(baseline).unwrap_or(Effort::High) as usize).min(offered.len() - 1)
            };
            let top = if p.speed_row { 2 } else { offered.len() - 1 };
            let at = if left {
                at.saturating_sub(1)
            } else {
                (at + 1).min(top)
            };
            if p.speed_row {
                p.speed = Speed::ALL[at];
            } else {
                p.chosen = Some(offered[at]);
            }
        }
        code if code == KeyCode::Enter || chose.is_some() => {
            let effort = p
                .chosen
                .or(baseline)
                .map(|e| e.min(offered[offered.len() - 1]));
            let speed = p.speed;
            let model = (name != app.choice().0).then_some(name);
            // A model that offers both asks first, keep highlighted, the makers' own default.
            let entry = model.as_deref().and_then(|m| switch(app, m));
            let both = entry.is_some_and(|s| s.keep.is_some() && s.handover.is_some());
            let handover = chose.or(entry.map(|s| s.keep.is_none()));
            if let Some(p) = app.picker.as_mut().filter(|_| both && chose.is_none()) {
                return p.handover = Some(false);
            }
            app.picker = None;
            stage(app, model, effort, Some(speed), handover);
        }
        _ => {}
    }
}
fn name<'a>(app: &'a App, p: &Picker) -> &'a str {
    app.models
        .as_ref()
        .and_then(|m| m.models.get(p.index))
        .map_or(app.choice().0, String::as_str)
}
/// Nearby models or efforts, then speed choices and their reason or cost hint.
pub fn draw(app: &mut App, b: &mut Buffer, area: Rect) {
    if app.picker.is_some() && (area.height < 3 || area.width < 8) {
        app.picker = None;
        app.say("There is no room for the picker; nothing was applied.");
    }
    let Some(p) = &app.picker else {
        return;
    };
    let name = name(app, p);
    let choices = app.models.as_ref().map_or(&[][..], |m| &m.models[..]);
    let visible = choices
        .len()
        .min(4)
        .min(area.height.saturating_sub(4) as usize);
    let first = p
        .index
        .saturating_sub(1)
        .min(choices.len().saturating_sub(visible));
    let title = if p.failed {
        "model list unavailable"
    } else if !p.ready {
        "model · fetching…"
    } else {
        "model · the model you pick"
    };
    let mut lines = vec![Line::styled(title, style(GRAY, RECESS))];
    let effort = |word| {
        let top = levels(app, word).last().copied().unwrap_or(Effort::Max);
        let effort = p.chosen.or(app.choice().1).unwrap_or(Effort::High);
        super::text::effort(effort.min(top))
    };
    if let (Some(handover), Some(s)) = (p.handover, switch(app, name)) {
        let title = format!("switch to {name} · {}", effort(name));
        lines = vec![Line::styled(title, style(GRAY, RECESS))];
        for (h, action) in [(false, "picker.keep"), (true, "picker.handover")] {
            let (words, cost) = uncached(s, h).unwrap_or_default();
            let mark = if h == handover { "▸" } else { " " };
            let row = format!("{mark} {}  {words}", bindings::hint(app, action));
            lines.push(Line::from(format!("{row:<36}{cost}")));
        }
    }
    let rows = choices.iter().enumerate().skip(first).take(visible);
    for (i, word) in rows.filter(|_| p.handover.is_none()) {
        let cost = switch(app, word).and_then(|s| uncached(s, s.keep.is_none()));
        let mark = if i == p.index { "▸" } else { " " };
        let row = format!("{mark} {word} · {}", effort(word));
        lines.push(Line::from(format!(
            "{row:<36}{}",
            cost.unwrap_or_default().1
        )));
    }
    lines.push(speeds(app, p, name));
    if let Some(c) = choice(app, name, p.speed) {
        lines.push(Line::styled(
            c.reason.as_deref().unwrap_or(&c.hint),
            style(GRAY, RECESS),
        ));
    }
    lines.push(Line::styled(
        bindings::hint(app, "picker.speed"),
        style(GRAY, RECESS),
    ));
    let h = (lines.len() as u16).min(area.height);
    let y = area.bottom() - h;
    fill(b, Rect::new(area.x, y, area.width, h), style(DATA, RECESS));
    for (line, y) in lines.iter().zip(y..area.bottom()) {
        b.set_line(area.x + 1, y, line, area.width - 2);
    }
}
// The speeds for `name`, the picked one marked and lit, those it cannot run grey.
fn speeds(app: &App, p: &Picker, name: &str) -> Line<'static> {
    let words = Speed::ALL
        .into_iter()
        .zip(["Standard", "Fast", "Ultrafast"]);
    let speeds = words.map(|(speed, word)| {
        let unavailable = choice(app, name, speed).is_some_and(|c| c.reason.is_some());
        let (mark, ink) = match speed == p.speed {
            true => ("▸", if unavailable { GRAY } else { CYAN }),
            false => ("", if unavailable { GRAY } else { DATA }),
        };
        Span::styled(format!("{mark}{word} · "), style(ink, RECESS))
    });
    speeds.collect::<Vec<_>>().into()
}
// The levels `model` takes, low up to its top in the model table: all five unless the table
// names a lower top.
fn levels(app: &App, model: &str) -> &'static [Effort] {
    let offered = app.models.as_ref().and_then(|m| m.efforts.get(model));
    let top = offered.and_then(|v| v.last()).unwrap_or(&Effort::Max);
    &Effort::ALL[..Effort::ALL.partition_point(|level| level <= top)]
}
fn choice<'a>(app: &'a App, name: &str, speed: Speed) -> Option<&'a crate::protocol::SpeedChoice> {
    app.models
        .as_ref()?
        .speeds
        .get(name)?
        .iter()
        .find(|c| c.speed == speed)
}
/// `/fast` and `/ultrafast` stage a speed through the picker's validation; `/fast` alone opens
/// the picker on its speed row.
pub fn fast(app: &mut App, word: &str, args: &str) {
    let speed = match (word, args) {
        ("/ultrafast", "") | ("/fast", "ultrafast") => Speed::Ultrafast,
        ("/ultrafast", _) => return app.say("Use /ultrafast without arguments."),
        (_, "") => return open(app, true),
        (_, "on" | "fast") => Speed::Fast,
        (_, "off" | "standard") => Speed::Standard,
        _ => return app.say("Choose standard, fast or ultrafast."),
    };
    stage(app, None, None, Some(speed), None);
}
