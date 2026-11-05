use super::super::{Error, FILE, other};
use super::resolve;
use crate::protocol::{Connection, Mcp, OAuth};
use crate::protocol::{EntryId, SessionId};
use rmcp::transport::auth::{
    AuthError, AuthorizationManager, CredentialRefreshGuard, CredentialStore, OAuthClientConfig,
    StoredCredentials,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::{Mutex, broadcast};

/// The machine's sign-ins, credential rotation and reconnect requests, shared by every session;
/// clones share one lock over the credential file.
#[derive(Clone)]
pub(crate) struct Auth(Arc<Shared>);

struct Shared {
    file: PathBuf,
    lock: Arc<Mutex<()>>,
    /// One unfinished authorization per server, replaced by its next sign-in.
    pending: Mutex<BTreeMap<String, Pending>>,
    /// A server's name, or every server, for each session's connections to start over.
    reconnects: broadcast::Sender<Option<String>>,
}

struct Pending {
    state: String,
    redirect: reqwest::Url,
    manager: AuthorizationManager,
    /// The question showing its address in the session that asked, once recorded.
    question: Option<(SessionId, EntryId)>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct Client {
    id: String,
    secret: Option<String>,
    redirect: String,
}

/// One saved sign-in, keyed by the server's name and full URL.
#[derive(Serialize, Deserialize)]
struct Saved {
    oauth: Option<OAuth>,
    client: Client,
    credentials: StoredCredentials,
}

#[derive(Clone)]
struct Store {
    auth: Auth,
    key: String,
    oauth: Option<OAuth>,
    client: Client,
    issuer: String,
}

impl Auth {
    /// Reads nothing and starts no network work.
    pub(crate) fn new(home: PathBuf) -> Self {
        Self(Arc::new(Shared {
            file: home.join("mcp-auth.json"),
            lock: Arc::default(),
            pending: Mutex::default(),
            reconnects: broadcast::channel(16).0,
        }))
    }

    /// Asks every loaded session's connection to `name`, or all of them, to start over.
    pub(crate) fn reconnect(&self, name: Option<&str>) {
        let _ = self.0.reconnects.send(name.map(str::to_owned));
    }

    pub(super) fn reconnects(&self) -> broadcast::Receiver<Option<String>> {
        self.0.reconnects.subscribe()
    }

    /// Returns a PKCE authorization address; no listener, browser or agent turn starts.
    pub(crate) async fn sign_in(
        &self,
        name: &str,
        config: &Mcp,
        https: Option<&str>,
    ) -> Result<String, Error> {
        let (url, oauth) = remote(config)?;
        let redirect = callback(https)?;
        let _guard = self.0.lock.lock().await;
        self.0.pending.lock().await.remove(name);
        let (mut manager, issuer) = discovered(url, oauth.as_ref()).await?;
        let key = key(name, url);
        let saved = self.read()?.remove(&key);
        let scopes = manager.select_initial_scopes(&[]).await;
        let scopes: Vec<_> = scopes.iter().map(String::as_str).collect();
        let redirected = redirect.to_string();
        // A configured client comes first, then the one registered earlier for this issuer and
        // redirect, then a new registration.
        let reusable = saved.filter(|s| {
            s.oauth == *oauth
                && s.credentials.issuer.as_deref() == Some(&issuer)
                && s.client.redirect == redirected
        });
        let client = match (oauth.as_ref().and_then(|o| o.client_id.clone()), reusable) {
            (Some(id), _) => Client {
                id,
                secret: None,
                redirect: redirected,
            },
            (None, Some(saved)) => saved.client,
            (None, None) => {
                let registered = manager.register_client("Razor", &redirected, &scopes).await;
                let registered = registered.map_err(|_| {
                    refused("The MCP server could not register Razor; configure an OAuth client ID and retry.")
                })?;
                let (id, secret) = (registered.client_id, registered.client_secret);
                Client {
                    id,
                    secret,
                    redirect: redirected,
                }
            }
        };
        let store = Store {
            auth: self.clone(),
            key,
            oauth: oauth.clone(),
            client,
            issuer,
        };
        store.configure(&mut manager).await?;
        let url = manager.get_authorization_url(&scopes).await.map_err(|_| {
            refused(
                "The MCP server could not start sign-in; check its OAuth configuration and retry.",
            )
        })?;
        let state = reqwest::Url::parse(&url).ok().and_then(|u| {
            u.query_pairs()
                .find(|(k, _)| k == "state")
                .map(|(_, v)| v.into_owned())
        });
        let state = state
            .ok_or_else(|| refused("The MCP server gave no authorization state; retry sign-in."))?;
        let attempt = Pending {
            state,
            redirect,
            manager,
            question: None,
        };
        self.0.pending.lock().await.insert(name.to_owned(), attempt);
        Ok(url)
    }

    /// Ties `name`'s pending sign-in to the question showing its address in a session.
    pub(crate) async fn asked(&self, name: &str, question: (SessionId, EntryId)) {
        if let Some(attempt) = self.0.pending.lock().await.get_mut(name) {
            attempt.question = Some(question);
        }
    }

    /// The question a landed address's pending sign-in was asked in; none when no session asked.
    pub(crate) async fn question(&self, redirect: &str) -> Option<(SessionId, EntryId)> {
        let url = reqwest::Url::parse(redirect).ok()?;
        let state = url.query_pairs().find(|(k, _)| k == "state")?.1;
        let pending = self.0.pending.lock().await;
        let attempt = pending.values().find(|p| p.state == state)?;
        attempt.question.clone()
    }

    /// Consumes one pending sign-in by its state, checks the whole callback address, and saves
    /// the credentials before reporting success; every session's connections then start over.
    pub(crate) async fn complete(&self, redirect: &str) -> Result<(), Error> {
        let mut url = reqwest::Url::parse(redirect).map_err(|_| {
            refused("The MCP redirect is not a URL; paste the whole browser address.")
        })?;
        let mut params = BTreeMap::new();
        for (name, value) in url.query_pairs() {
            if params
                .insert(name.into_owned(), value.into_owned())
                .is_some()
            {
                return Err(refused(
                    "The MCP redirect repeats a parameter; start sign-in again.",
                ));
            }
        }
        let gone = || refused("The MCP sign-in is no longer pending; start sign-in again.");
        let state = params.get("state").ok_or_else(gone)?;
        let _guard = self.0.lock.lock().await;
        let mut pending = self.0.pending.lock().await;
        let name = pending
            .iter()
            .find(|(_, p)| &p.state == state)
            .map(|(n, _)| n.clone());
        let name = name.ok_or_else(gone)?;
        url.set_query(None);
        if pending.get(&name).is_some_and(|p| p.redirect != url) {
            return Err(refused(
                "The MCP redirect has the wrong address; paste the one this sign-in registered.",
            ));
        }
        let attempt = pending.remove(&name).ok_or_else(gone)?;
        drop(pending);
        if params.contains_key("error") {
            return Err(refused(
                "The MCP sign-in was declined; start sign-in again when ready.",
            ));
        }
        let code = params
            .get("code")
            .ok_or_else(|| refused("The MCP redirect has no code; start sign-in again."))?;
        // rmcp checks the state-bound issuer before exchanging the PKCE verifier.
        let issuer = params.get("iss").map(String::as_str);
        let exchanged = attempt
            .manager
            .exchange_code_for_token_with_issuer(code, state, issuer)
            .await;
        exchanged
            .map_err(|_| refused("The MCP sign-in could not complete; start sign-in again."))?;
        self.reconnect(None);
        Ok(())
    }

    /// Forgets the tokens under the lock refresh and completion share; the client stays.
    pub(crate) async fn sign_out(&self, name: &str, config: &Mcp) -> Result<(), Error> {
        let (url, _) = remote(config)?;
        let _guard = self.0.lock.lock().await;
        self.0.pending.lock().await.remove(name);
        let mut saved = self.read()?;
        if let Some(entry) = saved.get_mut(&key(name, url)) {
            (
                entry.credentials.token_response,
                entry.credentials.token_received_at,
            ) = (None, None);
        }
        self.write(&saved)
    }

    /// Whether tokens are saved for this exact URL and configured client.
    pub(crate) async fn signed_in(&self, name: &str, config: &Mcp) -> bool {
        let _guard = self.0.lock.lock().await;
        self.saved(name, config).is_some()
    }

    /// A manager over saved credentials, or `None` before any sign-in.
    pub(super) async fn manager(
        &self,
        name: &str,
        config: &Mcp,
    ) -> Result<Option<AuthorizationManager>, Error> {
        let (url, oauth) = remote(config)?;
        let _guard = self.0.lock.lock().await;
        let Some(saved) = self.saved(name, config) else {
            return Ok(None);
        };
        let (mut manager, issuer) = discovered(url, oauth.as_ref()).await?;
        if saved.credentials.issuer.as_deref() != Some(&issuer) {
            return Err(refused(
                "The MCP authorization server changed; sign in again.",
            ));
        }
        let (key, oauth, client) = (key(name, url), oauth.clone(), saved.client);
        let store = Store {
            auth: self.clone(),
            key,
            oauth,
            client,
            issuer,
        };
        store.configure(&mut manager).await?;
        Ok(Some(manager))
    }

    fn saved(&self, name: &str, config: &Mcp) -> Option<Saved> {
        let (url, oauth) = remote(config).ok()?;
        let saved = self.read().ok()?.remove(&key(name, url))?;
        (saved.oauth == *oauth && saved.credentials.token_response.is_some()).then_some(saved)
    }

    fn read(&self) -> Result<BTreeMap<String, Saved>, Error> {
        let mut bytes = Vec::new();
        match std::fs::File::open(&self.0.file) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            file => file
                .and_then(|file| file.take(FILE + 1).read_to_end(&mut bytes))
                .map_err(|e| other(e, "Could not read MCP credentials."))?,
        };
        if bytes.len() as u64 > FILE {
            return Err(refused(
                "The MCP credential file is over 64 MiB; remove unused sign-ins.",
            ));
        }
        serde_json::from_slice(&bytes).map_err(|_| {
            refused("The MCP credential file is invalid; restore it before signing in.")
        })
    }

    /// Replaces the credential file atomically, readable by its owner alone.
    fn write(&self, saved: &BTreeMap<String, Saved>) -> Result<(), Error> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let next = NEXT.fetch_add(1, Ordering::Relaxed);
        let temp = self
            .0
            .file
            .with_extension(format!("{}-{next}.tmp", std::process::id()));
        let bytes =
            serde_json::to_vec(saved).map_err(|e| other(e, "Could not encode MCP credentials."))?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let written = options.open(&temp).and_then(|mut file| {
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(&temp, &self.0.file)
        });
        written.map_err(|e| other(e, "Could not save MCP credentials."))
    }
}

/// Waits for a reconnect request naming `name` or every server; a lagging receiver starts over.
pub(super) async fn requested(reconnects: &mut broadcast::Receiver<Option<String>>, name: &str) {
    loop {
        match reconnects.recv().await {
            Ok(Some(other)) if other != name => {}
            Err(broadcast::error::RecvError::Closed) => std::future::pending().await,
            _ => return,
        }
    }
}

/// The address every sign-in registers as its redirect, as the URL parser spells it.
pub(crate) fn callback(https: Option<&str>) -> Result<reqwest::Url, Error> {
    let base = https.unwrap_or("http://localhost").trim_end_matches('/');
    let redirect = reqwest::Url::parse(&format!("{base}/v1/mcp/callback"))
        .ok()
        .filter(|r| {
            (https.is_none() || r.scheme() == "https")
                && r.host_str().is_some()
                && r.username().is_empty()
                && r.password().is_none()
                && r.query().is_none()
                && r.fragment().is_none()
        });
    redirect.ok_or_else(|| {
        refused("The MCP callback needs a clean HTTPS address; fix server.https_url.")
    })
}

/// Discovers the authorization server from the resource's challenge; it must offer S256 PKCE,
/// name its issuer, and match the configured one.
async fn discovered(
    url: &str,
    oauth: Option<&OAuth>,
) -> Result<(AuthorizationManager, String), Error> {
    let mut manager = AuthorizationManager::new(url)
        .await
        .map_err(|_| refused("The MCP server address is invalid; check its URL."))?;
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build();
    let http = http.map_err(|e| other(e, "Could not create the MCP sign-in client."))?;
    let response = http.get(url).send().await.ok();
    let challenge = response
        .as_ref()
        .and_then(|r| r.headers().get(reqwest::header::WWW_AUTHENTICATE));
    let metadata = manager
        .resolve_metadata_from_challenge(challenge.and_then(|v| v.to_str().ok()))
        .await;
    let metadata = metadata
        .map_err(|_| {
            refused("The MCP authorization metadata is unavailable; check the server and retry.")
        })?
        .metadata;
    let methods = metadata.code_challenge_methods_supported.as_ref();
    if !methods.is_some_and(|m| m.iter().any(|v| v == "S256")) {
        return Err(refused(
            "The MCP authorization server must offer S256 PKCE; use a compliant server.",
        ));
    }
    let issuer = metadata.issuer.clone().filter(|s| !s.is_empty());
    let issuer = issuer.ok_or_else(|| {
        refused("The MCP authorization server names no issuer; use a compliant server.")
    })?;
    if oauth
        .and_then(|o| o.issuer.as_ref())
        .is_some_and(|expected| *expected != issuer)
    {
        return Err(refused(
            "The MCP authorization issuer is not the configured one; check its issuer.",
        ));
    }
    manager.set_metadata(metadata);
    Ok((manager, issuer))
}

impl Store {
    /// Gives the manager this client, its secret resolved now, and this store for its tokens.
    async fn configure(self, manager: &mut AuthorizationManager) -> Result<(), Error> {
        let mut config = OAuthClientConfig::new(&self.client.id, &self.client.redirect);
        config.client_secret = match self.oauth.as_ref().and_then(|o| o.client_secret.as_ref()) {
            Some(reference) => Some(resolve(reference).await?),
            None => self.client.secret.clone(),
        };
        let configured = manager.configure_client(config);
        configured
            .map_err(|_| refused("The MCP OAuth client is invalid; check its configuration."))?;
        manager.set_credential_store(self);
        Ok(())
    }
}

fn remote(config: &Mcp) -> Result<(&str, &Option<OAuth>), Error> {
    match &config.connection {
        Some(Connection::Http { url, oauth, .. }) => Ok((url, oauth)),
        _ => Err(refused(
            "This MCP server has no URL to sign in to; configure one first.",
        )),
    }
}

fn key(name: &str, url: &str) -> String {
    serde_json::to_string(&(name, url)).unwrap_or_default()
}

fn refused(text: &str) -> Error {
    Error::Refused(text.into())
}

fn stored(_: Error) -> AuthError {
    AuthError::CredentialStoreError(
        "Could not read or save MCP credentials; check the file.".into(),
    )
}

#[async_trait::async_trait]
impl CredentialStore for Store {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let saved = self.auth.read().map_err(stored)?.remove(&self.key);
        Ok(saved
            .filter(|s| {
                s.oauth == self.oauth
                    && s.client == self.client
                    && s.credentials.client_id == self.client.id
                    && s.credentials.issuer.as_deref() == Some(&self.issuer)
            })
            .map(|s| s.credentials))
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let mut saved = self.auth.read().map_err(stored)?;
        let (oauth, client) = (self.oauth.clone(), self.client.clone());
        saved.insert(
            self.key.clone(),
            Saved {
                oauth,
                client,
                credentials,
            },
        );
        self.auth.write(&saved).map_err(stored)
    }

    async fn clear(&self) -> Result<(), AuthError> {
        let mut saved = self.auth.read().map_err(stored)?;
        saved.remove(&self.key);
        self.auth.write(&saved).map_err(stored)
    }

    async fn on_refresh_token_rejected(&self, _: &StoredCredentials) -> Result<(), AuthError> {
        self.clear().await
    }

    async fn acquire_refresh_guard(&self) -> Result<Option<CredentialRefreshGuard>, AuthError> {
        let guard = self.auth.0.lock.clone().lock_owned().await;
        Ok(Some(CredentialRefreshGuard::new(guard)))
    }
}
