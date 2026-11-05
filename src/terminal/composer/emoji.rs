//! The emoji grid: a card flush right in the band with the search, every emoji under its tab's
//! name, the selected one's name or its tones, and the tabs. Every key goes to it while it is
//! open; what it inserts goes into the draft and is recorded with the API as one use.
use super::super::{App, data::Table, text::rgb, view::PHONE};
use crate::protocol::{Emoji as History, EmojiUse};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers as Mods};
use ratatui::style::Color;
use reqwest::Method;
use std::{cell::Cell, collections::BTreeMap, ops::Range};
use unicode_segmentation::UnicodeSegmentation;

mod card;
pub use card::draw;

const CELL: usize = 4;
const BUBBLE: usize = 7;
/// The lit cell's ground, and the ink of an emoji.
pub(super) const LIT: Color = rgb(0x1a6b80);
const INK: Color = rgb(0xffffff);
const TABS: [(&str, &str); 9] = [
    ("🕘", "Frequently Used"),
    ("😀", "Smileys & People"),
    ("🐻", "Animals & Nature"),
    ("🍔", "Food & Drink"),
    ("⚽", "Activity"),
    ("🚗", "Travel & Places"),
    ("💡", "Objects"),
    ("🔣", "Symbols"),
    ("🏳\u{fe0f}", "Flags"),
];
/// Frequently Used before any history.
const SEED: [&str; 12] = [
    "😂",
    "❤\u{fe0f}",
    "👍\u{fe0f}",
    "😭",
    "🙏",
    "😊",
    "🥰",
    "🔥",
    "✨",
    "🎉",
    "👀",
    "😅",
];
const TONE_NAMES: [&str; 6] = [
    "default",
    "light",
    "medium-light",
    "medium",
    "medium-dark",
    "dark",
];

/// The open grid: the search, the selected emoji's place in the grid, the first grid row shown,
/// the tone row when open, whether the next tap shows tones, Frequently Used and the remembered
/// tones as read when it opened, the `:code` it replaces, and the columns and rows last drawn.
pub struct Grid {
    search: String,
    at: usize,
    top: usize,
    toning: Option<u8>,
    armed: bool,
    frequent: Vec<usize>,
    results: Vec<usize>,
    tones: BTreeMap<String, u8>,
    replace: Option<Range<usize>>,
    size: Cell<(usize, usize)>,
}

/// Where a click on the grid landed.
#[derive(Clone, Copy, PartialEq)]
pub enum Hit {
    /// The card itself, under its buttons: a press there keeps the grid open and does nothing.
    Card,
    Cell(usize),
    Tab(usize),
    Tone(u8),
    ToneMode,
    Close,
}

// What goes in the draft, the default form it is recorded under, and the tone row's choice.
struct Choice {
    text: &'static str,
    key: &'static str,
    tone: Option<u8>,
}

#[derive(Clone, Copy)]
struct Section {
    tab: Option<usize>,
    start: usize,
    len: usize,
    row0: usize,
    labeled: bool,
}

enum Line {
    Label(usize),
    Cells(usize, usize, usize),
}

// The grid's rows found by arithmetic: each tab's label and rows of `cols`, or the results.
struct Layout {
    sections: Vec<Section>,
    cols: usize,
    rows: usize,
    total: usize,
}

impl Layout {
    fn line(&self, r: usize) -> Option<Line> {
        let (i, s) = (self.sections.iter().enumerate()).find(|(_, s)| {
            r >= s.row0 && r < s.row0 + usize::from(s.labeled) + s.len.div_ceil(self.cols)
        })?;
        let k = r - s.row0;
        match (s.labeled, k) {
            (true, 0) => s.tab.map(Line::Label),
            _ => {
                let from = (k - usize::from(s.labeled)) * self.cols;
                Some(Line::Cells(i, s.start + from, self.cols.min(s.len - from)))
            }
        }
    }
    fn section_at(&self, at: usize) -> Option<usize> {
        (self.sections.iter()).position(|s| at >= s.start && at < s.start + s.len)
    }
    fn row_of(&self, at: usize) -> usize {
        self.section_at(at).map_or(0, |i| {
            let s = self.sections[i];
            s.row0 + usize::from(s.labeled) + (at - s.start) / self.cols
        })
    }
}

// `top` moved as little as keeps row `row` among `shown`.
fn keep(top: usize, row: usize, shown: usize, rows: usize) -> usize {
    let n = shown.max(1);
    top.min(row)
        .max((row + 1).saturating_sub(n))
        .min(rows.saturating_sub(n))
}

impl Grid {
    fn items<'t>(
        &'t self,
        t: &'t Table,
        tab: Option<usize>,
    ) -> Box<dyn Iterator<Item = usize> + 't> {
        match tab {
            None => Box::new(self.results.iter().copied()),
            Some(0) => Box::new(self.frequent.iter().copied()),
            Some(n) => Box::new(t.tabs[n - 1].clone()),
        }
    }
    fn layout(&self, t: &Table) -> Layout {
        let cols = self.size.get().0.max(1);
        let tabs: Vec<_> = match self.search.trim().is_empty() {
            true => (0..9).map(Some).collect(),
            false => vec![None],
        };
        let (mut sections, mut start, mut row0) = (Vec::new(), 0, 0);
        for tab in tabs {
            let len = self.items(t, tab).count();
            let labeled = tab.is_some() && len > 0;
            sections.push(Section {
                tab,
                start,
                len,
                row0,
                labeled,
            });
            (start, row0) = (
                start + len,
                row0 + usize::from(labeled) + len.div_ceil(cols),
            );
        }
        Layout {
            sections,
            cols,
            rows: row0,
            total: start,
        }
    }
    fn selected(&self, t: &Table) -> Option<usize> {
        let l = self.layout(t);
        let s = l.sections[l.section_at(self.at)?];
        self.items(t, s.tab).nth(self.at - s.start)
    }
    fn glyph(&self, t: &Table, i: usize) -> &'static str {
        glyph(t, &self.tones, i)
    }
    fn select(&mut self, t: &Table, at: usize) {
        let l = self.layout(t);
        self.at = at.min(l.total.saturating_sub(1));
        self.toning = None;
        self.top = keep(self.top, l.row_of(self.at), self.size.get().1, l.rows);
    }
    fn refresh(&mut self, t: &Table) {
        self.results = search(t, &self.search);
        (self.at, self.top, self.toning) = (0, 0, None);
    }
    fn step(&mut self, t: &Table, code: KeyCode) {
        if let Some(tone) = self.toning {
            let last = self
                .selected(t)
                .map_or(0, |i| if t.emoji[i].tones.is_some() { 5 } else { 0 });
            self.toning = Some(match code {
                KeyCode::Left => tone.saturating_sub(1),
                KeyCode::Right => (tone + 1).min(last),
                _ => tone,
            });
            return;
        }
        let l = self.layout(t);
        let (down, row) = (code == KeyCode::Down, l.row_of(self.at));
        let Some(Line::Cells(_, start, _)) = l.line(row) else {
            return;
        };
        let column = self.at - start;
        let mut r = row;
        match code {
            KeyCode::Left => self.select(t, self.at.saturating_sub(1)),
            KeyCode::Right => self.select(t, self.at + 1),
            _ => loop {
                r = match down {
                    true => r + 1,
                    false if r == 0 => return,
                    false => r - 1,
                };
                match l.line(r) {
                    Some(Line::Label(_)) => {}
                    Some(Line::Cells(_, start, len)) => {
                        return self.select(t, start + column.min(len - 1));
                    }
                    None => return,
                }
            },
        }
    }
    // PageUp and PageDown: the previous or next tab's first emoji, its label at the top.
    fn jump(&mut self, t: &Table, back: bool) {
        let l = self.layout(t);
        let here = l.section_at(self.at);
        let mut i = match (here, back) {
            (Some(i), true) if self.at > l.sections[i].start => Some(i),
            (Some(i), true) => i.checked_sub(1),
            (Some(i), false) => Some(i + 1),
            (None, _) => None,
        };
        while let Some(s) = i.and_then(|n| l.sections.get(n)) {
            if s.len > 0 {
                self.select(t, s.start);
                self.top = s.row0.min(l.rows.saturating_sub(self.size.get().1));
                return;
            }
            i = if back {
                i.and_then(|n| n.checked_sub(1))
            } else {
                i.map(|n| n + 1)
            };
        }
    }
    fn tab(&mut self, t: &Table, tab: usize) {
        (self.replace, self.search) = (None, String::new());
        self.refresh(t);
        let l = self.layout(t);
        if let Some(s) = l.sections.iter().find(|s| s.tab == Some(tab) && s.len > 0) {
            self.select(t, s.start);
            self.top = s.row0.min(l.rows.saturating_sub(self.size.get().1));
        }
    }
    fn toggle(&mut self, t: &Table) {
        let toned = self.selected(t).filter(|&i| t.emoji[i].tones.is_some());
        self.toning = match (self.toning, toned) {
            (None, Some(i)) => Some(self.tones.get(t.emoji[i].glyph).copied().unwrap_or(0)),
            _ => None,
        };
    }
    fn choice(&self, t: &Table) -> Option<Choice> {
        let i = self.selected(t)?;
        let e = &t.emoji[i];
        let text = match (self.toning, e.tones) {
            (Some(n @ 1..=5), Some(tones)) => tones[usize::from(n) - 1],
            (Some(_), _) => e.glyph,
            (None, _) => self.glyph(t, i),
        };
        Some(Choice {
            text,
            key: e.glyph,
            tone: self.toning,
        })
    }
}

// Every typed word begins a word of the name, a keyword or a shortcode: exact names and codes
// first, then words of the name or codes, then a keyword, then the rest; table order within.
fn search(t: &Table, query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    let words: Vec<&str> = q.split_whitespace().collect();
    let code = words.join("_");
    let starts = |e: &super::super::data::Emoji, w: &str, keywords: bool| {
        let name = e.name.split(|c: char| !(c.is_alphanumeric() || c == '+'));
        name.filter(|n| !n.is_empty()).any(|n| n.starts_with(w))
            || keywords && e.keywords.iter().any(|k| k.starts_with(w))
            || e.codes
                .iter()
                .any(|c| c.starts_with(w) || c.split('_').any(|p| p.starts_with(w)))
    };
    let mut ranked: Vec<(u8, usize)> = (t.emoji.iter().enumerate())
        .filter(|(_, e)| !words.is_empty() && words.iter().all(|w| starts(e, w, true)))
        .map(|(i, e)| {
            let rank = match () {
                _ if e.name == q || e.codes.contains(&code.as_str()) => 0,
                _ if words.iter().all(|w| starts(e, w, false)) => 1,
                _ if e.keywords.contains(&words[0]) => 2,
                _ => 3,
            };
            (rank, i)
        })
        .collect();
    ranked.sort_by_key(|&(rank, _)| rank);
    ranked.into_iter().map(|(_, i)| i).collect()
}

/// Emoji `i` in the tone remembered for it.
pub(super) fn glyph(t: &Table, tones: &BTreeMap<String, u8>, i: usize) -> &'static str {
    let e = &t.emoji[i];
    match (tones.get(e.glyph), e.tones) {
        (Some(&n @ 1..=5), Some(toned)) => toned[usize::from(n) - 1],
        _ => e.glyph,
    }
}

/// The emoji whose default form is `glyph`, with or without U+FE0F.
fn find(t: &Table, glyph: &str) -> Option<usize> {
    let bare = |s: &str| s.chars().filter(|&c| c != '\u{fe0f}').collect::<String>();
    (t.emoji.iter())
        .position(|e| e.glyph == glyph)
        .or_else(|| t.emoji.iter().position(|e| bare(e.glyph) == bare(glyph)))
}

/// Opens the grid (Ctrl-G, `/emoji`, the ☺ or Emoji chip), or closes it if open; over a `:code`
/// being typed it searches for it and replaces it. Reads the history again beside the loop.
pub fn open(app: &mut App, over: Option<(String, Range<usize>)>) {
    if app.grid.take().is_some() {
        return;
    }
    let t = &app.emoji;
    let (search, replace) = over.map_or((String::new(), None), |(q, r)| (q, Some(r)));
    let mut grid = Grid {
        search,
        at: 0,
        top: 0,
        toning: None,
        armed: false,
        frequent: frequent(t, &app.used),
        results: Vec::new(),
        tones: app.used.tones.clone(),
        replace,
        size: Cell::new((9, 6)),
    };
    grid.refresh(t);
    app.grid = Some(grid);
    read(app);
}

/// Reads the person's emoji history; an untouched open grid takes its Frequently Used.
pub fn read(app: &mut App) {
    app.spawn(|api| async move {
        let read = api.json(Method::GET, "v1/emoji", None).await;
        let bad = "The server sent an invalid emoji history; reconnect to the server.";
        let read = read
            .and_then(|v| serde_json::from_value::<History>(v).map_err(super::super::fail(bad)));
        move |app: &mut App| match read {
            Ok(history) => {
                let fresh = frequent(&app.emoji, &history);
                if let Some(g) = app
                    .grid
                    .as_mut()
                    .filter(|g| g.at == 0 && g.search.is_empty())
                {
                    (g.frequent, g.tones) = (fresh, history.tones.clone());
                }
                app.used = history;
            }
            Err(e) => app.say(&e.to_string()),
        }
    });
}

// The person's own most used, then the seed fills in after them until their history covers it;
// thirty-two at most, as the cockpit's store kept them.
fn frequent(t: &Table, history: &History) -> Vec<usize> {
    let mine = history.recent.iter().map(String::as_str);
    let seed = SEED
        .into_iter()
        .filter(|s| !history.recent.iter().any(|r| r == s));
    mine.chain(seed)
        .take(32)
        .filter_map(|g| find(t, g))
        .collect()
}

/// Rows the card wants: twelve (nine on a phone) while it is open.
pub fn rows(app: &App) -> u16 {
    match (&app.grid, app.columns < PHONE) {
        (None, _) => 0,
        (Some(_), true) => 9,
        (Some(_), false) => 12,
    }
}

/// Arrows move, PageUp and PageDown change tab, Tab opens the tones, letters search, Enter
/// inserts and closes, Backspace with no search deletes from the draft, Esc steps back.
pub fn key(app: &mut App, k: KeyEvent) {
    let (Some(g), t) = (app.grid.as_mut(), &app.emoji) else {
        return;
    };
    let ctrl = k.modifiers.contains(Mods::CONTROL);
    let other = k.modifiers.intersects(Mods::ALT | Mods::SUPER);
    match k.code {
        KeyCode::Char('c' | 'g') if ctrl => app.grid = None,
        KeyCode::Esc if g.toning.is_some() => g.toning = None,
        KeyCode::Esc => app.grid = None,
        KeyCode::Enter => match g.choice(t) {
            Some(c) => insert(app, c, true),
            None => app.grid = None,
        },
        KeyCode::Tab => g.toggle(t),
        KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right => g.step(t, k.code),
        KeyCode::PageUp | KeyCode::PageDown => g.jump(t, k.code == KeyCode::PageUp),
        KeyCode::Backspace | KeyCode::Delete if !g.search.is_empty() => {
            let cut = g
                .search
                .grapheme_indices(true)
                .next_back()
                .map_or(0, |(i, _)| i);
            g.search.truncate(cut);
            g.refresh(t);
        }
        KeyCode::Backspace => {
            g.replace = g
                .replace
                .take()
                .filter(|r| r.end - 1 > r.start)
                .map(|r| r.start..r.end - 1);
            if app.composer.key(k) {
                app.edited();
            }
        }
        KeyCode::Char('u') if ctrl => {
            g.search.clear();
            g.refresh(t);
        }
        KeyCode::Char(c) if !ctrl && !other && !c.is_control() && g.toning.is_none() => {
            g.search.push(c);
            g.refresh(t);
        }
        _ => {}
    }
}

/// A click on the card: a cell inserts and stays open (or shows its tones in tone mode), a tab
/// goes to it, a tone inserts in it, the tone button arms tone mode, ✕ closes.
pub fn click(app: &mut App, hit: Hit, right: bool) {
    let (Some(g), t) = (app.grid.as_mut(), &app.emoji) else {
        return;
    };
    match hit {
        Hit::Card => {}
        Hit::Close => app.grid = None,
        Hit::Tab(n) => g.tab(t, n),
        Hit::ToneMode => g.armed = !g.armed,
        Hit::Tone(n) => {
            g.toning = Some(n);
            if let Some(c) = g.choice(t) {
                insert(app, c, false);
            }
        }
        Hit::Cell(_) if g.toning.is_some() && !right => g.toning = None,
        Hit::Cell(at) => {
            g.select(t, at);
            let toned = g.selected(t).is_some_and(|i| t.emoji[i].tones.is_some());
            if right || g.armed && toned {
                g.armed = false;
                g.toggle(t);
            } else if let Some(c) = g.choice(t) {
                insert(app, c, false);
            }
        }
    }
}

// Puts the choice in the draft (over the `:code` it opened on, while the cursor ends it),
// remembers its tone, and records one use.
fn insert(app: &mut App, c: Choice, close: bool) {
    let cursor = app.composer.cursor();
    let over = app
        .grid
        .as_mut()
        .and_then(|g| g.replace.take())
        .filter(|r| r.end == cursor);
    match over {
        Some(range) => app.composer.swap(range, c.text),
        None => app.composer.insert(c.text),
    };
    app.edited();
    match (close, app.grid.as_mut()) {
        (true, _) => app.grid = None,
        (false, Some(g)) => {
            match c.tone {
                Some(n @ 1..=5) => _ = g.tones.insert(c.key.into(), n),
                Some(_) => _ = g.tones.remove(c.key),
                None => {}
            }
            (g.toning, g.armed) = (None, false);
        }
        _ => {}
    }
    record(app, c.key, c.tone);
}

/// Records one use of the emoji whose default form is `key` with the API, and the tone chosen.
pub(super) fn record(app: &mut App, key: &str, tone: Option<u8>) {
    let used = serde_json::to_value(EmojiUse {
        emoji: key.into(),
        tone,
    })
    .unwrap_or_default();
    app.spawn(|api| async move {
        let posted = api.json(Method::POST, "v1/emoji", Some(used)).await;
        move |app: &mut App| posted.err().iter().for_each(|e| app.say(&e.to_string()))
    });
}
