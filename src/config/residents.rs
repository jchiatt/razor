//! `[residents.<name>]` (docs/design/residents.md): each resident's home, heartbeat, schedules
//! and night. Every key is live: the schedule re-reads the config on every wake (at least once a
//! minute), and the rest are read at their next use.

use super::Error;
use crate::protocol::Effort;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

/// One resident: plain config a pack can hold.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct Resident {
    /// Its home: its session's working directory, fixed when the session is made, as any
    /// session's is.
    pub(crate) cwd: PathBuf,
    /// Its own startup list, in place of the person's `startup`; its persona file is one of the
    /// entries, wherever the person puts it. Absent: the person's list.
    pub(crate) startup: Option<Vec<String>>,
    /// Defaults used only when the home session is first made.
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<Effort>,
    /// When its night comes, in local time. Default 03:00.
    #[serde(default = "night")]
    pub(crate) night: Time,
    pub(crate) heartbeat: Option<Heartbeat>,
    /// A JSON file the person's phone draws from, passed through on its row as `sky`: a list of
    /// `{day, n}` objects, oldest first, one per day the resident closed `n` commitments.
    /// Relative to `cwd`, or absolute. A file missing, over 64 KiB or not such a list is left out.
    pub(crate) sky: Option<PathBuf>,
    /// By name; the first 64 by name are read.
    #[serde(default)]
    pub(crate) schedules: BTreeMap<String, Schedule>,
}

fn night() -> Time {
    Time(3 * 3600)
}

/// The input that wakes a resident at set times. A heartbeat that finds its resident busy is
/// skipped, and one missed while Razor was down is not made up.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct Heartbeat {
    pub(crate) at: Vec<Time>,
    /// The weekdays it runs on; empty is every day.
    #[serde(default)]
    pub(crate) days: Vec<Day>,
    pub(crate) text: String,
    /// For the turn this input opens only. Absent: the session's own.
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<Effort>,
}

/// A named input that comes due at set times, a "named job". One that comes due while its
/// resident is busy waits for the turn to end. One that came due while Razor was down runs once
/// at the next start, if the record holds an earlier due time of the same name.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct Schedule {
    pub(crate) at: Vec<Time>,
    #[serde(default)]
    pub(crate) days: Vec<Day>,
    pub(crate) text: String,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<Effort>,
    #[serde(default)]
    pub(crate) deliver: Deliver,
    /// Each run gets a thread of its own, on this schedule's model and effort, which closes when
    /// the run ends; otherwise it runs in the resident's home session.
    #[serde(default)]
    pub(crate) fresh: bool,
}

/// What the schedule's turn is told to do with its result, in one sentence after its text.
/// `Ask`: end by asking the person. `Notify`: end by sending the person a note. `None`: nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Deliver {
    #[default]
    None,
    Ask,
    Notify,
}

/// A time of day in local time, written `HH:MM` or `HH:MM:SS`: seconds after midnight. A time
/// that a clock change skips comes due at the time `mktime` gives for it; a time that occurs
/// twice comes due once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct Time(pub(crate) u32);

/// A weekday, written `mon` to `sun`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Day {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl TryFrom<String> for Time {
    type Error = Error;
    fn try_from(text: String) -> Result<Time, Error> {
        let parts: Result<Vec<u32>, _> = text.split(':').map(str::parse).collect();
        match parts.as_deref() {
            Ok(&[h, m]) if h < 24 && m < 60 => Ok(Time(h * 3600 + m * 60)),
            Ok(&[h, m, s]) if h < 24 && m < 60 && s < 60 => Ok(Time(h * 3600 + m * 60 + s)),
            _ => Err(Error::Invalid(format!(
                "`{text}` is not a time of day; write it as HH:MM or HH:MM:SS, in local time."
            ))),
        }
    }
}

/// Written back as it is read: `HH:MM`, or `HH:MM:SS` when it has seconds.
impl From<Time> for String {
    fn from(Time(t): Time) -> String {
        let (h, m, s) = (t / 3600, t / 60 % 60, t % 60);
        match s {
            0 => format!("{h:02}:{m:02}"),
            _ => format!("{h:02}:{m:02}:{s:02}"),
        }
    }
}
