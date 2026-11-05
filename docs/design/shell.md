# Shell and jobs

One `Shell` per session; job ids and job events belong to `protocol`.

```rust
/// One session's jobs: at most 64 handles and their output files, never the session's record. Dropping it kills nothing.
pub struct Shell { /* private */ }
/// A fresh bash without startup files; env is its whole environment. A terminal is 40 rows by 120 columns.
pub struct Command { pub text: String, pub cwd: PathBuf, pub env: BTreeMap<String, String>, pub pty: bool, pub stdin: Stdin, pub deadline: Option<Duration> }
/// Plain stdin: /dev/null, finite text then its end, or a pipe open until Input::End. A terminal job's stdin is its terminal; finite text is typed there, then Ctrl-D.
pub enum Stdin { Closed, Text(String), Open }
/// A bounded result; path names the whole output when text is not all of it or the job still runs. A successful read advances the job's next plain pattern search.
pub struct Output { pub end: Option<JobEnd>, pub text: String, pub path: Option<PathBuf>, pub screen: Option<Screen> }
/// A terminal's 40 rows, unstyled, without trailing spaces; the zero-based cursor is always on the screen.
pub struct Screen { pub rows: Vec<String>, pub cursor: (u16, u16) }
/// Exact UTF-8 text, one named key, or the end of input.
pub enum Input { Text(String), Key(String), End }
/// What ends a wait besides the job's end: a literal pattern after its last returned plain output, or on a terminal's rendered rows; quiet, no output since the wait began or last heard any; timeout, which leaves the job running.
pub struct Wait { pub timeout: Duration, pub pattern: Option<String>, pub quiet: Option<Duration> }
/// Refusals are sentences; any other failure keeps its cause and a sentence of context.
pub enum Error { Refused(&'static str), Other { source: Box<dyn std::error::Error + Send + Sync>, context: &'static str } }
impl Shell {
    /// Takes back every recorded job whose process group still has members, without replaying its Started; returns an Unknown end, unqueued, for every other. A terminal job is never taken back: its terminal died with the old process, and what is left of it is killed.
    pub async fn open(dir: PathBuf, next: u32, running: &[JobEvent], events: mpsc::Sender<JobEvent>) -> Result<(Self, Vec<JobEvent>), Error>;
    /// The job's own task queues Started, then Ended; returns once Started is queued, even for a command already ended. A start whose caller is gone by then stops its job.
    pub async fn start(self: &Arc<Self>, call: EntryId, command: Command) -> Result<JobId, Error>;
    /// The job's output at the first of its end, the timeout, or what `until` names; a zero timeout is a peek.
    pub async fn wait(&self, job: JobId, until: Wait) -> Result<Output, Error>;
    /// End is Ctrl-D on a terminal and closes a plain pipe. An unknown key, an ended job or a closed stdin is refused before any write.
    pub async fn send(&self, job: JobId, input: Input) -> Result<(), Error>;
    /// TERM (HUP for a terminal job), then KILL, a second's grace each, to the job's process group. A zero-timeout wait reads its remaining output without waiting for a surviving group.
    pub async fn kill(&self, job: JobId) -> Result<(), Error>;
    /// Running jobs with a process matching pgrep arguments, scoped to each owned process group;
    /// invalid patterns and unsupported pgrep failures refuse without stopping anything.
    pub async fn matching(&self, args: &[String]) -> Result<Vec<JobId>, Error>;
    /// Lets the handle be evicted, once the record shows its end in a result or a notice.
    pub async fn ack(&self, job: JobId);
}
```

## Data flow

1. The controller seeds `JobId(u32)` (job-N) above the recorded ids and drains Started and Ended from its queue of 128, two for each of 64 handles; the shell only queues.
2. `start` gives the job its own task. Holding the handles, it evicts the oldest acknowledged one when 64 are held, or refuses; then, on a blocking thread, it creates the first free `job-N` file in the session's folder (a crash's leftover is skipped, not reused), opens a terminal if asked, and starts bash after `setsid` (and `TIOCSCTTY`), so the leader's pid is also the job's process group and session.
3. Plain stdout and stderr are that file itself, so a job outlives Razor and keeps writing. Plain stdin is /dev/null, a pipe written and then closed (a program may read only a prefix), or a pipe left open for `send`.
4. The task queues Started and answers `start`, then waits for the leader's exit, an event, and after it looks at the group every 50 ms until it is empty. Meanwhile it types finite input, copies a terminal's output into the file and the vt100 screen, answering the program's queries, and stops the job at its deadline.
5. When the group is empty it keeps what the terminal still holds without waiting on an escaped holder, queues Ended, and only then shows the end on the handle's watch, so no result shows an end the record has not been sent.
6. `wait` waits on that watch and its timeout. Only a pattern or quiet adds a 50 ms look at the file's length; new plain output is searched in windows of 64 KiB plus the pattern's length, a terminal on its rendered rows. Each handle retains the output length its last successful read reached, so another plain pattern wait skips previously shown output for as long as that handle remains available. No output log is held in memory. `kill` only stops the group; the explicit tool kill follows it with a zero-timeout wait for its output, while controller and command-guard stops do not read or advance that position.
7. A result is the output's last 30,000 characters (SPEC section 3), read from the file's end, never head and tail, or a terminal's screen; profiles shorten it to their family's shape.
8. `kill` and the deadline share one stop: TERM, or HUP for a terminal job, then a second's wait on the end, then KILL and another second.
9. A signal goes only to the job's own group, and only while the leader's pid names no process, or still names the leader (by boot and start time on Linux, by wall-clock start to the microsecond on macOS). The OS never reuses a pid while a group of that id has members, so another process at that pid means the group is gone. Pattern commands are resolved by `pgrep` restricted to each running job's group, then stopped through `kill`. When every stop is a plain `pkill`, the rest of the command then runs with `pkill` shadowed by a shell function that does nothing; a stop that is wrapped (`sudo`, `env`, `timeout` and the like) or named by its path stops the jobs and refuses the rest, which the agent sends again.
10. `open` takes back a plain job whose process group still has members; its end is Unknown, since it is no longer this process's child. Keys: Enter, Tab, Backspace, Escape, the arrows, Home and End (application mode when asked), Delete, PageUp, PageDown, Ctrl-A to Ctrl-Z. A secret reaches a job only through the `send` action's `secret_file`; the shell records no input. No Drop kills; the controller kills live jobs at session close.

## The hard decision: the process group, not the OS session

The job is the process group the shell created, the leader's pid; it ends when the group is empty, and every signal goes to that group.

The first version made the job its OS session instead: after the leader exited it enumerated every process on the machine (every `/proc/N/stat` on Linux, `proc_listpids` on macOS) once per look, waited until no member of the session was left, and signalled each group found in it. That catches one more case, a process that moves to another group of the same session, which happens when job control is on: `set -m` in a plain command, or an interactive shell under a terminal. It costs a scan of the whole machine every 50 ms per job whose leader has exited, and about 100 lines in two platform versions, the macOS one never run.

The group costs one system call to ask (`kill(-pid, 0)`) and one to signal, the same on both systems. What it gives up is narrower than it looks, because job control lives almost entirely under a terminal. There the kernel sends SIGHUP to the terminal's foreground group when the session's leader exits, and the stop sends a terminal job HUP first, as a closing terminal does, which interactive shells pass on to every job they started. Probed: `bash -i` under the terminal with `sleep 101 &` and `sleep 102`, each in a group of its own, loses both to `kill`; with TERM first, the background one survived. What remains is a group an interactive shell leaves behind when it exits by itself, and a plain command that turns job control on (`set -m`): their members are not waited for, and the second is not stopped. A program that calls `setsid` escapes both designs. SPEC section 3 asks for "a process group"; this design kills only by a process group the shell created.

## The terminal

libc `openpty` plus vt100 0.18.1, instead of portable-pty's blocking reader, writer and wait threads and twenty dependency packages. Tokio's `AsyncFd` drives the nonblocking master, with std's own reads and writes on it, inside the job's task. `pre_exec` uses only `setsid` and `TIOCSCTTY`. The vt100 callback answers cursor-position, status, primary and secondary device-attribute queries; fish otherwise stalls about ten seconds. Rows are trimmed of trailing spaces and the cursor's column clamped; there is no scrollback. memchr (already in vte's dependency graph) is the literal search.

vt100's default `std` feature stays off, so vte keeps OSC strings to 1,024 bytes; cells are always 32 bytes. Cargo feature unification can turn vte's `std` back on, so every commit that adds a dependency checks the resolved features. Scratch measurements by the first pass (release Rust 1.98.1, Linux x86_64; five 102 MiB streams, 24 by 80, no scrollback): vt100 57.45 MiB/s and 2.37 µs per screen, alacritty_terminal 0.26.0 119.77 MiB/s and 7.30 µs; a 16 MiB unfinished OSC held vt100 at 1,824 KiB RSS while Alacritty rose from 2,092 to 18,624 KiB. These establish Linux behaviour. The macOS code type-checks and passes clippy for aarch64-apple-darwin; its terminal, `proc_pidinfo` and query behaviour remain to be run on a Mac.

## Session-owned pattern stops (2026-11-02)

Trey approved inferring job handles from a submitted pattern kill and stopping them without a retry. The existing guard resolves literal `pkill [-INT|-TERM|-KILL|-9] [-f|-x] [--] pattern` commands against this session's running job groups, then stops the deduplicated handles through `Shell::kill`. A plain `pkill` leaves the rest of the command to run with `pkill` shadowed by a function that does nothing; a wrapped or path-named form stops the jobs and refuses the rest. Other selectors, substitutions and all-process targets retain the existing refusal. `-f` uses the full command line; `-x` uses an exact process name. No match stops nothing.

The 2 November session index contained 23 refused Bash calls when checked on 3 November: all termination attempts used `pkill`, with `-f`, `-x`, `-INT` or `-TERM`. The two `-USR1` commands intended signaling rather than termination and stay refused. No `killall` termination was observed. Supporting only these forms adds no second shell parser. The alternatives were a second process scanner/regex engine or PATH shims with executable files, environment plumbing and a group list; both add machinery. Native `pgrep -g` sends no signal, examines at most 64 owned groups per attempted stop, and requires no resident work or lifecycle state. A missing or failing matcher stops nothing.
