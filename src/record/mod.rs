//! A session's append-only JSONL record: one read at open, then indexed reads and a fold.

mod cursor;
mod rebuild;
mod runs;
mod search;
mod summary;
mod text;
pub(crate) use cursor::Summaries;
pub use search::backfill;
pub(crate) use search::search;
pub(crate) use summary::{ChildSummary, Meeting, MeetingState, Summary, ask, preview};
pub(crate) use text::{call_line, calls_left_out, handover, heard, interrupted, length};

use crate::protocol::{Body, Child, Entry, EntryId, JobEvent, Native, SessionId, Thread, now};
use std::{
    fmt,
    io::{BufRead as _, ErrorKind},
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader, Take},
};

/// A result whose action's outcome is not known after a restart.
pub(crate) const UNKNOWN: &str = include_str!("text/unknown.md");

/// What a model on Razor's own loop reads for a call the person's stop cut off.
pub(crate) const STOPPED: &str = include_str!("text/stopped.md");

/// What `recall` answers for a thinking entry, in any session (RULES.md 16).
pub(crate) const REASONING: &str = include_str!("../controller/session/reasoning.md");

/// The one separator between a turn's closing Text blocks, also used when bounding reports.
pub(crate) const TEXT_BREAK: &str = "\n\n";

/// Joins a turn's closing Text blocks identically in print output and a child's report.
pub fn closing_text(texts: &[String]) -> String {
    texts.join(TEXT_BREAK)
}

/// One session task owns the writer; readers use fixed byte ranges of the same file.
pub(crate) struct Record {
    /// The last entry's time at open, before recovery can append anything.
    pub(crate) last_activity: crate::protocol::Millis,
    dir: PathBuf,
    marker: PathBuf,
    /// Whether the marker exists; after a crash it may be stale until `mark` settles it.
    marked: bool,
    file: File,
    /// A failed append may have left bytes after the last indexed boundary.
    failed: bool,
    /// Entry n lies between offsets[n - 1] and offsets[n]: eight logical bytes per entry
    /// plus one boundary, the index's inherent lifetime cost. Entry bodies are not retained.
    offsets: Vec<u64>,
    summary: Summary,
}

impl Record {
    /// Create a record holding Start; an existing directory returns Exists.
    #[allow(clippy::too_many_arguments)] // Each is one field of Start, written once here.
    pub(crate) async fn create(
        home: &Path,
        session: &SessionId,
        cwd: PathBuf,
        model: String,
        settings: crate::protocol::ModelSettings,
        profile_version: String,
        name: String,
        child: Option<Child>,
        thread: Option<Thread>,
    ) -> Result<Self, Error> {
        let dir = home.join("sessions").join(session.as_str());
        fs::create_dir_all(home.join("sessions"))
            .await
            .map_err(io("making the sessions directory"))?;
        fs::create_dir_all(home.join("live"))
            .await
            .map_err(io("making the live directory"))?;
        match fs::create_dir(&dir).await {
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                return Err(Error::Exists(session.clone()));
            }
            result => result.map_err(io("making the session's directory"))?,
        }
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(dir.join("record.jsonl"))
            .await
            .map_err(io("creating the record"))?;
        let mut record = Self {
            last_activity: 0,
            dir,
            marker: home.join("live").join(session.as_str()),
            marked: false,
            file,
            failed: false,
            offsets: vec![0],
            summary: Summary::new(session, &cwd, &model, settings),
        };
        record
            .append(
                Body::Start {
                    cwd,
                    model,
                    settings,
                    profile_version,
                    name,
                    child,
                    thread: thread.map(Box::new),
                },
                None,
            )
            .await?;
        record.last_activity = record.summary.at;
        Ok(record)
    }

    /// Fold complete lines once; cut a torn tail, but refuse malformed or misplaced entries.
    pub(crate) async fn open(home: &Path, session: &SessionId) -> Result<Self, Error> {
        let dir = home.join("sessions").join(session.as_str());
        let path = dir.join("record.jsonl");
        let file = open(&dir, session).await?;
        let damaged = |line, why| Error::Damaged {
            session: session.clone(),
            line,
            why,
        };
        let reader = BufReader::with_capacity(64 * 1024, file);
        let mut offsets = vec![0];
        let mut summary: Option<Summary> = None;
        cursor::fold(&mut summary, session, reader, |size| {
            offsets.push(offsets[offsets.len() - 1] + size);
        })
        .await?;
        let summary = summary.ok_or_else(|| damaged(1, "the record is empty".into()))?;
        let file = OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .map_err(io("opening the record for appending"))?;
        file.set_len(offsets[offsets.len() - 1])
            .await
            .map_err(io("cutting off a torn last line"))?;
        let marker = home.join("live").join(session.as_str());
        let record = Self {
            last_activity: summary.at,
            dir,
            marked: fs::try_exists(&marker).await.unwrap_or(true),
            marker,
            file,
            failed: false,
            offsets,
            summary,
        };
        Ok(record)
    }

    /// Assign the next id and time, write and flush once, then fold and publish the stored
    /// line without its newline. TurnEnd, Withdrawn and Rebuild are also synced before returning.
    pub(crate) async fn append(
        &mut self,
        body: Body,
        native: Option<Native>,
    ) -> Result<(EntryId, Arc<str>), Error> {
        if self.failed {
            self.file
                .set_len(self.offsets[self.offsets.len() - 1])
                .await
                .map_err(io("cutting off a failed append"))?;
        }
        let remote_task = matches!(&body, Body::Child { session, event:
            crate::protocol::ChildEvent::Started(_) | crate::protocol::ChildEvent::Continued }
            if session.address().is_some());
        if remote_task
            || matches!(
                body,
                Body::Input(_) | Body::Rebuild { .. } | Body::Job(JobEvent::Started { .. })
            )
        {
            self.mark(true).await?;
        }
        let entry = Entry {
            id: EntryId(self.offsets.len() as u64),
            at: now(),
            body,
            native,
        };
        let mut line = serde_json::to_string(&entry)
            .map_err(|e| io("serializing an entry")(std::io::Error::other(e)))?;
        line.push('\n');
        self.failed = true;
        self.file
            .write_all(line.as_bytes())
            .await
            .map_err(io("appending to the record"))?;
        self.file
            .flush()
            .await
            .map_err(io("appending to the record"))?;
        if matches!(
            entry.body,
            Body::TurnEnd { .. }
                | Body::Withdrawn { .. }
                | Body::Rebuild { .. }
                | Body::Fresh { .. }
        ) {
            self.file
                .sync_data()
                .await
                .map_err(io("syncing the record"))?;
        }
        self.offsets
            .push(self.offsets[self.offsets.len() - 1] + line.len() as u64);
        self.failed = false;
        self.summary.fold(&entry);
        if let Some(line) = search::line(&entry, &self.summary) {
            let (mut options, path) = (OpenOptions::new(), self.dir.join(search::FILE));
            let file = options.create(true).append(true).open(path).await;
            // A failure loses the entry from search only; the record keeps it.
            if let Ok(mut file) = file
                && file.write_all(line.as_bytes()).await.is_ok()
            {
                let _ = file.flush().await;
            }
        }
        line.pop();
        Ok((entry.id, Arc::from(line)))
    }

    /// Keeps the live marker, which startup loads, while work is open or `due` maintenance
    /// remains; the owner calls it after each append and whenever `due` may have changed.
    pub(crate) async fn mark(&mut self, due: bool) -> Result<(), Error> {
        let marked = due || self.summary.live();
        if marked != self.marked {
            let changed = match marked {
                true => fs::write(&self.marker, b"").await,
                false => (fs::remove_file(&self.marker).await)
                    .or_else(|e| (e.kind() == ErrorKind::NotFound).then_some(()).ok_or(e)),
            };
            changed.map_err(io("updating the live marker"))?;
            self.marked = marked;
        }
        Ok(())
    }

    /// The same fold is used at open and append.
    pub(crate) fn summary(&self) -> &Summary {
        &self.summary
    }

    /// The session directory, also holding the shell's spool files.
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// Read one stored line by its indexed boundaries, without reading preceding entries.
    pub(crate) async fn get(&self, id: EntryId) -> Result<String, Error> {
        let index = usize::try_from(id.0)
            .ok()
            .filter(|i| *i > 0 && *i < self.offsets.len())
            .ok_or(Error::NoEntry(id))?;
        let (start, end) = (self.offsets[index - 1], self.offsets[index]);
        let mut file = open(&self.dir, &self.summary.session).await?;
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(io("reading an entry"))?;
        let mut bytes = vec![0; (end - start - 1) as usize];
        file.read_exact(&mut bytes)
            .await
            .map_err(io("reading an entry"))?;
        String::from_utf8(bytes).map_err(|e| self.damaged(id, e.to_string()))
    }

    /// Decode the same indexed read that recall uses.
    pub(crate) async fn entry(&self, id: EntryId) -> Result<Entry, Error> {
        serde_json::from_str(&self.get(id).await?).map_err(|e| self.damaged(id, e.to_string()))
    }

    /// Open stored lines after a cursor, ending at the current boundary; a missing record is Missing at open.
    pub(crate) async fn replay(&self, after: EntryId) -> Result<Replay, Error> {
        self.range(
            (after.0 as usize).min(self.offsets.len() - 1),
            self.offsets.len() - 1,
        )
        .await
    }

    /// Read at most limit entries before before, and at most bytes unless one entry exceeds it.
    pub(crate) async fn page(
        &self,
        before: Option<EntryId>,
        limit: usize,
        bytes: u64,
    ) -> Result<Replay, Error> {
        let end = before
            .map_or(self.offsets.len() - 1, |id| id.0.saturating_sub(1) as usize)
            .min(self.offsets.len() - 1);
        let mut start = end.saturating_sub(limit.max(1));
        while start + 1 < end && self.offsets[end] - self.offsets[start] > bytes {
            start += 1;
        }
        self.range(start, end).await
    }

    async fn range(&self, start: usize, end: usize) -> Result<Replay, Error> {
        let mut file = open(&self.dir, &self.summary.session).await?;
        file.seek(std::io::SeekFrom::Start(self.offsets[start]))
            .await
            .map_err(io("replaying the record"))?;
        Ok(Replay {
            reader: BufReader::with_capacity(
                64 * 1024,
                file.take(self.offsets[end] - self.offsets[start]),
            ),
        })
    }

    fn damaged(&self, id: EntryId, why: String) -> Error {
        Error::Damaged {
            session: self.summary.session.clone(),
            line: id.0,
            why,
        }
    }
}

/// A fixed byte range, streamed on the caller's task with a 64 KiB reader and one entry buffer.
pub(crate) struct Replay {
    reader: BufReader<Take<File>>,
}

impl Replay {
    /// Return the next stored line; later appends are outside this replay's range.
    pub(crate) async fn next_line(&mut self) -> Option<Result<String, Error>> {
        let mut line = String::new();
        match self.reader.read_line(&mut line).await {
            Ok(0) => None,
            Ok(_) => {
                line.pop();
                Some(Ok(line))
            }
            Err(e) => Some(Err(io("replaying the record")(e))),
        }
    }
}

async fn open(dir: &Path, session: &SessionId) -> Result<File, Error> {
    File::open(dir.join("record.jsonl"))
        .await
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => Error::Missing(session.clone()),
            _ => io("opening the record")(e),
        })
}

/// List root sessions newest first, reading at most each one's first line and stopping at limit
/// matches: id, folder, thread, stored name and when its record last changed; with `resident`,
/// only that resident's threads; fresh runs only when `jobs`. The scan is one blocking task, not
/// three pool handoffs per file, and once started runs to its end even if the caller drops it.
pub(crate) async fn list(
    home: &Path,
    cwd: Option<&Path>,
    resident: Option<&SessionId>,
    jobs: bool,
    limit: usize,
) -> Result<Vec<(SessionId, PathBuf, Option<Thread>, String, SystemTime)>, Error> {
    let dir = home.join("sessions");
    let (cwd, resident) = (cwd.map(PathBuf::from), resident.cloned());
    blocking(move || {
        let resident = resident.as_ref();
        let mut ids = sessions_in(&dir)?;
        ids.sort_unstable_by(|a, b| b.cmp(a));
        let mut found = Vec::new();
        for id in ids {
            if found.len() == limit {
                break;
            }
            let Ok(file) = std::fs::File::open(dir.join(id.as_str()).join("record.jsonl")) else {
                continue;
            };
            let changed = (file.metadata()).and_then(|m| m.modified());
            let changed = changed.map_err(io("reading when a record last changed"))?;
            let mut line = Vec::new();
            std::io::BufReader::new(file)
                .read_until(b'\n', &mut line)
                .map_err(io("reading a record's first line"))?;
            if let Ok(Entry {
                body:
                    Body::Start {
                        cwd: started,
                        name,
                        child: None,
                        thread,
                        ..
                    },
                ..
            }) = serde_json::from_slice(&line)
                && cwd.as_deref().is_none_or(|cwd| cwd == started)
                && resident.is_none_or(|r| thread.as_ref().is_some_and(|t| t.resident == *r))
                && (jobs || thread.as_ref().is_none_or(|t| t.job.is_none()))
            {
                found.push((id, started, thread.map(|t| *t), name, changed));
            }
        }
        Ok(found)
    })
    .await?
}

/// Markers may be stale after a crash, never absent for a live session or one with idle
/// maintenance due; the loaded session settles them.
pub(crate) async fn live(home: &Path) -> Result<Vec<SessionId>, Error> {
    let live = home.join("live");
    blocking(move || sessions_in(&live)).await?
}

/// One id per directory entry, no records are loaded.
fn sessions_in(dir: &Path) -> Result<Vec<SessionId>, Error> {
    let entries = match std::fs::read_dir(dir) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        result => result.map_err(io("listing sessions"))?,
    };
    let mut ids = Vec::new();
    for entry in entries {
        let name = entry.map_err(io("listing sessions"))?.file_name();
        if let Ok(Ok(id)) = name.into_string().map(SessionId::try_from) {
            ids.push(id);
        }
    }
    Ok(ids)
}

/// Run a scan of many small file calls on one blocking thread, not one pool handoff per call.
/// A failed thread is the outer Error; the scan's own result is the inner one.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, Error> {
    let done = tokio::task::spawn_blocking(f).await;
    done.map_err(|e| io("scanning the sessions")(std::io::Error::other(e)))
}

fn io(doing: &'static str) -> impl FnOnce(std::io::Error) -> Error {
    move |source| Error::Io { doing, source }
}

/// Causes a caller can act on, or the underlying failure with its context.
#[derive(Debug)]
pub(crate) enum Error {
    Missing(SessionId),
    Exists(SessionId),
    Damaged {
        session: SessionId,
        line: u64,
        why: String,
    },
    NoEntry(EntryId),
    Io {
        doing: &'static str,
        source: std::io::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(id) => write!(f, "There is no session {id}."),
            Self::Exists(id) => write!(f, "A session {id} exists already."),
            Self::Damaged { session, line, why } => write!(
                f,
                "The record of session {session} is damaged at line {line} ({why}); it was not opened."
            ),
            Self::NoEntry(id) => write!(f, "The record has no entry {}.", id.0),
            Self::Io { doing, source } => write!(f, "Razor failed {doing}: {source}."),
        }
    }
}
impl std::error::Error for Error {}
