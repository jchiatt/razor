//! The API client: every request carries the device key, every body and page is bounded, and
//! the NDJSON streams feed one bounded channel whose lines hold their bytes until folded.
use super::{Error, error, fail};
use crate::protocol::{BODY_BYTES, Delivery, Event, Live, Login, SessionId};
use futures_util::StreamExt;
use reqwest::{Method, RequestBuilder, Response, StatusCode, header};
use serde::de::IgnoredAny;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::time::Instant;
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore, mpsc},
    task::JoinHandle,
};

/// The transcript's byte budget, which also bounds a history page and the stream lines in flight.
pub const BYTES: usize = 8 << 20;
/// The record's line cap: no answer or stream line grows a buffer past it.
const LINE: usize = 64 << 20;
/// A saved job file is read in ranges of this many bytes.
pub const CHUNK: u64 = 64 << 10;
const CLOSED: &str = "The terminal stream closed; reconnect to the session.";
const DISCONNECTED: &str = "The API stream disconnected; reconnecting from the last entry.";
const INVALID: &str = "The API sent an invalid stream line; reconnect to the server.";

#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    server: reqwest::Url,
    key: String,
    // One history page, read or waiting for its fold: 8 MiB, or one whole 64 MiB line.
    pages: Arc<Semaphore>,
    // Set for good by the first skipped line: only a new terminal can read it.
    newer: Arc<AtomicBool>,
}

/// A history page's events in record order; it holds the one page permit until folded.
pub struct Page {
    pub events: Vec<Event>,
    _permit: OwnedSemaphorePermit,
}

/// One stream line; its bytes count against the shared budget until it is folded.
pub struct Incoming {
    pub session: Option<SessionId>,
    pub line: Line,
    _bytes: Option<OwnedSemaphorePermit>,
}
pub enum Line {
    Event(Event),
    Login(Login),
    /// The stream ended or failed and is reconnecting; the sentence says why.
    Failed(String),
    /// A line was skipped as unknown to this terminal: the screen redraws and asks `newer`.
    Newer,
}

impl Api {
    pub fn new(http: reqwest::Client, server: reqwest::Url, key: String) -> Self {
        let pages = Arc::new(Semaphore::new(1));
        Self {
            http,
            server,
            key,
            pages,
            newer: Arc::default(),
        }
    }

    /// True once any reader of this `Api` or a clone skipped what a newer server sent.
    pub fn newer(&self) -> bool {
        self.newer.load(Relaxed)
    }

    // `None` for valid JSON this terminal's types do not know: it is skipped and `newer` set.
    // Bytes that are not JSON stay the error.
    fn decode(&self, line: &[u8]) -> Result<Option<Event>, Error> {
        match serde_json::from_slice(line) {
            Err(_) if serde_json::from_slice::<IgnoredAny>(line).is_ok() => {
                self.newer.store(true, Relaxed);
                Ok(None)
            }
            known => known.map(Some).map_err(fail(INVALID)),
        }
    }

    fn request(&self, method: Method, path: &str) -> Result<reqwest::RequestBuilder, Error> {
        let at = "The API address is invalid; restart Razor with a working server address.";
        let url = self.server.join(path).map_err(fail(at))?;
        Ok(self.http.request(method, url).bearer_auth(&self.key))
    }

    // Sends `request` with its timeout; a failure to reach the API says `failed`.
    async fn send(request: RequestBuilder, secs: u64, failed: &str) -> Result<Response, Error> {
        let request = request.timeout(Duration::from_secs(secs)).send();
        request.await.map_err(fail(failed))
    }

    /// A JSON request answered within 30 seconds; a refusal is the API's own sentence.
    pub async fn json(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, Error> {
        let mut request = self.request(method, path)?;
        if let Some(body) = body {
            let body = serde_json::to_vec(&body).map_err(fail(
                "The input could not be encoded; keep its unsent words and attachments.",
            ))?;
            if body.len() > BODY_BYTES {
                let sentence = "The input exceeds the server's 32 MiB request limit; remove an image or shorten the words.";
                return Err(error(sentence));
            }
            request = request
                .header(header::CONTENT_TYPE, "application/json")
                .body(body);
        }
        let failed = "The API request failed; check the transcript before sending again.";
        let response = Self::send(request, 30, failed).await?;
        serde_json::from_slice(&read(response).await?).map_err(fail(
            "The API sent an invalid answer; reconnect to the server.",
        ))
    }

    /// Renames the addressed session; a refusal is the API's own sentence.
    pub async fn rename(&self, session: &SessionId, name: String) -> Result<(), Error> {
        let path = format!("v1/sessions/{session}/name");
        let request = self.request(Method::PUT, &path)?.body(name);
        let failed = "The rename request failed; reconnect and check the session's name.";
        read(Self::send(request, 30, failed).await?).await.map(drop)
    }

    /// Reads the session's saved draft, or replaces it with `text` (empty removes it).
    pub async fn draft(&self, session: &SessionId, text: Option<String>) -> Result<String, Error> {
        let path = format!("v1/sessions/{session}/draft");
        let request = match text {
            Some(text) => self.request(Method::PUT, &path)?.body(text),
            None => self.request(Method::GET, &path)?,
        };
        let failed = "The draft could not be saved or read; keep the unsent words and reconnect.";
        let response = Self::send(request, 15, failed).await?;
        String::from_utf8(read(response).await?).map_err(fail(
            "The saved draft is not UTF-8; replace it through the API.",
        ))
    }

    /// Up to `limit` entries before `before` (the newest without it), holding the page permit.
    pub async fn page(
        &self,
        session: &SessionId,
        before: Option<u64>,
        limit: usize,
    ) -> Result<Page, Error> {
        let permit = self.pages.clone().acquire_owned().await;
        let permit = permit.map_err(fail("The history reader closed; reconnect to the server."))?;
        let before = before.map_or(String::new(), |id| format!("&before={id}"));
        let path = format!("v1/sessions/{session}/entries?limit={limit}{before}");
        let failed = "The transcript page could not be read; reconnect to the server.";
        let request = self.request(Method::GET, &path)?;
        let bytes = read(Self::send(request, 30, failed).await?).await?;
        let lines: Vec<_> = bytes
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .collect();
        if lines.len() > limit || bytes.len() > BYTES && lines.len() != 1 {
            let sentence = "The transcript page exceeds the server's page bound; reconnect with a matching server.";
            return Err(error(sentence));
        }
        let events = lines.into_iter().filter_map(|l| self.decode(l).transpose());
        Ok(Page {
            events: events.collect::<Result<_, _>>()?,
            _permit: permit,
        })
    }

    /// One 64 KiB range of a session file (a job's saved output) from `offset`, and its size.
    pub async fn chunk(
        &self,
        session: &SessionId,
        file: &str,
        offset: u64,
    ) -> Result<(Vec<u8>, u64), Error> {
        let retry = "The saved output could not be read; close and reopen the card to retry.";
        let path = format!("v1/sessions/{session}/files/{file}");
        let range = format!("bytes={offset}-{}", offset.saturating_add(CHUNK - 1));
        let request = self
            .request(Method::GET, &path)?
            .header(header::RANGE, range);
        let mut response = Self::send(request, 30, retry).await?;
        let range = response
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok());
        let range = range.ok_or_else(|| error(retry))?.to_owned();
        if response.status() == StatusCode::RANGE_NOT_SATISFIABLE && range == "bytes */0" {
            return Ok((Vec::new(), 0));
        }
        let size: u64 = range
            .rsplit_once('/')
            .and_then(|(_, n)| n.parse().ok())
            .ok_or_else(|| error(retry))?;
        let end = offset.saturating_add(CHUNK).min(size);
        let expected = format!("bytes {offset}-{}/{size}", end.saturating_sub(1));
        if response.status() != StatusCode::PARTIAL_CONTENT || end <= offset || range != expected {
            return Err(error(retry));
        }
        let mut bytes = Vec::new();
        while let Some(part) = response.chunk().await.map_err(fail(retry))? {
            if bytes.len() + part.len() > CHUNK as usize {
                return Err(error(retry));
            }
            bytes.extend_from_slice(&part);
        }
        match bytes.len() as u64 == end - offset {
            true => Ok((bytes, size)),
            false => Err(error(retry)),
        }
    }

    /// A voice note's upload or admission, or audio to play: `body` as its content type, asked
    /// again each second on 409, 502 or a lost connection until `until`, each attempt bounded by
    /// what is left of it and at least 30 seconds; the answer streams into `out`, at most 32 MiB.
    pub async fn audio(
        &self,
        method: Method,
        path: &str,
        body: Option<(&str, Vec<u8>)>,
        until: Instant,
        out: &mut (impl AsyncWrite + Unpin),
    ) -> Result<(), Error> {
        let lost = "The audio request did not finish; check the server.";
        let mut answer = loop {
            let mut request = self.request(method.clone(), path)?;
            if let Some((kind, bytes)) = &body {
                request = request.header(header::CONTENT_TYPE, *kind);
                request = request.body(bytes.clone());
            }
            let left = until.saturating_duration_since(Instant::now());
            let answer = request.timeout(left.max(Duration::from_secs(30)));
            let answer = answer.send().await;
            // A lost connection is asked again, as a peer's 502 is.
            let status = answer.as_ref().map_or(502, |a| a.status().as_u16());
            let again = matches!(status, 409 | 502) && left.as_secs() > 0;
            match answer.map_err(fail(lost)) {
                Ok(answer) if answer.status().is_success() => break answer,
                // `read` turns every other answer into the API's own refusal.
                Ok(answer) if !again => return read(answer).await.map(drop),
                Err(e) if !again => return Err(e),
                _ => tokio::time::sleep(Duration::from_secs(1)).await,
            }
        };
        let mut size = 0;
        while let Some(part) = answer.chunk().await.map_err(fail(lost))? {
            size += part.len();
            if size > BODY_BYTES {
                return Err(error("The audio exceeds 32 MiB; play a shorter one."));
            }
            let unsaved = fail("The audio could not be saved; check the temporary folder.");
            out.write_all(&part).await.map_err(unsaved)?;
        }
        Ok(())
    }

    /// The session's stream after `after`, or status; transient ends retry after a second from
    /// the last entry passed on, while a 4xx is said once and ends this reader.
    pub fn stream(
        &self,
        session: Option<SessionId>,
        mut after: u64,
        tx: mpsc::Sender<Incoming>,
        budget: Arc<Semaphore>,
    ) -> JoinHandle<()> {
        let api = self.clone();
        tokio::spawn(async move {
            loop {
                let path = match &session {
                    Some(id) => format!("v1/sessions/{id}/events?after={after}&terminal=true"),
                    None => "v1/status?views=true&heartbeat=true".into(),
                };
                let ended = api
                    .lines(Method::GET, &path, &session, &mut after, &tx, &budget)
                    .await;
                let refused = ended.as_ref().is_err_and(Error::refused);
                let sentence = ended.err().map_or_else(
                    || "The API stream ended; reconnecting from the last entry.".into(),
                    |e| e.to_string(),
                );
                let line = Line::Failed(sentence);
                let failed = Incoming {
                    session: session.clone(),
                    line,
                    _bytes: None,
                };
                if tx.send(failed).await.is_err() || refused {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        })
    }

    /// The server's sign-in: its instruction lines arrive on `tx` until it is done or fails.
    pub async fn login(
        &self,
        tx: mpsc::Sender<Incoming>,
        budget: Arc<Semaphore>,
    ) -> Result<(), Error> {
        self.lines(Method::POST, "v1/login", &None, &mut 0, &tx, &budget)
            .await
    }

    async fn lines(
        &self,
        method: Method,
        path: &str,
        session: &Option<SessionId>,
        after: &mut u64,
        tx: &mpsc::Sender<Incoming>,
        budget: &Arc<Semaphore>,
    ) -> Result<(), Error> {
        let login = method == Method::POST;
        let response = self
            .request(method, path)?
            .send()
            .await
            .map_err(fail(DISCONNECTED))?;
        if !response.status().is_success() {
            return read(response).await.map(drop);
        }
        // These headers mean /status has subscribed, so the durable deliveries read next
        // overlap the live lines instead of leaving a gap; duplicates are dropped on fold.
        if session.is_none() && !login {
            self.deliveries(tx).await?;
        }
        let mut stream = response.bytes_stream();
        let mut line = Vec::new();
        while let Some(chunk) = stream.next().await {
            for segment in chunk
                .map_err(fail(DISCONNECTED))?
                .split_inclusive(|b| *b == b'\n')
            {
                if line.len() + segment.len() > LINE {
                    let sentence = "A transcript line exceeds the record's 64 MiB limit; use a matching server version.";
                    return Err(error(sentence));
                }
                line.extend_from_slice(segment);
                if !line.ends_with(b"\n") {
                    continue;
                }
                let bytes = budget
                    .clone()
                    .acquire_many_owned(line.len().min(BYTES) as u32)
                    .await;
                let bytes = Some(bytes.map_err(fail(CLOSED))?);
                let parsed = match login {
                    true => Line::Login(serde_json::from_slice(&line).map_err(fail(INVALID))?),
                    false => self.decode(&line)?.map_or(Line::Newer, Line::Event),
                };
                line.clear();
                if let Line::Event(Event::Entry(entry)) = &parsed {
                    *after = entry.id.0;
                }
                let incoming = Incoming {
                    session: session.clone(),
                    line: parsed,
                    _bytes: bytes,
                };
                tx.send(incoming).await.map_err(fail(CLOSED))?;
            }
        }
        match line.is_empty() {
            true => Ok(()),
            false => Err(error(
                "The API stream ended inside a line; reconnecting from the last entry.",
            )),
        }
    }

    // The newest 256 deliveries, the API's largest page, oldest first.
    async fn deliveries(&self, tx: &mpsc::Sender<Incoming>) -> Result<(), Error> {
        let value = self
            .json(Method::GET, "v1/deliveries?limit=256", None)
            .await?;
        let deliveries: Vec<Value> = serde_json::from_value(value).map_err(fail(
            "The delivery page is invalid; reconnect with a matching server.",
        ))?;
        for delivery in deliveries.into_iter().take(256).rev() {
            let Ok(delivery) = serde_json::from_value::<Delivery>(delivery) else {
                self.newer.store(true, Relaxed);
                continue;
            };
            let line = Line::Event(Event::Live(Live::Delivery(delivery.into())));
            let incoming = Incoming {
                session: None,
                line,
                _bytes: None,
            };
            tx.send(incoming).await.map_err(fail(CLOSED))?;
        }
        Ok(())
    }
}

// Read incrementally, so neither a missing length nor compression grows a buffer past the cap.
async fn read(response: Response) -> Result<Vec<u8>, Error> {
    let status = response.status();
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let interrupted = "The API answer was interrupted; reconnect to the server.";
        let chunk = chunk.map_err(fail(interrupted))?;
        if body.len() + chunk.len() > LINE {
            return Err(error(
                "The API answer exceeds the record's 64 MiB line cap; read a smaller page.",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    if status.is_success() {
        return Ok(body);
    }
    let sentence = serde_json::from_slice::<Value>(&body).ok();
    let sentence = sentence.as_ref().and_then(|v| v["error"].as_str());
    let sentence = sentence.map_or_else(|| String::from_utf8_lossy(&body), Into::into);
    Err(Error::Refused {
        status: status.as_u16(),
        sentence: sentence.into_owned(),
    })
}
