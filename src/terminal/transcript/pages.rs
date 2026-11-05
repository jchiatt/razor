//! Paging the record into the transcript: the page the reader reached, read beside the loop,
//! and the reader's place kept while it folds.
use super::{Direction, ITEMS, MOUNT, Transcript};
use crate::protocol::{Body, End, Entry, EntryId, Event};
use crate::terminal::{App, Error, api, api::Page, error, fail};

impl Transcript {
    // The page the reader has reached the edge of, if one is wanted: older entries before the
    // first retained one, newer ones after the last, or the newest page again.
    pub(super) fn page_edge(&self) -> Option<(Direction, Option<EntryId>)> {
        if self.loading || self.opened.is_some() {
            return None;
        }
        let at = match self.wanted? {
            Direction::Older if self.first_row == 0 && self.mounted() >= self.entries.len() => {
                Some(self.first().filter(|id| id.0 > 1)?)
            }
            Direction::Newer if self.first_row + self.height >= self.total => Some(
                *self
                    .entries
                    .last_key_value()
                    .filter(|(id, _)| **id < self.cursor)?
                    .0,
            ),
            Direction::Newest | Direction::At(_) => None,
            _ => return None,
        };
        Some((self.wanted?, at))
    }

    // Before a page folds: the held view keeps the reader's place across it, even when admitting
    // an older page evicts this window's newest end.
    pub(in crate::terminal) fn prepare_page(&mut self, direction: Direction) {
        if matches!(direction, Direction::Newest | Direction::At(_)) {
            self.retain_older = false;
            while !self.entries.is_empty() {
                self.evict();
            }
            (self.held, self.wanted, self.mounted) = (None, None, MOUNT);
            return;
        }
        self.retain_older = direction == Direction::Older;
        self.mounted = (self.mounted() + 256).min(ITEMS);
        self.children_at = None;
        self.dirty = true;
    }

    /// After a page folds: a page of only metadata reads further back; at the newest end the
    /// window keeps its newest entries again.
    pub(in crate::terminal) fn read_past_metadata(&mut self) {
        let content = |e: &Entry| match &e.body {
            Body::Input(_) | Body::Text { .. } | Body::Call { .. } | Body::Rebuild { .. } => true,
            Body::TurnEnd { end, .. } => matches!(end, End::Error(_)),
            _ => false,
        };
        if !self.entries.values().any(content) {
            self.wanted = Some(Direction::Older);
        } else if self.wanted != Some(Direction::Older) && self.newest() {
            self.retain_older = false;
        }
    }
}

/// Reads the history page the focused transcript's reader has reached, if any.
pub(in crate::terminal) fn read_visible(app: &mut App) {
    let Some(id) = app.focus.clone() else { return };
    let Some(t) = app.transcripts.get_mut(&id) else {
        return;
    };
    let Some((direction, at)) = t.page_edge() else {
        return;
    };
    if app.pending >= 8 {
        return;
    }
    t.loading = true;
    app.spawn(|api| async move {
        let read = async {
            match direction {
                Direction::Older => api.page(&id, at.map(|id| id.0), 256).await,
                Direction::Newer => newer(&api, &id, at.unwrap_or_default()).await,
                Direction::Newest => api.page(&id, None, 256).await,
                Direction::At(at) => api.page(&id, Some(at.0 + 1), 1).await,
            }
        };
        let timeout = "Reading transcript history timed out; try its page again.";
        let page = tokio::time::timeout(std::time::Duration::from_secs(30), read).await;
        let page = page.map_err(fail(timeout)).and_then(|page| page);
        move |app: &mut App| paged(app, id, direction, page)
    });
}

// Entry ids are record ordinals, so the before/limit route walks forward too: narrow the end
// whenever its byte bound skipped a large next entry.
async fn newer(
    api: &api::Api,
    id: &crate::protocol::SessionId,
    after: EntryId,
) -> Result<Page, Error> {
    let mut before = after.0.saturating_add(257);
    loop {
        let mut page = api.page(id, Some(before), 256).await?;
        let ids = page.events.iter().filter_map(|event| match event {
            Event::Entry(entry) => Some(entry.id.0),
            _ => None,
        });
        match ids.min().filter(|first| *first > after.0.saturating_add(1)) {
            Some(first) if first >= before => {
                return Err(error(
                    "The history page did not move; reconnect with a matching server.",
                ));
            }
            Some(first) => before = first,
            None => {
                page.events
                    .retain(|event| matches!(event, Event::Entry(entry) if entry.id > after));
                return Ok(page);
            }
        }
    }
}

fn paged(
    app: &mut App,
    id: crate::protocol::SessionId,
    direction: Direction,
    page: Result<Page, Error>,
) {
    let Some(t) = app.transcripts.get_mut(&id) else {
        return;
    };
    t.loading = false;
    if app.focus.as_ref() != Some(&id) {
        return;
    }
    let page = match page {
        Ok(page) if !page.events.is_empty() => page,
        failed => {
            t.wanted = None;
            return failed.err().iter().for_each(|e| app.say(&e.to_string()));
        }
    };
    t.prepare_page(direction);
    for event in page.events {
        app.fold(Some(id.clone()), event, crate::protocol::now());
    }
    if let Some(t) = app.transcripts.get_mut(&id) {
        t.read_past_metadata();
        if let Direction::At(at) = direction {
            (t.wanted, t.held) = (Some(Direction::Older), Some((at, 0)));
        }
    }
}

/// Jumps `session`'s transcript to entry `id`: a retained one is mounted and held at once;
/// otherwise the entry is read alone into the window's place, the addressed transcript only.
pub(in crate::terminal) fn jump(app: &mut App, session: &crate::protocol::SessionId, id: EntryId) {
    let Some(t) = app.transcripts.get_mut(session) else {
        return;
    };
    match t.entries.contains_key(&id) {
        true => {
            let after = t.entries.range(id..).count();
            t.mounted = t.mounted().max(after).min(ITEMS);
            (t.held, t.dirty) = (Some((id, 0)), true);
        }
        false => t.wanted = Some(Direction::At(id)),
    }
}
