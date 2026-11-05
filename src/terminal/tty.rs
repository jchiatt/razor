//! The terminal's events, with a report a read split after its ESC put back together: crossterm
//! takes a read that ends in a lone ESC for the Esc key, and the report's rest for typed keys.
//! A full PTY splits a trackpad's burst of wheel reports that way.
use crossterm::event::{Event, EventStream, KeyCode, KeyModifiers as Mods};
use crossterm::event::{MouseButton as B, MouseEvent, MouseEventKind as M};
use futures_util::StreamExt;
use std::{collections::VecDeque, io};
use tokio::time::{Duration, Instant, timeout_at};

/// How long a lone Esc waits for the rest of a split report; no hand types one this fast.
const REST: Duration = Duration::from_millis(50);

#[derive(Default)]
pub struct Events {
    stream: EventStream,
    /// A lone Esc's arrival and the characters since, while they may still be a report's rest.
    held: Option<(Instant, String)>,
    ready: VecDeque<io::Result<Event>>,
}

impl Events {
    /// The next event. Dropped unfinished, as `select!` drops it, it keeps what it holds.
    pub async fn next(&mut self) -> Option<io::Result<Event>> {
        loop {
            if let Some(event) = self.ready.pop_front() {
                return Some(event);
            }
            let Some((since, rest)) = &mut self.held else {
                let event = self.stream.next().await?;
                if !matches!(&event, Ok(Event::Key(k)) if *k == KeyCode::Esc.into()) {
                    return Some(event);
                }
                self.held = Some((Instant::now(), String::new()));
                continue;
            };
            let next = timeout_at(*since + REST, self.stream.next()).await;
            if let Ok(Some(Ok(Event::Key(k)))) = &next
                && let KeyCode::Char(c) = k.code
                && let Some(whole) = report(&format!("{rest}{c}"))
            {
                rest.push(c);
                // A whole report is the mouse report it was, or dropped: never the keys it spelled.
                if whole {
                    self.ready.extend(mouse(rest).map(Ok));
                    self.held = None;
                }
                continue;
            }
            let rest = self.held.take().map(|(_, rest)| rest).unwrap_or_default();
            let keys = std::iter::once(KeyCode::Esc).chain(rest.chars().map(KeyCode::Char));
            let keys = keys.map(|code| Ok(Event::Key(code.into())));
            self.ready.extend(keys.chain(next.ok().flatten()));
        }
    }
}

/// Whether the text after a lone ESC is a whole control sequence (`ESC [`, parameters, final
/// byte), only the start of one, or neither.
fn report(text: &str) -> Option<bool> {
    let body = text.strip_prefix('[')?;
    let mut tail = body.trim_start_matches(|c| matches!(c, ' '..='?')).chars();
    match (tail.next(), tail.next()) {
        (None, _) => Some(false),
        (Some('@'..='~'), None) => Some(true),
        _ => None,
    }
}

/// An SGR mouse report, `[<button;column;row` then M pressed or m released, decoded as
/// crossterm decodes one.
fn mouse(report: &str) -> Option<Event> {
    let body = report.strip_prefix("[<")?.strip_suffix(['M', 'm'])?;
    let mut n = body.split(';').map(|n| n.parse::<u16>().ok());
    let (code, column, row) = (n.next()??, n.next()??, n.next()??);
    let b = |n: u16| [B::Left, B::Middle, B::Right][usize::from(n)];
    let scroll = [M::ScrollUp, M::ScrollDown, M::ScrollLeft, M::ScrollRight];
    let kind = match (code & 0b1100_0011, code & 32 != 0) {
        (n @ 0..=2, false) if report.ends_with('m') => M::Up(b(n)),
        (n @ 0..=2, false) => M::Down(b(n)),
        (n @ 0..=2, true) => M::Drag(b(n)),
        (3, false) => M::Up(B::Left),
        (n @ 64..=67, false) => scroll[usize::from(n - 64)],
        _ => return None,
    };
    let bits = [(4, Mods::SHIFT), (8, Mods::ALT), (16, Mods::CONTROL)].into_iter();
    let modifiers = Mods::from_iter(bits.filter_map(|(bit, m)| (code & bit != 0).then_some(m)));
    Some(Event::Mouse(MouseEvent {
        kind,
        column: column.checked_sub(1)?,
        row: row.checked_sub(1)?,
        modifiers,
    }))
}
