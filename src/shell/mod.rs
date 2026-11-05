//! Shell jobs: one session and process group per command, output spooled to a file, and a fixed
//! terminal for interactive programs (docs/design/shell.md).

mod terminal;

use crate::protocol::{EntryId, JobEnd, JobEvent, JobId, SPILL, now};
use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::process::ExitStatusExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{fmt, io, io::SeekFrom, path::Path, path::PathBuf, process::Stdio, sync::Arc};
use terminal::Terminal;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use tokio::time::{Duration, Instant, sleep, timeout};

/// The most handles one session keeps: Codex's own soft cap.
const HANDLES: usize = 64;
/// How often a wait for a pattern or quiet, or a job whose leader has exited, looks again.
const TICK: Duration = Duration::from_millis(50);
/// How long `kill` gives TERM, and then KILL, to end the job.
const GRACE: Duration = Duration::from_secs(1);
/// The most output one read holds: a pattern search's window beyond the pattern, an end's tail.
const WINDOW: usize = 64 << 10;

/// One session's jobs: at most 64 handles and their output files, never the session's record.
/// Dropping it kills nothing.
pub(crate) struct Shell {
    dir: PathBuf,
    jobs: Mutex<Jobs>,
    events: mpsc::Sender<JobEvent>,
}

/// A fresh bash without startup files; `env` is its whole environment, not additions.
pub(crate) struct Command {
    pub(crate) text: String,
    pub(crate) cwd: PathBuf,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) pty: bool,
    pub(crate) stdin: Stdin,
    pub(crate) deadline: Option<Duration>,
}

/// A plain job's stdin: /dev/null, finite text then its end, or a pipe open until `Input::End`.
/// A terminal job's stdin is always its terminal; finite text is typed there, then Ctrl-D.
pub(crate) enum Stdin {
    Closed,
    Text(String),
    Open,
}

/// A bounded result. `path` names the whole output when the text is not all of it or the job
/// still runs. A successful read advances the job's next plain pattern search.
pub(crate) struct Output {
    pub(crate) end: Option<JobEnd>,
    pub(crate) text: String,
    pub(crate) path: Option<PathBuf>,
    pub(crate) screen: Option<Screen>,
}

/// A terminal's 40 rows, unstyled, without trailing spaces; the cursor, zero-based, is always
/// on the 40-by-120 screen.
pub(crate) struct Screen {
    rows: Vec<String>,
    pub(crate) cursor: (u16, u16),
}

/// Exact UTF-8 text, one named key, or the end of input.
pub(crate) enum Input {
    Text(String),
    Key(String),
    End,
}

/// What ends a wait besides the job's end: a literal pattern after its last returned plain
/// output, or on a terminal's rendered rows; `quiet` with no output since the wait began or last
/// heard any; `timeout`, which leaves the job running.
pub(crate) struct Wait {
    pub(crate) timeout: Duration,
    pub(crate) pattern: Option<String>,
    pub(crate) quiet: Option<Duration>,
}

/// Refusals are sentences; any other failure keeps its cause and a sentence of context.
#[derive(Debug)]
pub(crate) enum Error {
    Refused(&'static str),
    Other {
        source: Box<dyn std::error::Error + Send + Sync>,
        context: &'static str,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(text) => f.write_str(text),
            Self::Other { source, context } => write!(f, "{context} {source}"),
        }
    }
}

impl std::error::Error for Error {}

fn other<E: std::error::Error + Send + Sync + 'static>(
    context: &'static str,
) -> impl FnOnce(E) -> Error {
    move |source| Error::Other {
        source: Box::new(source),
        context,
    }
}

struct Jobs {
    next: u32,
    /// At most 64, the most `start` lets in; the record of a run that kept that cap reopens no more.
    live: BTreeMap<JobId, Arc<Job>>,
    /// Handles whose end the record shows; the oldest is evicted when a start needs room.
    acked: BTreeSet<JobId>,
}

#[derive(Default)]
struct Job {
    /// The leader's pid, which is also the job's session and process group id.
    pid: u32,
    process_start: String,
    path: PathBuf,
    terminal: Option<Terminal>,
    /// A plain job's open stdin, until `Input::End` closes it.
    input: Mutex<Option<ChildStdin>>,
    /// The output length this handle's last successful read reached.
    seen: AtomicU64,
    /// Set only once Ended is queued, so no result shows an end the record has not been sent.
    end: watch::Sender<Option<JobEnd>>,
}

impl Shell {
    /// Takes back every recorded job whose process group still has members, without replaying
    /// its Started; returns an Unknown end, unqueued, for every other one. A terminal job is never
    /// taken back, since its terminal died with the old process; what is left of it is killed.
    pub(crate) async fn open(
        dir: PathBuf,
        next: u32,
        running: &[JobEvent],
        events: mpsc::Sender<JobEvent>,
    ) -> Result<(Self, Vec<JobEvent>), Error> {
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(other("Could not create the session's job folder."))?;
        let (mut live, mut lost, acked) = (BTreeMap::new(), Vec::new(), BTreeSet::new());
        for event in running {
            let JobEvent::Started {
                job: id,
                pid,
                pty,
                process_start,
                deadline,
                ..
            } = event
            else {
                continue;
            };
            let job = Arc::new(Job {
                pid: *pid,
                process_start: process_start.clone(),
                path: dir.join(format!("job-{}", id.0)),
                ..Job::default()
            });
            let alive = job.alive().await;
            if alive && !pty {
                live.insert(*id, job.clone());
                let events = events.clone();
                let remaining = deadline.map(|at| Duration::from_millis(at.saturating_sub(now())));
                tokio::spawn(supervise(*id, job, None, None, None, remaining, events));
                continue;
            }
            if alive {
                job.signal(libc::SIGKILL).await;
            }
            lost.push(JobEvent::Ended {
                job: *id,
                end: JobEnd::Unknown,
                tail: Vec::new(),
            });
        }
        let jobs = Mutex::new(Jobs { next, live, acked });
        Ok((Self { dir, jobs, events }, lost))
    }

    /// Starts the command as a job whose own task queues Started, then Ended, and returns once
    /// Started is queued, even for a command that has already ended. A start whose caller is
    /// gone by then stops its job.
    pub(crate) async fn start(
        self: &Arc<Self>,
        call: EntryId,
        command: Command,
    ) -> Result<JobId, Error> {
        let (reply, started) = oneshot::channel();
        tokio::spawn(self.clone().run(call, command, reply));
        started
            .await
            .map_err(other("The job's task ended before the job started."))?
    }

    async fn run(
        self: Arc<Self>,
        call: EntryId,
        mut command: Command,
        reply: oneshot::Sender<Result<JobId, Error>>,
    ) {
        let text = match &mut command.stdin {
            Stdin::Text(text) => Some(std::mem::take(text)),
            _ => None,
        };
        let deadline = command.deadline;
        let mut jobs = self.jobs.lock().await;
        // A caller gone before its command started (Escape, for one) never has it run.
        if reply.is_closed() {
            return;
        }
        if jobs.live.len() >= HANDLES {
            let Some(oldest) = jobs.acked.pop_first() else {
                let _ = reply.send(Err(Error::Refused(
                    "All 64 handles are live or delivering end notices; stop a job or receive its notice first.",
                )));
                return;
            };
            jobs.live.remove(&oldest);
        }
        let (dir, next) = (self.dir.clone(), jobs.next);
        let launched = tokio::task::spawn_blocking(move || launch(&dir, next, &command)).await;
        let (id, job, child, spool) = match launched.map_err(other("The job's start failed.")) {
            Ok(Ok(launched)) => launched,
            Ok(Err(error)) | Err(error) => return drop(reply.send(Err(error))),
        };
        jobs.next = id.0.saturating_add(1);
        jobs.live.insert(id, job.clone());
        let deadline = deadline.map(|after| now().saturating_add(after.as_millis() as u64));
        let started = JobEvent::Started {
            job: id,
            call,
            pid: job.pid,
            pty: job.terminal.is_some(),
            process_start: job.process_start.clone(),
            deadline,
        };
        // Queued under the lock, for `started`; the channel holds two events per handle, so
        // this send never waits.
        let _ = self.events.send(started).await;
        drop(jobs);
        let deadline = match reply.send(Ok(id)) {
            Ok(()) => deadline.map(|at| Duration::from_millis(at.saturating_sub(now()))),
            Err(_) => Some(Duration::ZERO),
        };
        let spool = job.terminal.is_some().then(|| File::from_std(spool));
        let events = self.events.clone();
        supervise(id, job, Some(child), spool, text, deadline, events).await;
    }

    /// Returns once a start under way has queued its Started; any later start finds its caller
    /// gone, if the caller dropped it before, and never runs.
    pub(crate) async fn started(&self) {
        drop(self.jobs.lock().await);
    }

    async fn find(&self, job: JobId) -> Result<Arc<Job>, Error> {
        let jobs = self.jobs.lock().await;
        jobs.live.get(&job).cloned().ok_or(Error::Refused(
            "That job handle is unavailable; use a live handle.",
        ))
    }

    /// Returns the job's output at the first of its end, `timeout`, or what `until` names; a zero
    /// timeout is a peek.
    pub(crate) async fn wait(&self, job: JobId, until: Wait) -> Result<Output, Error> {
        let job = self.find(job).await?;
        let after = job.seen.load(Ordering::Relaxed);
        let mut ended = job.end.subscribe();
        let watching = until.pattern.is_some() || until.quiet.is_some();
        let watch = async {
            let (mut next, mut size, mut heard) = (after, None, Instant::now());
            loop {
                let now = tokio::fs::metadata(&job.path).await?.len();
                if size != Some(now) {
                    (size, heard) = (Some(now), Instant::now());
                    if let Some(pattern) = &until.pattern
                        && job.found(pattern, &mut next, now).await?
                    {
                        return io::Result::Ok(());
                    }
                }
                if until.quiet.is_some_and(|quiet| heard.elapsed() >= quiet) {
                    return Ok(());
                }
                sleep(TICK).await;
            }
        };
        tokio::select! {
            _ = ended.wait_for(Option::is_some) => {}
            () = sleep(until.timeout) => {}
            watched = watch, if watching => watched.map_err(other("Could not watch the job's output."))?,
        }
        job.output().await
    }

    /// Lets the job's handle be evicted, once the record shows its end in a result or a notice.
    pub(crate) async fn ack(&self, job: JobId) {
        let mut jobs = self.jobs.lock().await;
        if jobs.live.contains_key(&job) {
            jobs.acked.insert(job);
        }
    }

    /// Types text, one named key, or the end of input: Ctrl-D on a terminal, closing a plain
    /// job's pipe. An unknown key, an ended job or a closed stdin is refused before any write.
    pub(crate) async fn send(&self, job: JobId, input: Input) -> Result<(), Error> {
        let job = self.find(job).await?;
        if job.end.borrow().is_some() {
            return Err(Error::Refused(
                "The job has ended; start another command to send input.",
            ));
        }
        let key;
        let bytes: &[u8] = match &input {
            Input::Text(text) => text.as_bytes(),
            Input::Key(name) => {
                let application = job.terminal.as_ref().is_some_and(Terminal::application);
                key = terminal::key(name, application)?;
                &key
            }
            Input::End => b"\x04",
        };
        let sent = if let Some(terminal) = &job.terminal {
            terminal.send(bytes).await
        } else {
            let mut pipe = job.input.lock().await;
            let Some(writer) = pipe.as_mut() else {
                return Err(Error::Refused(
                    "This job's input is closed; start a command with open input.",
                ));
            };
            if matches!(input, Input::End) {
                *pipe = None;
                return Ok(());
            }
            writer.write_all(bytes).await
        };
        sent.map_err(other("Could not send input to the job."))
    }

    /// Running jobs with a process matching `pgrep` arguments within their own process group.
    /// A failed matcher refuses before any stop; this method sends no signal.
    pub(crate) async fn matching(&self, args: &[String]) -> Result<Vec<JobId>, Error> {
        let jobs = self.jobs.lock().await.live.clone();
        let mut found = Vec::new();
        for (id, job) in jobs {
            if job.end.borrow().is_some() || job.leader().await == Some(false) {
                continue;
            }
            let status = tokio::process::Command::new("pgrep")
                .args(["-g", &job.pid.to_string()])
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .status()
                .await
                .map_err(other("Could not match the session's jobs with pgrep."))?;
            match status.code() {
                Some(0) => found.push(id),
                Some(1) => {}
                _ => {
                    return Err(Error::Refused(
                        "The job pattern could not be matched; kill a job by its handle.",
                    ));
                }
            }
        }
        Ok(found)
    }

    /// Stops the job's process group: TERM (HUP for a terminal job), then KILL, a second's grace
    /// each. A zero-timeout wait reads its remaining output without waiting for a surviving group.
    pub(crate) async fn kill(&self, job: JobId) -> Result<(), Error> {
        self.find(job).await?.stop().await;
        Ok(())
    }
}

/// Creates the first free `job-N` output file from `id` on and starts the command in it; runs on
/// a blocking thread, since each step waits on the OS.
fn launch(
    dir: &Path,
    mut id: u32,
    command: &Command,
) -> Result<(JobId, Arc<Job>, Child, std::fs::File), Error> {
    let (path, spool) = loop {
        let path = dir.join(format!("job-{id}"));
        match std::fs::File::create_new(&path) {
            Ok(spool) => break (path, spool),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && id < u32::MAX => id += 1,
            Err(e) => return Err(other("Could not create the job's output file.")(e)),
        }
    };
    let (mut child, terminal) = spawn(command, &spool).map_err(|e| {
        let _ = std::fs::remove_file(&path);
        other("Could not start bash in")(io::Error::new(
            e.kind(),
            format!("{}: {e}", command.cwd.display()),
        ))
    })?;
    let identity = child.id().and_then(|pid| Some((pid, process_start(pid)?)));
    let Some((pid, process_start)) = identity else {
        let _ = child.start_kill();
        return Err(Error::Refused(
            "The job's process could not be identified; start the command again.",
        ));
    };
    let open = matches!(command.stdin, Stdin::Open);
    let input = Mutex::new(child.stdin.take_if(|_| open));
    let job = Job {
        pid,
        process_start,
        path,
        terminal,
        input,
        ..Job::default()
    };
    Ok((JobId(id), Arc::new(job), child, spool))
}

/// Starts fresh bash in a session and process group of its own, writing to `spool` or to a new
/// terminal, which becomes the session's controlling terminal.
fn spawn(command: &Command, spool: &std::fs::File) -> io::Result<(Child, Option<Terminal>)> {
    let mut bash = tokio::process::Command::new("bash");
    bash.args(["--noprofile", "--norc", "-c", &command.text])
        .current_dir(&command.cwd)
        .env_clear()
        .envs(&command.env);
    let terminal = if command.pty {
        let (terminal, [stdin, stdout, stderr]) = Terminal::open()?;
        bash.stdin(stdin).stdout(stdout).stderr(stderr);
        Some(terminal)
    } else {
        let stdin = match command.stdin {
            Stdin::Closed => Stdio::null(),
            _ => Stdio::piped(),
        };
        bash.stdin(stdin)
            .stdout(spool.try_clone()?)
            .stderr(spool.try_clone()?);
        None
    };
    let pty = command.pty;
    // SAFETY: between fork and exec the closure makes only the async-signal-safe calls setsid
    // and ioctl, which put the job in a session and group of its own and give it its terminal.
    unsafe {
        bash.pre_exec(move || {
            if libc::setsid() == -1 || (pty && libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1) {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok((bash.spawn()?, terminal))
}

/// Runs a job to its end: its leader's exit, then an empty process group. Meanwhile it types the
/// job's finite input, copies a terminal's output, and stops the job at its deadline. Queues
/// Ended, then shows the end on the handle.
async fn supervise(
    id: JobId,
    job: Arc<Job>,
    mut child: Option<Child>,
    mut spool: Option<File>,
    text: Option<String>,
    deadline: Option<Duration>,
    events: mpsc::Sender<JobEvent>,
) {
    let mut pipe = child.as_mut().and_then(|child| child.stdin.take());
    let exit = async {
        // A job taken back after a restart is not this process's child; its exit is unknown.
        let mut end = JobEnd::Unknown;
        if let Some(mut child) = child {
            match child.wait().await {
                Ok(status) => {
                    let code = status.code().map(JobEnd::Exit);
                    end = code.or(status.signal().map(JobEnd::Signal)).unwrap_or(end);
                }
                Err(e) => eprintln!("razor: job-{}: could not collect its exit: {e}", id.0),
            }
        }
        while job.alive().await {
            sleep(TICK).await;
        }
        end
    };
    let input = async {
        let Some(mut text) = text else { return };
        let written = match (&job.terminal, pipe.as_mut()) {
            (Some(terminal), _) => {
                text.push('\x04');
                terminal.send(text.as_bytes()).await
            }
            (None, Some(pipe)) => pipe.write_all(text.as_bytes()).await,
            (None, None) => Ok(()),
        };
        pipe = None;
        // A program may read only a prefix of its input (head, for one).
        if let Err(e) = written
            && e.kind() != io::ErrorKind::BrokenPipe
        {
            eprintln!("razor: job-{}: could not write its input: {e}", id.0);
        }
    };
    let mut saved = Ok(());
    let copy = async {
        if let (Some(terminal), Some(spool)) = (&job.terminal, spool.as_mut()) {
            saved = terminal.copy(spool, true).await;
        }
    };
    let stop = async {
        if let Some(deadline) = deadline {
            sleep(deadline).await;
            job.stop().await;
        }
    };
    let end = tokio::select! {
        end = exit => end,
        end = async { tokio::join!(input, copy, stop); std::future::pending().await } => end,
    };
    // The group is gone: keep what the terminal still holds, without waiting on an escaped holder.
    if let (Some(terminal), Some(spool)) = (&job.terminal, spool.as_mut())
        && saved.is_ok()
    {
        saved = terminal.copy(spool, false).await;
    }
    if let Err(e) = saved {
        eprintln!("razor: job-{}: could not save terminal output: {e}", id.0);
    }
    let tail = job.tail().await;
    let _ = events.send(JobEvent::Ended { job: id, end, tail }).await;
    job.end.send_replace(Some(end));
}

impl Job {
    /// Whether the process with the job's pid is its leader; None when no process has that pid.
    async fn leader(&self) -> Option<bool> {
        let pid = self.pid;
        let start = tokio::task::spawn_blocking(move || process_start(pid));
        Some(start.await.ok()?? == self.process_start)
    }

    /// Signals the job's process group, unless its pid now names another process, which means
    /// the group is gone: the OS never reuses a pid while a group of that id has members.
    async fn signal(&self, signal: i32) {
        if self.leader().await != Some(false) {
            // SAFETY: plain integers; the negative pid names only this job's own group.
            unsafe { libc::kill(-(self.pid as i32), signal) };
        }
    }

    /// Whether any process of the job's group is left.
    async fn alive(&self) -> bool {
        // SAFETY: as in `signal`; signal 0 only asks whether the group has members.
        let gone = unsafe { libc::kill(-(self.pid as i32), 0) } == -1
            && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        !gone && self.leader().await != Some(false)
    }

    /// TERM, or HUP for a terminal job, as a closing terminal sends it, which interactive shells
    /// pass on to every job they started; then KILL. Returns at the job's end or after a second's
    /// grace for each.
    async fn stop(&self) {
        let mut end = self.end.subscribe();
        let first = if self.terminal.is_some() {
            libc::SIGHUP
        } else {
            libc::SIGTERM
        };
        for signal in [first, libc::SIGKILL] {
            self.signal(signal).await;
            if timeout(GRACE, end.wait_for(Option::is_some)).await.is_ok() {
                return;
            }
        }
    }

    /// The end as far as it is queued, then the output's last SPILL characters, or a terminal's
    /// screen, and the cursor after them.
    async fn output(&self) -> Result<Output, Error> {
        // Read before the file: once the end shows, every writer is gone and the file complete.
        let end = *self.end.borrow();
        let read = async {
            let mut file = File::open(&self.path).await?;
            let after = file.metadata().await?.len();
            let mut bytes = Vec::new();
            if self.terminal.is_none() {
                // Four bytes per character at most, plus one character a cut may have split.
                let from = after.saturating_sub(4 * SPILL as u64 + 4);
                file.seek(SeekFrom::Start(from)).await?;
                file.take(after - from).read_to_end(&mut bytes).await?;
            }
            io::Result::Ok((after, bytes))
        };
        let (after, bytes) = read
            .await
            .map_err(other("Could not read the job's output."))?;
        let screen = self.terminal.as_ref().map(Terminal::screen);
        let text = match &screen {
            Some(screen) => screen.rows.join("\n"),
            None => {
                let text = String::from_utf8_lossy(&bytes);
                let skip = text.chars().count().saturating_sub(SPILL);
                text.chars().skip(skip).collect()
            }
        };
        let path = (after > SPILL as u64 || end.is_none()).then(|| self.path.clone());
        self.seen.store(after, Ordering::Relaxed);
        Ok(Output {
            end,
            text,
            path,
            screen,
        })
    }

    /// The output's last two lines with content, each its text after its last carriage return cut
    /// to 1 KiB, from one read of at most its last 64 KiB; none when the file cannot be read.
    async fn tail(&self) -> Vec<String> {
        let read = async {
            let mut file = File::open(&self.path).await?;
            let size = file.metadata().await?.len();
            let from = size.saturating_sub(WINDOW as u64);
            file.seek(SeekFrom::Start(from)).await?;
            let mut bytes = Vec::new();
            file.take(WINDOW as u64).read_to_end(&mut bytes).await?;
            io::Result::Ok(bytes)
        };
        let bytes = read.await.unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        let lines = text.lines().rev().filter_map(|line| {
            let line = line.trim_end_matches('\r').rsplit('\r').next()?;
            let cut = line.floor_char_boundary(1024);
            (!line.trim().is_empty()).then(|| line[..cut].to_owned())
        });
        let mut tail: Vec<String> = lines.take(2).collect();
        tail.reverse();
        tail
    }

    /// Whether `pattern` is on a terminal's screen, or starts at or after `next` in plain output's
    /// first `size` bytes; moves `next` past every start it has ruled out.
    async fn found(&self, pattern: &str, next: &mut u64, size: u64) -> io::Result<bool> {
        if let Some(terminal) = &self.terminal {
            return Ok(terminal.screen().rows.join("\n").contains(pattern));
        }
        let pattern = pattern.as_bytes();
        let Some(unread) = size.checked_sub(*next + pattern.len() as u64) else {
            return Ok(false);
        };
        let mut file = File::open(&self.path).await?;
        let mut window = vec![0; (unread as usize).min(WINDOW) + pattern.len()];
        while *next + pattern.len() as u64 <= size {
            let len = (size - *next).min(window.len() as u64) as usize;
            file.seek(SeekFrom::Start(*next)).await?;
            file.read_exact(&mut window[..len]).await?;
            if memchr::memmem::find(&window[..len], pattern).is_some() {
                return Ok(true);
            }
            *next += (len + 1 - pattern.len()) as u64;
        }
        Ok(false)
    }
}

/// The boot and start time of process `pid`, which no later process with that pid shares; None
/// when no process has it.
#[cfg(target_os = "linux")]
fn process_start(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The start time is field 22; the name before it is in parentheses and may hold any byte.
    let ticks = stat.rsplit_once(')')?.1.split_whitespace().nth(19)?;
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    Some(format!("{}:{ticks}", boot.trim()))
}

/// The wall-clock start of process `pid`, to the microsecond, which no later process with that
/// pid shares, in this boot or another; None when no process has it.
#[cfg(target_os = "macos")]
fn process_start(pid: u32) -> Option<String> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let into = info.as_mut_ptr().cast();
    // SAFETY: proc_pidinfo writes at most `size` bytes to `into` and returns how many it wrote.
    let wrote =
        unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTBSDINFO, 0, into, size) };
    // SAFETY: a full-size answer filled the struct.
    let info = (wrote == size).then(|| unsafe { info.assume_init() })?;
    let (seconds, micros) = (info.pbi_start_tvsec, info.pbi_start_tvusec);
    Some(format!("{seconds}.{micros:06}"))
}

/// A process group made with `process_group(0)` for a program run outside a job, killed whole
/// when this is dropped while it still holds the group's id; set it to None once the program
/// has been waited for.
pub(crate) struct Group(pub(crate) Option<u32>);

impl Drop for Group {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            // SAFETY: killpg only sends a signal, to the group `process_group(0)` made for it.
            unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL) };
        }
    }
}
