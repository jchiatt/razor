//! The opened card's window onto a job's saved output file, or `/startup`'s onto the session's
//! `startup.md`, read in 64 KiB byte ranges: a viewport and a byte checkpoint, never the whole
//! file or a line index.
use super::{App, Error, api::Api};
use crate::protocol::{EntryId, JobId, SessionId};
use std::collections::VecDeque;

/// `/startup`'s opened view and its window, which no record entry or job is: both count from 1.
pub(super) const STARTUP: (EntryId, JobId) = (EntryId(0), JobId(0));

// The retained window and one incoming window share 8 MiB beside one 64 KiB body.
const BYTES: usize = (super::api::BYTES - super::api::CHUNK as usize) / 2;

pub(super) struct Window {
    pub(super) job: JobId,
    pub(super) first: usize,
    pub(super) lines: VecDeque<String>,
    /// Displayed rows once a scan reached its end: source lines wide, wrapped rows on phone.
    pub(super) total: Option<usize>,
    // The closed tool row counts physical lines, not wrapped screen rows.
    physical: usize,
    physical_total: Option<usize>,
    skipping: bool,
    offset: u64,
    next: usize,
    columns: u16,
    pub(super) loading: bool,
    pub(super) failed: bool,
    finalized: bool,
}
impl Window {
    fn new(job: JobId, columns: u16) -> Self {
        Self {
            job,
            first: 0,
            lines: VecDeque::new(),
            total: None,
            physical: 0,
            physical_total: None,
            skipping: false,
            offset: 0,
            next: 0,
            columns,
            loading: false,
            failed: false,
            finalized: false,
        }
    }
    /// Rows wrap at the width on a phone, and always in `/startup`'s view, which is prose.
    fn phone(&self) -> bool {
        self.columns.saturating_add(14) < super::view::PHONE || self.job == STARTUP.1
    }
    fn covers(&self, first: usize, height: usize) -> bool {
        let last = first.saturating_add(height);
        let last = self.total.map_or(last, |n| last.min(n));
        first >= self.first && last <= self.first + self.lines.len()
    }
}

/// Reads what the focused opened card shows and its window lacks; one request at a time.
pub(super) fn read_visible(app: &mut App) {
    let focus = app.focus.clone();
    for (id, t) in &mut app.transcripts {
        if Some(id) != focus.as_ref() || t.opened.is_none() {
            t.output = None;
        }
    }
    let Some(id) = focus.filter(|_| app.pending < 8 && !app.output_loading) else {
        return;
    };
    let Some(t) = app.transcripts.get_mut(&id) else {
        return;
    };
    let Some((job, total)) = t.opened_job() else {
        t.output = None;
        return;
    };
    let columns = t.detail_columns;
    if t.output
        .as_ref()
        .is_none_or(|v| v.job != job || v.columns != columns)
    {
        t.output = Some(Window::new(job, columns));
    }
    let start = match t.opened.and_then(|id| t.entries.get(&id)) {
        Some(e) => t.call(e, None).output_start(usize::from(columns) + 14),
        // `/startup`'s view has no card head above its lines.
        None if job == STARTUP.1 => 0,
        None => 5,
    };
    let (first, height, finished) = (
        t.detail_scroll.saturating_sub(start),
        t.detail_height as usize,
        t.finished(job),
    );
    let Some(window) = t.output.as_mut() else {
        return;
    };
    let count = if window.phone() { window.total } else { total };
    let target = count.map_or(first, |n| {
        first.min((n + start + 1).saturating_sub(height).saturating_sub(start))
    });
    if window.loading || window.failed || window.covers(target, height) {
        return;
    }
    window.loading = true;
    let mut from = Window::new(job, columns);
    if target >= window.next || window.skipping && target >= window.first {
        (from.offset, from.next, from.physical, from.skipping) =
            (window.offset, window.next, window.physical, window.skipping);
        from.first = target.min(window.next);
        from.lines = window
            .lines
            .iter()
            .skip(target.saturating_sub(window.first))
            .cloned()
            .collect();
    }
    from.finalized = finished;
    from.physical_total = total;
    from.total = if from.phone() { window.total } else { total };
    app.output_loading = true;
    app.spawn(|api| async move {
        let scanned = scan(&api, &id, from, target, height).await;
        move |app: &mut App| apply(app, id, job, scanned)
    });
}

fn apply(app: &mut App, id: SessionId, job: JobId, scanned: Result<Window, Error>) {
    app.output_loading = false;
    let Some(t) = app.transcripts.get_mut(&id) else {
        return;
    };
    if t.opened_job().map(|(id, _)| id) != Some(job) {
        return;
    }
    match scanned {
        // A job that ended during the read is read again for its final count.
        Ok(window) if t.finished(job) && !window.finalized => t.output = None,
        Ok(window) => {
            if let (true, Some(total)) = (window.finalized, window.physical_total) {
                t.measured(job, total);
            }
            t.output = Some(window);
            t.invalidate();
        }
        Err(e) => {
            if let Some(window) = &mut t.output {
                (window.loading, window.failed) = (false, true);
            }
            app.say(&e.to_string());
        }
    }
}

// A checkpoint can sit inside a cut line: its rows are already emitted, and skipping resumes
// at that byte until the newline. Backwards paging outside the window restarts at byte zero.
async fn scan(
    api: &Api,
    session: &SessionId,
    mut window: Window,
    target: usize,
    height: usize,
) -> Result<Window, Error> {
    let limit = usize::from(window.columns).max(1) * 4 * if window.phone() { 64 } else { 1 };
    let (mut prefix, mut retained) = (
        Vec::new(),
        window.lines.iter().map(String::len).sum::<usize>(),
    );
    loop {
        let file = match window.job == STARTUP.1 {
            true => "startup.md".into(),
            false => format!("job-{}", window.job.0),
        };
        let (bytes, size) = api.chunk(session, &file, window.offset).await?;
        let base = window.offset;
        for (i, byte) in bytes.iter().enumerate() {
            if window.skipping {
                if *byte == b'\n' {
                    window.skipping = false;
                }
                continue;
            }
            if *byte != b'\n' {
                prefix.push(*byte);
                if prefix.len() < limit {
                    continue;
                }
                window.skipping = true;
            }
            push(&mut window, &mut prefix, &mut retained, height);
            window.offset = base + i as u64 + 1;
            if window.offset < size
                && (window.skipping && window.next > target
                    || window.next >= target.saturating_add(height).saturating_add(80))
            {
                return Ok(window);
            }
        }
        window.offset = base + bytes.len() as u64;
        if window.offset >= size {
            if !prefix.is_empty() {
                push(&mut window, &mut prefix, &mut retained, height);
            }
            window.total = Some(window.next);
            window.physical_total = Some(window.physical);
            return Ok(window);
        }
    }
}

fn push(window: &mut Window, prefix: &mut Vec<u8>, retained: &mut usize, height: usize) {
    // No replacement character where a width-bounded prefix ends inside a valid scalar.
    if window.skipping
        && let Err(e) = std::str::from_utf8(prefix)
        && e.error_len().is_none()
    {
        prefix.truncate(e.valid_up_to());
    }
    let text = String::from_utf8_lossy(prefix)
        .trim_end_matches('\r')
        .to_owned();
    let first = window.next;
    if window.phone() {
        let columns = usize::from(window.columns) + 1;
        super::text::visit_wrap(
            &[ratatui::text::Span::raw(&text)],
            columns,
            |parts, _, last| {
                let mut row: String = parts.iter().map(|part| part.text).collect();
                let cut = window.next - first == 63;
                if cut && !last || last && window.skipping {
                    row.push('…');
                    row = super::text::clip(&row, columns);
                }
                *retained += row.len();
                window.lines.push_back(row);
                window.next += 1;
                !cut
            },
        );
    } else {
        *retained += text.len();
        window.lines.push_back(text);
        window.next += 1;
    }
    window.physical += 1;
    while window.lines.len() > height.saturating_add(160) || *retained > BYTES {
        if let Some(old) = window.lines.pop_front() {
            *retained -= old.len();
        }
    }
    window.first = window.next - window.lines.len();
    prefix.clear();
}
