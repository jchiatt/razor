//! The residents list as `GET /v1/residents` shows it: each resident's timetable, its due times
//! for the next day and its runs of the last day, a fresh run shown as its thread's own.

use super::{
    DAY, Day, Deliver, Error, FILE, Resident, Residents, TEXT, Time, calendar, next, timers,
};
use crate::protocol::{Millis, ResidentDay, Run, RunState, SessionId, now};
use crate::record::Summary;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use tokio::io::AsyncReadExt;

/// Identity only: configured home ids for layouts, with no upcoming wakes or past runs.
pub(crate) fn homes(config: &super::Config) -> Vec<ResidentDay> {
    let day = |session: &SessionId| ResidentDay {
        session: session.clone(),
        upcoming: Vec::new(),
        runs: Vec::new(),
    };
    config.residents.keys().map(day).collect()
}

/// One resident as `GET /v1/residents` lists it. Its state, open asks and waiting inputs are its
/// home session's, on the status stream. Times of day are this machine's local time.
#[derive(Serialize)]
pub(crate) struct Listed {
    pub(crate) session: SessionId,
    pub(crate) stopped: bool,
    #[serde(flatten)]
    pub(crate) names: Names,
    pub(crate) heartbeat: Option<Due>,
    pub(crate) schedules: BTreeMap<String, Due>,
    pub(crate) night: Time,
    /// What comes due next (`heartbeat`, a schedule's name, or `night`) and when, within a week.
    /// None while stopped.
    pub(crate) next: Option<(String, Millis)>,
    /// Each time its heartbeat, a schedule or its night comes due in the next 24 hours, soonest
    /// first, from the function that fires them. Empty while stopped.
    pub(crate) upcoming: Vec<(String, Millis)>,
    /// Its runs opened in the last 24 hours, newest first.
    pub(crate) runs: Vec<Run>,
    /// The days of its `sky` file, oldest first; absent when it names none or the file is unusable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sky: Option<Vec<Star>>,
}

/// One day of a resident's `sky` file: `day` is `YYYY-MM-DD` and `n` the commitments closed.
#[derive(Serialize, Deserialize)]
pub(crate) struct Star {
    day: String,
    n: u32,
}

/// One of a resident's timers as the phone shows it.
#[derive(Serialize)]
pub(crate) struct Due {
    pub(crate) at: Vec<Time>,
    pub(crate) days: Vec<Day>,
    pub(crate) deliver: Deliver,
    /// The first sentence of its text, at most 200 characters.
    pub(crate) does: String,
    pub(crate) fresh: bool,
    /// Its newest run, however old.
    pub(crate) latest: Option<Run>,
}

impl Listed {
    /// The sessions its runs and its timers' latest runs ran in.
    fn shown(&self) -> impl Iterator<Item = SessionId> {
        let timers = self.heartbeat.iter().chain(self.schedules.values());
        let latest = timers.filter_map(|due| due.latest.as_ref());
        self.runs
            .iter()
            .chain(latest)
            .map(|run| run.session.clone())
    }
}

/// From a resident's `identity.toml`; empty when absent or when the file does not parse.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Names {
    pub(crate) display_name: String,
    pub(crate) role: String,
}

impl Residents {
    /// What resident `name` remembers, newest first, as memory lists it.
    pub(crate) async fn memory(&self, name: &SessionId) -> Result<Value, Error> {
        let cwd = self.resident(name).await?.cwd;
        let c = &self.inner.controller;
        let list = c.memory().list(c, &cwd).await;
        list.map_err(|e| Error::Refused(e.to_string()))
    }

    /// Deletes memory `id` for the person, words and all, as one of resident `name`'s.
    pub(crate) async fn forget(&self, name: &SessionId, id: &str) -> Result<(), Error> {
        let cwd = self.resident(name).await?.cwd;
        let c = &self.inner.controller;
        let forgot = c.memory().forget(c, &cwd, id).await;
        forgot.map(drop).map_err(|e| Error::Refused(e.to_string()))
    }

    /// Every resident in the config, by name: one identity file read and one copy of its loaded
    /// summary each, and a fresh run's thread summary until that run has ended.
    pub(crate) async fn list(&self) -> Vec<Listed> {
        let (config, now) = (self.config().await, now());
        let mut listed = Vec::new();
        for (name, resident) in config.residents {
            listed.push(self.row(name, &resident, now).await);
        }
        // Ended fresh runs are kept only while a row shows them.
        let shown: BTreeSet<_> = listed.iter().flat_map(Listed::shown).collect();
        self.state()
            .closed
            .retain(|thread, _| shown.contains(thread));
        listed
    }

    /// Each resident's day for `Server::residents`: its wakes, and its runs as `list` shows them.
    pub(crate) async fn days(&self) -> Vec<ResidentDay> {
        let (config, now) = (self.config().await, now());
        let (mut days, mut shown) = (Vec::new(), BTreeSet::new());
        for (name, resident) in config.residents {
            // A resident whose home is not made yet has no session to open.
            let Ok(summary) = self.inner.controller.summary(&name).await else {
                continue;
            };
            let mut runs = Vec::new();
            for run in summary.runs.since(now - DAY) {
                let run = self.shown(run, Some(&summary)).await;
                shown.insert(run.session);
                runs.push((run.what, run.ended.unwrap_or(run.at), run.state));
            }
            let stopped = self.state().stopped.contains(&name);
            let upcoming = if stopped {
                Vec::new()
            } else {
                upcoming(&resident, now)
            };
            days.push(ResidentDay {
                session: name,
                upcoming,
                runs,
            });
        }
        self.state()
            .closed
            .retain(|thread, _| shown.contains(thread));
        days
    }

    async fn row(&self, name: SessionId, resident: &Resident, now: Millis) -> Listed {
        let stopped = self.state().stopped.contains(&name);
        let summary = self.inner.controller.summary(&name).await.ok();
        let home = summary.as_ref();
        let mut runs = Vec::new();
        for run in home.into_iter().flat_map(|s| s.runs.since(now - DAY)) {
            runs.push(self.shown(run, home).await);
        }
        let heartbeat = resident.heartbeat.as_ref();
        let mut heartbeat = heartbeat.map(|h| timer(&h.at, &h.days, &h.text, Deliver::None, false));
        let mut schedules = BTreeMap::new();
        for (what, s) in resident.schedules.iter().take(64) {
            let due = timer(&s.at, &s.days, &s.text, s.deliver, s.fresh);
            schedules.insert(what.clone(), due);
        }
        let timers = (heartbeat.iter_mut().map(|due| ("heartbeat", due)))
            .chain(schedules.iter_mut().map(|(what, due)| (what.as_str(), due)));
        for (what, due) in timers {
            if let Some(run) = home.and_then(|s| s.runs.latest(what)) {
                due.latest = Some(self.shown(run, home).await);
            }
        }
        let (next, upcoming) = match stopped {
            true => (None, Vec::new()),
            false => (next(resident, now), upcoming(resident, now)),
        };
        Listed {
            names: self.names(&name).await,
            session: name,
            stopped,
            heartbeat,
            schedules,
            night: resident.night,
            next: next.map(|(what, at)| (what.into(), at)),
            upcoming,
            runs,
            sky: sky(resident).await,
        }
    }

    /// A run as the list shows it. A fresh run is its thread's own row: starting while its input
    /// is on its way, failed when no admission is in progress and the thread holds no run, and
    /// kept once it has ended, so an ended thread is read once.
    pub(super) async fn shown(&self, run: &Run, home: Option<&Summary>) -> Run {
        let thread = &run.session;
        if run.state != RunState::Starting || self.state().starting.contains(thread) {
            return current(run, home);
        }
        if let Some(ended) = self.state().closed.get(thread) {
            return ended.clone();
        }
        let summary = self.inner.controller.summary(thread).await.ok();
        let own = summary.as_ref().and_then(|s| s.runs.latest(&run.what));
        let own = own
            .map(|own| current(own, summary.as_ref()))
            .unwrap_or_else(|| Run {
                state: RunState::Failed,
                error: Some(TEXT.never_arrived.clone()),
                ..run.clone()
            });
        if own.ended.is_some() || own.state == RunState::Failed {
            self.state().closed.insert(thread.clone(), own.clone());
        }
        own
    }

    /// `display_name` and `role` from the resident's `identity.toml`.
    async fn names(&self, name: &SessionId) -> Names {
        let mut text = String::new();
        if let Ok(file) = self.inner.controller.file(name, "identity.toml").await {
            // An unreadable file shows as empty names, as one that does not parse does.
            let _ = file.take(FILE).read_to_string(&mut text).await;
        }
        toml::from_str(&text).unwrap_or_default()
    }
}

/// The resident's `sky` file, of which at most 64 KiB is read, so a longer one is cut mid-list
/// and does not parse. None when it names no file or the file is missing or not a list of days,
/// so one bad file leaves out its field, not the row.
async fn sky(resident: &Resident) -> Option<Vec<Star>> {
    let path = resident.cwd.join(resident.sky.as_ref()?);
    let mut bytes = Vec::new();
    let file = tokio::fs::File::open(path).await.ok()?;
    file.take(FILE).read_to_end(&mut bytes).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Every time each timer comes due in the 24 hours after `now`, soonest first, read as the
/// function that fires them reads it, so timers due together all appear.
fn upcoming(resident: &Resident, now: Millis) -> Vec<(String, Millis)> {
    let mut due: Vec<_> = (timers(resident))
        .flat_map(|(what, at, days)| {
            let ahead = calendar::ahead(at, days, now);
            ahead.into_iter().map(move |at| (what.to_string(), at))
        })
        .collect();
    due.sort_by_key(|(_, at)| *at);
    due
}

/// A timer as the phone shows it, before its latest run is found.
fn timer(at: &[Time], days: &[Day], text: &str, deliver: Deliver, fresh: bool) -> Due {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    let end = ([". ", "? ", "! "].iter())
        .filter_map(|s| line.find(s))
        .min();
    let does = line[..end.map_or(line.len(), |i| i + 1)]
        .chars()
        .take(200)
        .collect();
    Due {
        at: at.to_vec(),
        days: days.to_vec(),
        deliver,
        does,
        fresh,
        latest: None,
    }
}

/// A run as its session shows it: interrupted while open in an interrupted session.
fn current(run: &Run, session: Option<&Summary>) -> Run {
    let open = matches!(run.state, RunState::Waiting | RunState::Running);
    let state = if open && session.is_some_and(Summary::interrupted) {
        RunState::Interrupted
    } else {
        run.state
    };
    Run {
        state,
        ..run.clone()
    }
}
