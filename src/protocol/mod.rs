//! The vocabulary the modules share (docs/design/protocol.md): ids, the record's entries,
//! inputs, states, job events, and the lines the API server streams. Plain data; the only
//! behaviour is making and parsing a session id, checking an MCP server's definition, and
//! reading the clock.

mod api;
mod children;
mod conversation;
mod mcp;
pub(crate) mod views;
mod voice;
pub use api::*;
pub(crate) use children::*;
pub(crate) use conversation::{BridgeInput, FromBridge, Line, Resume, ToBridge};
pub(crate) use mcp::{Connection, Mcp, OAuth, Reference};
pub(crate) use voice::*;

use serde::{Deserialize, Serialize};
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// The length, in characters, past which a result's text spills to a file (SPEC section 3,
/// "Tools"). The shell sizes the end of a job's output that it returns by this number.
pub(crate) const SPILL: usize = 30_000;

// ---- ids ---------------------------------------------------------------------------------

/// A session's id: `YYYYMMDD-HHMMSS-xxxx`, the UTC second it started plus four random hex
/// digits. Sorts by start time as text, and holds only digits, `a`-`f` and `-`, so it is safe
/// as a directory name and in a URL path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct SessionId(String);

impl SessionId {
    /// A fresh id for a session starting now. Two sessions started in the same second share
    /// an id once in 65,536; `Record::create` refuses the second and the caller draws again.
    pub(crate) fn new() -> SessionId {
        let secs = now() / 1000;
        let (days, rest) = (secs / 86_400, secs % 86_400);
        // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
        let z = days + 719_468;
        let doe = z % 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + z / 146_097 * 400 + u64::from(month <= 2);
        // RandomState is seeded from the OS's randomness; four hex digits only break ties.
        let random = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish()
            & 0xffff;
        let (h, m, s) = (rest / 3600, rest / 60 % 60, rest % 60);
        SessionId(format!(
            "{year:04}{month:02}{day:02}-{h:02}{m:02}{s:02}-{random:04x}"
        ))
    }

    /// Qualify a native reference for a directly paired home; native records stay unchanged.
    pub(crate) fn on(&self, server: &str) -> Self {
        if server.is_empty() || self.0.contains('@') {
            self.clone()
        } else {
            Self(format!("{}@{server}", self.0))
        }
    }

    /// Splits a directly paired reference into its native id and enrollment name.
    pub(crate) fn address(&self) -> Option<(Self, &str)> {
        let (id, server) = self.0.split_once('@')?;
        Some((Self(id.to_owned()), server))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// The name of a session whose Start stores none and that is no child: a named id (a
    /// resident's) is its own; a generated id's (a root recorded before names were stored) is
    /// an adjective and a noun from `names.txt` (256 of each), one drawn by each byte of its
    /// four random hex digits, so it is the name that root always showed.
    pub(crate) fn name(&self) -> String {
        let generated = self.0.starts_with(|c: char| c.is_ascii_digit());
        let Some(random) = (self.0.get(16..).filter(|_| generated))
            .and_then(|hex| u16::from_str_radix(hex, 16).ok())
        else {
            return self.0.clone();
        };
        let mut words = include_str!("names.txt").split_whitespace();
        let adjective = words.clone().nth(usize::from(random >> 8));
        let noun = words.nth(256 + usize::from(random & 0xff));
        // Invariant: names.txt holds 512 words, so both draws are in it.
        let (adjective, noun) = (adjective.unwrap_or("quiet"), noun.unwrap_or("river"));
        format!("{adjective}-{noun}")
    }
}

/// Refuses any string not in the format above, or `<id>@<server>`: a session on the paired
/// server enrolled here as `server`, which no session made here is.
impl TryFrom<String> for SessionId {
    type Error = Error;
    fn try_from(text: String) -> Result<SessionId, Error> {
        let (id, server) = text.split_once('@').unwrap_or((text.as_str(), "here"));
        let paired = (1..=32).contains(&server.len())
            && (server.bytes()).all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-');
        let b = id.as_bytes();
        let generated = b.len() == 20
            && b.iter().enumerate().all(|(i, c)| match i {
                8 | 15 => *c == b'-',
                16.. => c.is_ascii_digit() || (b'a'..=b'f').contains(c),
                _ => c.is_ascii_digit(),
            });
        let named = (1..=32).contains(&b.len())
            && b[0].is_ascii_lowercase()
            && b.iter()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-');
        if (generated || named) && paired {
            Ok(SessionId(text))
        } else {
            Err(Error::NotSessionId(text))
        }
    }
}

/// A value `protocol` refused to parse.
#[derive(Debug)]
pub enum Error {
    /// The text is not a session id.
    NotSessionId(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self::NotSessionId(text) = self;
        write!(
            f,
            "`{text}` is not a session id (YYYYMMDD-HHMMSS-xxxx or a lowercase name up to 32 characters)."
        )
    }
}

impl std::error::Error for Error {}

impl From<SessionId> for String {
    fn from(id: SessionId) -> String {
        id.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An entry's position in its session's record, counting from 1: dense, never reused. It is
/// the API's cursor and the argument to `recall`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
pub struct EntryId(pub u64);

/// A tool call's id as the model gave it (`toolu_…` on Claude, `call_…` on Responses).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CallId(pub(crate) String);

/// A job's number within its session, shown to the model as `job-N`. The shell gives it out;
/// at open the controller seeds the shell's counter from the record (highest seen, plus one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct JobId(pub(crate) u32);

/// Milliseconds since the Unix epoch.
pub(crate) type Millis = u64;

/// The time now, in `Millis`.
pub(crate) fn now() -> Millis {
    millis(SystemTime::now())
}

/// A time in `Millis`: 0 before 1970, the only failure.
pub(crate) fn millis(time: SystemTime) -> Millis {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as Millis)
}

// ---- the record ----------------------------------------------------------------------------

/// One line of a session's record, and of the API stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub(crate) id: EntryId,
    pub(crate) at: Millis,
    #[serde(flatten)]
    pub body: Body,
    /// What a model path needs to continue its own conversation, kept beside the common
    /// form and read by nothing else. Absent on entries Razor makes itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) native: Option<Native>,
}

/// What an entry says, tagged `"kind"` on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Body {
    /// The first entry of every session, and only there.
    Start {
        cwd: PathBuf,
        model: String,
        #[serde(flatten)]
        settings: ModelSettings,
        profile_version: String,
        /// The name Razor gave a root as it started. Empty for a child (its name is its
        /// `Child::name`) and for a resident (its id is its name).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        child: Option<Child>,
        /// The resident this session is a thread of; boxed, as few sessions are.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread: Option<Box<Thread>>,
    },
    /// The session's new name, from the person or the agent.
    Name {
        name: String,
    },
    /// The person's effort choice, applied without an input or a new turn.
    Effort {
        effort: Effort,
    },
    /// In a resident's home session: a schedule's run started in a fresh thread. It advances
    /// the missed-run marker as a schedule's input does.
    Fresh {
        schedule: String,
        due: Millis,
        session: SessionId,
    },
    /// An admitted input.
    Input(Input),
    /// A fact of a live call or a recorded meeting: its start, one speaker's utterance, a
    /// meeting's naming, its end. It never starts a turn; the session's next batch carries it to
    /// the model unless a hand-off already did.
    VoiceCall {
        call: String,
        #[serde(default, skip_serializing_if = "Source::is_call")]
        source: Source,
        #[serde(flatten)]
        event: CallEvent,
    },
    /// Admitted inputs taken back before delivery; they stay recallable, not part of a turn.
    Withdrawn {
        inputs: Vec<EntryId>,
    },
    /// Text the model showed.
    Text {
        text: String,
    },
    /// A provider notice, kept once for the request that emitted it.
    Notice {
        text: String,
    },
    /// The model's reasoning as it showed it; never empty. Reasoning kept only encrypted
    /// lives in the model path's native data.
    Thinking {
        text: String,
    },
    /// A tool call's intent, in the record before the call's action starts. `name` is the
    /// name the model used.
    Call {
        call: CallId,
        name: String,
        input: serde_json::Value,
    },
    /// A tool call's result: exactly one per call, after its `Call`. `output` is what the
    /// model saw.
    Result {
        call: CallId,
        outcome: Outcome,
        output: String,
        /// News the call took as it waited, which this result answers: the output holds its words.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        inputs: Vec<EntryId>,
    },
    /// A server's question reaches the same person-facing ask surface as an agent's question.
    Question {
        text: String,
        options: Vec<String>,
        /// A pasted sign-in address is consumed before recording the answer.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        redirect: bool,
    },
    /// A job's start or end, as the shell reports it.
    Job(JobEvent),
    /// The explicit Input/Rebuild entries this turn answered, never an inferred numeric prefix.
    TurnEnd {
        inputs: Vec<EntryId>,
        end: End,
        usage: Usage,
        context: Option<u64>,
        /// The limits resolved when this turn ended; absent in synthetic ends.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        threshold: Option<f64>,
        /// The turn answered `/done`'s input: the session is closed until its next input.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        closed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        close_reason: Option<CloseReason>,
        /// A child's report to its parent, in the same write as the end of the turn it ends.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        report: Option<Box<Input>>,
    },
    /// A rebuild: what set it off, the note it carried, and the context it sent verbatim. The
    /// context counts as an input; taking it answers only the inputs at or before `cut`.
    Rebuild {
        trigger: Trigger,
        note: String,
        context: String,
        profile_version: String,
        cut: EntryId,
    },
    /// One completed request on a model path that renders its own requests (Responses), with
    /// `Native::Responses`. It is appended before any of its actions runs, and its `Text`,
    /// `Thinking` and `Call` entries follow it with no native data of their own.
    Response {
        usage: Usage,
    },
    Child {
        session: SessionId,
        event: ChildEvent,
    },
    Headline {
        text: String,
    },
    ChangedFiles {
        files: Vec<PathBuf>,
    },
    MailSent {
        #[serde(rename = "mail_id")]
        id: MailId,
        to: String,
        #[serde(rename = "mail_kind")]
        kind: MailKind,
        /// The message's first line, at most 200 Unicode characters.
        text: String,
    },
}

/// What makes a session one of a resident's threads: the resident, and for a fresh run, its job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub(crate) resident: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) job: Option<String>,
    /// The resident project a person's thread started in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) resident_project: Option<String>,
}

/// Anything that enters a session; every input goes through `Controller::admit`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Input {
    /// A server-requested handover, so clients can hide it without reading prompt text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) trigger: Option<Trigger>,
    pub(crate) from: Origin,
    /// The enrolled device whose key admitted the person's input, answer or call hand-off, a
    /// phone or a paired server forwarding it; absent for the local key and other origins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) device: Option<String>,
    pub(crate) text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) effort: Option<Effort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) speed: Option<Speed>,
    /// `ClientInput::handover`, kept so the choice survives a restart while the input waits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) handover: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) images: Vec<Image>,
    /// Files the person sent, relative to the session's folder; the model reads each one's path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) files: Vec<PathBuf>,
    /// `ClientInput::request`, kept so a repeat is recognized across a restart.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) request: Option<String>,
    /// The parent's latest Started/Continued entry on a child's task inputs and reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) child_task: Option<EntryId>,
    /// A report whose child has no commits beyond its base (also true without a branch).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) child_no_changes: bool,
    /// Set only on a voice note, whose `text` is its transcript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) voice: Option<Box<Voice>>,
    /// Set only on a live call's hand-off, whose `text` is the person's words in these lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) voice_call: Option<Box<VoiceCallInput>>,
    /// Set only on a meeting's notes input, the meeting's id: Razor sends it once the batch pass
    /// ends, and recording it finishes the meeting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) meeting: Option<String>,
    /// Set only on an input that carries a video.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) video: Option<Box<Video>>,
    /// Set only on the input a person's `/done` or `/handover` admits, so a client draws it as
    /// that command rather than Razor's text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) command: Option<Command>,
}

/// The person's command an input carries out; Razor writes its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Command {
    Done,
    Handover,
}

impl Command {
    /// Razor's input that carries out this command with `text`.
    pub(crate) fn input(self, text: String) -> Input {
        let command = Some(self);
        Input {
            command,
            ..Input::new(Origin::Razor, text)
        }
    }

    /// What the person types for it.
    pub(crate) fn typed(self) -> &'static str {
        match self {
            Command::Done => "/done",
            Command::Handover => "/handover",
        }
    }
}

/// A video the person sent; one type on the wire and in the record (docs/design/video.md).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Video {
    /// The video, relative to the session's folder as `POST /v1/sessions/{id}/files/{name}`
    /// returned it; MP4 or QuickTime. Absolute only in a watcher's first input.
    pub(crate) file: PathBuf,
    /// Its length as the sending client measured it; shown to the model and to people.
    pub(crate) duration_ms: u64,
    /// Set by the server, never a client: the child it started to watch this video, because the
    /// model reading it does not take video. Absent when the model sees it itself, or when no
    /// watcher could start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) watcher: Option<String>,
}

/// A spoken input or delivery's audio and waveform; one type on the wire and in the record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Voice {
    /// The audio, relative to the session's folder, as `POST /v1/sessions/{id}/files/{name}`
    /// returned it; Razor does not read its format.
    pub(crate) file: PathBuf,
    /// Its length as the recording client measured it; it sets the transcription's time limit.
    pub(crate) duration_ms: u64,
    /// Loudness from start to end in equal steps, 0 silent to 255 loudest, at most 64 values;
    /// kept and returned, never read. Empty draws as a flat line.
    #[serde(default)]
    pub(crate) levels: Vec<u8>,
}

impl Input {
    /// `text` from `from`, with no model, effort, images or request id.
    pub(crate) fn new(from: Origin, text: String) -> Input {
        Input {
            from,
            text,
            ..Default::default()
        }
    }
}

/// A session-owned image; paths in records are relative to the session directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Image {
    pub(crate) media_type: String,
    pub(crate) path: PathBuf,
}

impl Image {
    /// The images a tool result's JSON names in its `images` field; none for any other output,
    /// which is never parsed.
    pub(crate) fn named(output: &str) -> Vec<Image> {
        if !output.contains(r#""images":[{"#) {
            return Vec::new();
        }
        let value: serde_json::Value = serde_json::from_str(output).unwrap_or_default();
        serde_json::from_value(value["images"].clone()).unwrap_or_default()
    }
}

/// Who an input is from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Origin {
    /// A person, through any client.
    #[default]
    Person,
    /// Razor's threshold request or job-end notice.
    Razor,
    /// Skipped without appending when the resident is busy.
    Heartbeat,
    /// Its selection overrides apply only to the turn it opens.
    Schedule {
        name: String,
        due: Millis,
    },
    /// A person's answer to a recorded ask.
    Answer {
        ask: EntryId,
    },
    Program {
        name: String,
    },
    Mail {
        id: MailId,
        kind: MailKind,
        /// The sender as its reader sees it: a tree member's name, `<name> in <project>` from
        /// outside the tree, `<name>@<server> in <project>` from a paired server. Empty on a
        /// brief, which renders bare.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        from: String,
    },
    Report {
        child: SessionId,
        status: ReportStatus,
    },
}

impl Origin {
    pub(crate) fn is_person(&self) -> bool {
        matches!(self, Self::Person)
    }
}
/// How a tool call came out. `Unknown`: the call has no recorded result because Razor
/// stopped first, or the model path never reported one; it is never run again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Success,
    Error,
    Unknown,
}

/// How a turn ended. `Error` carries one sentence for the person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum End {
    Done,
    Stopped,
    Error(String),
}

/// Why a conversation closed; absent on internal interruptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseReason {
    Person,
    Idle,
}

/// What set off a rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Threshold,
    Overflow,
    Handover,
    Idle,
    Restart,
    Change,
    Night,
}

/// A turn's token counts, summed over its model requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) cache_read: u64,
    pub(crate) cache_write: u64,
    /// Model requests in the turn: counted on the Responses route, the SDK's `num_turns` on
    /// Claude's.
    pub(crate) requests: u64,
    /// Milliseconds the requests waited on a chat host's 429s before they were answered.
    pub(crate) waited: u64,
}

/// The effort a model is asked for, from least to most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    /// Every effort Razor can request, from least to most.
    pub(crate) const ALL: [Self; 5] = [Self::Low, Self::Medium, Self::High, Self::Xhigh, Self::Max];
}

/// Effort and speed travel together without changing model or context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSettings {
    pub(crate) effort: Effort,
    pub(crate) speed: Speed,
}
/// Requested serving speed, independent of effort.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Speed {
    #[default]
    Standard,
    Fast,
    Ultrafast,
}
impl Speed {
    pub(crate) const ALL: [Self; 3] = [Self::Standard, Self::Fast, Self::Ultrafast];
    pub(crate) fn suffix(self) -> &'static str {
        match self {
            Self::Standard => "",
            Self::Fast => " · fast",
            Self::Ultrafast => " · ultrafast",
        }
    }
}
/// Native continuation data, one variant per model path that needs it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Native {
    /// The SDK conversation an entry was mirrored from, and that message's uuid (the point a
    /// resume cuts at).
    Claude { conversation: String, uuid: String },
    /// On a `Response`: the request-side items first sent with that request (a live tool
    /// addition), then its whole output array, each byte for byte. The next request replays
    /// additions before output, so the cached prefix is unchanged.
    Responses {
        additions: Vec<serde_json::Value>,
        items: Vec<serde_json::Value>,
    },
}

// ---- jobs: the seam with the shell ------------------------------------------------------------

/// A job's start or end. Its output is the session directory plus `job-N`; Started names
/// the durable Call entry so a notice can recover its original input even after restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum JobEvent {
    /// A job's session of processes started; `pid` leads it. `process_start` is the boot
    /// identity plus the OS start time of `pid`, so a reused pid is never mistaken for the job.
    Started {
        job: JobId,
        call: EntryId,
        pid: u32,
        pty: bool,
        process_start: String,
        /// The absolute deadline in Unix milliseconds; absent for an unlimited job.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deadline: Option<Millis>,
    },
    /// A job ended: every process of its session is gone. `tail` is its output's last two lines
    /// with content as it left them, at most 1 KiB each; empty for a lost job.
    Ended {
        job: JobId,
        end: JobEnd,
        tail: Vec<String>,
    },
}

/// How a job ended, as the OS reported it. `Unknown`: its exit could not be collected (a restart
/// found it gone, or reattached it and could not reap it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobEnd {
    Exit(i32),
    Signal(i32),
    Unknown,
}

// ---- the API stream -------------------------------------------------------------------------

/// One agent's one state. `Done` and `Stuck` arrive with children and residents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum State {
    NeedsYou,
    Working,
    Done,
    Stuck,
}

/// One line of the API's event stream, and of `--print stream`: a record entry exactly as
/// stored, or a live line that is never stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Event {
    Entry(Box<Entry>),
    Live(Live),
}
