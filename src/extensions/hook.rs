//! A hook: a program run once per event, the payload on its standard input.

use super::{DEADLINE, Error, Job, Plugin, QUEUE, Registered, Reply, late, other, refused, report};
use crate::shell::Group;
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::{process::Command, sync::mpsc};

/// The most bytes of a run's standard output read; past it, the hook's pipe is closed.
const OUT: u64 = 4 << 20;
/// The most bytes of its standard error read, which become its refusal's sentence.
const ERR: u64 = 4 << 10;

/// A hook as its config names it. One that observes entries gets a task running them in order.
pub(super) fn load(
    name: &str,
    path: &Path,
    config: &Plugin,
) -> (Registered, Option<mpsc::Sender<Job>>) {
    let queue = (!config.after.is_empty()).then(|| {
        let (queue, mut jobs) = mpsc::channel::<Job>(QUEUE);
        let (name, path) = (name.to_owned(), path.to_path_buf());
        tokio::spawn(async move {
            // A hook's queue holds only observed entries: its before-events run in their caller.
            while let Some(Job { payload, reply }) = jobs.recv().await {
                let Reply::Report { stream, cwd } = reply else {
                    continue;
                };
                let ran = tokio::time::timeout(DEADLINE, run(&name, &path, &cwd, &payload)).await;
                if let Err(e) = ran.unwrap_or_else(|_| Err(late(&name))) {
                    report(&stream, &e.to_string());
                }
            }
        });
        queue
    });
    let (before, after) = (config.before.clone(), config.after.clone());
    let registered = Registered {
        before,
        after,
        ..Default::default()
    };
    (registered, queue)
}

/// Runs the hook once in `cwd`, `payload` on its standard input: its standard output when it
/// exits 0, else its refusal, whose sentence is its standard error. Dropped before the hook
/// exits, it kills the hook's whole process group.
pub(super) async fn run(
    name: &str,
    path: &Path,
    cwd: &Path,
    payload: &str,
) -> Result<Vec<u8>, Error> {
    let mut child = Command::new(path)
        .current_dir(cwd)
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| other(name, e))?;
    let mut group = Group(child.id());
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let (mut out, mut err) = (Vec::new(), Vec::new());
    // A hook need not read its input or finish writing, so a closed pipe is no failure.
    let write = async {
        if let Some(mut stdin) = stdin {
            let _ = stdin.write_all(payload.as_bytes()).await;
        }
    };
    let read_out = async {
        if let Some(stdout) = stdout {
            let _ = stdout.take(OUT).read_to_end(&mut out).await;
        }
    };
    let read_err = async {
        if let Some(stderr) = stderr {
            let _ = stderr.take(ERR).read_to_end(&mut err).await;
        }
    };
    let ((), (), (), status) = tokio::join!(write, read_out, read_err, child.wait());
    group.0 = None;
    let status = status.map_err(|e| other(name, e))?;
    if status.success() {
        return Ok(out);
    }
    match String::from_utf8_lossy(&err).trim() {
        "" => Err(refused(name, format!("it failed ({status})."))),
        sentence => Err(refused(name, sentence)),
    }
}
