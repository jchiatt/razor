//! Markdown to styled rows with pulldown-cmark. Every logical row is counted, but only the
//! requested range owns strings; link destinations survive wrapping, quotes and tables.
mod table;

use super::text::*;
use pulldown_cmark::{Event, LinkType, Options, Parser, RefDefs, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use std::{borrow::Cow, collections::VecDeque, ops::Range, sync::Arc};
use table::Table;

const OPTIONS: Options = Options::ENABLE_TABLES
    .union(Options::ENABLE_STRIKETHROUGH)
    .union(Options::ENABLE_TASKLISTS);

/// A link's cells within its row, drawn again inside OSC 8 escapes.
#[derive(Clone, Default)]
pub(super) struct Link {
    pub(super) columns: Range<usize>,
    pub(super) url: Arc<str>,
}
/// What a copy takes from a drawn row after its lead.
#[derive(Clone, Default, PartialEq)]
pub(super) enum Kind {
    /// Its words as drawn.
    #[default]
    Words,
    /// No words of its own: a cap, a fold or copy line, a card's edge, a table's rule.
    Frame,
    /// Words between borders in these columns, which a copy leaves out: a table's cells, parted
    /// by ` | `, or a card's body inside its right edge.
    Cells(Vec<u16>),
    /// A code block's line, a tab's two cells starting in each of these columns.
    Code(Vec<u16>),
}
impl Kind {
    /// The same columns `by` cells further right.
    pub(super) fn shift(&mut self, by: u16) {
        if let Kind::Cells(at) | Kind::Code(at) = self {
            at.iter_mut().for_each(|x| *x += by);
        }
    }
}
#[derive(Default, Clone)]
pub(super) struct Rendered {
    pub(super) words: Line<'static>,
    pub(super) links: Vec<Link>,
    pub(super) kind: Kind,
}
/// The requested rows of a rendering, its row count, and the rows that fold or copy it.
pub(super) struct Reply {
    pub(super) lines: Vec<Rendered>,
    pub(super) first: usize,
    pub(super) total: usize,
    pub(super) fold: Option<usize>,
    pub(super) copy: Option<usize>,
}

/// Renders `src` at `columns`. `open` is a completed reply's fold state (input and live text
/// pass None and never pay for the fold scan); a reply of more than 120 top-level blocks folds.
pub(super) fn render(
    src: &str,
    columns: usize,
    human: bool,
    open: Option<bool>,
    range: Range<usize>,
) -> Reply {
    let safe = screen_safe(src);
    let bounded = bound_runs(&safe);
    let cut = matches!(bounded, Cow::Owned(_));
    let text = if cut { bounded } else { safe };
    let hidden = open.and_then(|_| fold(&text));
    let folded = open == Some(false) && hidden.is_some();
    let lines = (hidden.as_ref()).map(|r| text[r.clone()].bytes().filter(|b| *b == b'\n').count());
    let forms = match (open, lines.map(thousands)) {
        (Some(true), _) => [
            "▾ the whole reply is shown · click to fold the earlier lines".into(),
            "▾ the whole reply · click to fold".into(),
            "▾ shown · fold".into(),
        ],
        (_, Some(n)) => [
            format!("▸ {n} earlier lines not shown · click to open"),
            format!("▸ {n} earlier lines · click to open"),
            format!("▸ {n} earlier lines · open"),
        ],
        _ => Default::default(),
    };
    // The fold line's longest form the width holds.
    let label = forms
        .iter()
        .find(|f| width(f) <= columns)
        .unwrap_or(&forms[2])
        .clone();
    let first = range.start;
    let mut out = Lines {
        range,
        ..Lines::default()
    };
    let prefix = !folded && hidden.is_some();
    if prefix {
        out.push(Line::styled(
            clip(&label, columns),
            Style::default().fg(CYAN),
        ));
    }
    let hidden = hidden.filter(|_| folded).map(|r| (r, label.as_str()));
    let fold = parse(&text, columns, human, hidden, &mut out).or(prefix.then_some(0));
    let copy = (open.is_some() && (folded || cut)).then(|| {
        let words = format!(
            "⧉ copy the full text · {} characters",
            thousands(src.encode_utf16().count())
        );
        out.push(Line::styled(words, Style::default().fg(CYAN)));
        out.total - 1
    });
    Reply {
        lines: out.lines,
        first,
        total: out.total,
        fold,
        copy,
    }
}

// Only the requested range owns row strings and link ranges; other rows add to the count.
#[derive(Default)]
struct Lines {
    range: Range<usize>,
    total: usize,
    lines: Vec<Rendered>,
    last_nonempty: bool,
    // The rows `linked` adds are a code block's.
    code: bool,
}
impl Lines {
    fn wants(&self) -> bool {
        self.range.contains(&self.total)
    }
    fn push(&mut self, words: Line<'static>) {
        self.rendered(Rendered {
            words,
            ..Rendered::default()
        });
    }
    fn rendered(&mut self, line: Rendered) {
        self.last_nonempty = !line.words.spans.is_empty();
        if self.wants() {
            self.lines.push(line);
        }
        self.total += 1;
    }
    fn linked(
        &mut self,
        spans: &[Span<'_>],
        columns: usize,
        quote: bool,
        destinations: &[(Range<usize>, Arc<str>)],
    ) {
        let indent = usize::from(quote) * 2;
        visit_wrap(spans, columns.saturating_sub(indent), |parts, _, last| {
            if last && parts.is_empty() {
                return true;
            }
            self.last_nonempty = !parts.is_empty() || quote;
            if self.wants() {
                let mut words = to_line(parts);
                let (mut links, mut tabs) = (Vec::<Link>::new(), Vec::new());
                let mut x = indent;
                let first = parts.first().map_or(0, |p| p.at);
                let mut at = destinations.partition_point(|(range, _)| range.end <= first);
                for part in parts {
                    // The wrap draws a tab as two spaces, never one grapheme otherwise.
                    if self.code && part.text == "  " {
                        tabs.push(x as u16);
                    }
                    let end = x + width(part.text);
                    while destinations
                        .get(at)
                        .is_some_and(|(range, _)| range.end <= part.at)
                    {
                        at += 1;
                    }
                    if let Some((_, url)) = destinations
                        .get(at)
                        .filter(|(range, _)| range.contains(&part.at))
                    {
                        match links
                            .last_mut()
                            .filter(|link| link.columns.end == x && link.url == *url)
                        {
                            Some(link) => link.columns.end = end,
                            None => links.push(Link {
                                columns: x..end,
                                url: url.clone(),
                            }),
                        }
                    }
                    x = end;
                }
                if quote {
                    words
                        .spans
                        .insert(0, Span::styled("│ ", Style::default().fg(GRAY)));
                }
                let kind = if self.code {
                    Kind::Code(tabs)
                } else {
                    Kind::Words
                };
                self.lines.push(Rendered { words, links, kind });
            }
            self.total += 1;
            true
        });
    }
}

type Cell<'a> = (Vec<Span<'a>>, Vec<(Range<usize>, Arc<str>)>);

#[derive(Default)]
struct State<'a> {
    source: &'a str,
    columns: usize,
    begin: usize,
    spans: Vec<Span<'a>>,
    text_at: usize,
    // An open link: where its words start, its address, and whether it is an autolink.
    link: Option<(usize, Arc<str>, bool)>,
    destinations: Vec<(Range<usize>, Arc<str>)>,
    styles: Vec<Style>,
    previous: usize,
    block_end: usize,
    code: bool,
    list: Vec<Option<u64>>,
    // How many of the pending spans are item markers, which open a row rather than fill it.
    markers: usize,
    after_list: bool,
    quote: usize,
    table: Option<Table>,
    cells: Vec<Cell<'a>>,
}

// One parser for complete or partial text; offsets preserve gaps and folded link definitions.
fn parse(
    source: &str,
    columns: usize,
    human: bool,
    hidden: Option<(Range<usize>, &str)>,
    out: &mut Lines,
) -> Option<usize> {
    if human {
        return typed(source, columns, out);
    }
    let mut state = State {
        source,
        columns,
        begin: out.total,
        styles: vec![Style::default().fg(DATA)],
        ..State::default()
    };
    let mut fold = None;
    let mut fold_at = |out: &mut Lines, label: &str| {
        fold.get_or_insert(out.total);
        out.push(Line::styled(
            clip(label, columns),
            Style::default().fg(CYAN),
        ));
    };
    let mut parser = Parser::new_ext(source, OPTIONS).into_offset_iter();
    while let Some((event, range)) = parser.next() {
        if let Some((hidden, label)) = &hidden
            && range.start >= hidden.start
        {
            if state.previous < hidden.end {
                fold_at(out, label);
                (state.previous, state.block_end) = (hidden.end, hidden.end);
            }
            if hidden.contains(&range.start) {
                continue;
            }
        }
        state.event(event, range, parser.reference_definitions(), out);
    }
    out.code = state.code;
    out.linked(&state.spans, columns, false, &state.destinations);
    // Reference definitions can consume the entire tail without producing an event.
    if let Some((hidden, label)) = &hidden
        && state.previous < hidden.end
    {
        fold_at(out, label);
    }
    fold
}

// The person's words exactly as typed, every marker kept and drawn in its part's style: bold,
// italic, struck, code on its chip, a link cyan and live; quotes and tables stay plain text.
fn typed(source: &str, columns: usize, out: &mut Lines) -> Option<usize> {
    let (mut spans, mut links, mut at, mut plain) = (Vec::new(), Vec::new(), 0, 0);
    let mut styles = vec![Style::default().fg(WHITE)];
    for (event, range) in Parser::new_ext(source, OPTIONS).into_offset_iter() {
        let current = styles.last().copied().unwrap_or_default();
        let cut = match event {
            Event::Start(_) | Event::Code(_) => range.start,
            Event::End(_) => range.end,
            _ => continue,
        };
        spans.push(Span::styled(&source[at..cut.max(at)], current));
        at = cut.max(at);
        match event {
            Event::Start(tag) => {
                plain += usize::from(matches!(tag, Tag::BlockQuote(_) | Tag::Table(_)));
                styles.push(match tag {
                    _ if plain > 0 => current,
                    Tag::Strong => current.add_modifier(Modifier::BOLD),
                    Tag::Emphasis => current.add_modifier(Modifier::ITALIC),
                    Tag::Strikethrough => current.fg(GRAY).add_modifier(Modifier::DIM),
                    Tag::Link { dest_url, .. } => {
                        links.push((range, Arc::from(dest_url.as_ref())));
                        current.fg(CYAN).add_modifier(Modifier::UNDERLINED)
                    }
                    _ => current,
                });
            }
            Event::End(tag) => {
                styles.pop();
                plain -= usize::from(matches!(tag, TagEnd::BlockQuote(_) | TagEnd::Table));
            }
            Event::Code(_) if plain == 0 && range.start >= at => {
                spans.push(Span::styled(&source[range.clone()], current.bg(CHIP)));
                at = range.end;
            }
            _ => {}
        }
    }
    spans.push(Span::styled(&source[at..], Style::default().fg(WHITE)));
    out.linked(&spans, columns, false, &links);
    None
}

impl<'a> State<'a> {
    fn separate(&mut self, tag: &Tag<'_>, range: &Range<usize>, out: &mut Lines) {
        let block = matches!(
            tag,
            Tag::Paragraph
                | Tag::Heading { .. }
                | Tag::CodeBlock(_)
                | Tag::BlockQuote(_)
                | Tag::Table(_)
                | Tag::List(_)
                | Tag::HtmlBlock
        );
        // A tight item's words have no paragraph to end their row; a block after them does.
        if block && self.table.is_none() && self.spans.len() > self.markers {
            self.flush(range.start, out);
        }
        if block && self.spans.is_empty() && self.table.is_none() {
            let gap = self.source.get(self.previous..range.start).unwrap_or("");
            if out.total > self.begin
                && (self.after_list || gap.contains('\n') || range.start > self.block_end + 1)
            {
                out.push(Line::default());
            }
            self.after_list = false;
        }
        let list = matches!(tag, Tag::List(_)) && self.list.is_empty();
        if (list || matches!(tag, Tag::CodeBlock(_))) && out.total > self.begin && out.last_nonempty
        {
            out.push(Line::default());
        }
    }

    fn start(&mut self, tag: Tag<'_>, range: Range<usize>, refs: &RefDefs<'_>, out: &mut Lines) {
        self.separate(&tag, &range, out);
        let current = self.styles.last().copied().unwrap_or_default();
        let look = match tag {
            Tag::Strong => current.fg(WHITE).add_modifier(Modifier::BOLD),
            Tag::Emphasis => current.add_modifier(Modifier::ITALIC),
            Tag::Strikethrough => current.fg(GRAY).add_modifier(Modifier::DIM),
            Tag::Link {
                link_type,
                dest_url,
                ..
            } => {
                if !dest_url.chars().any(char::is_control) {
                    let auto = matches!(link_type, LinkType::Autolink | LinkType::Email);
                    self.link = Some((self.text_at, Arc::from(dest_url.as_ref()), auto));
                }
                current.fg(CYAN).add_modifier(Modifier::UNDERLINED)
            }
            // An image reads as its alt text, or `image`, linked to its address; inside a link,
            // its alt text is that link's words.
            Tag::Image { dest_url, .. }
                if self.link.is_none() && !dest_url.chars().any(char::is_control) =>
            {
                self.link = Some((self.text_at, Arc::from(dest_url.as_ref()), true));
                current.fg(CYAN).add_modifier(Modifier::UNDERLINED)
            }
            Tag::CodeBlock(_) => {
                self.code = true;
                current
            }
            Tag::List(n) => {
                self.after_list = false;
                self.list.push(n);
                current
            }
            Tag::Item => {
                // A nested item indents unless its parent's marker still opens the row.
                let depth = self.list.len().saturating_sub(1) * usize::from(self.spans.is_empty());
                let indent = "  ".repeat(depth);
                let marker = match self.list.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{indent}{}. ", *n - 1)
                    }
                    _ => format!("{indent}- "),
                };
                self.spans.push(Span::styled(marker, current.fg(GRAY)));
                self.markers = self.spans.len();
                current
            }
            Tag::BlockQuote(_) => {
                self.quote += 1;
                current.fg(CYAN).add_modifier(Modifier::ITALIC)
            }
            Tag::Table(alignments) => {
                let source = &self.source[range];
                self.table = Some(Table::new(
                    source,
                    refs,
                    alignments.len(),
                    self.columns,
                    out,
                ));
                current
            }
            // Header cells in the heading's white bold.
            Tag::TableHead | Tag::TableRow => {
                self.cells.clear();
                let strong = current.fg(WHITE).add_modifier(Modifier::BOLD);
                [current, strong][usize::from(matches!(tag, Tag::TableHead))]
            }
            _ => current,
        };
        self.styles.push(look);
    }

    fn end(&mut self, tag: TagEnd, range: Range<usize>, out: &mut Lines) {
        match tag {
            TagEnd::Link => {
                if let Some((start, url, auto)) = self.link.take() {
                    self.destinations.push((start..self.text_at, url.clone()));
                    // The address in `label (url)` is linked too.
                    let pending: String = self.spans.iter().map(|s| s.content.as_ref()).collect();
                    if !auto && pending.get(start..) != Some(&*url) {
                        let (gray, at) = (Style::default().fg(GRAY), self.text_at + 2);
                        let address = Span::styled(url.to_string(), gray.fg(CYAN));
                        let parts = [Span::styled(" (", gray), address, Span::styled(")", gray)];
                        self.spans.extend(parts);
                        self.destinations.push((at..at + url.len(), url));
                    }
                }
            }
            // A link with no words yet, the image's own or the one around it, reads `image`.
            TagEnd::Image => {
                let empty = self.link.as_ref().is_some_and(|l| l.0 == self.text_at);
                if empty {
                    let look = Style::default().fg(CYAN).add_modifier(Modifier::UNDERLINED);
                    self.spans.push(Span::styled("image", look));
                }
                if let Some((start, url, _)) = self.link.take_if(|(.., auto)| *auto) {
                    self.destinations
                        .push((start..self.text_at + 5 * usize::from(empty), url));
                }
            }
            TagEnd::TableCell => {
                let (spans, links) = (
                    std::mem::take(&mut self.spans),
                    std::mem::take(&mut self.destinations),
                );
                match self.table.as_mut() {
                    Some(t) if self.cells.len() < t.widths.len() => self.cells.push((spans, links)),
                    Some(t) => {
                        let mut cell = Lines::default();
                        cell.linked(&spans, 1, false, &[]);
                        t.hidden_height = t.hidden_height.max(cell.total);
                    }
                    None => {}
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    table.row(&self.cells, out);
                }
                self.cells.clear();
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    table.border("└", "┴", "┘", out);
                }
            }
            TagEnd::BlockQuote(None) => self.quote = self.quote.saturating_sub(1),
            TagEnd::List(_) => {
                self.list.pop();
                self.after_list = self.list.is_empty();
            }
            _ => {}
        }
        if matches!(
            tag,
            TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::CodeBlock
                | TagEnd::HtmlBlock
        ) && self.table.is_none()
        {
            self.flush(range.end, out);
        }
        if tag == TagEnd::CodeBlock {
            self.code = false;
            self.previous = range.end.saturating_sub(1);
        }
        if self.styles.len() > 1 {
            self.styles.pop();
        }
    }

    // Draws the pending spans as rows; the gap before the next block is measured from `at`.
    fn flush(&mut self, at: usize, out: &mut Lines) {
        let spans = std::mem::take(&mut self.spans);
        out.code = self.code;
        out.linked(&spans, self.columns, self.quote > 0, &self.destinations);
        self.destinations.clear();
        (self.previous, self.block_end, self.markers) = (at, at, 0);
    }

    fn event(
        &mut self,
        event: Event<'a>,
        range: Range<usize>,
        refs: &RefDefs<'_>,
        out: &mut Lines,
    ) {
        let current = self.styles.last().copied().unwrap_or_default();
        let before = self.spans.len();
        match event {
            Event::Start(tag) => self.start(tag, range, refs, out),
            Event::End(tag) => self.end(tag, range, out),
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                self.spans.push(Span::styled(text, current))
            }
            Event::Code(text) => self
                .spans
                .push(Span::styled(text, current.fg(MAGENTA).bg(AGENT_BG))),
            Event::SoftBreak => self
                .spans
                .push(Span::styled(if self.code { "\n" } else { " " }, current)),
            Event::HardBreak => self.spans.push(Span::styled("\n", current)),
            Event::Rule => {
                if self.spans.len() > self.markers {
                    self.flush(range.start, out);
                }
                out.push(Line::styled(
                    "─".repeat(self.columns),
                    Style::default().fg(GRAY),
                ));
                self.previous = range.end;
            }
            Event::TaskListMarker(yes) => self.spans.push(Span::styled(
                if yes { "[x] " } else { "[ ] " },
                current.fg(GRAY),
            )),
            _ => {}
        }
        // Byte offset of the next text within the pending spans, which a flush empties.
        let (base, from) = if self.spans.len() < before {
            (0, 0)
        } else {
            (self.text_at, before)
        };
        self.text_at = base
            + self.spans[from..]
                .iter()
                .map(|s| s.content.len())
                .sum::<usize>();
    }
}

fn js_space(c: char) -> bool {
    matches!(c, '\t'..='\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}'
        | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

// The first or last `units` UTF-16 units of `s`, on character boundaries.
fn units(s: &str, units: usize, head: bool) -> &str {
    let mut n = 0;
    let mut cut = |c: char| {
        n += c.len_utf16();
        n > units
    };
    match head {
        true => s
            .char_indices()
            .find(|(_, c)| cut(*c))
            .map_or(s, |(i, _)| &s[..i]),
        false => s
            .char_indices()
            .rev()
            .find(|(_, c)| cut(*c))
            .map_or(s, |(i, c)| &s[i + c.len_utf8()..]),
    }
}

// A run over 4,000 UTF-16 units keeps 1,500 at its head and 500 at its
// tail. The original remains in the entry for full-text copy.
fn bound_runs(src: &str) -> Cow<'_, str> {
    let mut out: Option<String> = None;
    let mut previous = 0;
    for run in src.split_inclusive(js_space) {
        let word = run.trim_end_matches(js_space);
        let count = word.chars().map(char::len_utf16).sum::<usize>();
        if count > 4000 {
            let out = out.get_or_insert_with(|| src[..previous].to_owned());
            let skipped = thousands(count - 2000);
            out.push_str(&format!(
                "{} ⋯ {skipped} characters not drawn ⋯ ",
                units(word, 1500, true)
            ));
            out.push_str(units(word, 500, false));
            out.push_str(&run[word.len()..]);
        } else if let Some(out) = out.as_mut() {
            out.push_str(run);
        }
        previous += run.len();
    }
    out.map_or(Cow::Borrowed(src), Cow::Owned)
}

// Top-level blocks (link definitions in parser gaps included), not wrapped rows. Only the
// thirteenth start and the newest 100 are kept, so a million short paragraphs stay bounded.
fn fold(src: &str) -> Option<Range<usize>> {
    // Each block starts a line of its own, so under 120 line breaks nothing folds: no parse.
    if src.bytes().filter(|b| *b == b'\n').count() < 120 {
        return None;
    }
    let (mut count, mut head, mut last) = (0, 0, VecDeque::with_capacity(100));
    let mut add = |start| {
        if count == 12 {
            head = start;
        }
        count += 1;
        if last.len() == 100 {
            last.pop_front();
        }
        last.push_back(start);
    };
    let gaps = |from: usize, to: usize, add: &mut dyn FnMut(usize)| {
        let mut at = from;
        for gap in src[from..to].split_inclusive('\n') {
            if !gap.trim_matches(js_space).is_empty() {
                add(at);
            }
            at += gap.len();
        }
    };
    let (mut depth, mut previous) = (0usize, 0);
    for (event, range) in Parser::new_ext(src, OPTIONS).into_offset_iter() {
        if depth == 0 {
            gaps(previous, range.start, &mut add);
            add(range.start);
        }
        match event {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 {
            previous = range.end;
        }
    }
    gaps(previous, src.len(), &mut add);
    (count > 120).then(|| head..*last.front().unwrap_or(&head))
}
