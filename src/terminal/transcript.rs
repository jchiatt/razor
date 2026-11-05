//! A session's transcript: a bounded window of its entries, laid out at the screen's width with
//! only the viewport's rows owning strings, paged from the API in both directions.
mod entries;
pub(super) mod map;
mod pages;
mod question;
mod rows;
pub(super) mod select;

use super::{markdown, motion, text::*, tools, view};
use crate::protocol::{Body, CallId, End, Entry, EntryId, JobId, SessionId};
pub(super) use pages::{jump, read_visible};
use ratatui::{buffer::Buffer, layout::Rect, style::Color, text::Line};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

/// Entries kept across every transcript, and the window mounted first and widened by paging.
pub(super) const ITEMS: usize = 2000;
pub(super) const MOUNT: usize = 240;

#[derive(Clone, Copy)]
pub(super) enum Action {
    /// A voice note's waveform: plays it.
    Play(EntryId),
    Toggle(EntryId),
    Copy(EntryId),
    Jump(EntryId),
    Pin(EntryId),
    /// The newer entries' line: goes to the latest, as Ctrl-End does.
    Latest,
    /// A worn handed skill's line under the input that handed it.
    HandBack(EntryId),
    /// A folded run's `looked` row: draws the run's calls again.
    Unfold(EntryId),
}
#[derive(Clone, Default)]
pub(super) struct Row {
    pub(super) links: Vec<markdown::Link>,
    pub(super) words: Line<'static>,
    pub(super) bg: Color,
    pub(super) action: Option<Action>,
    /// Columns of decoration before the row's words: the margin, a rail, a label.
    pub(super) lead: u16,
    pub(super) kind: markdown::Kind,
    /// A blinking cursor drawn past the words, which no copy takes.
    pub(super) cursor: bool,
}
impl Row {
    pub(super) fn frame(mut self) -> Self {
        self.kind = markdown::Kind::Frame;
        self
    }
    /// A row whose words end at a card's right edge, in its last column.
    pub(super) fn boxed(mut self) -> Self {
        let drawn: usize = self.words.spans.iter().map(|s| width(&s.content)).sum();
        self.kind = markdown::Kind::Cells(vec![drawn.saturating_sub(1) as u16]);
        self
    }
    pub(super) fn plain(words: impl Into<Line<'static>>) -> Self {
        Self::on(words, GLASS)
    }
    pub(super) fn on(words: impl Into<Line<'static>>, bg: Color) -> Self {
        Self {
            words: words.into(),
            bg,
            lead: 2,
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Direction {
    Older,
    Newer,
    Newest,
    // The page holding this entry alone, in the window's place; paging goes on from there.
    At(EntryId),
}

#[derive(Default)]
pub(super) struct Transcript {
    pub(super) zone: Option<jiff::tz::TimeZone>,
    // The session's context window, for a rebuild card's fill, and its name, which replies sign.
    pub(super) window: Option<u64>,
    pub(super) name: String,
    pub(super) entries: BTreeMap<EntryId, Entry>,
    /// Spoken notes from the app's newest 256 deliveries, keyed by their call entry.
    pub(super) deliveries: BTreeMap<EntryId, crate::protocol::Delivery>,
    pub(super) cursor: EntryId,
    pub(super) partial: String,
    // Streamed words a stopped turn never recorded, by its retained turn end: the only copy
    // there is, counted in that entry's size and evicted with it.
    unsaved: BTreeMap<EntryId, String>,
    pub(super) delta_at: Option<u64>,
    // Where the reader holds the view: `None` follows the newest row; else the entry whose rows
    // hold the top line and that line's row within it, negative while PageUp presses wait for an
    // older page. Only the reader's own moves change it; a width change rescales its row.
    held: Option<(EntryId, isize)>,
    pub(super) opened: Option<EntryId>,
    // Retained replies opened and runs of calls unfolded, by the run's first call.
    expanded: BTreeSet<EntryId>,
    pub(super) detail_scroll: usize,
    pub(super) detail_height: u16,
    pub(super) detail_columns: u16,
    pub(super) output: Option<super::output::Window>,
    pub(super) queued: BTreeSet<EntryId>,
    pub(super) taken: BTreeSet<EntryId>,
    // Queued inputs whose withdrawal is in flight; the band leaves them out.
    pub(super) taking: BTreeSet<EntryId>,
    // The input that last left the queue, for the band's light: when, and whether this
    // terminal took it back (otherwise the model took it).
    pub(super) left: Option<(EntryId, u64, bool)>,
    // The last entry this terminal showed as the person left the session, and when: the recap's
    // fallback for the server's `seen`, which a restart loses; one per transcript.
    pub(super) seen: Option<(EntryId, u64)>,
    // Inputs a `Withdrawn` entry names, at most `ITEMS`: no row draws them.
    pub(super) withdrawn: BTreeSet<EntryId>,
    // How the newest turn to end since the stream attached ended, until the next key: the tab
    // says done after `Done` and needs you after an error.
    pub(super) ended: Option<End>,
    // The stream has attached: until then the card's mood says attaching.
    pub(super) live: bool,
    pub(super) marks: Option<map::Marks>,
    // The reply pinned above the composer and its first 16,384 characters.
    pub(super) pin: Option<(EntryId, String)>,
    // The newest turn end and whether it failed; the failed end the usage limit took, with its
    // reset if given and the minute its words were written for.
    end: Option<(EntryId, bool)>,
    limited: Option<(EntryId, Option<u64>, u64)>,
    // The newest of the person's messages and the model's texts: a message's first 512
    // characters, or none for a text.
    heard: Option<(EntryId, Option<String>)>,
    // At most 128 running jobs' last lines, 1 KiB each.
    tail: BTreeMap<JobId, (String, u64)>,
    // Serialized sizes, measured once; eviction never copies a retained large entry.
    sizes: BTreeMap<EntryId, usize>,
    // Each retained shell call's command lines, counted once for its opened card.
    commands: BTreeMap<EntryId, usize>,
    results: HashMap<CallId, EntryId>,
    decoded: BTreeMap<EntryId, tools::Decoded>,
    // Each retained job's call, end, start, measured line count and recorded failure lines.
    jobs: BTreeMap<JobId, tools::Job>,
    width: usize,
    tick: u64,
    model: String,
    rows: Vec<Row>,
    // Each mounted entry's first row and row count; only the viewport owns drawn strings.
    counts: BTreeMap<EntryId, (usize, usize)>,
    pub(super) retain_older: bool,
    pub(super) wanted: Option<Direction>,
    pub(super) loading: bool,
    total: usize,
    first_row: usize,
    height: usize,
    working: bool,
    motion: bool,
    presence: crate::terminal::motion::Presence,
    // When a drawn row next looks different by itself, and when a mounted entry's row count next
    // changes: the first rebuilds only the viewport's rows, the second counts every row again.
    look: u64,
    size: u64,
    // The open card's deadline leaves the hidden transcript's deadline intact.
    card: u64,
    // `dirty` counts every row again; `recount` keeps the saved counts of entries whose rows
    // cannot have changed (a reply's words, a child card's rows, one job's tail changed).
    dirty: bool,
    recount: bool,
    mounted: usize,
    // The default card of each retained launch and its child's name and model, rebuilt when its
    // key changes.
    pub(super) children: BTreeMap<EntryId, (Vec<Row>, tools::Next, tools::Child)>,
    pub(super) children_at: Option<(u64, u16, u64)>,
    // The working line says Esc stops the turn: not in a split, where Esc goes back.
    stops: String,
    // The session can take an input: neither done nor settled, so a worn skill can go back.
    takes: bool,
    /// Skill names in the order this reading first saw each load, which numbers the lamps.
    pub(super) skills: Vec<String>,
    // The session's open asks, as its status row lists them.
    asks: Vec<EntryId>,
    // The open ask the band shows, when it is this session's: it alone takes the two-row form.
    band: Option<EntryId>,
    // This session's children's names and models by session, as the status rows give them; a
    // report from a child not named says `child`, and mail from one is drawn as other mail.
    names: BTreeMap<SessionId, tools::Child>,
    server: String,
    // Where the rows were last drawn, and the screen row of the first.
    area: Option<(Rect, u16)>,
}

impl Transcript {
    pub(super) fn invalidate(&mut self) {
        self.dirty = true;
    }
    pub(super) fn asks(&mut self, asks: Vec<EntryId>, band: Option<EntryId>) {
        self.dirty |= self.asks != asks || self.band != band;
        (self.asks, self.band) = (asks, band);
    }
    /// The session's rejected usage window (its reset, if given) marks its newest turn end, when
    /// that failed, as the usage limit's; the mark outlives the window.
    pub(super) fn limit(&mut self, rejected: Option<Option<u64>>, now: u64) {
        // Its words count down by the minute until a minute past the reset, then stay.
        let minute = now / 60_000;
        let limited = match (rejected, self.end, self.limited) {
            (Some(at), Some((id, true)), _) => Some((id, at, minute)),
            (_, _, Some((id, Some(at), _))) if now < at + 60_000 => Some((id, Some(at), minute)),
            (_, _, kept) => kept,
        };
        self.dirty |= self.limited != limited;
        self.limited = limited;
    }
    /// The person's newest message, when a turn ended after it with no text since and nothing
    /// is queued: the band's `No reply to your message` line.
    pub(super) fn unanswered(&self) -> Option<&str> {
        let (id, words) = self.heard.as_ref()?;
        let ended = self.end.is_some_and(|(end, _)| end > *id) && self.queued.is_empty();
        words.as_deref().filter(|_| ended)
    }
    pub(super) fn stops(&mut self, stops: &str) {
        self.dirty |= self.stops != stops;
        self.stops = stops.into();
    }
    /// Whether this frame drew its rows over the cell `at`.
    pub(super) fn holds(&self, at: ratatui::layout::Position) -> bool {
        self.area.is_some_and(|(r, _)| r.contains(at))
    }
    /// A frame starts: its rows are nowhere until the frame draws them.
    pub(super) fn undraw(&mut self) {
        self.area = None;
    }
    pub(super) fn takes(&mut self, takes: bool) {
        self.dirty |= self.takes != takes;
        self.takes = takes;
    }
    pub(super) fn names(&mut self, names: BTreeMap<SessionId, tools::Child>, server: &str) {
        self.server = server.into();
        self.dirty |= self.names != names;
        self.names = names;
    }
    /// Refreshes cached rows, recounting only when their number changed.
    pub(super) fn repaint(&mut self, size: bool) {
        self.recount |= size;
        self.look = 0;
    }
    /// Sets the motion switch and the person's presence; a change lays every row out again.
    pub(super) fn motion(&mut self, on: bool, presence: crate::terminal::motion::Presence) {
        self.dirty |= self.motion != on || self.presence != presence;
        (self.motion, self.presence) = (on, presence);
    }
    /// When the rows last drawn next look different by themselves; `u64::MAX` for never.
    pub(super) fn next(&self) -> u64 {
        if self.opened.is_some() {
            self.card
        } else {
            self.look.min(self.size)
        }
    }
    pub(super) fn finished(&self, job: JobId) -> bool {
        self.jobs.get(&job).is_some_and(|state| state.end.is_some())
    }
    /// Keeps a finished job's measured line count; a running job has none yet.
    pub(super) fn measured(&mut self, job: JobId, total: usize) {
        if let Some(state) = self.jobs.get_mut(&job).filter(|state| state.end.is_some()) {
            state.total = Some(total);
        }
    }
    /// The opened view's job and measured count, when it reads a saved file.
    pub(super) fn opened_job(&self) -> Option<(JobId, Option<usize>)> {
        if self.opened == Some(super::output::STARTUP.0) {
            return Some((super::output::STARTUP.1, None));
        }
        let call = self.call(self.entries.get(&self.opened?)?, None);
        call.job
            .filter(|_| call.spooled())
            .map(|j| (j.0, j.1.total))
    }
    pub(super) fn toggle(&mut self, id: EntryId) {
        if self
            .entries
            .get(&id)
            .is_some_and(|e| matches!(e.body, Body::Text { .. } | Body::Thinking { .. }))
        {
            return self.expand([id], !self.expanded.contains(&id));
        }
        self.output = None;
        self.opened = if self.opened == Some(id) {
            None
        } else {
            Some(id)
        };
        self.detail_scroll = 0;
    }
    /// Opens or folds the replies, and the runs of calls by their first, that `ids` names; the
    /// view holds its top line, so what a reply gains goes below.
    pub(super) fn expand(&mut self, ids: impl IntoIterator<Item = EntryId>, open: bool) {
        let bottom = self.total.saturating_sub(self.height) as isize;
        self.held = self.entry_at(self.top().unwrap_or(bottom).clamp(0, bottom));
        for id in ids {
            match open {
                true => self.expanded.insert(id),
                false => self.expanded.remove(&id),
            };
        }
        self.dirty = true;
    }
    /// Ctrl-O: opens every reply and unfolds every run, or folds them all when every one is
    /// open; says which.
    pub(super) fn open_all(&mut self) -> &'static str {
        let ids = (self.entries.values())
            .filter(|e| {
                matches!(
                    e.body,
                    Body::Text { .. } | Body::Thinking { .. } | Body::Call { .. }
                )
            })
            .map(|e| e.id);
        let ids: BTreeSet<_> = ids.collect();
        let all = !ids.is_empty() && ids.is_subset(&self.expanded);
        self.expand(ids, !all);
        if all {
            "everything folded"
        } else {
            "everything open"
        }
    }
    /// Pins reply `id`, or the newest, or unpins it when it is the one pinned.
    pub(super) fn pin(&mut self, id: Option<EntryId>) {
        let mut replies = self.entries.values().rev().filter_map(|e| match &e.body {
            Body::Text { text } | Body::Thinking { text } if id.is_none_or(|id| id == e.id) => {
                Some((e.id, text))
            }
            _ => None,
        });
        let Some((id, text)) = replies.next() else {
            return;
        };
        let text = text.chars().take(16 << 10).collect();
        let same = self.pin.take().is_some_and(|(pinned, _)| pinned == id);
        self.pin = (!same).then_some((id, text));
    }
    pub(super) fn mounted(&self) -> usize {
        self.mounted.max(MOUNT)
    }
    pub(super) fn page_up(&mut self, rows: usize) {
        if self.opened.is_some() {
            self.detail_scroll = self.detail_scroll.saturating_sub(rows);
            return;
        }
        self.wanted = Some(Direction::Older);
        // From the line shown; presses past the first row wait for an older page.
        let bottom = self.total.saturating_sub(self.height) as isize;
        let top = self.top().unwrap_or(bottom).min(bottom);
        self.held = self.entry_at(top.saturating_sub(rows.min(isize::MAX as usize) as isize));
        if self.mounted() < self.entries.len() {
            self.mounted = (self.mounted() + 80).min(self.entries.len()).min(ITEMS);
            self.children_at = None;
            self.dirty = true;
        }
    }
    pub(super) fn page_down(&mut self, rows: usize) {
        if self.opened.is_some() {
            self.detail_scroll = self.detail_scroll.saturating_add(rows);
            return;
        }
        self.wanted = Some(Direction::Newer);
        // From the line shown: reaching the end of a window that holds the record's newest entry
        // follows again; else the view holds at the end until a newer page folds.
        let Some(top) = self.top() else { return };
        let bottom = self.total.saturating_sub(self.height) as isize;
        let rows = rows.min(isize::MAX as usize) as isize;
        let top = top.clamp(0, bottom).saturating_add(rows);
        match top >= bottom && self.newest() {
            true => self.bottom(),
            false => self.held = self.entry_at(top.min(bottom)),
        }
    }
    /// Follows the newest row: at once when the window holds the record's newest entry, else
    /// once the newest page is read. Ctrl-End and every input or answer the person sends.
    pub(super) fn bottom(&mut self) {
        (self.held, self.mounted, self.dirty, self.children_at) = (None, MOUNT, true, None);
        match self.newest() {
            true => self.retain_older = false,
            false => self.wanted = Some(Direction::Newest),
        }
    }
    /// Whether the window holds the record's newest entry.
    fn newest(&self) -> bool {
        let last = self.entries.last_key_value();
        last.is_none_or(|(id, _)| *id >= self.cursor)
    }
    // The entry whose rows hold `line` and the line's row within it; before every entry's first
    // row (the earlier items' line, or presses waiting for an older page), the first entry's,
    // negative.
    fn entry_at(&self, line: isize) -> Option<(EntryId, isize)> {
        let rows = self.counts.iter().filter(|(_, (_, count))| *count > 0);
        let above = rows.clone().rev().find(|(_, row)| row.0 as isize <= line);
        let (id, (first, _)) = above.or_else(|| rows.clone().next())?;
        Some((*id, line - *first as isize))
    }
    // The held line's row, as the rows were last counted; an entry gone from the window holds at
    // the next retained one.
    fn top(&self) -> Option<isize> {
        let (id, offset) = self.held?;
        let at = self.counts.range(id..).next().map(|(_, row)| row.0);
        Some((at.unwrap_or(self.total) as isize).saturating_add(offset))
    }
    pub(super) fn running_since(&self) -> Option<u64> {
        self.jobs
            .values()
            .filter(|job| job.end.is_none())
            .map(|job| job.start)
            .min()
    }

    /// The retained result of a call, using the transcript's existing call index.
    pub(super) fn result(&self, entry: &Entry) -> Option<&Entry> {
        match &entry.body {
            Body::Call { call, .. } => self.entries.get(self.results.get(call)?),
            _ => None,
        }
    }

    /// Borrows one retained call and its decoded result; skills include their first rebuild.
    pub(super) fn call<'a>(
        &'a self,
        entry: &'a Entry,
        place: Option<&tools::Place>,
    ) -> tools::Call<'a> {
        let result = self.result(entry);
        let job = match place {
            Some(place) => place.job.and_then(|id| self.jobs.get_key_value(&id)),
            None => self.jobs.iter().find(|(_, state)| state.call == entry.id),
        }
        .map(|(id, state)| (*id, state));
        let worn = matches!(&entry.body, Body::Call { name, .. } if tools::kind(name) == "skill")
            .then(|| {
                self.entries
                    .range(entry.id..)
                    .find_map(|(_, e)| matches!(e.body, Body::Rebuild { .. }).then_some(e.at))
            })
            .flatten();
        let mut place = place.cloned().unwrap_or_default();
        // A run the reader unfolded draws each of its calls.
        place.run = place.run.filter(|run| !self.expanded.contains(&run.0));
        tools::Call {
            place,
            child: match &entry.body {
                // Another agent call naming a child, by name or session, reads as the child's.
                Body::Call { name, input, .. }
                    if tools::kind(name) == "agent" && !tools::launch(entry) =>
                {
                    let c = input["child"].as_str();
                    let of = |(id, n): &(&SessionId, &tools::Child)| {
                        c == Some(id.as_str())
                            || c == Some(&n.0)
                            || c == id.as_str().split_once('@').map(|(native, _)| native)
                    };
                    c.and_then(|_| self.names.iter().find(of))
                        .map(|(_, child)| child)
                }
                _ => self.children.get(&entry.id).map(|c| &c.2),
            },
            entry,
            worn,
            zone: self.zone.as_ref(),
            result,
            decoded: result.and_then(|e| self.decoded.get(&e.id)),
            spool: self
                .output
                .as_ref()
                .filter(|v| job.is_some_and(|j| j.0 == v.job)),
            tail: job.and_then(|j| self.tail.get(&j.0)),
            lines: self.commands.get(&entry.id).copied().unwrap_or(0),
            job,
        }
    }

    /// Lays out at `columns` and returns the viewport's rows: the newest, or from the held line.
    fn layout(&mut self, columns: usize, model: &str, height: usize) {
        // Saved counts hold while no mounted entry's row count is still animating.
        let full = self.dirty || self.width != columns || self.model != model;
        let full = full || self.size != u64::MAX;
        let changed = full || self.recount;
        if changed {
            let (rows, counts) = self.layout_rows(columns, model, 0..0, !full);
            // A new width keeps the held line's words near the top: its row within its entry
            // scales with the entry's rows.
            if self.width != columns
                && let Some((id, row)) = self.top().and_then(|top| self.entry_at(top))
                && let (Some(old), Some(new)) = (self.counts.get(&id), counts.get(&id))
                && row >= 0
            {
                self.held = Some((id, row * new.1 as isize / old.1.max(1) as isize));
            }
            (self.total, self.counts, self.size) = (rows.total, counts, rows.size);
            (self.width, self.model) = (columns, model.to_owned());
        }
        // A held line too near the end to fill the view draws the end and stays held.
        let bottom = self.total.saturating_sub(height);
        let top = self.top().unwrap_or(bottom as isize);
        let first = top.clamp(0, bottom as isize) as usize;
        let end = (first + height).min(self.total);
        let moved = self.tick >= self.look;
        if changed || moved || self.first_row != first || self.height != end - first {
            let rows = self.layout_rows(columns, model, first..end, true).0;
            (self.rows, self.look) = (rows.rows, rows.look);
            (self.first_row, self.height) = (first, end - first);
        }
        (self.dirty, self.recount) = (false, false);
    }

    fn first(&self) -> Option<EntryId> {
        self.entries.keys().next().copied()
    }

    /// Draws the transcript, or the opened card over it; returns the rows' click actions.
    pub(super) fn draw(
        &mut self,
        b: &mut Buffer,
        r: Rect,
        model: &str,
        working: bool,
        now: u64,
        links: &mut Vec<(Rect, Arc<str>)>,
    ) -> Vec<(Action, Rect)> {
        self.detail_height = r.height.saturating_sub(1);
        self.detail_columns = r.width.saturating_sub(14);
        self.area = None;
        if self.opened == Some(super::output::STARTUP.0) {
            self.detail_height = r.height;
            self.card = u64::MAX;
            let window = self.output.as_ref();
            if let Some(end) = window.and_then(|v| v.total) {
                self.detail_scroll = self.detail_scroll.min(end.saturating_sub(r.height.into()));
            }
            // The wrapped rows, inset as an opened card's body is.
            let body = Rect::new(r.x + 7, r.y, r.width.saturating_sub(13), r.height);
            for (n, y) in (0..)
                .map(|i| self.detail_scroll.saturating_add(i))
                .zip(r.y..r.bottom())
            {
                let text = window.and_then(|v| v.lines.get(n.checked_sub(v.first)?));
                let text = Line::raw(text.map_or("", String::as_str));
                line(b, body, y, &text, style(DATA, GLASS));
            }
            return Vec::new();
        }
        if let Some(entry) = self.opened.and_then(|id| self.entries.get(&id)) {
            let mut scroll = self.detail_scroll;
            if let Body::Input(input) = &entry.body {
                self.read(b, r, input, &mut scroll);
                (self.detail_scroll, self.card) = (scroll, u64::MAX);
                return vec![(Action::Toggle(entry.id), Rect::new(r.x, r.y, r.width, 1))];
            }
            // A skill's card, and a worn-off one's line that hands it back.
            let call = self.call(entry, None);
            let skill =
                matches!(&entry.body, Body::Call { name, .. } if tools::kind(name) == "skill");
            let skill = skill && !call.failed();
            let (back, next) = if skill {
                (
                    tools::skill::detail(&call, b, r, &mut scroll, now),
                    tools::skill::next(entry.at, now),
                )
            } else {
                call.detail(b, r, &mut scroll);
                (None, u64::MAX)
            };
            self.card = next;
            self.detail_scroll = scroll;
            let back = back.map(|y| (Action::HandBack(entry.id), Rect::new(r.x, y, r.width, 1)));
            let head = (Action::Toggle(entry.id), Rect::new(r.x, r.y, r.width, 1));
            return back.into_iter().chain([head]).collect();
        }
        self.dirty |= now >= self.size || self.working != working;
        (self.tick, self.working) = (now, working);
        let (wide, height) = (r.width.saturating_sub(2) as usize, r.height as usize);
        // Before the first layout, assume overflow: a long transcript is laid out once.
        let over = self.total == 0 || self.total > height;
        self.layout(wide.saturating_sub(usize::from(over)), model, height);
        if over != (self.total > height) {
            self.layout(wide.saturating_sub(usize::from(!over)), model, height);
        }
        let top = r.y + r.height.saturating_sub(self.rows.len() as u16);
        self.area = Some((r, top));
        // An overflowing transcript gives its last column to the scrollbar and its rows' grounds
        // one column less.
        let column = r.right().saturating_sub(1);
        let over = scrollbar(b, column, r.y, height, self.total, self.first_row);
        let mut hits = Vec::new();
        for (row, y) in self.rows.iter().zip(top..) {
            if row.bg != GLASS {
                let ground = r.width.saturating_sub(4 + u16::from(over));
                fill(b, Rect::new(r.x + 2, y, ground, 1), style(DATA, row.bg));
            }
            line(b, r, y, &row.words, style(DATA, row.bg));
            if row.cursor {
                let x = r.x + row.words.width() as u16;
                let room = r.right().saturating_sub(x);
                put(b, x, y, "▍", room, style(VIOLET, row.bg));
            }
            if row.bg != GLASS {
                fill(b, Rect::new(r.x, y, 2.min(r.width), 1), style(DATA, GLASS));
            }
            for link in &row.links {
                let x =
                    r.x.saturating_add(link.columns.start.min(r.width as usize) as u16);
                let len = (link.columns.end - link.columns.start)
                    .min(r.right().saturating_sub(x) as usize);
                if len > 0 {
                    links.push((Rect::new(x, y, len as u16, 1), link.url.clone()));
                }
            }
            if let Some(action) = row.action {
                hits.push((action, Rect::new(r.x, y, r.width, 1)));
            }
        }
        // An input the model just took fades in from the glass where the record puts it.
        let left = self.left.filter(|l| self.motion && !l.2 && now < l.1 + 320);
        if let Some(&(first, count)) = left.and_then(|l| self.counts.get(&l.0)) {
            let k = 1. - motion::ease_out(motion::progress(now, left.map_or(now, |l| l.1), 320));
            let rows = first.max(self.first_row)..(first + count).min(self.first_row + self.height);
            for y in rows.map(|i| top + (i - self.first_row) as u16) {
                view::tint(b, Rect::new(r.x, y, r.width, 1), GLASS, GLASS, k);
            }
            self.look = self.look.min(now + 1);
        }
        hits
    }
}
