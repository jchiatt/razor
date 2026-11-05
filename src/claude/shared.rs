//! Lazy process ownership and per-conversation routing, including its inbound byte permits.

use super::{Bridge, Error, FromBridge, Line, Lines, ToBridge, task};
use crate::{config, profiles::Profile, protocol::SessionId};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::{process::Stdio, sync::Arc, time::Duration, time::Instant};
use tokio::{
    io::AsyncReadExt,
    process::Command,
    sync::{Mutex, OwnedSemaphorePermit, mpsc, watch},
};

/// A native line retains its share of the bridge budget through controller admission.
pub(crate) type Output = (Line, Option<OwnedSemaphorePermit>);
type Route = (u64, mpsc::Sender<Output>);
/// How long after its close a resume may reuse a conversation's sign-in: the prompt cache's
/// lifetime (the bridge asks for an hour), which is the account's, so a resume on another account
/// writes the whole conversation to the cache again.
const KEEP: Duration = Duration::from_secs(3600);
/// How many closed conversations keep a sign-in: about three times the most sessions the busiest
/// server opened in one hour (22, 29 to 31 October), so in practice only a sign-in past its hour
/// is pushed out.
const KEPT: usize = 64;

/// One lazy Node process; only its own conversations hear about an exit. The path is the folder
/// a conversation signed in by command keeps the CLI's files in, so a resume signed in to
/// another account still finds its conversation.
#[derive(Clone)]
pub(crate) struct SharedBridge(Arc<Mutex<State>>, Arc<PathBuf>);
#[derive(Default)]
struct State {
    bridge: Option<Bridge>,
    routes: HashMap<SessionId, Route>,
    /// Each closed conversation's variables and account, and when it closed, oldest first: one per
    /// session, at most KEPT.
    signed: VecDeque<(SessionId, (Value, Value), Instant)>,
}

impl SharedBridge {
    /// The bridge of the server in `home`.
    pub(crate) fn new(home: &Path) -> Self {
        Self(Default::default(), Arc::new(home.join("claude")))
    }

    /// Open starts a conversation task with the same CodeMode owner as Responses.
    /// Both queues must be drained together; native byte permits reach `outgoing` unchanged.
    /// `claude` is `[claude]` as the session reads it now.
    pub(crate) async fn start(
        &self,
        dir: PathBuf,
        claude: &config::Claude,
        child: bool,
        profile: watch::Receiver<Arc<Profile>>,
        mut incoming: mpsc::Receiver<ToBridge>,
        outgoing: mpsc::Sender<Output>,
    ) -> Result<(), Error> {
        let message = incoming.recv().await.ok_or(Error::Exited)?;
        let ToBridge::Open { session, open, .. } = &message else {
            return Err(Error::Exited);
        };
        let (session, open) = (session.clone(), *open);
        let mut wire = json!(message);
        let resumed = !wire["resume"].is_null();
        let kept = {
            let kept = &mut self.0.lock().await.signed;
            let at = kept.iter().position(|(id, ..)| *id == session);
            at.and_then(|at| kept.remove(at))
        };
        let signed = match kept.filter(|(.., closed)| resumed && closed.elapsed() < KEEP) {
            Some((_, signed, _)) => signed,
            // Outside the lock: an account pick may take a second, and every line waits on it.
            None => env(&claude.sign_in, &self.1).await?,
        };
        (wire["env"], wire["account"]) = signed.clone();
        if child {
            wire["env"]["CLAUDE_CODE_SILENT_TURN_REMINDER"] = json!("false");
        }
        // Invariant: a path parsed from TOML is UTF-8, so it serializes.
        wire["usage_file"] = json!(claude.usage_file);
        let mut state = self.0.lock().await;
        if state.bridge.is_none() {
            let (bridge, lines) = Bridge::start().await?;
            state.bridge = Some(bridge);
            tokio::spawn(self.clone().route(lines));
        }
        let bridge = state.bridge.clone().ok_or(Error::Exited)?;
        let (send, lines) = mpsc::channel(64);
        state.routes.insert(session.clone(), (open, send.clone()));
        bridge.send(&wire).await?;
        drop(state);
        let shared = self.clone();
        tokio::spawn(async move {
            task::serve(
                bridge.clone(),
                (session.clone(), open),
                dir,
                profile,
                incoming,
                lines,
                outgoing,
            )
            .await;
            let mut state = shared.0.lock().await;
            if !signed.0.is_null() {
                let kept = &mut state.signed;
                kept.retain(|(id, ..)| *id != session);
                if kept.len() == KEPT {
                    kept.pop_front();
                }
                kept.push_back((session.clone(), signed, Instant::now()));
            }
            if state
                .routes
                .get(&session)
                .is_some_and(|(_, current)| current.same_channel(&send))
            {
                state.routes.remove(&session);
                let _ = bridge.send(&ToBridge::Close { session }).await;
            }
        });
        Ok(())
    }

    /// Hands each line the bridge writes to its conversation while that conversation is open; once
    /// the bridge exits, every open conversation hears that it closed.
    async fn route(self, mut lines: Lines) {
        while let Some((line, held)) = lines.recv().await {
            let route = self.0.lock().await.routes.get(&line.session).cloned();
            if let Some((open, send)) = route
                && open == line.open
            {
                let _ = send.send((line, Some(held))).await;
            }
        }
        let routes = {
            let mut state = self.0.lock().await;
            state.bridge = None;
            std::mem::take(&mut state.routes)
        };
        for (session, (open, send)) in routes {
            let line = Line {
                session,
                open,
                says: FromBridge::Closed {
                    error: Some(Error::Exited.to_string()),
                },
            };
            let _ = send.send((line, None)).await;
        }
    }
}

/// The variables one conversation's CLI starts with, and its account: none, or the token on the
/// first line `command` prints in place of every other sign-in, with the CLI's files in `folder`,
/// and the account its optional second line names. The token goes only to the bridge, never into
/// a protocol message or the record.
pub(crate) async fn env(command: &[String], folder: &Path) -> Result<(Value, Value), Error> {
    let Some((program, args)) = command.split_first() else {
        return Ok((Value::Null, Value::Null));
    };
    let mut run = Command::new(program);
    // Its stderr (an account pick, a failure) reaches the server's log.
    run.args(args).stdin(Stdio::null()).stdout(Stdio::piped());
    run.kill_on_drop(true);
    let read = async {
        let mut child = run.spawn()?;
        // Invariant: stdout is piped above. A token is one line; past 64 KiB the pipe closes.
        let stdout = child.stdout.take().expect("the command's stdout is piped");
        let mut out = Vec::new();
        stdout.take(64 << 10).read_to_end(&mut out).await?;
        Ok::<_, std::io::Error>((child.wait().await?, out))
    };
    let why = match tokio::time::timeout(Duration::from_secs(30), read).await {
        Ok(Ok((status, out))) if status.success() => {
            let out = String::from_utf8_lossy(&out);
            let mut lines = out.lines().map(str::trim);
            if let Some(token) = lines.next().filter(|token| !token.is_empty()) {
                let mut env = json!({"CLAUDE_CONFIG_DIR": folder});
                for key in super::KEYS {
                    env[key] = json!("");
                }
                env["CLAUDE_CODE_OAUTH_TOKEN"] = json!(token);
                let account = lines.next().filter(|account| !account.is_empty());
                return Ok((env, json!(account)));
            }
            "it printed no token".into()
        }
        Ok(Ok((status, _))) => status.to_string(),
        Ok(Err(e)) => e.to_string(),
        Err(_) => "no exit within 30 seconds".into(),
    };
    Err(Error::Missing(format!(
        "The Claude sign-in command {program} failed: {why}."
    )))
}
