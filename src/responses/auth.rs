//! Razor's own ChatGPT sign-in: `$RAZOR_HOME/auth.json`, or `$RAZOR_HOME/key` for the public API.

use super::{Error, Responses, bounded, other};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio::{fs, io::AsyncReadExt, sync::mpsc, time::Instant};

const AUTH: &str = "https://auth.openai.com";
const CLIENT: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// The most bytes a credential file or an auth response may hold.
const FILE: usize = 1 << 20;

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct Tokens {
    id_token: String,
    pub(super) access_token: String,
    refresh_token: String,
    pub(super) account_id: String,
}

#[derive(Deserialize, Serialize)]
struct Auth {
    tokens: Tokens,
}

impl Responses {
    /// The bearer for one request. A sign-in token is refreshed within five minutes of expiry, or
    /// when `stale` names the token a 401 just refused.
    pub(super) async fn credentials(
        &self,
        public: bool,
        stale: Option<&str>,
    ) -> Result<Tokens, Error> {
        let _lock = self.auth.lock().await;
        if public {
            let bytes = read(&self.home.join("key"))
                .await?
                .ok_or_else(|| Error::Refused(super::key_missing(&self.home)))?;
            let key = String::from_utf8(bytes).map_err(|_| Error::Auth)?;
            let access_token = Some(key.trim())
                .filter(|k| !k.is_empty())
                .ok_or(Error::Auth)?
                .into();
            return Ok(Tokens {
                access_token,
                ..Tokens::default()
            });
        }
        let bytes = read(&self.home.join("auth.json"))
            .await?
            .ok_or_else(|| Error::Refused(super::SIGN_IN_MISSING.into()))?;
        let mut auth: Auth = serde_json::from_slice(&bytes).map_err(|_| Error::Auth)?;
        let expiry = claims(&auth.tokens.access_token)?["exp"]
            .as_u64()
            .ok_or(Error::Auth)?;
        if stale != Some(&auth.tokens.access_token) && expiry > crate::protocol::now() / 1000 + 300
        {
            return Ok(auth.tokens);
        }
        let refresh = json!({"client_id": CLIENT, "grant_type": "refresh_token", "refresh_token": auth.tokens.refresh_token});
        let fresh = answer(
            self.client
                .post(format!("{AUTH}/oauth/token"))
                .json(&refresh)
                .send()
                .await,
        )
        .await?;
        let tokens = &mut auth.tokens;
        for (key, token) in [
            ("id_token", &mut tokens.id_token),
            ("access_token", &mut tokens.access_token),
            ("refresh_token", &mut tokens.refresh_token),
        ] {
            if let Some(value) = fresh[key].as_str() {
                *token = value.into();
            }
        }
        self.save(&auth).await?;
        Ok(auth.tokens)
    }

    /// Writes a unique 0600 sibling, syncs it, then renames it over `auth.json`.
    async fn save(&self, auth: &Auth) -> Result<(), Error> {
        fs::create_dir_all(&self.home)
            .await
            .map_err(other("making Razor's sign-in directory"))?;
        let path = self
            .home
            .join(format!("auth-{}.tmp", crate::protocol::SessionId::new()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&path)
            .await
            .map_err(other("creating the sign-in file"))?;
        let bytes = serde_json::to_vec(auth).map_err(other("encoding sign-in credentials"))?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &bytes)
            .await
            .map_err(other("writing the sign-in file"))?;
        file.sync_all()
            .await
            .map_err(other("syncing the sign-in file"))?;
        fs::rename(&path, self.home.join("auth.json"))
            .await
            .map_err(other("saving sign-in credentials"))
    }

    /// Codex's device-code flow: show the code, poll for at most fifteen minutes, then exchange.
    pub(super) async fn device_login(
        &self,
        instructions: mpsc::Sender<String>,
    ) -> Result<(), Error> {
        let _lock = self.auth.lock().await;
        let post = |path: &str| self.client.post(format!("{AUTH}{path}"));
        let code = answer(
            post("/api/accounts/deviceauth/usercode")
                .json(&json!({"client_id": CLIENT}))
                .send()
                .await,
        )
        .await?;
        let user = code["user_code"]
            .as_str()
            .or_else(|| code["usercode"].as_str())
            .ok_or(Error::Auth)?;
        let id = code["device_auth_id"].as_str().ok_or(Error::Auth)?;
        let show = format!(
            "Open https://auth.openai.com/codex/device and enter {user}; continue only if you started this Razor sign-in."
        );
        instructions.send(show).await.map_err(|_| Error::Stopped)?;
        let interval = code["interval"]
            .as_u64()
            .or_else(|| code["interval"].as_str()?.parse().ok());
        let interval = Duration::from_secs(interval.unwrap_or(5).clamp(1, 60));
        let deadline = Instant::now() + Duration::from_secs(900);
        let granted = loop {
            let poll = post("/api/accounts/deviceauth/token")
                .json(&json!({"device_auth_id": id, "user_code": user}));
            let response = tokio::time::timeout_at(deadline, poll.send())
                .await
                .map_err(|_| Error::Auth)?;
            match response {
                Ok(r) if matches!(r.status().as_u16(), 403 | 404) && Instant::now() < deadline => {
                    tokio::time::sleep_until((Instant::now() + interval).min(deadline)).await;
                }
                response => break answer(response).await?,
            }
        };
        let (code, verifier) = (
            granted["authorization_code"].as_str(),
            granted["code_verifier"].as_str(),
        );
        let form = [
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT),
            ("code", code.ok_or(Error::Auth)?),
            ("code_verifier", verifier.ok_or(Error::Auth)?),
            (
                "redirect_uri",
                "https://auth.openai.com/deviceauth/callback",
            ),
        ];
        let tokens = answer(post("/oauth/token").form(&form).send().await).await?;
        let mut tokens: Tokens = serde_json::from_value(tokens).map_err(|_| Error::Auth)?;
        let account = claims(&tokens.id_token)?
            .pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        tokens.account_id = account.ok_or(Error::Auth)?;
        self.save(&Auth { tokens }).await
    }
}

async fn read(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    let mut bytes = Vec::new();
    let file = match fs::File::open(path).await {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Error::Auth),
    };
    file.take(FILE as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| Error::Auth)?;
    if bytes.len() > FILE {
        Err(Error::Auth)
    } else {
        Ok(Some(bytes))
    }
}

fn claims(token: &str) -> Result<Value, Error> {
    let payload = URL_SAFE_NO_PAD
        .decode(token.split('.').nth(1).ok_or(Error::Auth)?)
        .map_err(|_| Error::Auth)?;
    serde_json::from_slice(&payload).map_err(|_| Error::Auth)
}

/// An auth endpoint's JSON answer; its body never reaches an error, even when malformed.
async fn answer(response: reqwest::Result<reqwest::Response>) -> Result<Value, Error> {
    let response = response
        .ok()
        .filter(|r| r.status().is_success())
        .ok_or(Error::Auth)?;
    serde_json::from_slice(&bounded(response, FILE).await.map_err(|_| Error::Auth)?)
        .map_err(|_| Error::Auth)
}
