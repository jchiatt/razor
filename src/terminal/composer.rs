//! The composer: the person's words edited in place with bounded undo and wrapped with only their
//! visible rows owned, drawn as the full card (`card`) or the phone form (`phone`).
use super::{App, motion, text::*};
use crate::protocol::{AgentView, Skill, State, now};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers as Mods};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    ops::Range,
};
use unicode_segmentation::UnicodeSegmentation;

mod card;
pub mod completion;
pub mod emoji;
mod images;
mod phone;
pub mod shortcodes;
pub use images::{draw_images, image_rows, paste};

/// The editor's cap, which a saved draft and a recalled input keep to as well.
pub const DRAFT_BYTES: usize = 1 << 20;
// Undo and redo together keep at most 8 MiB and 256 snapshots.
const UNDO_BYTES: usize = 8 << 20;

type Snapshot = (String, usize, Option<usize>);

#[derive(Default)]
pub struct Composer {
    pub text: String,
    cursor: usize,
    // At most the server's 20 images of 10 MB base64, with removed ones held for undo.
    images: Vec<images::Loaded>,
    next_image: usize,
    undo: VecDeque<Snapshot>,
    redo: VecDeque<Snapshot>,
    undo_bytes: usize,
    anchor: Option<usize>,
    // The column Up and Down keep to across short rows.
    goal: Option<usize>,
    window: RefCell<Option<Window>>,
    // The words' wrapped rows and the width they were counted at, until the words change.
    wrapped: Cell<Option<(usize, usize)>>,
    columns: Cell<usize>,
    // Where the words were last drawn and the first wrapped row shown there, for the mouse.
    area: Cell<(Rect, usize)>,
    // A left press landed in the words, so the drag and the release that follow are theirs.
    held: bool,
    /// The image tray's chip the keys are on, by its place among the waiting images.
    pub chosen: Option<usize>,
}

// The visible rows around the cursor, kept until the cursor, width, height, selection or the
// skills list (by its allocation and length) changes.
#[derive(Clone, Default)]
struct Window {
    key: (usize, usize, usize, Option<usize>, (usize, usize)),
    rows: VecDeque<Line<'static>>,
    row: usize,
    column: usize,
    first: usize,
}

fn flags(k: KeyEvent) -> (bool, bool, bool, bool) {
    let alt = k.modifiers.intersects(Mods::ALT | Mods::META);
    let m = |flag| k.modifiers.contains(flag);
    (m(Mods::CONTROL), alt, m(Mods::SUPER), m(Mods::SHIFT))
}

impl Composer {
    /// Replaces the words, with the cursor at their end.
    pub fn replace(&mut self, text: String) {
        (*self.window.get_mut(), *self.wrapped.get_mut()) = (None, None);
        (self.goal, self.anchor, self.cursor, self.text) = (None, None, text.len(), text);
    }

    /// Nothing to send: blank words and no image chip.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.active_images().next().is_none()
    }

    fn selection(&self) -> Option<Range<usize>> {
        let at = self.anchor.filter(|at| *at != self.cursor)?;
        Some(at.min(self.cursor)..at.max(self.cursor))
    }

    fn bound_undo(&mut self) {
        while self.undo_bytes > UNDO_BYTES || self.undo.len() + self.redo.len() > 256 {
            let Some((text, _, _)) = self.undo.pop_front().or_else(|| self.redo.pop_front()) else {
                break;
            };
            self.undo_bytes = self.undo_bytes.saturating_sub(text.len());
        }
    }

    fn save(&mut self) {
        (*self.window.get_mut(), *self.wrapped.get_mut()) = (None, None);
        self.goal = None;
        self.undo_bytes -= self
            .redo
            .drain(..)
            .map(|state| state.0.len())
            .sum::<usize>();
        self.undo_bytes += self.text.len();
        self.undo
            .push_back((self.text.clone(), self.cursor, self.anchor));
        self.bound_undo();
    }

    fn restore(&mut self, redo: bool) {
        let (from, to) = match redo {
            true => (&mut self.redo, &mut self.undo),
            false => (&mut self.undo, &mut self.redo),
        };
        let Some((text, cursor, anchor)) = from.pop_back() else {
            return;
        };
        self.undo_bytes = self.undo_bytes - text.len() + self.text.len();
        to.push_back((
            std::mem::replace(&mut self.text, text),
            self.cursor,
            self.anchor,
        ));
        (self.cursor, self.anchor, self.goal) = (cursor, anchor, None);
        (*self.window.get_mut(), *self.wrapped.get_mut()) = (None, None);
        self.bound_undo();
    }

    /// Inserts words over the selection with control sequences removed and line ends made `\n`;
    /// false, changing nothing, past the 1 MiB cap.
    fn insert(&mut self, s: &str) -> bool {
        // The raw bytes are checked before any cleaned copy is made.
        let selected = self.selection();
        if self.text.len() - selected.as_ref().map_or(0, Range::len) + s.len() > DRAFT_BYTES {
            return false;
        }
        let s = screen_safe(s).replace("\r\n", "\n").replace('\r', "\n");
        self.save();
        if let Some(range) = selected {
            self.cursor = range.start;
            self.text.replace_range(range, "");
        }
        self.anchor = None;
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
        true
    }

    /// The cursor's byte offset in the words.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Replaces the words in `range` and puts the cursor after them, as one undo step.
    pub fn swap(&mut self, range: Range<usize>, s: &str) -> bool {
        (self.anchor, self.cursor) = (Some(range.start), range.end);
        self.insert(s)
    }

    /// Inserts words at the end, as one undo step; false past the 1 MiB cap.
    pub fn append(&mut self, s: &str) -> bool {
        (self.cursor, self.anchor) = (self.text.len(), None);
        self.insert(s)
    }

    fn previous(&self) -> usize {
        self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i)
    }
    fn next(&self) -> usize {
        self.text[self.cursor..]
            .graphemes(true)
            .next()
            .map_or(self.cursor, |g| self.cursor + g.len())
    }
    // Forward to the next word's start, back to this word's.
    fn word(&self, forward: bool) -> usize {
        match forward {
            true => {
                let rest = &self.text[self.cursor..];
                let gap = rest.find(|c| !wordy(c)).unwrap_or(rest.len());
                self.cursor + rest[gap..].find(wordy).map_or(rest.len(), |n| gap + n)
            }
            false => {
                let before = self.text[..self.cursor].trim_end_matches(|c| !wordy(c));
                before.trim_end_matches(wordy).len()
            }
        }
    }
    fn line_edge(&self, end: bool) -> usize {
        match end {
            true => self.text[self.cursor..]
                .find('\n')
                .map_or(self.text.len(), |i| self.cursor + i),
            false => self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1),
        }
    }

    fn destination(&self, k: KeyEvent) -> Option<usize> {
        let (ctrl, alt, sup, _) = flags(k);
        let right = k.code == KeyCode::Right;
        Some(match k.code {
            KeyCode::Up if sup => 0,
            KeyCode::Down if sup => self.text.len(),
            KeyCode::Left | KeyCode::Right if sup => self.visual_edge(right),
            KeyCode::Left | KeyCode::Right if ctrl || alt => self.word(right),
            KeyCode::Left => self.previous(),
            KeyCode::Right => self.next(),
            KeyCode::Home => self.line_edge(false),
            KeyCode::End => self.line_edge(true),
            KeyCode::Char('e' | 'E') if ctrl => self.line_edge(true),
            KeyCode::Char(c @ ('a' | 'e' | 'A' | 'E')) if alt => {
                self.visual_edge(c.eq_ignore_ascii_case(&'e'))
            }
            KeyCode::Char(c @ ('f' | 'b')) if alt => self.word(c == 'f'),
            KeyCode::Char('f') if ctrl => self.next(),
            KeyCode::Char('b') if ctrl => self.previous(),
            _ => return None,
        })
    }

    fn navigate(&mut self, k: KeyEvent) -> bool {
        if matches!(k.code, KeyCode::Char('a' | 'A'))
            && k.modifiers.intersects(Mods::CONTROL | Mods::SUPER)
        {
            (self.anchor, self.cursor, self.goal) = (Some(0), self.text.len(), None);
            return true;
        }
        // A plain Left or Right out of a selection lands on its edge.
        if let (true, Some(range), KeyCode::Left | KeyCode::Right) =
            (k.modifiers.is_empty(), self.selection(), k.code)
        {
            let at = [range.start, range.end][usize::from(k.code == KeyCode::Right)];
            (self.cursor, self.anchor, self.goal) = (at, None, None);
            return true;
        }
        let (old, shift) = (self.cursor, k.modifiers.contains(Mods::SHIFT));
        let moved = match self.destination(k) {
            Some(at) => {
                (self.cursor, self.goal) = (at, None);
                true
            }
            // Shift-Up on the first row selects to the start, Shift-Down on the last to the end.
            None if matches!(k.code, KeyCode::Up | KeyCode::Down) => {
                let down = k.code == KeyCode::Down;
                let edge = [0, self.text.len()][usize::from(down)];
                self.vertical(down) || shift && std::mem::replace(&mut self.cursor, edge) != edge
            }
            None => return false,
        };
        if !shift {
            self.anchor = None;
        } else if moved {
            self.anchor.get_or_insert(old);
        }
        // Shift-Up at an edge must not replace the selection with input history.
        moved || shift
    }

    /// The selection, or all the words when none is selected; a cut removes the selection only
    /// when one copy can carry it (not blank, at most 72 KiB), and with none selected there is
    /// nothing to cut.
    pub fn copy_selection(&mut self, cut: bool) -> Option<String> {
        let Some(range) = self.selection() else {
            return (!cut).then(|| self.text.clone());
        };
        let text = self.text[range.clone()].to_owned();
        if cut && text.len() <= super::clipboard::MAX && !text.trim().is_empty() {
            self.delete(range);
        }
        Some(text)
    }

    fn delete(&mut self, range: Range<usize>) {
        if !range.is_empty() {
            self.save();
            self.cursor = range.start;
            self.text.replace_range(range, "");
            self.anchor = None;
        }
    }

    /// Applies an editing or movement key; false when the key is not the editor's.
    pub fn key(&mut self, k: KeyEvent) -> bool {
        if self.navigate(k) {
            return true;
        }
        let (ctrl, alt, sup, shift) = flags(k);
        let at = self.cursor;
        let around = |edge: usize| edge.min(at)..edge.max(at);
        match k.code {
            KeyCode::Char('z' | 'Z') if ctrl || sup => self.restore(shift),
            KeyCode::Char('-' | '_' | '7') if ctrl => self.restore(false),
            KeyCode::Char('.' | '^' | '6') if ctrl => self.restore(true),
            KeyCode::Backspace | KeyCode::Delete => {
                let forward = k.code == KeyCode::Delete;
                let edge = match () {
                    _ if sup => self.line_edge(forward),
                    _ if ctrl || alt => self.word(forward),
                    _ if forward => self.next(),
                    _ => self.previous(),
                };
                self.delete(self.selection().unwrap_or(around(edge)));
            }
            KeyCode::Char('w') if ctrl => {
                self.delete(self.selection().unwrap_or(self.word(false)..at))
            }
            // Ctrl-K at a line's end joins the next line to it.
            KeyCode::Char(c @ ('k' | 'u')) if ctrl => {
                let edge = match self.line_edge(c == 'k') {
                    edge if edge == at && c == 'k' => self.next(),
                    edge => edge,
                };
                self.delete(self.selection().unwrap_or(around(edge)));
            }
            // The line and its break; the last line takes the break before it.
            KeyCode::Char('d' | 'D') if ctrl && shift => {
                let (start, end) = (self.line_edge(false), self.line_edge(true));
                match end < self.text.len() {
                    true => self.delete(start..end + 1),
                    false => self.delete(start.saturating_sub(1)..end),
                }
            }
            KeyCode::Char('d') if ctrl || alt => {
                let end = if alt { self.word(true) } else { self.next() };
                self.delete(self.selection().unwrap_or(at..end));
            }
            KeyCode::Enter if shift || alt => return self.insert("\n"),
            KeyCode::Char('j') if ctrl => return self.insert("\n"),
            KeyCode::Char(c) if !ctrl && !alt && !sup => {
                return self.insert(c.encode_utf8(&mut [0; 4]));
            }
            _ => return false,
        }
        true
    }

    // The rows the words wrap to at `columns`, or their lines when that is more; counted once
    // per change of words or width, since a draft may be a MiB.
    fn wrapped(&self, columns: usize) -> usize {
        match self.wrapped.get() {
            Some((at, rows)) if at == columns => rows,
            _ => {
                let rows = (self.height(columns, usize::MAX)).max(self.text.split('\n').count());
                self.wrapped.set(Some((columns, rows)));
                rows
            }
        }
    }

    // Wrapped rows of the words, counted up to `cap`.
    fn height(&self, columns: usize, cap: usize) -> usize {
        let mut count = 0;
        visit_wrap(&[Span::raw(&self.text)], columns, |_, _, _| {
            count += 1;
            count < cap
        });
        count
    }

    fn window(&self, columns: usize, visible: usize, skills: &[Skill]) -> Window {
        let names = (skills.as_ptr() as usize, skills.len());
        let key = (self.cursor, columns, visible, self.anchor, names);
        if let Some(window) = self.window.borrow().as_ref().filter(|w| w.key == key) {
            return window.clone();
        }
        let mut window = Window {
            key,
            ..Window::default()
        };
        let (mut n, mut found, selection) = (0, false, self.selection());
        visit_wrap(&lit(&self.text, skills), columns, |parts, range, last| {
            if !found && (self.cursor < range.end || last) {
                found = true;
                let before = parts
                    .iter()
                    .take_while(|p| p.at < self.cursor)
                    .map(|p| width(p.text));
                (window.row, window.column) =
                    (n, before.sum::<usize>().min(columns.saturating_sub(1)));
            }
            let row = match &selection {
                Some(s) => {
                    let mut parts = parts.to_vec();
                    let reversed = Modifier::REVERSED;
                    for part in parts.iter_mut().filter(|p| p.at < s.end && p.end > s.start) {
                        part.style = part.style.add_modifier(reversed);
                    }
                    // A selected line break: one reversed cell past the row.
                    if self.text[..range.end].ends_with('\n') && s.contains(&(range.end - 1)) {
                        let style = Style::new().add_modifier(reversed);
                        let (at, end) = (range.end - 1, range.end);
                        parts.push(Part {
                            text: " ",
                            style,
                            at,
                            end,
                        });
                    }
                    to_line(&parts)
                }
                None => to_line(parts),
            };
            window.rows.push_back(row);
            if window.rows.len() > visible {
                window.rows.pop_front();
                window.first += 1;
            }
            n += 1;
            !found || window.rows.len() < visible
        });
        *self.window.borrow_mut() = Some(window.clone());
        window
    }

    // The start or end of the cursor's wrapped row.
    fn visual_edge(&self, end: bool) -> usize {
        let mut at = self.cursor;
        visit_wrap(
            &[Span::raw(&self.text)],
            self.columns.get().max(1),
            |parts, range, last| {
                if self.cursor >= range.end && !last {
                    return true;
                }
                at = if end {
                    parts.last().map_or(range.start, |p| p.end)
                } else {
                    range.start
                };
                false
            },
        );
        at
    }

    /// Whether a vertical movement would leave the editor's wrapped rows.
    pub fn vertical_edge(&self, down: bool) -> bool {
        let columns = self.columns.get().max(1);
        let row = self.window(columns, 1, &[]).row;
        if down {
            self.offset(columns, row + 1, 0).is_none()
        } else {
            row == 0
        }
    }

    // Moves to the wrapped row above or below at the goal column; false at the first or last.
    fn vertical(&mut self, down: bool) -> bool {
        let columns = self.columns.get().max(1);
        let current = self.window(columns, 1, &[]);
        let Some(target) = (if down {
            current.row.checked_add(1)
        } else {
            current.row.checked_sub(1)
        }) else {
            return false;
        };
        let goal = self.goal.unwrap_or(current.column);
        let Some(at) = self.offset(columns, target, goal) else {
            return false;
        };
        (self.cursor, self.goal) = (at, Some(goal));
        true
    }

    // The byte at `column` of wrapped row `row`: before the character drawn there, or the row's
    // end left of the next row's first character; none past the last row.
    fn offset(&self, columns: usize, row: usize, column: usize) -> Option<usize> {
        let (mut n, mut found) = (0, None);
        visit_wrap(&[Span::raw(&self.text)], columns, |parts, range, last| {
            if n < row {
                n += 1;
                return true;
            }
            let (mut used, mut at) = (0, range.start);
            for p in parts.iter().take_while(|p| {
                used += width(p.text);
                used <= column
            }) {
                at = p.end;
            }
            if !last && at == range.end && !self.text[..at].ends_with('\n') {
                at = parts.last().map_or(range.start, |p| p.at);
            }
            found = Some(at);
            false
        });
        found
    }

    /// A left press at `cell`: inside the drawn words it puts the cursor on the character there
    /// and starts a selection, which Shift extends from the cursor; a second press on the same
    /// cell within 500 ms (`double`) selects the word there. False, changing nothing, outside them.
    pub fn press(&mut self, cell: (u16, u16), shift: bool, double: bool) -> bool {
        if !self.area.get().0.contains(cell.into()) {
            return false;
        }
        let at = self.at(cell);
        self.anchor = Some(match shift {
            true => self.anchor.unwrap_or(self.cursor),
            false => at,
        });
        (self.cursor, self.goal, self.held) = (at, None, true);
        let end = self.text[at..]
            .find(|c| !wordy(c))
            .map_or(self.text.len(), |n| at + n);
        if double && end > at {
            let start = self.text[..at].trim_end_matches(wordy).len();
            (self.anchor, self.cursor) = (Some(start), end);
        }
        true
    }

    /// The pointer moved with the button down: after a press in the words the selection follows
    /// it, rows above or below the words reaching their first or last shown row.
    pub fn drag(&mut self, cell: (u16, u16)) -> bool {
        if self.held {
            (self.cursor, self.goal) = (self.at(cell), None);
        }
        self.held
    }

    /// The button came up: the selection a press in the words made, if any, for the clipboard;
    /// none when the press was elsewhere.
    pub fn release(&mut self) -> Option<Option<String>> {
        std::mem::take(&mut self.held).then(|| self.selection().map(|r| self.text[r].to_owned()))
    }

    // The byte under `cell` of the words as last drawn, clamped into their rows.
    fn at(&self, (x, y): (u16, u16)) -> usize {
        let (r, first) = self.area.get();
        let y = y.clamp(r.y, r.bottom().saturating_sub(1)) - r.y;
        let (columns, row) = (r.width.max(1) as usize, first + y as usize);
        let column = x.saturating_sub(r.x) as usize;
        self.offset(columns, row, column).unwrap_or(self.text.len())
    }

    // The words in `words`' ink, or the hint while there are none; returns the cursor cell.
    fn content(&self, b: &mut Buffer, r: Rect, app: &App) -> Option<(u16, u16)> {
        // Asked every frame: a taken-back draft's ink asks for the frames that settle it.
        let words = ink(app);
        if r.width == 0 || r.height == 0 {
            return None;
        }
        self.columns.set(r.width as usize);
        let window = match self.text.is_empty() {
            true => {
                let mut rows = VecDeque::new();
                let hint = [Span::raw(hint(app))];
                visit_wrap(&hint, r.width as usize, |parts, _, _| {
                    rows.push_back(to_line(parts));
                    rows.len() < r.height as usize
                });
                Window {
                    rows,
                    ..Window::default()
                }
            }
            false => self.window(r.width as usize, r.height as usize, &app.skills),
        };
        self.area.set((r, window.first));
        // The hint is grey, and brighter while Tab browses, as the cockpit drew it.
        let ink = match (self.text.is_empty(), app.selected.is_some()) {
            (true, false) => GRAY,
            (true, true) => DATA,
            (false, _) => words,
        };
        let ink = style(ink, BEZEL);
        for (y, row) in (r.y..).zip(&window.rows) {
            line(b, r, y, row, ink);
        }
        Some((
            r.x + window.column as u16,
            r.y + (window.row - window.first) as u16,
        ))
    }
}

// The words as spans with each known `/name` lit: a command or skill opening the draft, or a
// skill after whitespace; trailing punctuation stays unlit.
fn lit<'a>(text: &'a str, skills: &[Skill]) -> Vec<Span<'a>> {
    let look = Style::new().fg(LAMP).add_modifier(Modifier::BOLD);
    let (mut spans, mut from) = (Vec::new(), 0);
    for (at, _) in text.match_indices('/') {
        if at > 0 && !text[..at].ends_with(char::is_whitespace) {
            continue;
        }
        let rest = &text[at + 1..];
        let name = &rest[..rest.find(char::is_whitespace).unwrap_or(rest.len())];
        let name = name.trim_end_matches(|c: char| !c.is_alphanumeric());
        let known = skills.iter().any(|s| s.name == name) || at == 0 && completion::command(name);
        if !name.is_empty() && known {
            let end = at + 1 + name.len();
            spans.extend([
                Span::raw(&text[from..at]),
                Span::styled(&text[at..end], look),
            ]);
            from = end;
        }
    }
    spans.push(Span::raw(&text[from..]));
    spans
}

// A word's character: not blank and not ASCII punctuation, `_` aside, so `foo.bar` is two words.
fn wordy(c: char) -> bool {
    !(c.is_whitespace() || c.is_ascii_punctuation() && c != '_')
}

// The words' ink: taken back, they arrive cyan and settle to white over 240 ms.
fn ink(app: &App) -> Color {
    let left = app.transcript().and_then(|t| t.left);
    match left.filter(|l| l.2 && motion::on(app)) {
        Some((_, at, _)) if now() < at + 240 => {
            app.next.at(now());
            let k = motion::ease_out(motion::progress(now(), at, 240));
            mix(CYAN, WHITE, k)
        }
        _ => WHITE,
    }
}

// The next input's model as the cockpit names it, and its effort with the speed.
fn chosen(app: &App) -> (String, String) {
    let (model, effort) = app.choice();
    let effort = effort.map_or("effort pending", super::text::effort);
    (
        super::text::model(model),
        format!("{effort}{}", app.speed().suffix()),
    )
}

// The ☺ chip, lit while the grid is open.
fn emoji_chip(app: &App) -> Style {
    match app.grid {
        Some(_) => style(CYAN, YOU_BG),
        None => style(DATA, BEZEL_HI),
    }
}

// The empty editor names its effective send, navigation and stop keys.
fn hint(app: &App) -> String {
    let words = if app.answering.is_some() {
        "Answer here · <composer.send> sends only this answer · <composer.back> leaves"
    } else if app.selected.is_some() {
        "Browsing · <composer.send> opens · <targets.next> moves · <composer.back> back"
    } else if app.split_view() {
        "Say it to the child · <composer.send> sends · <composer.back> back · <global.stop> stops it"
    } else if super::audio::available(app) {
        "Message the agent · Voice <global.record>"
    } else if app.row().is_some_and(|a| a.state == State::Working) {
        "Message the agent · <composer.send> queues it · <composer.back> stops it"
    } else {
        "Message the agent · <composer.send> sends · /keys for shortcuts"
    };
    let new = (app.resident.as_ref().filter(|_| app.focus.is_none()))
        .map(|r| format!("New thread with {r} · <composer.send> sends · /keys for shortcuts"));
    super::bindings::text(app, new.as_deref().unwrap_or(words))
}

/// What the person's words do in a child's split: reach it as theirs, continuing it once it has
/// reported, until it is settled; and the colour that says so.
fn speaks_as(agent: Option<&AgentView>) -> (&'static str, Color) {
    match agent {
        Some(a) if a.settled.is_some() => ("ended · nothing reaches it", GRAY),
        _ => ("as you", CYAN),
    }
}
