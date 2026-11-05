//! A tool call as the transcript draws it: a quiet row when routine, a card while a shell call
//! runs or when a call fails, a native read row, and the opened card's viewport.
mod card;
mod decoded;
pub(super) use decoded::Decoded;
pub(super) mod diff;
pub(super) mod lamps;
mod row;
pub(super) mod skill;
pub(super) mod test;
mod turn;
use super::{text::*, transcript::Row, view::PHONE};
pub(super) mod child;
pub(super) mod rim;

use crate::protocol::{Body, Entry, EntryId, JobEnd, JobId, Outcome};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier},
    text::{Line, Span},
};
use serde_json::Value;
pub(super) use turn::{Place, launch, places, slots};

/// A child's name and its model in short words.
pub(super) type Child = (String, String);

/// One retained job's display facts, dropped with its call or past the transcript's job limit.
#[derive(Default)]
pub(super) struct Job {
    pub(super) call: EntryId,
    pub(super) end: Option<JobEnd>,
    pub(super) start: u64,
    pub(super) total: Option<usize>,
    /// A failed job's last two recorded lines, at most 1 KiB each.
    pub(super) last: Vec<String>,
}

pub(super) struct Call<'a> {
    pub(super) entry: &'a Entry,
    /// The name and model of the child this call launched or names.
    pub(super) child: Option<&'a Child>,
    pub(super) worn: Option<u64>,
    pub(super) zone: Option<&'a jiff::tz::TimeZone>,
    pub(super) place: Place,
    pub(super) result: Option<&'a Entry>,
    pub(super) decoded: Option<&'a Decoded>,
    pub(super) spool: Option<&'a super::output::Window>,
    pub(super) job: Option<(JobId, &'a Job)>,
    pub(super) tail: Option<&'a (String, u64)>,
    /// A shell call's command lines, counted once when the call was retained.
    pub(super) lines: usize,
}

/// When a call's rows next look different by themselves, and when their number next changes:
/// wall ms, `u64::MAX` for never.
#[derive(Clone, Copy)]
pub(super) struct Next {
    pub(super) look: u64,
    pub(super) size: u64,
}
impl Next {
    pub(super) const NEVER: Next = Next {
        look: u64::MAX,
        size: u64::MAX,
    };
    /// A look change at `at` that keeps the row count.
    pub(super) fn at(at: u64) -> Next {
        Next {
            look: at,
            ..Next::NEVER
        }
    }
}

impl Call<'_> {
    fn input(&self) -> (&str, &Value) {
        match &self.entry.body {
            Body::Call { name, input, .. } => (kind(name), input),
            _ => ("", &Value::Null),
        }
    }
    fn raw(&self) -> &str {
        match self.result.map(|e| &e.body) {
            Some(Body::Result { output, .. }) => output,
            _ => "",
        }
    }
    // Whether this is Claude Code's Bash text opening with its exit code, which the job event
    // already gives.
    fn exited(&self) -> bool {
        let first = self.raw().lines().next();
        let code = first.and_then(|line| line.strip_prefix("Exit code "));
        self.input().0 == "shell"
            && self.decoded.is_none()
            && code.is_some_and(|s| s.parse::<i32>().is_ok())
    }
    // Output lines, without Claude Code's exit line.
    fn lines(&self) -> impl DoubleEndedIterator<Item = &str> {
        let mut lines = self.decoded.map_or(self.raw(), |v| v.text.as_str()).lines();
        if self.exited() {
            lines.next();
        }
        lines
    }
    /// A test command's newest runner summary, read from its output.
    fn run(&self) -> Option<test::Run> {
        let (kind, input) = self.input();
        test::read(
            command(kind, input)?,
            self.decoded.map_or(self.raw(), |v| v.text.as_str()),
        )
    }
    fn count(&self) -> usize {
        self.decoded
            .map_or_else(|| self.lines().count(), |v| v.count)
    }
    // How its job ended, from the record or Razor's own shell result.
    fn end(&self) -> Option<JobEnd> {
        self.job
            .and_then(|j| j.1.end)
            .or_else(|| self.decoded.and_then(|v| v.end))
    }
    fn outcome(&self) -> Option<Outcome> {
        match self.end() {
            Some(end) => Some(job_outcome(end)),
            None if self.job.is_some() || self.decoded.and_then(|v| v.job).is_some() => None,
            None => match self.result.map(|e| &e.body) {
                Some(Body::Result { outcome, .. }) => Some(*outcome),
                _ => None,
            },
        }
    }
    /// The call's transcript rows, settled with motion off but with live clocks and dots.
    pub(super) fn rows(&self, columns: usize, now: u64, motion: bool) -> Vec<Row> {
        let slots = turn::slots(self.place.threads, columns);
        let inner = columns.saturating_sub(2 * slots - 2);
        let mut rows = turn::fold(self, inner, now, motion)
            .unwrap_or_else(|| card::rows(self, inner, now, motion));
        // A result's pictures, one line each: the media type and where it was saved.
        let (shown, look) = (!rows.is_empty(), |c| ratatui::style::Style::default().fg(c));
        for image in self
            .decoded
            .iter()
            .flat_map(|d| &d.images)
            .filter(|_| shown)
        {
            let kind = image.media_type.trim_start_matches("image/");
            let path = clip_middle(&super::mcp::home(&image.path), inner.saturating_sub(12));
            let words = [(format!("    ▣ {kind} · "), GRAY), (path, DATA)];
            let words = words.map(|(w, c)| Span::styled(w, look(c)));
            rows.push(Row::plain(Line::from(words.to_vec())));
        }
        if !rows.is_empty() {
            let gap = match self.place.first {
                true => Line::raw(""),
                false => Line::from(turn::gap(self.place.joined, self, now, motion)),
            };
            rows.insert(0, Row::plain(gap));
        }
        // A turn that launched children closes under their cards instead (`close`). The closing
        // line is a frame: no words to copy and no click to the call above it.
        if slots == 1 {
            rows.extend(
                turn::footer(self, columns, now, motion)
                    .into_iter()
                    .map(Row::frame),
            );
        }
        let tie = self.place.tie.map(|n| (n + 1).min(slots - 1));
        for (i, row) in rows.iter_mut().enumerate().filter(|_| slots > 1) {
            // The threads are decoration a copy leaves out.
            let by = 2 * (slots - 1) as u16;
            row.lead += by;
            row.kind.shift(by);
            turn::thread(
                &mut row.words,
                slots,
                self.place.open,
                tie.filter(|_| i == 1),
                false,
            );
        }
        // The first launch onto the last slot, when later children share it, says how many do.
        let shared = (self.place.threads + 2).saturating_sub(slots);
        if shared > 1 && self.place.tie == Some(slots - 2) && rows.len() > 1 {
            let mut words = Line::from(span("│", turn::tone(self, now, motion)));
            turn::thread(&mut words, slots, self.place.open + 1, None, false);
            words.spans.truncate(2 * slots - 3);
            words.spans.push(span(format!("+{shared}"), GRAY));
            rows.insert(2, Row::plain(words));
        }
        if self.place.after_reply && !self.place.first && !rows.is_empty() {
            rows.insert(0, Row::plain(""));
        }
        rows
    }
    /// The closing rule of a turn that launched children, under their cards and across their
    /// threads.
    pub(super) fn close(&self, columns: usize, now: u64, motion: bool) -> Vec<Row> {
        let slots = turn::slots(self.place.threads, columns);
        let mut rows = turn::footer(self, columns.saturating_sub(2 * slots - 2), now, motion);
        for (i, row) in rows.iter_mut().enumerate() {
            turn::thread(&mut row.words, slots, self.place.threads, None, i == 1);
        }
        rows
    }
    /// The next visible change and row-count change; settled calls ask for nothing.
    pub(super) fn next(&self, columns: usize, now: u64, motion: bool) -> Next {
        // The card's rows are counted at the width they are drawn at, inside the threads.
        let inner = columns.saturating_sub(2 * turn::slots(self.place.threads, columns) - 2);
        let mut next = turn::next(self, now, motion, card::next(self, inner, now, motion));
        // A replayed finish starts at its beat, where its card may open again.
        if let Some(beat) = self.place.beat.filter(|beat| *beat > now) {
            next.size = next.size.min(beat);
            next.look = next.look.min(beat);
        }
        next
    }
    fn ended(&self) -> Option<u64> {
        self.outcome()?;
        self.place.finished.or_else(|| self.result.map(|e| e.at))
    }
    // How long since the call's finish began playing; `None` while it runs. A finish waiting to
    // replay holds its first instant, unlit (`waiting`).
    fn since(&self, now: u64) -> Option<u64> {
        Some(now.saturating_sub(self.place.beat.or_else(|| self.ended())?))
    }
    fn waiting(&self, now: u64) -> bool {
        self.place.beat.is_some_and(|beat| beat > now)
    }
    fn asking(&self) -> bool {
        let (kind, input) = self.input();
        kind == "ask" || kind == "message" && input["type"] == "question"
    }
    /// Whether the call failed: its job or result ended in error, or nobody knows how.
    pub(super) fn failed(&self) -> bool {
        matches!(self.outcome(), Some(Outcome::Error | Outcome::Unknown))
    }
    /// Whether the output lives in the job's saved file rather than the result. Claude's shaped
    /// text holds the whole output only after its exit line and outside the spill wording.
    pub(super) fn spooled(&self) -> bool {
        self.job.is_some()
            && match self.decoded {
                Some(value) => !value.complete,
                None => !self.exited() || self.lines().next() == Some("<persisted-output>"),
            }
    }
    // The opened card's input strip: a shell's command as written, `$ ` quiet and a heredoc's
    // body grey; else each field's name grey and its first line white; past twelve, counted.
    fn detail_input(&self, columns: usize, phone: bool) -> Vec<Line<'static>> {
        let (kind, input) = self.input();
        let mut rows: Vec<(String, String, Color)> = Vec::new();
        let hidden = if let Some(command) = command(kind, input) {
            let (mut end, mut lines) = (None::<String>, command.trim_end().lines());
            for (i, l) in lines.by_ref().take(12).enumerate() {
                let quiet = end.as_deref().is_some_and(|e| l.trim() != e);
                end = if end.is_some() {
                    end.filter(|_| quiet)
                } else {
                    heredoc(l)
                };
                let mark = if i == 0 { "$ " } else { "" };
                rows.push((mark.into(), l.into(), if quiet { GRAY } else { WHITE }));
            }
            self.lines.saturating_sub(12)
        } else {
            for (key, value) in input.as_object().into_iter().flatten().take(12) {
                let words = match value {
                    Value::String(s) => s.lines().next().unwrap_or("").to_owned(),
                    Value::Array(a) => format!("{} items", a.len()),
                    Value::Object(o) => format!("{} fields", o.len()),
                    other => other.to_string(),
                };
                rows.push((format!("{key} "), words, WHITE));
            }
            input.as_object().map_or(0, |o| o.len().saturating_sub(12))
        };
        let room = columns.saturating_sub(15);
        let mut lines: Vec<_> = (rows.into_iter())
            .flat_map(|(label, words, ink)| {
                let words = clip(&words, room * if phone { 3 } else { 1 });
                let label = Span::styled(label, style(GRAY_DIM, STRIP));
                let spans = vec![label, Span::styled(words, style(ink, STRIP))];
                match phone {
                    true => wrap(spans, room),
                    false => vec![Line::from(spans)],
                }
            })
            .collect();
        if hidden > 0 {
            let words = format!("⋮ {hidden} more line{}", plural(hidden));
            lines.push(Line::styled(words, style(GRAY, STRIP)));
        }
        lines
    }
    // The opened card's state tag and input strip, scrolled with its lines.
    fn detail_head(
        &self,
        b: &mut Buffer,
        r: Rect,
        scroll: usize,
        command: &[Line<'_>],
        phone: bool,
    ) {
        let height = r.height.saturating_sub(1) as usize;
        let width = r.width.saturating_sub(14);
        let (tag, tone, bg): (String, _, _) = match self.outcome() {
            None => (" running ".into(), GREEN, RECESS),
            Some(Outcome::Success) => (" ✓ ok ".into(), GREEN, ADDED_BG),
            Some(Outcome::Error) => (format!(" ✗ {} ", row::failed(self)), AMBER, AMBER_BG),
            Some(Outcome::Unknown) => (" ? unknown ".into(), RED, RED_BG),
        };
        if scroll <= 1 && 1 < scroll.saturating_add(height) {
            let y = r.y + 2 - scroll as u16;
            let mut x = put(b, r.x + 9, y, &tag, width, bold(tone, bg));
            // Beside the tag: its job, how long it took, its output's size and where the whole
            // output was saved, labels dim and values grey.
            let took = self
                .ended()
                .map(|end| dur(end.saturating_sub(self.entry.at)));
            let size: usize = self.lines().map(|l| l.len() + 1).sum();
            let size = Some(size).filter(|n| *n > 0 && !self.spooled());
            let saved = self
                .lines()
                .find_map(|l| l.strip_prefix("The whole output is in: "));
            let kb = |n: usize| match n {
                ..1024 => format!("{n} B"),
                1024..1_048_576 => format!("{:.1} KB", n as f64 / 1024.),
                _ => format!("{:.1} MB", n as f64 / 1_048_576.),
            };
            let facts = [
                ("job", self.job.map(|j| j.0.0.to_string())),
                ("took", took),
                ("size", size.map(kb)),
                (
                    "full text",
                    (self.decoded.and_then(|d| d.path.as_deref()))
                        .or(saved)
                        .map(std::path::Path::new)
                        .map(super::mcp::home),
                ),
            ];
            for (label, value) in facts.into_iter().filter_map(|(l, v)| Some((l, v?))) {
                let room = (r.x + 9 + width).saturating_sub(x + 4 + label.len() as u16);
                x = put(b, x + 3, y, label, room, style(GRAY_DIM, RECESS));
                let value = clip_middle(&value, room.saturating_sub(1).into());
                x = put(b, x + 1, y, &value, room, style(GRAY, RECESS));
            }
        }
        for (i, words) in command.iter().enumerate() {
            if let Some(y) = (i + 3).checked_sub(scroll).filter(|y| *y < height) {
                let y = r.y + 1 + y as u16;
                // A full phone strip's padding overruns one cell.
                let ground =
                    width + u16::from(phone && words.width() >= width.saturating_sub(1) as usize);
                fill(b, Rect::new(r.x + 9, y, ground, 1), style(DATA, STRIP));
                line(
                    b,
                    Rect::new(r.x + 10, y, width.saturating_sub(1), 1),
                    y,
                    words,
                    style(WHITE, STRIP),
                );
            }
        }
    }
    fn detail_preview(&self) -> String {
        let out: String = self
            .lines()
            .enumerate()
            .flat_map(|(i, line)| (i > 0).then_some('\n').into_iter().chain(line.chars()))
            .take(crate::protocol::SPILL)
            .collect();
        if out.is_empty() {
            "no output".to_owned()
        } else {
            out
        }
    }
    fn detail_output(
        &self,
        b: &mut Buffer,
        r: Rect,
        scroll: usize,
        start: usize,
        output: Option<&str>,
    ) {
        let height = r.height.saturating_sub(1) as usize;
        let columns = r.width.saturating_sub(14) + u16::from(r.width < PHONE);
        // Output in data, a failed call's in amber, and `no output` grey; a read keeps its file's
        // line numbers, dim, before each line.
        let (failed, kind) = (self.failed(), self.input().0);
        let search = kind == "search";
        // A web result's addresses in cyan, each source's title above its address bold white.
        let address = |t: &str| t.starts_with("http") && !t.contains(' ') && kind == "web";
        let first =
            (self.decoded.and_then(|d| d.first)).filter(|_| output.is_none() && self.count() > 0);
        let digits = first.map_or(0, |f| {
            (f + self.count() as u64 - 1).to_string().len() as u16
        });
        let gutter = if digits > 0 { digits + 2 } else { 0 };
        let mut paint = |n: usize, text: &str, title: bool| {
            if let Some(y) = (n + start).checked_sub(scroll).filter(|y| *y < height) {
                let ink = match text {
                    "no output" => GRAY,
                    _ if failed => AMBER,
                    _ if title => WHITE,
                    _ if address(text) => CYAN,
                    _ => DATA,
                };
                let y = r.y + 1 + y as u16;
                if let Some(first) = first {
                    let number = format!("{:>w$}", first + n as u64, w = digits as usize);
                    put(b, r.x + 9, y, &number, digits, style(GRAY_DIM, RECESS));
                }
                let (mut x, end) = (r.x + 9 + gutter, r.x + 9 + columns);
                // A search hit: its path white, `:12:` grey, the match in the output's ink.
                let hit = text
                    .split_once(':')
                    .filter(|_| search)
                    .and_then(|(path, rest)| {
                        let (n, words) = rest.split_once(':')?;
                        let at = path.len()..path.len() + n.len() + 2;
                        let plain = !path.contains(char::is_whitespace) && n.parse::<u64>().is_ok();
                        plain.then(|| vec![(path, WHITE), (&text[at], GRAY), (words, ink)])
                    });
                let bold = [Modifier::empty(), Modifier::BOLD][usize::from(title)];
                for (words, ink) in hit.unwrap_or_else(|| vec![(text, ink)]) {
                    let look = style(ink, RECESS).add_modifier(bold);
                    x = put(b, x, y, words, end.saturating_sub(x), look);
                }
            }
        };
        if let Some(output) = output {
            let mut n = 0;
            visit_wrap(&[Span::raw(output)], columns as usize, |parts, _, last| {
                if last && parts.is_empty() || n + start >= scroll.saturating_add(height) {
                    return false;
                }
                if n + start >= scroll {
                    paint(
                        n,
                        &parts.iter().map(|part| part.text).collect::<String>(),
                        false,
                    );
                }
                n += 1;
                true
            });
        } else {
            let mut lines = self
                .detail_lines(scroll.saturating_sub(start).saturating_add(5))
                .peekable();
            while let Some((n, text)) = lines.next() {
                if n + start >= scroll.saturating_add(height) {
                    break;
                }
                let title = !address(text) && lines.peek().is_some_and(|(_, t)| address(t));
                paint(n, text, title);
            }
        }
    }
    // The opened card's numbered lines from the result, or from the saved file's window.
    fn detail_lines(&self, scroll: usize) -> Box<dyn Iterator<Item = (usize, &str)> + '_> {
        let note = |text| Box::new(std::iter::once((scroll.saturating_sub(5), text)));
        if !self.spooled() {
            let empty = (self.count() == 0).then_some("no output");
            return Box::new(self.lines().chain(empty).enumerate());
        }
        match self.spool {
            Some(v) if !v.lines.is_empty() => {
                Box::new(v.lines.iter().zip(v.first..).map(|(s, i)| (i, s.as_str())))
            }
            Some(v) if v.failed => note("output unavailable · close and reopen to retry"),
            Some(v) if v.total == Some(0) => Box::new(std::iter::once((0, "no output"))),
            _ => note("loading saved output…"),
        }
    }
    /// The opened card: a viewport, never a list of every output row, so every source line
    /// stays reachable, a single oversized entry's included; only screen rows allocate.
    pub(super) fn detail(&self, b: &mut Buffer, r: Rect, scroll: &mut usize) {
        if r.height == 0 {
            return;
        }
        line(
            b,
            r,
            r.y,
            &row::draw(self, r.width.saturating_sub(2) as usize, u64::MAX, false),
            style(DATA, GLASS),
        );
        if diff::draw(self, b, r, scroll) {
            return;
        }
        let phone = r.width < PHONE;
        let command = self.detail_input(r.width as usize, phone);
        let start = 4 + command.len().max(1);
        let output = (phone && !self.spooled()).then(|| self.detail_preview());
        let count = if let Some(output) = &output {
            let mut rows = 0;
            visit_wrap(
                &[Span::raw(output)],
                r.width.saturating_sub(13) as usize,
                |parts, _, last| {
                    rows += usize::from(!last || !parts.is_empty());
                    true
                },
            );
            Some(rows)
        } else if self.spooled() {
            self.spool.and_then(|v| v.total)
        } else {
            Some(self.count())
        };
        let total = count.map(|n| start + n.max(1) + 1);
        let height = r.height.saturating_sub(1) as usize;
        if let Some(total) = total {
            *scroll = (*scroll).min(total.saturating_sub(height));
        }
        let body = Rect::new(
            r.x + 7,
            r.y + 1,
            r.width.saturating_sub(11),
            r.height.saturating_sub(1),
        );
        fill(b, body, style(DATA, RECESS));
        self.detail_head(b, r, *scroll, &command, phone);
        self.detail_output(b, r, *scroll, start, output.as_deref());
        // The thumb the diff view and the transcript draw, in proportion down the body.
        let (x, total) = (r.right().saturating_sub(1), total.unwrap_or(0));
        scrollbar(b, x, r.y + 1, height, total, *scroll);
    }
    /// The first output row inside an opened body, in the same screen-row units as scrolling.
    pub(super) fn output_start(&self, columns: usize) -> usize {
        4 + (self
            .detail_input(columns, columns < usize::from(PHONE))
            .len())
        .max(1)
    }
    // A completed native read: its line range, and where in the file, as a ten-cell tape.
    fn read_row(&self, columns: usize) -> Option<Line<'static>> {
        let read = self.decoded?;
        let first = read.first?;
        let last = first
            .saturating_add(read.count as u64)
            .checked_sub(1)
            .filter(|_| read.count > 0)?;
        let figure = match read.complete {
            true => format!("{first}–{last} of {last}"),
            false => format!("{first}–{last}"),
        };
        let fig_start = columns.saturating_sub(11).saturating_sub(width(&figure));
        let gauge_at = columns.saturating_sub(36);
        let target = clip(&target("read", self.input().1), gauge_at.saturating_sub(15));
        let (green, end) = (rgb(0x22773c), 13 + width(&target));
        let mut spans = vec![
            span("┼", green),
            Span::styled("     read   ", bold(DATA, GLASS)),
            span(target, rgb(0xc849ae)),
            span(dots(end, gauge_at), rgb(0x3d464f)),
        ];
        for i in 0..10 {
            let dim = (i as f64) < (first as f64 / last as f64 * 10.).round();
            let glyph = match (read.complete, dim) {
                (false, _) => " ",
                (true, true) => "╌",
                (true, false) => "━",
            };
            spans.push(span(glyph, if dim { GRAY_DIM } else { green }));
        }
        spans.push(Span::raw(
            " ".repeat(fig_start.saturating_sub(gauge_at + 10)),
        ));
        spans.push(span(figure, rgb(0xc2cad0)));
        Some(Line::from(spans))
    }
}

// The first time after `now` a monotone height changes before `end`; never when it ends unchanged.
fn change(now: u64, end: u64, height: impl Fn(u64) -> usize) -> u64 {
    let current = height(now);
    if height(end) == current {
        return u64::MAX;
    }
    let (mut lo, mut hi) = (now.saturating_add(1), end);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if height(mid) == current {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

// The leader between a target ending at `end` and a figure at `to`: a dot on even columns.
fn dots(end: usize, to: usize) -> String {
    (end..to)
        .map(|i| {
            if i > end && i + 1 < to && i % 2 == 0 {
                '·'
            } else {
                ' '
            }
        })
        .collect()
}

/// What a job's end says of its call.
pub(super) fn job_outcome(end: JobEnd) -> Outcome {
    match end {
        JobEnd::Exit(0) => Outcome::Success,
        JobEnd::Unknown => Outcome::Unknown,
        _ => Outcome::Error,
    }
}

/// A call's mark and what it is aimed at, as a child's card and its history say them.
pub(super) fn glance(entry: &Entry) -> Option<(&'static str, String)> {
    let Body::Call { name, input, .. } = &entry.body else {
        return None;
    };
    let kind = kind(name);
    let mark = match kind {
        "read" => "≡",
        "edit" | "write" => "±",
        "shell" => "$",
        "search" => "⌕",
        "web" => "◎",
        "agent" if input["action"] == "wait" => "◌",
        "agent" => "✦",
        _ => "·",
    };
    let target = match kind {
        "shell" => command(kind, input).unwrap_or("shell").to_owned(),
        "search" => "search".into(),
        "message" | "ask" => "message".into(),
        _ => target(kind, input),
    };
    Some((mark, target))
}

/// A call's kind and target as one label (a shell's target already says `bash`).
pub(super) fn label(name: &str, input: &Value) -> String {
    match kind(name) {
        "shell" => target("shell", input),
        kind => format!("{kind} {}", target(kind, input)),
    }
}

/// Razor's, Claude Code's and Codex's names fold to one tool kind.
pub(super) fn kind(name: &str) -> &str {
    let name = name.strip_prefix("mcp__razor__").unwrap_or(name);
    match name.to_ascii_lowercase().as_str() {
        "shell" | "bash" | "exec_command" => "shell",
        "read" => "read",
        "write" => "write",
        "edit" | "multiedit" => "edit",
        "grep" | "glob" => "search",
        "search" | "fetch" | "answer" | "websearch" | "webfetch" => "web",
        _ => name,
    }
}

/// A shell call's command as written, by its `kind`: `command`, or Codex's `cmd`.
pub(super) fn command<'a>(kind: &str, input: &'a Value) -> Option<&'a str> {
    let command = input["command"].as_str().or(input["cmd"].as_str());
    command.filter(|_| kind == "shell")
}

// The word ending a heredoc (`<<EOF`, `<<-'EOF'`), excluding `<<<` strings.
fn heredoc(line: &str) -> Option<String> {
    let rest = &line[line.find("<<")? + 2..];
    let rest = rest.strip_prefix('-').unwrap_or(rest).trim_start();
    let word = rest.trim_start_matches(['\'', '"', '\\']);
    let word: String = (word.chars())
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!rest.starts_with('<') && !word.is_empty()).then_some(word)
}

// What a call is aimed at, in words: its first naming field's first line, never its JSON.
fn target(kind: &str, input: &Value) -> String {
    let line = |k: &str| {
        input[k]
            .as_str()
            .and_then(|s| s.lines().find(|l| !l.trim().is_empty()))
    };
    let names = [
        "description",
        "path",
        "file_path",
        "command",
        "cmd",
        "name",
        "child",
        "query",
    ];
    let more = [
        "url", "note", "line", "task_id", "shell_id", "text", "action", "type",
    ];
    let job = input["job"].as_u64().map(|n| format!("job {n}"));
    let text = if kind == "deliver" {
        line("line").unwrap_or("").into()
    } else if kind == "message" {
        let words = [line("to"), line("text")].into_iter().flatten();
        words.collect::<Vec<_>>().join(" · ")
    } else if kind == "search"
        && let Some(p) = line("pattern")
    {
        line("path").map_or(p.into(), |at| format!("{p} in {at}"))
    } else if let Some(name) = names.into_iter().find_map(line) {
        name.into()
    } else if let Some(job) = job.or(line("job").map(str::to_owned)) {
        job
    } else if let Some(id) = input["id"].as_u64() {
        format!("#{id}")
    } else if let Some(children) = input["children"].as_array() {
        let names = children.iter().filter_map(Value::as_str);
        names.collect::<Vec<_>>().join(", ")
    } else {
        more.into_iter().find_map(line).unwrap_or("").into()
    };
    match kind {
        "shell" => format!("bash {text}"),
        _ => text,
    }
}
