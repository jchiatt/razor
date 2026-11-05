//! Deliveries: files and links an agent hands the person, kept in one append-only index for
//! listing. A delivery is identified by its session and its `deliver` call's entry.

use super::{Error, TEXT, other};
use crate::controller::Controller;
use crate::protocol::{Delivery, EntryId, SessionId, now};
use serde::Deserialize;
use serde_json::Value;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::{Mutex, broadcast};

/// The largest file `deliver` copies.
const FILE: u64 = 64 << 20;
/// Index lines are at most 64 KiB; reverse reads retain at most two such chunks.
const LINE: usize = 64 << 10;

/// Every delivery on this machine: one line each in `$RAZOR_HOME/deliveries.jsonl`. One per
/// server; cheap to clone.
#[derive(Clone)]
pub struct Deliveries {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    file: Mutex<Writer>,
    /// At most 64 lines behind; true marks a new delivery, false a replacement.
    made: broadcast::Sender<(Delivery, bool)>,
}

/// The `deliver` action's input.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    line: String,
    path: Option<PathBuf>,
    url: Option<String>,
}

struct Writer {
    file: File,
    end: u64,
    failed: bool,
}

impl Writer {
    async fn repair(&mut self) -> Result<(), Error> {
        if self.failed {
            self.file
                .set_len(self.end)
                .await
                .map_err(other("cutting off a failed delivery append"))?;
            self.failed = false;
        }
        Ok(())
    }

    async fn append(&mut self, value: &impl serde::Serialize) -> Result<(), Error> {
        self.repair().await?;
        let mut line = serde_json::to_vec(value).map_err(other("writing the delivery index"))?;
        line.push(b'\n');
        if line.len() > LINE {
            return Err(Error::Refused(
                "A delivery index line must fit in 64 KiB.".into(),
            ));
        }
        self.failed = true;
        self.file
            .write_all(&line)
            .await
            .map_err(other("appending to deliveries.jsonl"))?;
        self.file
            .flush()
            .await
            .map_err(other("appending to deliveries.jsonl"))?;
        self.end += line.len() as u64;
        self.failed = false;
        Ok(())
    }
}

/// A fixed file boundary, read backwards without retaining historical delivery ids.
struct Index {
    file: File,
    end: u64,
    bytes: Vec<u8>,
    skip: bool,
    /// The last complete newline, including when the following tail is malformed.
    boundary: Option<u64>,
}

impl Index {
    async fn open(path: &Path) -> Result<Self, Error> {
        let file = File::open(path)
            .await
            .map_err(other("opening deliveries.jsonl"))?;
        let end = file
            .metadata()
            .await
            .map_err(other("reading the delivery index size"))?
            .len();
        Ok(Self {
            file,
            end,
            bytes: Vec::new(),
            skip: true,
            boundary: None,
        })
    }

    async fn next(&mut self) -> Result<Option<Delivery>, Error> {
        loop {
            let line = if let Some(at) = memchr::memrchr(b'\n', &self.bytes) {
                self.boundary.get_or_insert(self.end + at as u64 + 1);
                let line = self.bytes.split_off(at + 1);
                self.bytes.truncate(at);
                if std::mem::replace(&mut self.skip, false) {
                    continue;
                }
                line
            } else if self.end == 0 {
                if self.bytes.is_empty() || self.skip {
                    return Ok(None);
                }
                std::mem::take(&mut self.bytes)
            } else {
                if self.bytes.len() > LINE {
                    self.bytes.clear();
                    self.skip = true;
                }
                let start = self.end.saturating_sub(LINE as u64);
                let mut bytes = vec![0; (self.end - start) as usize];
                self.file
                    .seek(SeekFrom::Start(start))
                    .await
                    .map_err(other("reading deliveries.jsonl"))?;
                self.file
                    .read_exact(&mut bytes)
                    .await
                    .map_err(other("reading deliveries.jsonl"))?;
                self.end = start;
                bytes.append(&mut self.bytes);
                self.bytes = bytes;
                continue;
            };
            if line.len() < LINE
                && let Ok(stored) = serde_json::from_slice(&line)
            {
                return Ok(Some(stored));
            }
        }
    }
}

impl Deliveries {
    /// Open the index and discard an incomplete tail.
    pub(crate) async fn open(home: &Path) -> Result<Deliveries, Error> {
        let home = std::path::absolute(home).map_err(other("locating the delivery home"))?;
        let path = home.join("deliveries.jsonl");
        let mut open = OpenOptions::new();
        let opened = open.read(true).append(true).create(true).open(&path).await;
        let file = opened.map_err(other("opening deliveries.jsonl"))?;
        let mut index = Index::open(&path).await?;
        index.next().await?;
        let end = index.boundary.unwrap_or(0);
        file.set_len(end)
            .await
            .map_err(other("cutting off a torn delivery line"))?;
        let file = Writer {
            file,
            end,
            failed: false,
        };
        file.file
            .sync_data()
            .await
            .map_err(other("syncing the delivery index"))?;
        Ok(Deliveries {
            inner: Arc::new(Inner {
                path,
                file: Mutex::new(file),
                made: broadcast::channel(64).0,
            }),
        })
    }

    /// The `deliver` action, which the controller runs for any session.
    /// - Input: `line`, one line for the person; and exactly one of `path` or `url`. A `path` is
    ///   a regular file of at most 64 MiB, relative to `cwd`, copied as it is now to
    ///   `dir/delivered/<entry>/<its name>`. A `url` is http or https, and is never fetched.
    /// - Output: the `Delivery`.
    /// - The copy is made before the index line is written, so a listed delivery always has its
    ///   file.
    pub(crate) async fn deliver(
        &self,
        c: &Controller,
        session: &SessionId,
        entry: EntryId,
        dir: &Path,
        cwd: &Path,
        input: Value,
    ) -> Result<Value, Error> {
        let t = &*TEXT;
        let refuse = |sentence: &str| Err(Error::Refused(sentence.to_string()));
        let input: Input = match serde_json::from_value(input) {
            Ok(input) => input,
            Err(e) => return refuse(&t.deliver_input.replace("{why}", &e.to_string())),
        };
        let line = input.line.trim();
        if line.is_empty() || line.contains('\n') || line.len() > 2000 {
            return refuse(&t.deliver_line);
        }
        let (file, url) = match (input.path, input.url) {
            (Some(path), None) => {
                let source = cwd.join(path);
                let shown = source.display().to_string();
                let fits = fs::metadata(&source).await;
                let fits = fits.is_ok_and(|m| m.is_file() && m.len() <= FILE);
                let Some(name) = source.file_name().filter(|_| fits) else {
                    return refuse(&t.deliver_file.replace("{path}", &shown));
                };
                let folder = Path::new("delivered").join(entry.0.to_string());
                let file = folder.join(name);
                let copy = async {
                    fs::create_dir_all(dir.join(&folder)).await?;
                    fs::copy(&source, dir.join(&file)).await
                };
                copy.await.map_err(other("copying the delivered file"))?;
                (Some(file), None)
            }
            (None, Some(url))
                if url.len() <= 2048
                    && ["https://", "http://"].iter().any(|s| url.starts_with(s)) =>
            {
                (None, Some(url))
            }
            (None, Some(_)) => return refuse(&t.deliver_url),
            _ => return refuse(&t.deliver_one),
        };
        let delivery = Delivery {
            session: session.clone(),
            entry,
            at: now(),
            line: line.to_string(),
            file,
            url,
            name: String::new(),
            project: String::new(),
            archived: false,
            read: false,
            path: None,
            voice: None,
            sound: None,
            terminal: None,
        };
        let delivery = self.add(c, delivery).await?;
        serde_json::to_value(delivery).map_err(other("writing the delivery"))
    }

    /// Keep a note an agent sent the person as a delivery with neither file nor url, at its
    /// `message` call's entry: its first non-empty line, at most 2,000 characters. Its whole
    /// text stays in the record at that entry.
    pub(crate) async fn note(
        &self,
        c: &Controller,
        session: &SessionId,
        entry: EntryId,
        text: &str,
        spoken: Option<crate::protocol::Spoken>,
    ) -> Result<(), Error> {
        let line = text.lines().map(str::trim).find(|l| !l.is_empty());
        let (voice, sound, terminal) = match spoken {
            Some(s) => (Some(Box::new(s.voice)), Some(s.sound), s.terminal),
            None => (None, None, None),
        };
        let note = Delivery {
            session: session.clone(),
            entry,
            at: now(),
            line: line.unwrap_or_default().chars().take(2000).collect(),
            file: None,
            url: None,
            name: String::new(),
            project: String::new(),
            archived: false,
            read: false,
            path: None,
            voice,
            sound,
            terminal,
        };
        self.add(c, note).await.map(drop)
    }

    /// Store the sender's name and project once at delivery time, then append and announce
    /// the delivery under the writer's lock.
    async fn add(&self, c: &Controller, mut delivery: Delivery) -> Result<Delivery, Error> {
        if let Ok(sender) = c.summary(&delivery.session).await {
            delivery.project = crate::children::project(&sender);
            delivery.name = sender.name;
        }
        let mut index = self.inner.file.lock().await;
        index.append(&delivery).await?;
        self.enrich(&mut delivery);
        // No subscriber is not a failure: the index still lists it.
        let _ = self.inner.made.send((delivery.clone(), true));
        Ok(delivery)
    }

    /// Deliveries newest first before the named delivery, at most 256; receipts keep its position.
    pub(crate) async fn list(
        &self,
        before: Option<(SessionId, EntryId)>,
        limit: usize,
        words: &[String],
    ) -> Result<Vec<Delivery>, Error> {
        let before = match before {
            Some((session, entry)) => {
                let delivery = self.get(&session, entry).await?;
                Some((delivery.at, delivery.session, delivery.entry))
            }
            None => None,
        };
        self.page(before, limit, words).await
    }

    /// The newest original `(at, session, entry)` keys before `before`, retaining at most 256 rows.
    async fn page(
        &self,
        before: Option<(u64, SessionId, EntryId)>,
        limit: usize,
        words: &[String],
    ) -> Result<Vec<Delivery>, Error> {
        let limit = limit.clamp(1, 256);
        let mut kept: Vec<Delivery> = Vec::with_capacity(limit);
        let mut index = Index::open(&self.inner.path).await?;
        while let Some(delivery) = index.next().await? {
            let key = (delivery.at, &delivery.session, delivery.entry);
            if before
                .as_ref()
                .is_some_and(|(at, s, e)| key >= (*at, s, *e))
                || !delivery.matches(words)
            {
                continue;
            }
            let at = kept.partition_point(|d| (d.at, &d.session, d.entry) > key);
            if kept
                .get(at)
                .is_some_and(|d| (d.at, &d.session, d.entry) == key)
            {
                continue;
            }
            if at < limit {
                if kept.len() == limit {
                    kept.pop();
                }
                kept.insert(at, delivery);
            }
        }
        for delivery in &mut kept {
            self.enrich(delivery);
        }
        Ok(kept)
    }

    /// Find a delivery's latest replacement without retaining other ids.
    async fn get(&self, session: &SessionId, entry: EntryId) -> Result<Delivery, Error> {
        let mut index = Index::open(&self.inner.path).await?;
        while let Some(delivery) = index.next().await? {
            if delivery.session == *session && delivery.entry == entry {
                return Ok(delivery);
            }
        }
        Err(Error::Refused(
            "There is no delivery at that session entry.".into(),
        ))
    }

    /// Mark read or unread, preserving archive state and original position.
    pub(crate) async fn read(
        &self,
        session: &SessionId,
        entry: EntryId,
        read: bool,
    ) -> Result<(), Error> {
        self.change(session, entry, |d| d.read = read).await
    }

    /// Archive or unarchive, preserving read state and original position.
    pub(crate) async fn archive(
        &self,
        session: &SessionId,
        entry: EntryId,
        archived: bool,
    ) -> Result<(), Error> {
        self.change(session, entry, |d| d.archived = archived).await
    }

    /// Sync a full replacement under the writer's lock before broadcasting it.
    async fn change(
        &self,
        session: &SessionId,
        entry: EntryId,
        change: impl FnOnce(&mut Delivery),
    ) -> Result<(), Error> {
        let mut index = self.inner.file.lock().await;
        index.repair().await?;
        let mut delivery = self.get(session, entry).await?;
        let previous = (delivery.read, delivery.archived);
        change(&mut delivery);
        let changed = previous != (delivery.read, delivery.archived);
        if changed {
            index.append(&delivery).await?;
        }
        index
            .file
            .sync_data()
            .await
            .map_err(other("syncing the delivery receipt"))?;
        if changed {
            self.enrich(&mut delivery);
            let _ = self.inner.made.send((delivery, false));
        }
        Ok(())
    }

    /// A delivered file's path on this server.
    fn enrich(&self, delivery: &mut Delivery) {
        let sessions = self.inner.path.with_file_name("sessions");
        let dir = sessions.join(delivery.session.as_str());
        delivery.path = delivery.file.as_ref().map(|file| dir.join(file));
    }

    /// Each new delivery (true) or shared inbox replacement (false), in index order.
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<(Delivery, bool)> {
        self.inner.made.subscribe()
    }
}
