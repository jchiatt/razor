//! Residents (docs/design/residents.md): one task fires every resident's heartbeat, schedules
//! and night through the controller; a resident's identity record; and the programs that carry
//! notifications and asks to the person.

mod calendar;
mod deliveries;
mod install;
mod list;
mod projects;

pub use deliveries::Deliveries;
pub(crate) use install::permissions;
pub use install::{
    TEARDOWN_TIMEOUT, check_service, install, installed, server_lock, server_running, start, stop,
};
pub(crate) use list::{Listed, homes};

use crate::config::{Config, Day, Deliver, Resident, Schedule, Time};
use crate::controller::{self, Controller};
use crate::protocol::views::{SectionKind, Views, ViewsStatus};
use crate::protocol::{
    Answer, Ask, Body, Effort, End, Entry, EntryId, Input, Millis, Origin, Run, SessionId, Thread,
    now,
};
use crate::record::{Record, Summary};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{Notify, Semaphore, watch};

/// The most an identity record, an ask command's answer, or a sky file may hold.
const FILE: u64 = 64 * 1024;
/// Asks and notifications share at most 16 detached commands.
static RUNNING: Semaphore = Semaphore::const_new(16);
/// How far ahead `upcoming` looks, and how far back a row's runs go.
const DAY: Millis = 86_400_000;

#[derive(Deserialize)]
struct Text {
    ask: String,
    notify: String,
    option: String,
    identity: String,
    identity_heading: String,
    identity_thread: String,
    unreadable: String,
    too_large: String,
    unparsed: String,
    schedule_failed: String,
    schedule_stopped: String,
    no_resident: String,
    no_schedule: String,
    no_project: String,
    project_name: String,
    day: String,
    never_arrived: String,
    other: String,
    deliver_input: String,
    deliver_line: String,
    deliver_one: String,
    deliver_file: String,
    deliver_url: String,
    unit: String,
    plist: String,
    boot: String,
    login: String,
    no_manager: String,
    taken: String,
}

static TEXT: LazyLock<Text> = LazyLock::new(|| {
    // Invariant: the file is compiled into the binary, so it parses in every run of a build or
    // in none, and the first resident of any flow shows which.
    toml::from_str(include_str!("text.toml")).expect("src/residents/text.toml parses")
});

/// Every resident's heartbeat, schedules and night: one task in `razor serve`. Cheap to clone.
#[derive(Clone)]
pub struct Residents {
    inner: Arc<Inner>,
}

type Sort = (Option<SessionId>, Vec<SessionId>, BTreeSet<SessionId>);

struct Inner {
    controller: Controller,
    state: Mutex<State>,
    wake: Notify,
    sorting: watch::Sender<Sort>,
}

/// At most one entry per configured resident in `stopped` and `started`.
#[derive(Default)]
struct State {
    stopped: BTreeSet<SessionId>,
    /// When each running resident started in this process; at most one per configured home.
    started: BTreeMap<SessionId, Millis>,
    /// The fresh threads whose run's input is on its way: at most one per schedule being admitted.
    starting: BTreeSet<SessionId>,
    /// Closed fresh runs, by thread: only those a configured resident's rows still show.
    closed: BTreeMap<SessionId, Run>,
}

impl Residents {
    /// Start the task. On every wake, for each resident in the config that is not stopped:
    /// - On its first wake in this process it is started: `Controller::create` with the name as
    ///   the id, in its cwd, which makes the session the first time and opens it after that.
    /// - A schedule is admitted once when its latest due time is later than the latest `due` of
    ///   that name in the record, or, when the record has none, than the resident's start in
    ///   this process. The first occurrence of a schedule the record has never seen is lost if
    ///   Razor is down or crashes at its due time.
    /// - A heartbeat that came due since the last wake is admitted; `Busy` skips it.
    /// - A night that came due since the last wake asks the controller to rebuild.
    ///
    /// The task sleeps until the next due time, and at most 60 seconds.
    pub fn open(controller: Controller) -> Residents {
        let (sorting, snapshots) = watch::channel(Sort::default());
        let residents = Residents {
            inner: Arc::new(Inner {
                controller,
                state: Mutex::default(),
                wake: Notify::new(),
                sorting,
            }),
        };
        tokio::spawn(residents.clone().fire());
        tokio::spawn(residents.clone().sorts(snapshots));
        residents
    }

    /// Hand off a resolved snapshot without waiting for resident admission. One pending snapshot
    /// replaces the last while the single delivery task waits for queue space.
    pub(crate) fn sort(
        &self,
        definition: Option<&Views>,
        status: &ViewsStatus,
        present: impl Iterator<Item = SessionId>,
    ) {
        let auto = definition.and_then(|d| d.views.iter().find(|v| v.name == "Auto"));
        let wake = auto.and_then(|v| v.wake.clone());
        let roots = status
            .layouts
            .iter()
            .filter(|l| l.name == "Auto")
            .flat_map(|l| &l.sections)
            .filter(|s| s.kind == SectionKind::EverythingElse)
            .flat_map(|s| &s.sessions)
            .filter(|r| r.depth == 0)
            .map(|r| r.session.clone())
            .collect();
        self.inner
            .sorting
            .send_replace((wake, roots, present.collect()));
    }

    async fn sorts(self, mut snapshots: watch::Receiver<Sort>) {
        // Per-process deduplication, bounded by retained agents, independent of view edits.
        let mut sorted = BTreeSet::new();
        while snapshots.changed().await.is_ok() {
            let (wake, mut roots, present) = snapshots.borrow_and_update().clone();
            sorted.retain(|id| present.contains(id));
            let Some(wake) = wake else {
                continue;
            };
            if !self.config().await.residents.contains_key(&wake) {
                continue;
            }
            roots.retain(|id| !sorted.contains(id));
            if !roots.is_empty() {
                let text = format!("New unmatched roots in Auto: {}", serde_json::json!(roots));
                let from = Origin::Program {
                    name: "smart-sort".into(),
                };
                tokio::select! {
                    biased;
                    _ = snapshots.changed() => snapshots.mark_changed(),
                    admitted = self.inner.controller.admit(&wake, Input::new(from, text)) => {
                        match admitted {
                            Ok(_) => sorted.extend(roots),
                            Err(e) => eprintln!("razor: Auto view could not wake {wake}: {e}"),
                        }
                    }
                }
            }
        }
    }

    /// Whether a resident's timers run in `session`: its home session, or a fresh run's thread.
    /// Neither sends an alert when a turn ends, nor is followed.
    pub(crate) async fn timed(&self, session: &Summary) -> bool {
        let job = session.thread.as_ref().is_some_and(|t| t.job.is_some());
        job || self.config().await.residents.contains_key(&session.session)
    }

    /// Run `schedule` now, as when it comes due, with `due` now: in the home session, waiting for
    /// a busy turn, or in a fresh thread. Its entry in the home session.
    pub(crate) async fn run(&self, name: &SessionId, schedule: &str) -> Result<EntryId, Error> {
        let resident = self.resident(name).await?;
        let Some(s) = resident.schedules.get(schedule) else {
            let sentence =
                (TEXT.no_schedule.replace("{name}", name.as_str())).replace("{schedule}", schedule);
            return Err(Error::Refused(sentence));
        };
        self.schedule(name, schedule, s, now()).await
    }

    /// A thread of resident `name`: its folder, or its resident project's, and the thread its
    /// Start records, the person's when `job` is none, else a fresh run's of that schedule.
    pub(crate) async fn thread(
        &self,
        name: &SessionId,
        job: Option<String>,
        resident_project: Option<String>,
    ) -> Result<(PathBuf, Option<Thread>), Error> {
        let cwd = match &resident_project {
            Some(id) => self.project(name, id).await?,
            None => self.resident(name).await?.cwd,
        };
        let thread = Thread {
            resident: name.clone(),
            job,
            resident_project,
        };
        Ok((cwd, Some(thread)))
    }

    /// Start a stopped resident, as at the server's start: its missed schedules run once.
    pub(crate) async fn start(&self, name: &SessionId) -> Result<(), Error> {
        self.resident(name).await?;
        let mut state = self.state();
        if state.stopped.remove(name) {
            state.started.remove(name);
        }
        drop(state);
        self.inner.wake.notify_one();
        Ok(())
    }

    /// Stop a resident: nothing of its schedule fires until `start` or the server's next start,
    /// and its current turn is stopped as by Escape. Its session and record stay. Input sent to
    /// it still runs, and that is how the person wakes it.
    pub(crate) async fn stop(&self, name: &SessionId) -> Result<(), Error> {
        self.resident(name).await?;
        self.state().stopped.insert(name.clone());
        let stopped = self.inner.controller.stop(name).await;
        stopped.map_err(other("stopping the resident's turn"))
    }

    async fn fire(self) {
        let mut last = now();
        loop {
            let config = self.config().await;
            let now = now();
            if now / 60_000 > last / 60_000 {
                self.inner.controller.memory().tick(&self.inner.controller);
            }
            let mut sleep = 60_000;
            for (name, resident) in &config.residents {
                if self.state().stopped.contains(name) {
                    continue;
                }
                self.wake(name, resident, last, now).await;
                if let Some((_, at)) = next(resident, now) {
                    sleep = sleep.min(at.saturating_sub(now));
                }
            }
            let configured = |name: &SessionId| config.residents.contains_key(name);
            {
                let mut state = self.state();
                state.stopped.retain(|name| configured(name));
                state.started.retain(|name, _| configured(name));
            }
            last = last.max(now);
            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(sleep)) => {}
                () = self.inner.wake.notified() => {}
            }
        }
    }

    /// One resident's wake at `now`: its start in this process, then whatever came due since
    /// `last`.
    async fn wake(&self, name: &SessionId, resident: &Resident, last: Millis, now: Millis) {
        let c = &self.inner.controller;
        let started = self.state().started.get(name).copied();
        let since = match started {
            Some(since) => since,
            None => {
                let (cwd, effort) = (resident.cwd.clone(), resident.effort);
                let model = resident.model.clone().map(crate::profiles::id);
                if let Err(e) = c
                    .create(cwd, model, (effort, None), Some(name.clone()), None, None)
                    .await
                {
                    return eprintln!("razor: resident {name} cannot start: {e}");
                }
                self.state().started.insert(name.clone(), now);
                now
            }
        };
        let came = |at: &[Time], days: &[Day]| {
            calendar::due(at, days, now, true).is_some_and(|due| due > last)
        };
        if let Some(h) = (resident.heartbeat.as_ref()).filter(|h| came(&h.at, &h.days)) {
            let heartbeat = input(Origin::Heartbeat, h.text.clone(), &h.model, h.effort);
            match c.admit(name, heartbeat).await {
                Ok(_) | Err(controller::Error::Busy(_)) => {}
                Err(e) => eprintln!("razor: resident {name}'s heartbeat: {e}"),
            }
        }
        let recorded = match c.summary(name).await {
            Ok(summary) => summary.scheduled,
            Err(e) => return eprintln!("razor: resident {name}'s schedules: {e}"),
        };
        for (schedule, s) in resident.schedules.iter().take(64) {
            let Some(due) = calendar::due(&s.at, &s.days, now, true) else {
                continue;
            };
            if due <= recorded.get(schedule).copied().unwrap_or(since) {
                continue;
            }
            if let Err(e) = self.schedule(name, schedule, s, due).await {
                eprintln!("razor: resident {name}'s schedule {schedule}: {e}");
            }
        }
        if came(&[resident.night], &[]) {
            let input = self.day(name, &resident.cwd, now).await;
            if let Err(e) = c.night(name, input).await {
                eprintln!("razor: resident {name}'s night: {e}");
            }
        }
    }

    /// Before the night's rebuild, the home session gets one input asking it to tidy its memory
    /// and tasks, with the day's threads and fresh runs, a line each, so the resident keeps one
    /// mind (a thread's name and its last reply's first line, and a fresh run's job and line),
    /// and its memories' lines; the input's entry, for the rebuild, which waits for that input's
    /// turn to end. A busy home session settles its current batch, and gets the input after its
    /// turn, past the rebuild.
    async fn day(&self, name: &SessionId, cwd: &Path, now: Millis) -> Option<EntryId> {
        let (c, since) = (&self.inner.controller, now - DAY);
        let mut lines = Vec::new();
        let roots = crate::record::list(c.home(), None, Some(name), false, usize::MAX).await;
        for (session, .., changed) in roots.unwrap_or_default() {
            if crate::protocol::millis(changed) >= since
                && let Ok(summary) = c.summary(&session).await
            {
                let reply = self.reply(&session).await;
                lines.push(format!("- {session} {}: {reply}", summary.name));
            }
        }
        let home = c.summary(name).await.ok();
        let runs = home.iter().flat_map(|s| s.runs.since(since));
        for run in runs.filter(|r| r.session != *name) {
            let run = self.shown(run, home.as_ref()).await;
            lines.push(format!("- {}: {}", run.what, run.line));
        }
        let index = c.memory().index(c, cwd).await;
        let text = format!("{}\n\n{}\n\n{index}", TEXT.day, lines.join("\n"));
        let from = Origin::Program {
            name: "razor".into(),
        };
        let admitted = c.admit(name, Input::new(from, text)).await;
        let report = |e: &controller::Error| eprintln!("razor: resident {name}'s day: {e}");
        admitted.inspect_err(report).ok()
    }

    /// The first line of a session's last reply among its newest entries.
    async fn reply(&self, session: &SessionId) -> String {
        let mut said = String::new();
        if let Ok(mut page) = self.inner.controller.page(session, None, 64).await {
            while let Some(Ok(line)) = page.next_line().await {
                if let Ok(entry) = serde_json::from_str::<Entry>(&line)
                    && let Body::Text { text } = entry.body
                {
                    said = crate::record::preview(&text);
                }
            }
        }
        said
    }

    /// Admit a schedule's input as when due: in the home session, or in a fresh thread named
    /// for it after the home session records the run, so a crash between them loses the run
    /// rather than repeating it. The schedule's entry in the home session.
    async fn schedule(
        &self,
        name: &SessionId,
        schedule: &str,
        s: &Schedule,
        due: Millis,
    ) -> Result<EntryId, Error> {
        let c = &self.inner.controller;
        let text = match s.deliver {
            Deliver::None => s.text.clone(),
            Deliver::Ask => format!("{}\n\n{}", s.text, TEXT.ask),
            Deliver::Notify => format!("{}\n\n{}", s.text, TEXT.notify),
        };
        let from = Origin::Schedule {
            name: schedule.into(),
            due,
        };
        let input = input(from, text, &s.model, s.effort);
        if !s.fresh {
            return c
                .admit(name, input)
                .await
                .map_err(other("admitting a schedule"));
        }
        let (cwd, thread) = self.thread(name, Some(schedule.into()), None).await?;
        let created = c.create(
            cwd,
            input.model.clone(),
            (s.effort, None),
            None,
            None,
            thread,
        );
        let session = created
            .await
            .map_err(other("starting a fresh run's thread"))?;
        let name_it = Body::Name {
            name: format!("{schedule} · {}", calendar::label(due)),
        };
        c.append(&session, name_it)
            .await
            .map_err(other("naming a fresh run's thread"))?;
        self.state().starting.insert(session.clone());
        let fresh = Body::Fresh {
            schedule: schedule.into(),
            due,
            session: session.clone(),
        };
        let started = match c.append(name, fresh).await {
            Ok(entry) => c.admit(&session, input).await.map(|_| entry),
            Err(e) => Err(e),
        };
        self.state().starting.remove(&session);
        started.map_err(other("starting a fresh run"))
    }

    /// The resident's config; `NoResident` when there is none.
    pub(crate) async fn resident(&self, name: &SessionId) -> Result<Resident, Error> {
        let resident = self.config().await.residents.remove(name);
        resident.ok_or_else(|| Error::NoResident(name.clone()))
    }

    async fn config(&self) -> Config {
        self.inner.controller.config().await
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // A panic while the lock is held is already a bug; the sets are still whole.
        self.inner.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// The resident's heartbeat, schedules and night, each named as runs and `next` name it.
fn timers(resident: &Resident) -> impl Iterator<Item = (&str, &[Time], &[Day])> {
    let heartbeat = (resident.heartbeat.iter()).map(|h| ("heartbeat", &h.at[..], &h.days[..]));
    let schedules = (resident.schedules.iter().take(64))
        .map(|(name, s)| (name.as_str(), &s.at[..], &s.days[..]));
    let night = std::iter::once(("night", std::slice::from_ref(&resident.night), &[][..]));
    heartbeat.chain(schedules).chain(night)
}

/// What of `resident` comes due first after `now` (`heartbeat`, a schedule's name, or `night`),
/// and when: through the same function that fires it.
fn next(resident: &Resident, now: Millis) -> Option<(&str, Millis)> {
    (timers(resident))
        .filter_map(|(what, at, days)| Some((what, calendar::due(at, days, now, false)?)))
        .min_by_key(|(_, at)| *at)
}

fn input(from: Origin, text: String, model: &Option<String>, effort: Option<Effort>) -> Input {
    Input {
        from,
        text,
        model: model.clone().map(crate::profiles::id),
        effort,
        ..Default::default()
    }
}

/// A resident's identity record as its first input carries it: the heading naming
/// `identity.toml` in its session's directory (made from a template at its first start), then
/// the record, at most 64 KiB. A record that cannot be read, or does not parse, becomes one
/// sentence saying so, so the resident can fix it. None for a session that is not a resident.
/// A thread reads its resident's record, and its heading says the state note is the home's.
pub(crate) async fn identity(
    config: &Config,
    session: &SessionId,
    dir: &Path,
    thread: bool,
) -> Option<String> {
    config.residents.get(session)?;
    let path = dir.join("identity.toml");
    let mut new = tokio::fs::OpenOptions::new();
    if let Ok(mut file) = new.write(true).create_new(true).open(&path).await {
        let template = TEXT.identity.replace("{name}", session.as_str());
        // A failed write shows in the read below.
        let _ = file.write_all(template.as_bytes()).await;
    }
    let mut heading = TEXT
        .identity_heading
        .replace("{path}", &path.display().to_string());
    if thread {
        heading = format!("{heading} {}", TEXT.identity_thread);
    }
    Some(format!("{heading}\n\n{}", read(&path).await))
}

/// The identity record's text, or the sentence saying why it cannot be used.
async fn read(path: &Path) -> String {
    let t = &*TEXT;
    let shown = path.display().to_string();
    let mut text = String::new();
    let read = async {
        let file = tokio::fs::File::open(path).await?;
        file.take(FILE + 1).read_to_string(&mut text).await
    };
    match read.await {
        Err(e) => (t.unreadable.replace("{path}", &shown)).replace("{why}", &e.to_string()),
        Ok(n) if n as u64 > FILE => t.too_large.replace("{path}", &shown),
        Ok(_) => match toml::from_str::<toml::Table>(&text) {
            Err(e) => (t.unparsed.replace("{path}", &shown)).replace("{why}", e.message()),
            Ok(_) => text,
        },
    }
}

/// Notify for each scheduled input in a committed failed turn or withdrawal.
/// Reads only the named input entries (at most 64); detached notifications share the asks'
/// 16-command bound and `notify`'s 30-second limit; failures are logged, never recorded.
pub(crate) async fn failed(
    config: &Config,
    session: &SessionId,
    record: &Record,
    inputs: &[EntryId],
    end: &End,
) {
    let template = match end {
        End::Done => return,
        End::Error(_) => &TEXT.schedule_failed,
        End::Stopped => &TEXT.schedule_stopped,
    };
    // A fresh run's thread reports as its resident.
    let session = record
        .summary()
        .thread
        .as_ref()
        .map_or(session, |t| &t.resident);
    if !config.residents.contains_key(session) {
        return;
    }
    for id in inputs {
        let entry = match record.entry(*id).await {
            Ok(entry) => entry,
            Err(e) => {
                eprintln!("razor: resident {session}'s failed turn: {e}");
                continue;
            }
        };
        let Body::Input(Input {
            from: Origin::Schedule { name, .. },
            ..
        }) = entry.body
        else {
            continue;
        };
        let mut text = template
            .replace("{name}", session.as_str())
            .replace("{schedule}", &name);
        if let End::Error(why) = end {
            text = text.replace("{why}", why.trim_end_matches('.'));
        }
        notify(config, text);
    }
}

/// Send the person a notification through the notify command, detached, under the bound of 16
/// commands asks share. A failure, a 30-second timeout or a full bound is logged; with no
/// command configured, nothing runs. A note to the person is already in the inbox.
pub(crate) fn notify(config: &Config, text: String) {
    let Some((program, args)) = config.notify.split_first() else {
        return;
    };
    let Ok(permit) = RUNNING.try_acquire() else {
        return eprintln!("razor: 16 asks or notifications are running; this one is not sent.");
    };
    let mut run = Command::new(program);
    run.args(args).arg(&text);
    run.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    tokio::spawn(async move {
        let _permit = permit;
        match tokio::time::timeout(Duration::from_secs(30), run.status()).await {
            Ok(Ok(status)) if status.success() => {}
            Ok(Ok(status)) => eprintln!("razor: The notify command failed ({status}): {text}"),
            Ok(Err(e)) => eprintln!("razor: The notify command did not run ({e}): {text}"),
            Err(_) => eprintln!("razor: The notify command did not exit in 30 seconds: {text}"),
        }
    });
}

/// Carry an ask to the person through the ask command, and return at once. The command runs
/// detached, at most 16 at a time; when it exits 0 and prints an answer, Razor answers the ask
/// with `Controller::answer`, which refuses an ask already answered from a client.
pub(crate) fn ask(controller: &Controller, config: &Config, ask: &Ask) {
    let Some((program, args)) = config.ask.split_first() else {
        return;
    };
    let Ok(permit) = RUNNING.try_acquire() else {
        return eprintln!(
            "razor: 16 asks or failure notes are already running; this ask waits for a client."
        );
    };
    let mut question = ask.text.clone();
    for (i, option) in ask.options.iter().enumerate() {
        let number = (i + 1).to_string();
        question.push_str(
            &TEXT
                .option
                .replace("{n}", &number)
                .replace("{option}", option),
        );
    }
    let mut command = Command::new(program);
    command.args(args).arg(question);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let (controller, (session, entry)) = (controller.clone(), ask.id.clone());
    tokio::spawn(async move {
        let _permit = permit;
        let Ok(mut child) = command.spawn() else {
            return;
        };
        let mut answer = String::new();
        if let Some(out) = child.stdout.take() {
            let _ = out.take(FILE).read_to_string(&mut answer).await;
        }
        let answered = child.wait().await.is_ok_and(|status| status.success());
        let answer = answer.trim();
        if answered && !answer.is_empty() {
            let answer = Answer::Text(answer.to_string());
            // Refused when a client answered first, which is the one way it can fail here.
            let _ = controller.answer(&session, entry, answer, None).await;
        }
    });
}

/// Why a residents operation failed.
#[derive(Debug)]
pub enum Error {
    /// No resident by this name in the config.
    NoResident(SessionId),
    /// The sentence saying the resident has no such project.
    NoProject(String),
    /// One sentence for the model or the person: the `deliver` action's refusal, no notify
    /// command, a config value that does not parse.
    Refused(String),
    /// Any other failure, with its cause and what was being done.
    Other {
        source: Box<dyn std::error::Error + Send + Sync>,
        context: &'static str,
    },
}

fn other<E>(context: &'static str) -> impl FnOnce(E) -> Error
where
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    move |e| Error::Other {
        source: e.into(),
        context,
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoResident(name) => {
                f.write_str(&TEXT.no_resident.replace("{name}", name.as_str()))
            }
            Error::Refused(sentence) | Error::NoProject(sentence) => f.write_str(sentence),
            Error::Other { source, context } => f.write_str(
                &(TEXT.other)
                    .replace("{context}", context)
                    .replace("{source}", &source.to_string()),
            ),
        }
    }
}

impl std::error::Error for Error {}
