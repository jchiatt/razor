//! The Claude path's Rust half (docs/design/claude.md): one Node bridge for the whole server,
//! every Claude session multiplexed over its pipes as JSON lines.

pub(crate) mod shared;
mod task;

use crate::protocol::{FromBridge, Line, ToBridge};
pub(crate) use shared::{Output, SharedBridge};
use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

/// Each direction holds at most this many bytes of lines, and no line may be longer.
pub(crate) const BUDGET: usize = 64 << 20;

/// The environment variables that hold a Claude sign-in. The bridge's CLI reads them from
/// Razor's environment; commands and MCP servers never get them.
pub(crate) const KEYS: [&str; 3] = [
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
];

/// The bridge's lines, each with the share of the inbound budget it holds.
type Lines = mpsc::UnboundedReceiver<(Line, OwnedSemaphorePermit)>;

/// The bridge: one `node <dir>/main.ts`, started on first use. When it exits, its receiver
/// ends; every open conversation is then gone, and the next use starts another bridge.
#[derive(Clone)]
struct Bridge {
    queue: mpsc::UnboundedSender<(Vec<u8>, OwnedSemaphorePermit)>,
    budget: Arc<Semaphore>,
}

impl Bridge {
    /// Start the bridge in `dir()`. There is no config key: nobody tunes it, and swapping the
    /// one bridge would stop every Claude session. Its lines arrive
    /// on the returned receiver, each with the share of the 64 MiB budget it holds; dropping
    /// that returns the line's bytes. A line over 64 MiB stops the bridge.
    async fn start() -> Result<(Bridge, Lines), Error> {
        let dir = dir().await?;
        let main = dir.join("main.ts");
        let mut child = (Command::new(node().await).arg(&main).current_dir(dir))
            .env("RAZOR_VERSION", env!("CARGO_PKG_VERSION"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| match e.kind() {
                ErrorKind::NotFound => Error::Missing(
                    "Node is not on PATH; the Claude bridge needs Node 22.18 or later.".into(),
                ),
                _ => Error::Io {
                    doing: "starting the Claude bridge",
                    source: e,
                },
            })?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(Error::Exited);
        };
        let (queue, outbound) = mpsc::unbounded_channel();
        tokio::spawn(write(stdin, outbound));
        let (lines, receiver) = mpsc::unbounded_channel();
        tokio::spawn(read(child, stdout, lines));
        Ok((
            Bridge {
                queue,
                budget: Arc::new(Semaphore::new(BUDGET)),
            },
            receiver,
        ))
    }

    /// Queue one message for the bridge, waiting while 64 MiB are already queued.
    async fn send(&self, message: &impl serde::Serialize) -> Result<(), Error> {
        // Invariant: every field is a string, number, bool, sequence or string-keyed map.
        let mut line = serde_json::to_vec(message).expect("a bridge message serializes");
        line.push(b'\n');
        if line.len() > BUDGET {
            return Err(Error::TooLong(line.len()));
        }
        let held = self
            .budget
            .clone()
            .acquire_many_owned(line.len() as u32)
            .await
            .map_err(|_| Error::Exited)?;
        self.queue.send((line, held)).map_err(|_| Error::Exited)
    }
}

/// The Node the bridge runs on: the one Razor ships, else `node` on PATH.
pub(crate) async fn node() -> PathBuf {
    let shipped = tokio::task::spawn_blocking(|| crate::config::shipped("node/bin/node"));
    shipped.await.ok().flatten().unwrap_or("node".into())
}

/// The bridge's directory, the one Razor ships (`config::shipped`), with its `node_modules`
/// installed. `Missing` says what to do.
pub(crate) async fn dir() -> Result<PathBuf, Error> {
    let main = tokio::task::spawn_blocking(|| crate::config::shipped("bridge/main.ts"));
    let Some(main) = main.await.ok().flatten() else {
        let why = "There is no Claude bridge beside Razor or in its source tree; reinstall Razor.";
        return Err(Error::Missing(why.into()));
    };
    let dir = main.with_file_name("");
    if tokio::fs::metadata(dir.join("node_modules")).await.is_err() {
        return Err(Error::Missing(format!(
            "The Claude bridge at {} has no node_modules; run `npm ci` there.",
            dir.display()
        )));
    }
    Ok(dir)
}

/// The sign-in the SDK's bundled Claude Code CLI in `dir` finds in this environment, as its own
/// `auth status` reports it: the method, never a credential. `Missing` says what to do.
pub(crate) async fn sign_in(dir: &Path) -> Result<String, Error> {
    let io = |doing| move |source| Error::Io { doing, source };
    let mut status = Command::new(cli(dir).await?);
    status
        .args(["auth", "status"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let status = status
        .output()
        .await
        .map_err(io("asking the Claude Code CLI for its sign-in"))?;
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap_or_default();
    if status["loggedIn"] != true {
        let sentence = "Claude is not signed in; run razor setup.";
        return Err(Error::Missing(sentence.into()));
    }
    let said =
        [&status["authMethod"], &status["apiKeySource"]].map(|v| v.as_str().unwrap_or_default());
    Ok(said
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", "))
}

/// Runs the bundled CLI's own `auth login` in this terminal: a Claude subscription, or with
/// `console` an Anthropic Console account billed for API use. The CLI keeps the credentials and
/// says how it went; `sign_in` says whether Razor can use them.
pub(crate) async fn login(console: bool) -> Result<(), Error> {
    let mut login = Command::new(cli(&dir().await?).await?);
    login
        .args(["auth", "login"])
        .args(console.then_some("--console"));
    let (ran, doing) = (login.status().await, "starting the Claude sign-in");
    ran.map(drop).map_err(|source| Error::Io { doing, source })
}

/// The platform's own Claude Code CLI inside the SDK's packages in `dir`.
async fn cli(dir: &Path) -> Result<PathBuf, Error> {
    let io = |doing| move |source| Error::Io { doing, source };
    let packages = tokio::fs::read_dir(dir.join("node_modules/@anthropic-ai")).await;
    let mut packages = packages.map_err(io("reading the Claude bridge"))?;
    // The platform's own package: npm installs only the ones that fit this machine.
    let mut clis = Vec::new();
    while let Some(package) =
        (packages.next_entry().await).map_err(io("reading the Claude bridge"))?
    {
        let sdk = package
            .file_name()
            .to_string_lossy()
            .starts_with("claude-agent-sdk-");
        clis.extend(sdk.then(|| package.path().join("claude")));
    }
    clis.sort();
    clis.into_iter().next().ok_or_else(|| {
        let dir = dir.display();
        Error::Missing(format!(
            "The Claude bridge at {dir} has no Claude Code CLI; run `npm ci` there."
        ))
    })
}

/// One tool-free Haiku answer at medium effort on this machine's Claude sign-in, in `cwd`;
/// no session file, at most 64 KiB read back, killed when the request is dropped.
pub(crate) async fn recap(cwd: &Path, prompt: &str, evidence: &str) -> Result<String, Error> {
    let io = |doing| move |source| Error::Io { doing, source };
    let mut run = Command::new(cli(&dir().await?).await?)
        .args([
            "-p",
            "--model",
            "haiku",
            "--effort",
            "medium",
            "--output-format",
            "json",
            "--tools=",
            "--setting-sources=",
            "--strict-mcp-config",
            "--max-turns",
            "1",
            "--no-session-persistence",
            "--system-prompt",
            prompt,
        ])
        .env("DISABLE_PROMPT_CACHING", "1")
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(io("starting the Claude Code CLI"))?;
    let (Some(mut stdin), Some(stdout)) = (run.stdin.take(), run.stdout.take()) else {
        return Err(Error::Exited);
    };
    stdin
        .write_all(evidence.as_bytes())
        .await
        .map_err(io("writing the recap's evidence"))?;
    drop(stdin);
    let mut answer = Vec::new();
    stdout
        .take(64 << 10)
        .read_to_end(&mut answer)
        .await
        .map_err(io("reading the recap"))?;
    let status = run.wait().await.map_err(io("waiting for the recap"))?;
    let answer: serde_json::Value = serde_json::from_slice(&answer).unwrap_or_default();
    match (
        status.success(),
        answer["is_error"].as_bool(),
        answer["result"].as_str(),
    ) {
        (true, Some(false), Some(text)) => Ok(text.into()),
        _ => Err(Error::Missing("The Claude Code CLI gave no recap.".into())),
    }
}

/// Write queued lines to the bridge until either side is gone. Dropping its input tells the
/// bridge Razor is gone, and it closes its conversations.
async fn write(
    mut stdin: ChildStdin,
    mut outbound: mpsc::UnboundedReceiver<(Vec<u8>, OwnedSemaphorePermit)>,
) {
    while let Some((line, _held)) = outbound.recv().await {
        if stdin.write_all(&line).await.is_err() {
            break;
        }
    }
}

/// Read the bridge's lines until it exits or sends one over the budget, which stops it.
async fn read(
    mut child: Child,
    stdout: ChildStdout,
    lines: mpsc::UnboundedSender<(Line, OwnedSemaphorePermit)>,
) {
    let budget = Arc::new(Semaphore::new(BUDGET));
    let mut reader = BufReader::with_capacity(64 * 1024, stdout);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = match (&mut reader)
            .take(BUDGET as u64)
            .read_until(b'\n', &mut buf)
            .await
        {
            Ok(0) | Err(_) => break,
            Ok(_) if buf.last() != Some(&b'\n') => {
                eprintln!("razor: the Claude bridge sent a line over 64 MiB, so it was stopped.");
                break;
            }
            Ok(n) => n,
        };
        let Ok(held) = budget.clone().acquire_many_owned(n as u32).await else {
            break;
        };
        match serde_json::from_slice(&buf) {
            Ok(line) => {
                if lines.send((line, held)).is_err() {
                    break;
                }
            }
            Err(e) => eprintln!(
                "razor: the Claude bridge sent a line Razor cannot read ({e}); it was skipped."
            ),
        }
    }
    let _ = child.kill().await;
}

#[derive(Debug)]
pub(crate) enum Error {
    /// Node, the bridge directory or a sign-in is missing. The sentence says what to fix.
    Missing(String),
    /// The bridge exited.
    Exited,
    /// A message longer than the bridge's 64 MiB budget.
    TooLong(usize),
    Io {
        doing: &'static str,
        source: std::io::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Missing(sentence) => f.write_str(sentence),
            Error::Exited => {
                f.write_str("The Claude bridge exited; the next message starts it again.")
            }
            Error::TooLong(n) => write!(
                f,
                "A message of {n} bytes is over the Claude bridge's 64 MiB limit."
            ),
            Error::Io { doing, source } => write!(f, "Razor failed {doing}: {source}."),
        }
    }
}

impl std::error::Error for Error {}
