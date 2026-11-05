//! Post, when it is installed (docs/design/mail.md): the participant each session acts as, and
//! one `post watch` per participant, whose mail and mentions ring the session. Every call of the
//! `post` program is in `bind` and `watch`, so a change in how Post is called touches those two.

use super::message::words;
use crate::controller::{Controller, Error};
use crate::protocol::{Input, Origin, SessionId};
use crate::shell::Group;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::{process::Command, sync::Mutex, task::JoinHandle};

/// The most sessions with a participant and a watch; past it the oldest is dropped.
const MOST: usize = 32;
/// The longest watch line read; a longer one is skipped.
const LINE: usize = 64 << 10;

/// Cheap to clone; one serves the whole server.
#[derive(Clone)]
pub(crate) struct Post {
    inner: Arc<Inner>,
}

struct Inner {
    home: PathBuf,
    /// Set once the controller that owns this Post exists: a ring is any program's input.
    controller: OnceLock<Controller>,
    /// Each bound session, oldest first, at most `MOST`. Its holder alone writes `post.json`.
    bound: Mutex<Vec<Bound>>,
}

/// A session's participant, the folder it was bound in, and the watch that rings it.
struct Bound {
    session: String,
    participant: String,
    cwd: PathBuf,
    watch: JoinHandle<()>,
}

impl Drop for Bound {
    fn drop(&mut self) {
        self.watch.abort();
    }
}

impl Post {
    /// Binds and watches nothing until `start`.
    pub(super) fn open(home: PathBuf) -> Post {
        let (controller, bound) = (OnceLock::new(), Mutex::default());
        let inner = Arc::new(Inner {
            home,
            controller,
            bound,
        });
        Post { inner }
    }

    /// Rings through `controller` from now on; reads `post.json` and binds and watches each
    /// session it names again. Starts nothing when Post is absent, and never fails.
    pub(crate) fn start(&self, controller: &Controller) {
        let _ = self.inner.controller.set(controller.clone());
        let restart = self.clone();
        tokio::spawn(async move {
            let saved = tokio::fs::read(restart.inner.home.join("post.json")).await;
            let saved: Vec<(String, PathBuf)> =
                serde_json::from_slice(&saved.unwrap_or_default()).unwrap_or_default();
            for (session, cwd) in saved {
                restart.bind(&session, &cwd).await;
            }
        });
    }

    /// The participant `session`'s jobs act as. When `runs_post` and the session has none,
    /// binds one in `cwd` and starts its watch first. None while unbound, or when `post` is
    /// absent or refuses; never an error.
    pub(crate) async fn participant(
        &self,
        session: &str,
        cwd: &Path,
        runs_post: bool,
    ) -> Option<String> {
        let known = (self.inner.bound.lock().await)
            .iter()
            .find(|b| b.session == session)
            .map(|b| b.participant.clone());
        match known {
            None if runs_post => self.bind(session, cwd).await,
            known => known,
        }
    }

    /// Binds `session`'s participant in `cwd` and watches it, then saves the bound sessions.
    async fn bind(&self, session: &str, cwd: &Path) -> Option<String> {
        let mut bind = Command::new("post");
        bind.args([
            "participant",
            "bind",
            "--harness",
            "razor",
            "--key",
            session,
            "--json",
        ]);
        bind.current_dir(cwd)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let bound = tokio::time::timeout(Duration::from_secs(5), bind.output()).await;
        let answer: Value = serde_json::from_slice(&bound.ok()?.ok()?.stdout).ok()?;
        let participant = answer["id"].as_str()?.to_string();
        // Register before an exiting watch can remove itself; hold the lock through the rename.
        let mut all = self.inner.bound.lock().await;
        let watch = self.watch(session, &participant, cwd)?;
        all.retain(|b| b.session != session);
        all.push(Bound {
            session: session.into(),
            participant: participant.clone(),
            cwd: cwd.into(),
            watch,
        });
        let over = all.len().saturating_sub(MOST);
        all.drain(..over);
        self.save(&all).await;
        Some(participant)
    }

    /// A closed session's watch stops, `post.json` forgets it, and its participant ends, so
    /// nothing rings it again; the person's next input and `post` command bind it anew.
    pub(crate) fn closed(&self, session: &str) {
        let (post, session) = (self.clone(), session.to_string());
        tokio::spawn(async move {
            let mut all = post.inner.bound.lock().await;
            let Some(at) = all.iter().position(|b| b.session == session) else {
                return;
            };
            let bound = all.remove(at);
            post.save(&all).await;
            drop(all);
            let mut end = Command::new("post");
            end.args(["participant", "end"]).current_dir(&bound.cwd);
            end.env("POST_PARTICIPANT", &bound.participant)
                .kill_on_drop(true);
            drop(bound);
            let ended = end
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let _ = tokio::time::timeout(Duration::from_secs(5), ended.status()).await;
        });
    }

    /// Writes the bound sessions to `post.json`, under the lock its caller holds.
    async fn save(&self, all: &[Bound]) {
        let saved: Vec<_> = all.iter().map(|b| (&b.session, &b.cwd)).collect();
        let new = self.inner.home.join("post.json.new");
        let written = async {
            tokio::fs::write(&new, serde_json::to_vec(&saved)?).await?;
            tokio::fs::rename(&new, self.inner.home.join("post.json")).await?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        };
        if let Err(e) = written.await {
            eprintln!("razor: post.json cannot be written: {e}");
        }
    }

    /// Starts `post watch --from now` as `participant`, one process in a group of its own,
    /// killed when the watch's task ends, as it does when the server stops; each line whose
    /// reason is mail or a mention rings `session`. A watch that ends by itself is forgotten,
    /// so the session's next `post` command starts it again.
    fn watch(&self, session: &str, participant: &str, cwd: &Path) -> Option<JoinHandle<()>> {
        let mut watch = Command::new("post");
        watch
            .args(["watch", "--from", "now"])
            .env("POST_PARTICIPANT", participant);
        watch.current_dir(cwd).process_group(0);
        let mut child = watch
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut lines = BufReader::new(child.stdout.take()?);
        let (post, session) = (self.clone(), session.to_string());
        Some(tokio::spawn(async move {
            let mut group = Group(child.id());
            let (mut line, mut long) = (Vec::new(), false);
            loop {
                line.clear();
                match (&mut lines)
                    .take(LINE as u64)
                    .read_until(b'\n', &mut line)
                    .await
                {
                    Ok(0) | Err(_) => break,
                    Ok(_) if !line.ends_with(b"\n") => long = true,
                    Ok(_) if std::mem::take(&mut long) => {}
                    Ok(_) => {
                        if let Some(text) = rung(&line) {
                            post.ring(&session, text).await;
                        }
                    }
                }
            }
            let _ = child.wait().await;
            group.0 = None;
            eprintln!(
                "razor: session {session}'s post watch ended; its next post command starts it again."
            );
            let mut bound = post.inner.bound.lock().await;
            bound.retain(|b| b.watch.id() != tokio::task::id());
        }))
    }

    /// Admits `text` to `session` as the program Post's input, as any program's.
    async fn ring(&self, session: &str, text: String) {
        let Some(controller) = self.inner.controller.get() else {
            return;
        };
        let from = Origin::Program {
            name: "post".into(),
        };
        let failed = match SessionId::try_from(session.to_string()) {
            Ok(id) => (controller.admit(&id, Input::new(from, text)).await).err(),
            Err(e) => Some(Error::Invalid(e.to_string())),
        };
        if let Some(e) = failed {
            eprintln!("razor: Post could not ring session {session}: {e}");
        }
    }
}

/// The input a watch line makes: a header naming the sender, Post's own preview, and the post
/// command that reads the rest. None for channel traffic that names nobody, and any other line.
fn rung(line: &[u8]) -> Option<String> {
    let event: Value = serde_json::from_slice(line).ok()?;
    let field = |name: &str| event[name].as_str().unwrap_or_default();
    let channel = field("channel");
    let (header, pointer) = match field("reason") {
        "mail" => (
            words("post_mail").to_string(),
            words("post_read").replace("{id}", field("id")),
        ),
        "mention" => (
            words("post_mention").replace("{channel}", channel),
            words("post_chat").replace("{channel}", channel),
        ),
        _ => return None,
    };
    Some(header.replace("{from}", field("from")) + field("preview") + &pointer)
}
