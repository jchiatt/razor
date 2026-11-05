//! The session map: while the transcript holds only part of the session or overflows, its last
//! column maps the whole session's entries, each cell the strongest mark of its stretch.
use super::{Action, Transcript};
use crate::protocol::{Body, End, Entry, EntryId, Outcome, SessionId};
use crate::terminal::{App, text::*};
use ratatui::{buffer::Buffer, layout::Rect, style::Color};
use reqwest::Method;
use std::collections::{BTreeMap, BTreeSet};

/// What a cell can show, weakest first, its glyph and tone; the route names each in snake case.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum Mark {
    Failed,
    You,
    Question,
    Rebuild,
    Error,
}
const GLYPHS: [&str; 5] = ["✗", "━", "?", "═", "✗"];
const TONES: [Color; 5] = [AMBER, CYAN, CYAN, AMBER, RED];

/// The newest 4,096 marks of a transcript on screen: seeded from the session's marks route
/// and folded live by the route's own rules. Ids before `from` were never read; no `from`
/// until the route answers.
#[derive(Default)]
pub(in crate::terminal) struct Marks {
    marks: BTreeMap<EntryId, Mark>,
    from: Option<u64>,
    // A child's asks go to its parent, not the person.
    child: bool,
}

impl Marks {
    /// Folds `entry` as `src/api/marks.rs` reads the record: a person's input, a question, a
    /// rebuild, a turn that ended in an error, a failed result; a withdrawal forgets its inputs.
    pub(super) fn saw(&mut self, entry: &Entry) {
        let mark = match &entry.body {
            Body::Input(input) if input.from.is_person() => Mark::You,
            Body::Withdrawn { inputs } => {
                return self.marks.retain(|id, _| !inputs.contains(id));
            }
            Body::Question { .. } => Mark::Question,
            Body::Rebuild { .. } => Mark::Rebuild,
            Body::TurnEnd {
                end: End::Error(_), ..
            } => Mark::Error,
            Body::Call { name, input, .. }
                if !self.child && crate::record::ask(name, input).is_some() =>
            {
                Mark::Question
            }
            Body::Result { outcome, .. } if *outcome == Outcome::Error => Mark::Failed,
            _ => return,
        };
        self.put(entry.id, mark);
    }

    fn put(&mut self, id: EntryId, mark: Mark) {
        self.marks.insert(id, mark);
        if self.marks.len() > 4096
            && let Some((first, _)) = self.marks.pop_first()
        {
            self.from = self.from.map(|from| from.max(first.0 + 1));
        }
    }
}

/// Asks the session's marks route for `id`'s transcript, once while it is on screen at 120
/// columns or more; until the route answers, or when it fails, the scrollbar stays.
pub(in crate::terminal) fn show(app: &mut App, id: &SessionId) {
    let child = app.agents.get(id).is_some_and(|a| a.parent.is_some());
    let wanted = app.columns >= 120 && app.pending < 8;
    match app.transcripts.get_mut(id) {
        Some(t) if wanted && t.marks.is_none() => t.marks.insert(Marks::default()).child = child,
        _ => return,
    }
    let id = id.clone();
    app.spawn(|api| async move {
        let path = format!("v1/sessions/{id}/marks");
        let read = api.json(Method::GET, &path, None).await;
        let read = read.ok().and_then(|v| serde_json::from_value(v).ok());
        move |app: &mut App| {
            if let (Some(t), Some(read)) = (app.transcripts.get_mut(&id), read)
                && let Some(marks) = &mut t.marks
            {
                seed(marks, read, &t.withdrawn);
            }
        }
    });
}

// The route's newest 4,096 marks, oldest first: when it returned all 4,096, older ids are unread.
fn seed(marks: &mut Marks, read: Vec<(EntryId, Mark)>, withdrawn: &BTreeSet<EntryId>) {
    let full = read.len() >= 4096;
    marks.from = Some(if full { read[0].0.0 } else { 0 });
    for (id, mark) in read.into_iter().filter(|(id, _)| !withdrawn.contains(id)) {
        marks.put(id, mark);
    }
}

impl Transcript {
    /// Once seeded, while the session is not all on screen and no card is open: draws the map
    /// into `r`'s last column, over the scrollbar. Cell k of h covers entries from k·n/h to
    /// (k+1)·n/h of the newest n; the entries on screen sit on the bezel's light ground, unread
    /// ones dotted. A click jumps to a cell's strongest mark, or to its first entry.
    pub(in crate::terminal) fn map(&self, b: &mut Buffer, r: Rect, hits: &mut Vec<(Action, Rect)>) {
        let whole = self.first().is_none_or(|id| id.0 <= 1) && self.total <= r.height as usize;
        let shown = !whole && self.opened.is_none();
        let Some((m, Some(from))) = self.marks.as_ref().filter(|_| shown).map(|m| (m, m.from))
        else {
            return;
        };
        let (n, h, x) = (self.cursor.0, r.height as u64, r.right().saturating_sub(1));
        let rows = self.first_row..self.first_row + self.height;
        for k in 0..h {
            let ids = EntryId(1 + k * n / h)..EntryId(1 + (k + 1) * n / h);
            if ids.is_empty() {
                continue;
            }
            let strongest = m.marks.range(ids.clone()).max_by_key(|(_, mark)| **mark);
            let (glyph, fg) = match strongest {
                Some((_, mark)) => (GLYPHS[*mark as usize], TONES[*mark as usize]),
                None if ids.start.0 < from => ("┊", GRAY_DIM),
                None => (" ", GRAY_DIM),
            };
            let mut counts = self.counts.range(ids.clone());
            let lit = counts.any(|(_, (first, n))| *first < rows.end && first + n > rows.start);
            let (y, to) = (r.y + k as u16, strongest.map_or(ids.start, |(id, _)| *id));
            let ground = [GLASS, BEZEL_HI][usize::from(lit)];
            put(b, x, y, glyph, 1, style(fg, ground));
            hits.push((Action::Jump(to), Rect::new(x, y, 1, 1)));
        }
    }
}
