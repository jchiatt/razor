//! What the app writes to the terminal itself: each frame inside a synchronized update with
//! its links repainted in OSC 8, the tab's title, and the modes it enters at start and restores
//! on drop.
use super::{App, Error, text, view};
use crate::protocol::{AgentView, End, State, now};
use aws_lc_rs::digest::{SHA256, digest};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use crossterm::event::KeyboardEnhancementFlags as Flags;
use crossterm::event::{self as term, KeyCode::Char, KeyEvent, KeyModifiers as Mods};
use ratatui::{Terminal, backend::CrosstermBackend, buffer::Buffer, layout::Rect};
use std::{collections::BTreeMap, io::Write, sync::Arc};

pub fn draw(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
) -> Result<(), Error> {
    app.next.take();
    let backend = terminal.backend_mut();
    crossterm::execute!(backend, crossterm::terminal::BeginSynchronizedUpdate)
        .map_err(Error::Terminal)?;
    app.transcripts.values_mut().for_each(|t| t.undraw());
    let drawn = terminal.draw(|frame| {
        let (area, now) = (frame.area(), now());
        if let Some((x, y)) = view::draw(app, frame.buffer_mut(), area, now) {
            frame.set_cursor_position((x, y));
        }
    });
    let mut drawn = drawn.map(|frame| links(&app.links, &mut app.painted, frame.buffer));
    let title = title(app);
    if let (Ok(bytes), false) = (&mut drawn, title == app.title) {
        bytes.extend(format!("\x1b]2;{title}\x07").bytes());
        app.title = title;
    }
    if let Ok(bytes) = &mut drawn {
        program(app, bytes);
        terminal
            .backend_mut()
            .write_all(bytes)
            .map_err(Error::Terminal)?;
    }
    let ended = crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::EndSynchronizedUpdate
    );
    drawn.map_err(Error::Terminal)?;
    ended.map_err(Error::Terminal)
}

// The tab's title for this terminal's session tree, the folder first: who asks, who is stuck,
// that the addressed session's turn just failed, that something works, or that it ended well.
// An idle session is `NeedsYou` too, so that state alone says nothing here.
fn title(app: &App) -> String {
    let folder = app.cwd.file_name().unwrap_or_default().to_string_lossy();
    let mine = |a: &&AgentView| {
        let up = std::iter::successors(Some(*a), |a| app.agents.get(a.parent.as_ref()?));
        up.take(64).any(|a| app.root.as_ref() == Some(&a.session))
    };
    let tree: Vec<_> = app.agents.values().filter(mine).collect();
    let ask = tree.iter().find(|a| !a.asks.is_empty());
    let stuck = tree.iter().find(|a| a.state == State::Stuck);
    let ended = app.transcript().and_then(|t| t.ended.as_ref());
    let addressed = app.row();
    let title = match (ask, stuck, ended) {
        (Some(a), _, _) => format!("? {folder} · {} asks", a.name),
        (_, Some(a), _) => format!("! {folder} · {} stuck", a.name),
        (_, _, Some(End::Error(_))) if addressed.is_none_or(|a| a.state != State::Working) => {
            let name = addressed.map_or("main", |a| a.name.as_str());
            format!("! {folder} · {name} needs you")
        }
        _ if tree.iter().any(|a| a.state == State::Working) => format!("● {folder} · working"),
        (_, _, Some(End::Done)) => format!("✓ {folder} · done"),
        _ => folder.into_owned(),
    };
    title.chars().filter(|c| !c.is_control()).take(64).collect()
}

/// What the app tells its terminal through OSC 7501 (the Program Status Protocol): nothing until
/// the terminal answers the query `Modes::enter` sends; then one OSC 7501 record for the attached
/// session and one for each live descendant, each written when its bytes change, and every one
/// cleared when the app drops.
#[derive(Default)]
pub struct ProgramStatus {
    /// The terminal answered the query, so it keeps OSC 7501 records.
    pub answered: bool,
    // Characters of the answer read so far: at most 4,096, the protocol's whole sequence.
    reading: Option<usize>,
    // Each OSC 7501 record's pairs as last written, by its id pair ("" for the attached
    // session's): at most 64, the fewest a terminal keeps, so none Razor still means is evicted.
    sent: BTreeMap<String, String>,
}

impl ProgramStatus {
    /// Takes a key of the terminal's answer, `ESC ] 7501;? … ST`, which crossterm reads as Alt-],
    /// one Char per byte, then Alt-\ (ST) or Ctrl-G (BEL). True when the key was the answer's.
    /// Only an unmodified or Shift-only Char continues a read; any other key (Ctrl-C, say), a
    /// Char that breaks the `7501;?` prefix, and every key once answered, is the app's.
    pub fn answer(&mut self, key: &KeyEvent) -> bool {
        let plain = matches!(key.modifiers, Mods::NONE | Mods::SHIFT);
        let at = match (self.answered, self.reading, key.code, key.modifiers) {
            (true, ..) => return false,
            (_, None, Char(']'), Mods::ALT) => 0,
            (_, Some(6..), Char('\\'), Mods::ALT) | (_, Some(6..), Char('g'), Mods::CONTROL) => {
                (self.answered, self.reading) = (true, None);
                return true;
            }
            (_, Some(at @ 0..6), Char(c), _) if plain && b"7501;?"[at] as char == c => at + 1,
            (_, Some(at @ 6..4096), Char(_), _) if plain => at + 1,
            _ => {
                self.reading = None;
                return false;
            }
        };
        self.reading = Some(at);
        true
    }
}

impl Drop for ProgramStatus {
    fn drop(&mut self) {
        if !self.sent.is_empty() {
            let clear = crossterm::style::Print("\x1b]7501;state=clear\x1b\\");
            let _ = crossterm::execute!(std::io::stdout(), clear);
        }
    }
}

// Appends each OSC 7501 report whose bytes changed since the last draw, after a clear for each
// record no longer live. Descendants come in the status pane's order, each path kept as its
// `:id=` pair. Local session ids are protocol-safe; qualified ids become 128-bit hashes in
// URL-safe base64, preserving their server without lookup state. A settled child retires its subtree, and
// the protocol's 128-byte path limit bounds the depth below its eight-level limit.
fn program(app: &mut App, bytes: &mut Vec<u8>) {
    if !app.program.answered {
        return;
    }
    let (agents, mut live) = (&app.agents, BTreeMap::new());
    let transcript = app.root.as_ref().and_then(|id| app.transcripts.get(id));
    let ended = transcript.and_then(|t| t.ended.as_ref());
    let root = app.root.as_ref().and_then(|id| agents.get(id));
    let mut stack = Vec::from_iter(root.map(|a| (a, String::new())));
    while let Some((a, id)) = stack.pop().filter(|_| live.len() < 64) {
        live.insert(id.clone(), pairs(a, ended.filter(|_| id.is_empty()), &id));
        let child = |c: &&AgentView| c.parent.as_ref() == Some(&a.session) && c.settled.is_none();
        let mut under: Vec<_> = agents.values().filter(child).collect();
        under.sort_by(|a, b| (b.launched, &b.name).cmp(&(a.launched, &a.name)));
        let sep = if id.is_empty() { ":id=" } else { "/" };
        for c in under {
            let path = match c.session.as_str() {
                session if session.contains('@') => format!(
                    "{id}{sep}{}",
                    URL_SAFE_NO_PAD.encode(&digest(&SHA256, session.as_bytes()).as_ref()[..16])
                ),
                session => format!("{id}{sep}{session}"),
            };
            if path.len() <= ":id=".len() + 128 {
                stack.push((c, path));
            }
        }
    }
    let sent = std::mem::replace(&mut app.program.sent, live);
    for id in sent.keys().filter(|id| !app.program.sent.contains_key(*id)) {
        bytes.extend(format!("\x1b]7501;state=clear{id}\x1b\\").bytes());
    }
    for (id, pairs) in &app.program.sent {
        if sent.get(id) != Some(pairs) {
            bytes.extend(format!("\x1b]7501;{pairs}\x1b\\").bytes());
        }
    }
}

// One row's OSC 7501 pairs: the attached session's carry `app`, a descendant's its id and its
// name as the title. The attached session is `done` or `error` while its last turn's end is
// unseen, as the tab's title says. `msg` is one line with content, the open ask's or else the
// headline, cut to 512 characters so its UTF-8 stays within the protocol's 2,048 bytes.
fn pairs(a: &AgentView, ended: Option<&End>, id: &str) -> String {
    let ask = a.asks.first().filter(|_| a.state == State::NeedsYou);
    let state = match (a.state, ask, ended) {
        (State::Working, ..) => "working",
        (State::Done, ..) => "done",
        (State::Stuck, ..) => "error",
        (_, Some(_), _) => "blocked:kind=question",
        (.., Some(End::Done)) => "done",
        (.., Some(End::Error(_))) => "error",
        _ => "idle",
    };
    let text = ask.map_or(&a.headline, |ask| &ask.text);
    let line = text.lines().map(str::trim).find(|l| !l.is_empty());
    let msg = line.unwrap_or_default().chars().filter(|c| !c.is_control());
    let msg: String = msg.take(512).collect();
    let who = match id {
        "" => ":app=razor".into(),
        id => format!("{id}:title={}", STANDARD.encode(&a.name)),
    };
    let msg = match msg.is_empty() {
        true => String::new(),
        false => format!(":msg={}", STANDARD.encode(msg)),
    };
    format!("state={state}{who}{msg}")
}

// Paints the visible link cells inside OSC 8 after ratatui's diff, which writes them without it.
// A link painted last frame keeps its OSC 8 unless the diff wrote one of its cells, so an
// unchanged frame writes no link bytes. Frames list their links in draw order, so a link is
// looked up at its own index in last frame's list; a shifted list repaints the links after the
// shift, which costs bytes, never a lost link. The diff is ratatui's own, recomputed against
// the last frame's buffer, so it holds whatever width ratatui gives a glyph; a new area is
// diffed against a blank one, as ratatui redraws it, which paints every link. The URL is parsed
// Markdown data with controls refused; cursor and SGR are restored for the next frame.
fn links(links: &[(Rect, Arc<str>)], last: &mut Painted, b: &Buffer) -> Vec<u8> {
    if last.0.area != b.area {
        last.0 = Buffer::empty(b.area);
    }
    let wrote: Vec<_> = last.0.diff_iter(b).map(|(x, y, _)| (y, x)).collect();
    let mut out = String::new();
    for (i, (r, url)) in links
        .iter()
        .enumerate()
        .filter(|(_, (_, url))| !url.is_empty() && !url.chars().any(char::is_control))
    {
        let first = wrote.partition_point(|&at| at < (r.y, r.x));
        let touched = matches!(wrote.get(first), Some(&(y, x)) if y == r.y && x < r.right());
        if !touched && last.1.get(i).is_some_and(|(at, to)| at == r && to == url) {
            continue;
        }
        out.push_str(&format!("\x1b[{};{}H\x1b]8;;{url}\x1b\\", r.y + 1, r.x + 1));
        let mut x = r.x;
        while x < r.right().min(b.area.right()) {
            let cell = &b[(x, r.y)];
            out.push_str(&format!("\x1b[{};{}H\x1b[0", r.y + 1, x + 1));
            // Modifier's first nine bits are SGR 1 to 9 in order.
            for code in (0..9).filter(|i| cell.modifier.bits() & 1 << i != 0) {
                out.push_str(&format!(";{}", code + 1));
            }
            for (colour, base) in [(cell.fg, 38), (cell.bg, 48)] {
                if let ratatui::style::Color::Rgb(r, g, b) = colour {
                    out.push_str(&format!(";{base};2;{r};{g};{b}"));
                }
            }
            out.push('m');
            out.push_str(cell.symbol());
            x = x.saturating_add(text::width(cell.symbol()).max(1) as u16);
        }
        out.push_str("\x1b]8;;\x1b\\\x1b[0m");
    }
    last.0.content.clone_from(&b.content);
    last.1 = links.to_vec();
    match out.is_empty() {
        true => Vec::new(),
        false => format!("\x1b7{out}\x1b8").into_bytes(),
    }
}

/// The last frame as ratatui wrote it, and the links painted over it. A new `Terminal` redraws
/// every cell, so it starts from the default, whose empty area is a new one.
pub type Painted = (Buffer, Vec<(Rect, Arc<str>)>);

/// Raw mode, the alternate screen, paste, mouse, focus and kitty key reports, restored on drop.
pub struct Modes;
impl Modes {
    pub fn enter() -> Result<Self, Error> {
        crossterm::terminal::enable_raw_mode().map_err(Error::Terminal)?;
        let modes = Self;
        let flags = Flags::DISAMBIGUATE_ESCAPE_CODES | Flags::REPORT_EVENT_TYPES;
        crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            term::EnableBracketedPaste,
            // Presses, drags and wheel in SGR form; no bare-motion reports, which nothing reads.
            crossterm::style::Print("\x1b[?1000h\x1b[?1002h\x1b[?1015h\x1b[?1006h"),
            term::PushKeyboardEnhancementFlags(flags),
            term::EnableFocusChange,
            crossterm::style::Print("\x1b[22;2t\x1b]7501;?\x1b\\")
        )
        .map_err(Error::Terminal)?;
        Ok(modes)
    }
}
impl Drop for Modes {
    fn drop(&mut self) {
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::EndSynchronizedUpdate,
            term::DisableFocusChange,
            term::PopKeyboardEnhancementFlags,
            term::DisableMouseCapture,
            term::DisableBracketedPaste,
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show,
            crossterm::style::Print("\x1b]2;\x07\x1b[23;2t")
        );
        let _ = crossterm::terminal::disable_raw_mode();
    }
}
