//! The Responses loop over Razor's own ChatGPT sign-in (docs/design/responses.md).

mod auth;
mod chat;
mod image;
mod search;
mod stream;
mod task;
mod upload;

use crate::{
    profiles::Endpoint,
    protocol::{FromBridge, SessionId, Usage, UsageWindow},
};
use reqwest::{Client, StatusCode, header::HeaderMap};
use serde_json::Value;
use std::{
    fmt,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::sync::{Mutex, mpsc, watch};

/// The most bytes a request or a response stream may hold.
const CAP: usize = 64 << 20;
/// How long a response stream may stay silent.
const IDLE: Duration = Duration::from_secs(300);
/// The longest one request waits in all on a chat host's 429s, its `Retry-After`s included.
const BUSY: Duration = Duration::from_secs(60);
/// A missing local ChatGPT sign-in, shared by readiness checks and a request's refusal.
pub(crate) const SIGN_IN_MISSING: &str = "ChatGPT is not signed in; run razor login.";

/// A missing public API key, shared by readiness checks and a request's refusal.
pub(crate) fn key_missing(home: &Path) -> String {
    let key = home.join("key");
    format!(
        "There is no OpenAI API key in '{}'; save a key: ( umask 077; mkdir -p '{}'; ${{EDITOR:-vi}} '{}' ). API billing instead of a ChatGPT plan uses responses.base_url = \"https://api.openai.com/v1\" (docs/models.md#with-an-openai-api-key); run razor restart after changing it.",
        key.display(),
        home.display().to_string().replace('\'', "'\\''"),
        key.display().to_string().replace('\'', "'\\''")
    )
}

/// The shared HTTP client; sign-in and refresh share one lock, read credentials per request.
pub struct Responses {
    client: Client,
    home: PathBuf,
    auth: Mutex<()>,
}

/// A completed response: its done items in output order, with every native field kept.
struct Response {
    items: Vec<Value>,
    usage: Usage,
    end_turn: bool,
    turn_state: Option<String>,
    windows: Vec<UsageWindow>,
    tier: Option<String>,
}

/// Why a request or sign-in failed, as one sentence for the person.
#[derive(Debug)]
pub enum Error {
    Auth,
    ApiKey,
    Stopped,
    Incomplete,
    Failed(String),
    ContextOverflow(String),
    Size,
    Rejected {
        status: u16,
        message: String,
    },
    /// The chat host's key variable, unset or refused.
    Key(String),
    /// The reply stopped at this finish reason; the same request would stop the same way.
    Cut(String),
    /// A call's arguments the route refuses, as one sentence for the model.
    Refused(String),
    Other {
        source: Box<dyn std::error::Error + Send + Sync>,
        context: &'static str,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auth => f.write_str("ChatGPT sign-in failed; run razor login and enable device code authorization for Codex in ChatGPT Security Settings."),
            Self::ApiKey => f.write_str("The OpenAI API key could not be used; check the key file in your Razor home (docs/models.md#with-an-openai-api-key), then try again."),
            Self::Stopped => f.write_str("The request was stopped."),
            Self::Incomplete => f.write_str("The model response did not complete; send another input to try again."),
            Self::Failed(message) => write!(f, "The model request failed: {message}."),
            Self::ContextOverflow(message) => write!(f, "The model context is too large; {message}; Razor will rebuild it from the record and retry once."),
            Self::Size => f.write_str("The model request or response exceeds 64 MiB; rebuild with a smaller context."),
            Self::Rejected { status, message } => write!(f, "The model endpoint returned HTTP {status}: {message}."),
            Self::Key(name) if name.is_empty() => f.write_str("The model host requires a key; name its environment variable in the chat row's key setting."),
            Self::Key(name) => write!(f, "The model host refused or never got a key; set {name} in the server's environment."),
            Self::Cut(reason) => write!(f, "The model's reply stopped early ({reason}); send another input to go on."),
            Self::Refused(sentence) => f.write_str(sentence),
            Self::Other { source, context } => write!(f, "Failed {context}: {source}."),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// The same error with the bearer taken out of the host's text.
    fn without(mut self, bearer: &str) -> Self {
        if let Self::ContextOverflow(text)
        | Self::Failed(text)
        | Self::Cut(text)
        | Self::Refused(text)
        | Self::Rejected { message: text, .. } = &mut self
        {
            redact(text, bearer);
        }
        self
    }
}

/// A host may echo the bearer it was sent, in an error or a reply; it never leaves `post`.
fn redact(text: &mut String, bearer: &str) {
    if !bearer.is_empty() && text.contains(bearer) {
        *text = text.replace(bearer, "[key removed]");
    }
}

/// `redact` on every string of a reply, field names included.
fn redact_all(value: &mut Value, bearer: &str) {
    match value {
        Value::String(text) => redact(text, bearer),
        Value::Array(values) => values.iter_mut().for_each(|v| redact_all(v, bearer)),
        Value::Object(fields) => {
            if !bearer.is_empty() && fields.keys().any(|k| k.contains(bearer)) {
                let named = std::mem::take(fields).into_iter();
                *fields = named
                    .map(|(k, v)| (k.replace(bearer, "[key removed]"), v))
                    .collect();
            }
            fields.values_mut().for_each(|v| redact_all(v, bearer));
        }
        _ => {}
    }
}

fn other<E: std::error::Error + Send + Sync + 'static>(
    context: &'static str,
) -> impl FnOnce(E) -> Error {
    move |source| Error::Other {
        source: Box::new(source),
        context,
    }
}

impl Responses {
    /// Reads no credentials and contacts nothing until a sign-in or a request.
    pub fn new(home: PathBuf) -> Result<Self, Error> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(IDLE);
        let client = client
            .user_agent(concat!("razor/", env!("CARGO_PKG_VERSION")))
            .build();
        let client = client.map_err(other("creating the model HTTP client"))?;
        Ok(Self {
            client,
            home,
            auth: Mutex::new(()),
        })
    }

    /// The device sign-in, run by the server; `instructions` carries what the person must do.
    pub(crate) async fn login(
        &self,
        instructions: mpsc::Sender<String>,
        mut stop: watch::Receiver<bool>,
    ) -> Result<(), Error> {
        tokio::select! {
            Ok(_) = stop.wait_for(|stop| *stop) => Err(Error::Stopped),
            result = self.device_login(instructions) => result,
        }
    }

    /// One tool-free Terra answer at low effort on this client's sign-in and supplied endpoint;
    /// `post` owns retries and stream completion, bounded by the caller's deadline.
    pub(crate) async fn recap(
        &self,
        base: &str,
        session: &SessionId,
        prompt: &str,
        evidence: &str,
    ) -> Result<String, Error> {
        let body = serde_json::json!({
            "model": crate::profiles::id("terra".into()), "reasoning": {"effort": "low"}, "store": false, "stream": true,
            "input": [
                {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": prompt}]},
                {"type": "message", "role": "user", "content": [{"type": "input_text", "text": evidence}]},
            ],
        });
        let (url, body) = (base.into(), body.to_string().into_bytes());
        let endpoint = Endpoint {
            url,
            ..Default::default()
        };
        let response = self.post(&endpoint, body, session, false, None, |_| {});
        let items = response.await?.items;
        let texts = items.iter().filter(|i| i["type"] == "message");
        let texts = texts.flat_map(|i| i["content"].as_array().into_iter().flatten());
        Ok(texts.filter_map(|c| c["text"].as_str()).collect())
    }

    /// One request, retried here and nowhere else: four times for a failed send, a server error
    /// or a chat host's 429, five times for a stream that did not complete. A 429 waits the
    /// host's `Retry-After`, else the backoff, while the request's waits stay within `BUSY`; the
    /// response's usage says what was waited, and a refusal that outlasts it says so. On the
    /// sign-in a 429 is a plan limit and fails at once. A 401 refreshes the sign-in once.
    /// `turn_state` is the routing header to resend within the turn. With `key`, a chat request:
    /// that variable's value is the bearer, read for each attempt, and a 401 or 403 is `Key`. No
    /// error or reply it returns holds the bearer.
    async fn post(
        &self,
        endpoint: &Endpoint,
        body: Vec<u8>,
        session: &SessionId,
        lite: bool,
        mut turn_state: Option<String>,
        say: impl Fn(FromBridge),
    ) -> Result<Response, Error> {
        let body = axum::body::Bytes::from(body);
        let (base, key) = (endpoint.url.as_str(), endpoint.key.as_deref());
        let public = endpoint.public();
        let path = key.map_or("responses", |_| "chat/completions");
        let url = format!("{}/{path}", base.trim_end_matches('/'));
        let mut tokens =
            match key {
                Some(_) => None,
                None => Some(self.credentials(public, None).await.map_err(
                    |error| match error {
                        Error::Auth if public => Error::ApiKey,
                        error => error,
                    },
                )?),
            };
        let (mut refreshed, mut sends, mut streams, mut waited) = (false, 0, 0, Duration::ZERO);
        loop {
            let bearer = match (key, &tokens) {
                (Some(""), _) => String::new(),
                (Some(name), _) => std::env::var(name)
                    .ok()
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| Error::Key(name.into()))?,
                (None, tokens) => tokens
                    .as_ref()
                    .map(|t| t.access_token.clone())
                    .unwrap_or_default(),
            };
            let mut post = self
                .client
                .post(&url)
                .header("content-type", "application/json")
                .header("accept", "text/event-stream");
            if !bearer.is_empty() {
                post = post.bearer_auth(&bearer);
            }
            post = match key {
                // RunInfra's cache placement; other hosts ignore it.
                Some(_) => post.header("x-session-id", session.as_str()),
                None => post.header("session_id", session.as_str()),
            };
            if let Some(account) = tokens
                .as_ref()
                .map(|t| &t.account_id)
                .filter(|a| !a.is_empty())
            {
                post = post.header("chatgpt-account-id", account);
            }
            if let Some(state) = &turn_state {
                post = post.header("x-codex-turn-state", state);
            }
            if lite {
                post = post.header("x-openai-internal-codex-responses-lite", "true");
            }
            let sent = post.body(body.clone()).send().await;
            let busy = match &sent {
                Ok(r) if key.is_some() && r.status().as_u16() == 429 && sends < 4 => {
                    let wait = retry_after(r.headers()).unwrap_or(backoff(sends + 1));
                    Some(wait).filter(|wait| waited.saturating_add(*wait) <= BUSY)
                }
                _ => None,
            };
            if let Some(wait) = busy {
                (sends, waited) = (sends + 1, waited + wait);
                tokio::time::sleep(wait).await;
                continue;
            }
            let response = match sent {
                Ok(r) if key.is_some() && matches!(r.status().as_u16(), 401 | 403) => {
                    return Err(Error::Key(key.unwrap_or_default().into()));
                }
                Ok(r) if r.status() == StatusCode::UNAUTHORIZED && !public && !refreshed => {
                    tokens = Some(self.credentials(false, Some(&bearer)).await?);
                    refreshed = true;
                    continue;
                }
                Ok(r) if !r.status().is_server_error() || sends == 4 => r,
                Err(source) if sends == 4 => {
                    return Err(other("sending the model request")(source));
                }
                _ => {
                    sends += 1;
                    tokio::time::sleep(backoff(sends)).await;
                    continue;
                }
            };
            let status = response.status();
            if !status.is_success() {
                let mut error = rejected(status, response).await.without(&bearer);
                if public && matches!(error, Error::Auth) {
                    error = Error::ApiKey;
                }
                if let Error::Rejected { message, .. } = &mut error
                    && !waited.is_zero()
                {
                    let seconds = waited.as_secs_f64().round();
                    let waits = format!("; Razor retried for {seconds} s");
                    *message = message.trim_end_matches('.').to_owned() + &waits;
                }
                return Err(error);
            }
            let headers = response.headers();
            turn_state = header(headers, "x-codex-turn-state")
                .map(Into::into)
                .or(turn_state);
            let windows = windows(headers);
            let tier = header(headers, "x-gemini-service-tier").map(str::to_owned);
            let read = match key {
                Some(_) => chat::read(response, &say).await,
                None => stream::read(response, &say).await,
            };
            match read {
                Ok(mut done) => {
                    done.items.iter_mut().for_each(|i| redact_all(i, &bearer));
                    done.usage.waited = u64::try_from(waited.as_millis()).unwrap_or(u64::MAX);
                    return Ok(Response {
                        turn_state,
                        windows,
                        tier: tier.or(done.tier),
                        ..done
                    });
                }
                Err(Error::Incomplete | Error::Failed(_)) if streams < 5 => {
                    streams += 1;
                    tokio::time::sleep(backoff(streams)).await;
                    // The cut attempt's thoughts will have no entries; `say` numbers the line.
                    say(FromBridge::Void { n: 0 });
                }
                Err(error) => return Err(error.without(&bearer)),
            }
        }
    }
}

// Only reported windows: a zero length or an invalid percentage is not a measured 0%.
fn windows(headers: &HeaderMap) -> Vec<UsageWindow> {
    let mut windows = Vec::with_capacity(2);
    for key in ["primary", "secondary"] {
        let get = |field| header(headers, format!("x-codex-{key}-{field}"));
        let minutes = get("window-minutes")
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|m| *m > 0);
        let percent = get("used-percent")
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|p| p.is_finite() && (0.0..=100.0).contains(p));
        let (Some(minutes), Some(percent)) = (minutes, percent) else {
            continue;
        };
        let name = match minutes {
            m if m % 1440 == 0 => format!("{}-day", m / 1440),
            m if m % 60 == 0 => format!("{}-hour", m / 60),
            m => format!("{m}-minute"),
        };
        windows.push(UsageWindow {
            name,
            utilization: Some(percent),
            resets_at: get("reset-at")
                .and_then(|v| v.parse::<u64>().ok())
                .and_then(|t| t.checked_mul(1000))
                .filter(|t| *t > 0),
            rejected: false,
            reason: None,
        });
    }
    windows
}

/// A header's value, when it is visible ASCII.
fn header(headers: &HeaderMap, name: impl reqwest::header::AsHeaderName) -> Option<&str> {
    headers.get(name)?.to_str().ok()
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(100 << attempt.min(6))
}

/// A 429's `Retry-After`: delay seconds, or an HTTP date from now (RFC 9110, 10.2.3).
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    static DATE: jiff::fmt::rfc2822::DateTimeParser = jiff::fmt::rfc2822::DateTimeParser::new();
    let value = header(headers, reqwest::header::RETRY_AFTER)?.trim();
    if let Ok(seconds) = value.parse() {
        return Some(Duration::from_secs(seconds));
    }
    let at = DATE.parse_timestamp(value).ok()?;
    Some(Duration::try_from(at.duration_since(jiff::Timestamp::now())).unwrap_or_default())
}

/// The endpoint's own error message, never its raw body: an error page may echo credentials.
async fn rejected(status: StatusCode, response: reqwest::Response) -> Error {
    let body = bounded(response, 30_000).await.unwrap_or_default();
    let error: Value = serde_json::from_slice(&body).unwrap_or_default();
    // Google's compatible endpoint wraps its error in a one-element array.
    let error = error.get(0).unwrap_or(&error);
    let message = error.pointer("/error/message").and_then(Value::as_str);
    if error.pointer("/error/code").and_then(Value::as_str) == Some("context_length_exceeded") {
        return Error::ContextOverflow(message.unwrap_or("reduce the context").into());
    }
    match status {
        StatusCode::UNAUTHORIZED => Error::Auth,
        _ => {
            let message = message
                .unwrap_or("check the endpoint and model configuration")
                .into();
            Error::Rejected {
                status: status.as_u16(),
                message,
            }
        }
    }
}

async fn bounded(mut response: reqwest::Response, cap: usize) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    while let Some(chunk) = tokio::time::timeout(IDLE, response.chunk())
        .await
        .map_err(|_| Error::Incomplete)?
        .map_err(other("reading an HTTP response"))?
    {
        if bytes.len() + chunk.len() > cap {
            return Err(Error::Size);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
