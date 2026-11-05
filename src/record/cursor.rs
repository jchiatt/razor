//! Read-only summary snapshots that resume without retaining an entry-offset index.

use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::Metadata,
    os::unix::fs::MetadataExt,
    sync::Mutex,
};

/// Dormant folds for configured resident homes only. Each home serializes its readers;
/// removing a name drops its fold without retaining a loaded Record or entry index.
#[derive(Default)]
pub(crate) struct Summaries {
    homes: Mutex<BTreeMap<SessionId, Arc<tokio::sync::Mutex<Cursor>>>>,
}

impl Summaries {
    /// Keeps exactly these configured names; no historical names survive config removal.
    pub(crate) fn configure(&self, names: impl Iterator<Item = SessionId>) {
        let names: BTreeSet<_> = names.collect();
        // A panic leaves the map whole; each entry is independent.
        let mut homes = self.homes.lock().unwrap_or_else(|p| p.into_inner());
        homes.retain(|id, _| names.contains(id));
        for id in names {
            homes.entry(id).or_default();
        }
    }

    /// The writer's last fold replaces its dormant snapshot when a configured home unloads.
    pub(crate) async fn unload(&self, record: Record) -> Result<(), Error> {
        if let Some(home) = self.home(&record.summary.session) {
            *home.lock().await = Cursor {
                summary: Some(record.summary),
                offset: record.offsets[record.offsets.len() - 1],
                file: Some(
                    record
                        .file
                        .metadata()
                        .await
                        .map_err(io("reading record metadata"))?,
                ),
            };
        }
        Ok(())
    }

    /// Reads an unloaded record, resuming the configured home's fold when its file still matches.
    pub(crate) async fn read(&self, home: &Path, session: &SessionId) -> Result<Summary, Error> {
        if let Some(cached) = self.home(session) {
            cached.lock().await.read(home, session).await
        } else {
            Cursor::default().read(home, session).await
        }
    }

    fn home(&self, session: &SessionId) -> Option<Arc<tokio::sync::Mutex<Cursor>>> {
        // A panic while this lock is held leaves the map whole; it contains only configured homes.
        self.homes
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(session)
            .cloned()
    }
}

/// One record's fold through a committed byte boundary, with fixed-size file metadata.
/// Retaining it holds no entry bodies or per-entry index; Summary's existing bounds apply.
#[derive(Default)]
struct Cursor {
    summary: Option<Summary>,
    offset: u64,
    file: Option<Metadata>,
}

/// Folds only appended complete entries through the length captured at open; a replaced,
/// shortened or rewritten file starts over. Never repairs a tail or changes the live marker.
impl Cursor {
    async fn read(&mut self, home: &Path, session: &SessionId) -> Result<Summary, Error> {
        let dir = home.join("sessions").join(session.as_str());
        let mut file = open(&dir, session).await?;
        let metadata = file
            .metadata()
            .await
            .map_err(io("reading record metadata"))?;
        let same = self.file.as_ref().is_some_and(|old| {
            old.dev() == metadata.dev()
                && old.ino() == metadata.ino()
                && old.len() <= metadata.len()
                && (old.len() < metadata.len()
                    || (old.ctime(), old.ctime_nsec()) == (metadata.ctime(), metadata.ctime_nsec()))
        });
        if !same {
            *self = Self::default();
        }
        let length = metadata.len();
        self.file = Some(metadata);
        file.seek(std::io::SeekFrom::Start(self.offset))
            .await
            .map_err(io("seeking to the summary's boundary"))?;
        let reader = BufReader::with_capacity(64 * 1024, file.take(length - self.offset));
        fold(&mut self.summary, session, reader, |size| {
            self.offset += size;
        })
        .await?;
        self.summary.clone().ok_or_else(|| Error::Damaged {
            session: session.clone(),
            line: 1,
            why: "the record is empty".into(),
        })
    }
}

/// The same complete-line loop and validation for indexed loading and read-only resumption.
pub(super) async fn fold(
    state: &mut Option<Summary>,
    session: &SessionId,
    mut reader: impl tokio::io::AsyncBufRead + Unpin,
    mut commit: impl FnMut(u64),
) -> Result<(), Error> {
    let mut line = Vec::new(); // One stored entry at a time, never the record.
    loop {
        line.clear();
        let size = reader
            .read_until(b'\n', &mut line)
            .await
            .map_err(io("reading the record"))?;
        if size == 0 || line.last() != Some(&b'\n') {
            break;
        }
        let id = state.as_ref().map_or(1, |s| s.last.0 + 1);
        let damaged = |why| Error::Damaged {
            session: session.clone(),
            line: id,
            why,
        };
        let entry: Entry = serde_json::from_slice(&line).map_err(|e| damaged(e.to_string()))?;
        if entry.id.0 != id {
            return Err(damaged(format!("its id is {}", entry.id.0)));
        }
        match (&entry.body, &state) {
            (
                Body::Start {
                    cwd,
                    model,
                    settings,
                    ..
                },
                None,
            ) => {
                *state = Some(Summary::new(session, cwd, model, *settings));
            }
            (Body::Start { .. }, Some(_)) => {
                return Err(damaged("it is another start entry".into()));
            }
            (_, None) => return Err(damaged("it is not a start entry".into())),
            _ => {}
        }
        if let Some(state) = state {
            state.fold(&entry);
        }
        commit(size as u64);
    }
    Ok(())
}
