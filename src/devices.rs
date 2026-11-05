//! Shared device enrollment and authenticated transport to directly paired servers.
use crate::protocol::{
    AgentView, ChildRequest, DEVICES, Device, Follow, Live, Mail, Notifications, Push, SessionId,
    now,
};
use reqwest::{Method, StatusCode};
use serde_json::Value;
use std::{
    fmt,
    io::ErrorKind,
    path::{Path as FilePath, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::{
    fs::OpenOptions,
    io::AsyncWriteExt,
    sync::{RwLock, watch},
};

pub const KEY: &str = "device.key";
pub(crate) const HOP: &str = "x-razor-forwarded";
pub(crate) type Gone = watch::Receiver<()>;
const MOST: usize = 64;

/// The keys this server accepts. Each device's open streams hold a receiver of that device's
/// channel; removing the device drops the sender, which ends exactly those streams.
pub(crate) struct Devices {
    path: PathBuf,
    http: reqwest::Client,
    child: reqwest::Client,
    mail: reqwest::Client,
    local: String,
    /// Never dropped: the local key's streams end only with the server.
    forever: watch::Sender<()>,
    enrolled: RwLock<Vec<(Device, watch::Sender<()>)>>,
    /// Each open session stream's device (None for the local key) and session; the stream holds
    /// the strong half. One entry per open stream at most: ended ones go at the next open.
    streams: Mutex<Vec<Stream>>,
}

type Stream = (Option<String>, SessionId, Weak<()>);

impl Devices {
    /// Read `device.key`, making it at the first start, and `devices.json`.
    pub(crate) async fn open(home: &FilePath) -> Result<Devices, Error> {
        let local = kept(&home.join(KEY), 32, "Reading or making device.key").await?;
        let path = home.join(DEVICES);
        let enrolled: Vec<Device> = match tokio::fs::read(&path).await {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                Error::Refused(format!(
                    "devices.json does not parse ({e}); fix it, or remove it and enroll the devices again."
                ))
            })?,
            Err(e) if e.kind() == ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(other("Reading devices.json")(e)),
        };
        let enrolled = enrolled
            .into_iter()
            .map(|d| (d, watch::channel(()).0))
            .collect();
        Ok(Devices {
            path,
            // Reads and relay reuse their pool with two seconds to connect.
            http: client(2, usize::MAX)?,
            // Fresh child connections preserve "not sent" on refusal after an earlier read.
            child: client(2, 0)?,
            // Mail also connects afresh, with its five-second connection deadline.
            mail: client(5, 0)?,
            local,
            forever: watch::channel(()).0,
            enrolled: RwLock::new(enrolled),
            streams: Mutex::default(),
        })
    }

    /// Current direct pairings, with the receiver each open stream watches for revocation.
    pub(crate) async fn paired(&self) -> Vec<(Device, Gone)> {
        let enrolled = self.enrolled.read().await;
        enrolled
            .iter()
            .filter(|(d, _)| d.address.is_some() && d.key.is_some())
            .map(|(d, gone)| (d.clone(), gone.subscribe()))
            .collect()
    }

    /// Changes the enrolled device `name`, if it still is, and rewrites `devices.json`.
    pub(crate) async fn change(
        &self,
        name: &str,
        change: impl FnOnce(&mut Device),
    ) -> Result<(), Error> {
        let mut enrolled = self.enrolled.write().await;
        let Some(i) = enrolled.iter().position(|(d, _)| d.name == name) else {
            return Ok(());
        };
        let mut device = enrolled[i].0.clone();
        change(&mut device);
        let all = enrolled.iter().enumerate();
        let all: Vec<&Device> = all
            .map(|(j, (d, _))| if i == j { &device } else { d })
            .collect();
        self.save(&all).await?;
        enrolled[i].0 = device;
        Ok(())
    }

    /// Each device's notification token and what its notifications bring, by its name.
    pub(crate) async fn pushes(&self) -> Vec<(String, Push, Notifications)> {
        let enrolled = self.enrolled.read().await;
        let pushes = enrolled.iter().filter_map(|(d, _)| {
            let notifications = d.notifications.clone().unwrap_or_default();
            Some((d.name.clone(), d.push.clone()?, notifications))
        });
        pushes.collect()
    }

    /// Marks `session`'s stream open on `device` until the answer is dropped.
    pub(crate) fn stream(&self, device: Option<String>, session: &SessionId) -> Arc<()> {
        let held = Arc::new(());
        let mut streams = self.streams.lock().unwrap_or_else(|e| e.into_inner());
        streams.retain(|(.., w)| w.strong_count() > 0);
        streams.push((device, session.clone(), Arc::downgrade(&held)));
        held
    }

    /// The enrolled devices with `session`'s stream open.
    pub(crate) fn watching(&self, session: &SessionId) -> Vec<String> {
        let streams = self.streams.lock().unwrap_or_else(|e| e.into_inner());
        let here = streams
            .iter()
            .filter(|(_, s, w)| s == session && w.strong_count() > 0);
        here.filter_map(|(d, ..)| d.clone()).collect()
    }

    /// Every device's followed sessions, by its name.
    pub(crate) async fn follows(&self) -> Vec<(String, Follow)> {
        let enrolled = self.enrolled.read().await;
        let follows = enrolled
            .iter()
            .flat_map(|(d, _)| d.follows.iter().map(|f| (d.name.clone(), f.clone())));
        follows.collect()
    }

    /// Removes `token` from the device `name`, as its notification token or a follow's.
    pub(crate) async fn forget(&self, name: &str, token: &str) -> Result<(), Error> {
        self.change(name, |d| {
            d.push.take_if(|p| p.token == token);
            d.follows.retain(|f| f.push.token != token);
        })
        .await
    }

    /// Notification preferences for this enrolled device, or the defaults after revocation.
    pub(crate) async fn notifications(&self, name: &str) -> Notifications {
        let enrolled = self.enrolled.read().await;
        enrolled
            .iter()
            .find(|(d, _)| d.name == name)
            .and_then(|(d, _)| d.notifications.clone())
            .unwrap_or_default()
    }

    /// The enrolled devices without private keys or notification tokens.
    pub(crate) async fn list(&self) -> Vec<Device> {
        let enrolled = self.enrolled.read().await;
        enrolled
            .iter()
            .map(|(d, _)| Device {
                key: None,
                push: None,
                follows: Vec::new(),
                notifications: None,
                ..d.clone()
            })
            .collect()
    }

    /// The authenticated device and its revocation receiver; None for an unknown key.
    pub(crate) async fn authorize(&self, given: &[u8]) -> Option<(Gone, Option<String>)> {
        let mut found =
            same(given, self.local.as_bytes()).then(|| (self.forever.subscribe(), None));
        for (device, gone) in self.enrolled.read().await.iter() {
            if same(given, device.key.as_deref().unwrap_or_default().as_bytes()) {
                found = Some((gone.subscribe(), Some(device.name.clone())));
            }
        }
        found
    }

    /// Validates and durably enrolls one device; publication follows the successful rename.
    pub(crate) async fn enroll(
        &self,
        name: String,
        address: Option<String>,
        key: Option<String>,
    ) -> Result<Device, Error> {
        let refuse = |status, sentence| Err(Error::Enrollment(status, sentence));
        if name.is_empty()
            || name.len() > 32
            || !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            let sentence =
                format!("A device name is 1 to 32 lowercase letters, digits and -, not {name:?}.");
            return refuse(StatusCode::BAD_REQUEST, sentence);
        }
        let hex = |k: &String| {
            k.len() == 64
                && k.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if key.as_ref().is_some_and(|k| !hex(k)) {
            let sentence = "A pairing key is the 64 hex digits the other server printed.".into();
            return refuse(StatusCode::BAD_REQUEST, sentence);
        }
        let mut enrolled = self.enrolled.write().await;
        if enrolled.iter().any(|(d, _)| d.name == name) {
            let sentence = format!("A device named {name} is enrolled already; remove it first.");
            return refuse(StatusCode::CONFLICT, sentence);
        }
        if enrolled.len() >= MOST {
            let sentence = format!("{MOST} devices are enrolled already; remove one first.");
            return refuse(StatusCode::CONFLICT, sentence);
        }
        let key = match key {
            Some(key) => key,
            None => fresh(32)?,
        };
        let (added, key) = (now(), Some(key));
        let (push, follows) = (None, Vec::new());
        let device = Device {
            name,
            added,
            key,
            address,
            push,
            follows,
            notifications: None,
        };
        let mut all: Vec<&Device> = enrolled.iter().map(|(d, _)| d).collect();
        all.push(&device);
        self.save(&all).await?;
        enrolled.push((device.clone(), watch::channel(()).0));
        Ok(device)
    }

    /// Persists removal before dropping the sender that ends this device's streams.
    pub(crate) async fn revoke(&self, name: &str) -> Result<(), Error> {
        let mut enrolled = self.enrolled.write().await;
        let Some(i) = enrolled.iter().position(|(d, _)| d.name == name) else {
            let sentence = format!("No device named {name} is enrolled.");
            return Err(Error::Enrollment(StatusCode::NOT_FOUND, sentence));
        };
        let rest: Vec<&Device> = (enrolled.iter().enumerate())
            .filter(|(j, _)| *j != i)
            .map(|(_, (d, _))| d)
            .collect();
        self.save(&rest).await?;
        // Dropping the device's sender ends its open streams.
        enrolled.remove(i);
        Ok(())
    }

    /// One bounded child mutation attempt, with no retry after an uncertain acknowledgment.
    pub(crate) async fn child(
        &self,
        server: &str,
        operation: &ChildRequest,
    ) -> Result<Value, Error> {
        let device = self.peer(server).await.ok_or_else(|| {
            Error::Refused(format!(
                "{server} is not paired; pair it before starting a child."
            ))
        })?;
        let response = request(&self.child, &device, Method::POST, "/v1/children").timeout(Duration::from_secs(30))
        .json(operation).send().await.map_err(|e| Error::Refused(if e.is_connect() {
            format!("Could not connect to {server}; the child operation was not sent.")
        } else {
            format!("{server} did not acknowledge the child operation; it may have run. Inspect its session before trying again.")
        }))?;
        let status = response.status();
        let bytes = axum::body::to_bytes(
            axum::body::Body::from_stream(response.bytes_stream()),
            512 << 10,
        )
        .await
        .map_err(other("reading the paired child's answer"))?;
        let body: Value =
            serde_json::from_slice(&bytes).map_err(other("reading the paired child's answer"))?;
        if !status.is_success() {
            return refuse(format!(
                "{server}: {}",
                body["error"]
                    .as_str()
                    .unwrap_or("The child operation was refused.")
            ));
        }
        Ok(body)
    }
    /// One mail attempt with five seconds to connect and ten overall; no mutation retry.
    pub(crate) async fn mail(&self, server: &str, mail: &Mail) -> Result<String, Error> {
        let device = self.peer(server).await
            .ok_or_else(|| Error::Refused(format!(
                "No server named {server} is paired; razor devices add {server} --address <host:port> pairs one."
            )))?;
        let request =
            request(&self.mail, &device, Method::POST, "/v1/mail").timeout(Duration::from_secs(10));
        let answer = match request.json(mail).send().await {
            Ok(answer) => answer,
            Err(e) if e.is_connect() => {
                return refuse(format!("{server} did not answer; nothing was sent."));
            }
            Err(_) => {
                return refuse(format!(
                    "{server} did not answer; the mail may not have arrived."
                ));
            }
        };
        let status = answer.status();
        let body: Value = answer.json().await.unwrap_or_default();
        match (body["session"].as_str(), body["error"].as_str()) {
            (Some(session), _) if status.is_success() => Ok(format!("{session}@{server}")),
            _ if status == reqwest::StatusCode::UNAUTHORIZED => refuse(format!(
                "{server} refused this server's key; pair them again."
            )),
            (_, Some(sentence)) => refuse(format!("{server}: {sentence}")),
            _ => refuse(format!(
                "{server} answered {status}; the mail may not have arrived."
            )),
        }
    }
    /// A current direct pairing, without cloning unrelated enrollment or subscribing streams.
    pub(crate) async fn peer(&self, server: &str) -> Option<Device> {
        let enrolled = self.enrolled.read().await;
        enrolled
            .iter()
            .find(|(d, _)| d.name == server && d.address.is_some() && d.key.is_some())
            .map(|(d, _)| d.clone())
    }

    /// Uses this peer's key and the shared pool, with two seconds to connect.
    pub(crate) fn request(
        &self,
        device: &Device,
        method: Method,
        path: &str,
    ) -> reqwest::RequestBuilder {
        request(&self.http, device, method, path)
    }

    /// A bounded JSON read within two seconds; no recursive forwarding.
    pub(crate) async fn fetch<T: serde::de::DeserializeOwned>(
        &self,
        device: &Device,
        path: &str,
        query: &impl serde::Serialize,
        bound: usize,
    ) -> Option<T> {
        let response = self
            .request(device, Method::GET, path)
            .query(query)
            .header(HOP, "1")
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?;
        let body = axum::body::Body::from_stream(response.bytes_stream());
        serde_json::from_slice(&axum::body::to_bytes(body, bound).await.ok()?).ok()
    }

    /// A fresh status line within two seconds, independent of the API's retained snapshot.
    pub(crate) async fn snapshot(&self, device: &Device) -> Option<Vec<AgentView>> {
        let request = self.request(device, Method::GET, "/v1/status?local=true");
        let read = async {
            let mut answer = request.send().await.ok()?;
            let mut line = Vec::new();
            while !line.contains(&b'\n') && line.len() < 8 << 20 {
                line.extend_from_slice(&answer.chunk().await.ok()??);
            }
            let end = line.iter().position(|b| *b == b'\n')?;
            match serde_json::from_slice(&line[..end]).ok()? {
                Live::Agents { agents, .. } => Some(agents),
                _ => None,
            }
        };
        tokio::time::timeout(Duration::from_secs(2), read)
            .await
            .ok()?
    }

    /// Rewrite `devices.json` (mode 0600) by write-and-rename, so a crash leaves the old list or
    /// the new one.
    async fn save(&self, devices: &[&Device]) -> Result<(), Error> {
        let new = self.path.with_extension("json.new");
        let write = async {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(&new)
                .await?;
            file.write_all(&serde_json::to_vec(devices)?).await?;
            file.sync_all().await?;
            tokio::fs::rename(&new, &self.path).await
        };
        write.await.map_err(other("Writing devices.json"))
    }
}

/// The hex in the file at `path`, made at the first start from `bytes` random bytes (mode 0600).
pub(crate) async fn kept(
    path: &FilePath,
    bytes: usize,
    context: &'static str,
) -> Result<String, Error> {
    let made = match tokio::fs::read_to_string(path).await {
        Ok(text) => return Ok(text.trim().to_string()),
        Err(e) if e.kind() == ErrorKind::NotFound => fresh(bytes)?,
        Err(e) => return Err(other(context)(e)),
    };
    let mut options = OpenOptions::new();
    let options = options.create_new(true).write(true).mode(0o600);
    let mut file = options.open(path).await.map_err(other(context))?;
    file.write_all(made.as_bytes())
        .await
        .map_err(other(context))?;
    Ok(made)
}

/// `n` random bytes in hex.
pub(crate) fn fresh(n: usize) -> Result<String, Error> {
    let mut bytes = vec![0u8; n];
    aws_lc_rs::rand::fill(&mut bytes).map_err(other("Making random bytes"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Every byte is compared, so the time taken says nothing about the key. No key is never one.
fn same(given: &[u8], key: &[u8]) -> bool {
    !given.is_empty()
        && given.len() == key.len()
        && given.iter().zip(key).fold(0, |d, (a, b)| d | (a ^ b)) == 0
}

fn request(
    client: &reqwest::Client,
    device: &Device,
    method: Method,
    path: &str,
) -> reqwest::RequestBuilder {
    let address = device.address.as_deref().unwrap_or_default();
    let key = device.key.as_deref().unwrap_or_default();
    client
        .request(method, format!("http://{address}{path}"))
        .bearer_auth(key)
}

fn client(seconds: u64, idle: usize) -> Result<reqwest::Client, Error> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(seconds))
        .pool_max_idle_per_host(idle)
        .build()
        .map_err(other("Starting paired connections"))
}
/// A refused operation with its HTTP status, or a failure carrying its source and context.
#[derive(Debug)]
pub enum Error {
    Refused(String),
    Enrollment(StatusCode, String),
    Other {
        source: Box<dyn std::error::Error + Send + Sync>,
        context: &'static str,
    },
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(sentence) | Self::Enrollment(_, sentence) => f.write_str(sentence),
            Self::Other { source, context } => write!(f, "{context} failed: {source}."),
        }
    }
}
impl std::error::Error for Error {}
pub(crate) fn other<E: std::error::Error + Send + Sync + 'static>(
    context: &'static str,
) -> impl FnOnce(E) -> Error {
    move |source| Error::Other {
        source: Box::new(source),
        context,
    }
}
fn refuse<T>(sentence: String) -> Result<T, Error> {
    Err(Error::Refused(sentence))
}
