//! Data shared by API clients and the controller; no client owns a second definition.

use super::{Ask, Decision, Effort, EntryId, JobId, Millis, Report, SessionId, State, views};
use crate::protocol::Speed;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

/// The upload limits apply before any decoded bytes enter admission.
pub(crate) const IMAGE_BASE64: usize = 10_000_000;
/// The most bytes one image may hold, from a person or a file: what `IMAGE_BASE64` characters of
/// base64 decode to (7.5 MB).
pub(crate) const IMAGE_BYTES: usize = IMAGE_BASE64 / 4 * 3;
pub(crate) const IMAGES_PER_INPUT: usize = 20;
pub(crate) const BODY_BYTES: usize = 32 << 20;
pub(crate) const PAGE_BYTES: u64 = 8 << 20;

/// A PNG, JPEG or WebP image's media type and file extension, from its first bytes; `None` for
/// any other bytes, whatever the file is named.
pub(crate) fn image_type(head: &[u8]) -> Option<(&'static str, &'static str)> {
    match head {
        [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', ..] => Some(("image/png", "png")),
        [0xff, 0xd8, 0xff, ..] => Some(("image/jpeg", "jpg")),
        _ if head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP") => {
            Some(("image/webp", "webp"))
        }
        _ => None,
    }
}

/// `video/mp4` or `video/quicktime` from a file's first bytes, an ISO media `ftyp` box (brand
/// `qt  ` for QuickTime); `None` for any other bytes.
pub(crate) fn video_type(head: &[u8]) -> Option<&'static str> {
    match (head.get(4..8), head.get(8..12)) {
        (Some(b"ftyp"), Some(b"qt  ")) => Some("video/quicktime"),
        (Some(b"ftyp"), Some(_)) => Some("video/mp4"),
        _ => None,
    }
}

/// An input from a client; an outside program supplies its name in `from`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientInput {
    pub text: String,
    #[serde(default)]
    pub images: Vec<String>,
    /// Paths that `POST /v1/sessions/{id}/files` returned, relative to the session's folder.
    #[serde(default)]
    pub files: Vec<PathBuf>,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub speed: Option<Speed>,
    /// Only when `model` names another model: true starts it from a handover note, false keeps
    /// the conversation; absent, the server keeps it when it can. Never a reason to refuse.
    pub handover: Option<bool>,
    pub from: Option<String>,
    /// A skill handed with the words: the `SKILL.md` the `skill` tool finds by this name is
    /// recorded ahead of them, between `<skill name="…">` and `</skill>` lines.
    pub skill: Option<String>,
    /// The client's own id for this input, 1 to 64 ASCII letters, digits and `-`. A repeat of an
    /// id among the session's newest 64 is answered with that input's entry and admits nothing.
    pub request: Option<String>,
    /// A voice note: answered once its transcript, which follows `text`, is admitted.
    #[serde(default)]
    pub voice: Option<super::Voice>,
    /// A video uploaded through the files route; answered at once, as typed words are.
    #[serde(default)]
    pub video: Option<super::Video>,
}

/// A skill a session's `skill` tool can load, with its front matter's description.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Skill {
    pub(crate) name: String,
    pub(crate) description: String,
}

/// The person's emoji history, ranked by the server: at most 32 base emoji by uses, then
/// recency, and the skin tone (1 to 5) last chosen for each that took one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Emoji {
    pub(crate) recent: Vec<String>,
    pub(crate) tones: BTreeMap<String, u8>,
}

/// One insertion of a base emoji; a tone sets the one remembered for it, 0 clears it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct EmojiUse {
    pub(crate) emoji: String,
    pub(crate) tone: Option<u8>,
}

/// Named choices from the shipped profile table and the live configuration's defaults;
/// prefix-accepted model names are not falsely enumerated as a closed set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SpeedChoice {
    pub(crate) speed: Speed,
    pub(crate) reason: Option<String>,
    pub(crate) hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Models {
    pub(crate) models: Vec<String>,
    /// The named models whose context window is known before a turn; no guessed Claude window.
    pub(crate) windows: BTreeMap<String, u64>,
    /// Every listed model's offered efforts, from least to most, including uncapped models.
    pub(crate) efforts: BTreeMap<String, Vec<Effort>>,
    pub(crate) speeds: BTreeMap<String, Vec<SpeedChoice>>,
    /// Checked only with `?ready=true`; absent means Razor found the credentials, not that a
    /// model request will succeed. Without that query this map is empty.
    pub(crate) unavailable: BTreeMap<String, String>,
    pub(crate) model: String,
    pub(crate) effort: crate::protocol::Effort,
    /// What a session started for a call runs on.
    pub(crate) call_model: String,
    pub(crate) call_effort: crate::protocol::Effort,
    pub(crate) speed: crate::protocol::Speed,
    /// With `?session=`: each listed model other than the session's own, as a switch to it.
    pub(crate) switch: BTreeMap<String, Switch>,
}

/// What switching a session to a model reads without the prompt cache.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Switch {
    /// The conversation kept: its latest measured size; absent when it cannot be kept.
    pub(crate) keep: Option<u64>,
    /// A fresh conversation from a handover note, estimated; absent when the context is under
    /// `handover_tokens` and is kept.
    pub(crate) handover: Option<u64>,
    /// Why the conversation cannot be kept, as one sentence.
    pub(crate) reason: Option<String>,
}

/// An option's zero-based index, or the person's own words.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Answer {
    Option(usize),
    Text(String),
}

/// Newest-first roots with their latest model and effort; title is the first person's first line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Listed {
    pub session: SessionId,
    pub(crate) cwd: PathBuf,
    pub(crate) model: String,
    pub(crate) title: String,
    pub(crate) at: Millis,
    #[serde(default)]
    pub(crate) name: String,
    pub(crate) repo: Option<String>,
    pub(crate) effort: Option<Effort>,
    pub(crate) speed: Option<Speed>,
    /// The resident a person's thread belongs to; absent on any other session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) resident: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) resident_project: Option<String>,
    /// A fresh run's schedule; absent on any other session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) job: Option<String>,
}

/// One heartbeat or schedule input, or one night rebuild, followed to the turn end or withdrawal
/// that answers it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Run {
    /// The session it ran in: the resident's home session, or a fresh run's thread.
    pub(crate) session: SessionId,
    /// The entry that opened it; 0 for a fresh run whose input never reached its thread.
    pub(crate) entry: EntryId,
    /// The first work entry of the turn that took it; none until it starts, or if it never ran.
    pub(crate) start: Option<EntryId>,
    /// The turn end or withdrawal that closed it.
    pub(crate) closed: Option<EntryId>,
    /// When `entry` was recorded: admission, not the start of work.
    pub(crate) at: Millis,
    /// `heartbeat`, the schedule's name, or `night`.
    pub(crate) what: String,
    /// The schedule's due time (for Run now, when it was asked for); `at` for the others.
    pub(crate) due: Millis,
    pub(crate) state: RunState,
    /// When `closed` was recorded.
    pub(crate) ended: Option<Millis>,
    /// The first line, at most 200 characters, of the last note or question its turn sent the
    /// person, else of its last reply; empty while open and for a run that never ran.
    pub(crate) line: String,
    /// A failed run's error sentence, from its turn end.
    pub(crate) error: Option<String>,
}

/// The record holds `Waiting`, `Running` from a run's start, and the closing states; a reader
/// shows an open run `Interrupted` while its session's flag is set, and a fresh run `Starting`
/// while its input is on its way to its thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunState {
    Starting,
    Waiting,
    Running,
    Interrupted,
    Ok,
    Failed,
    Stopped,
}

/// A delivered file is inside the named session's directory; a link is never fetched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Delivery {
    pub(crate) session: SessionId,
    pub(crate) entry: EntryId,
    pub(crate) at: Millis,
    pub(crate) line: String,
    pub(crate) file: Option<PathBuf>,
    pub(crate) url: Option<String>,
    /// Sender's recorded name.
    pub(crate) name: String,
    /// Sender's project at delivery time.
    pub(crate) project: String,
    pub(crate) archived: bool,
    /// Shared across this home's devices; a receipt never changes delivery order.
    pub(crate) read: bool,
    /// Owning server's absolute copy path, enriched when listed or streamed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) path: Option<PathBuf>,
    /// Spoken note's audio; word timings are in the adjacent .json file, served by files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) voice: Option<Box<super::Voice>>,
    /// Automatic playback destination, absent on ordinary deliveries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sound: Option<super::Sound>,
    /// The one terminal selected for automatic playback, absent on phone or silent notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) terminal: Option<String>,
}

impl Delivery {
    /// Every lowercased query word occurs in a searchable field; an empty query matches all.
    pub(crate) fn matches(&self, words: &[String]) -> bool {
        if words.is_empty() {
            return true;
        }
        let file = self
            .file
            .as_deref()
            .and_then(|p| p.file_name())
            .unwrap_or_default();
        let text = format!(
            "{}\n{}\n{}\n{}\n{}",
            self.line,
            self.name,
            self.project,
            file.to_string_lossy(),
            self.url.as_deref().unwrap_or_default()
        )
        .to_lowercase();
        words.iter().all(|word| text.contains(word))
    }
}

/// The enrolled devices and paired servers, with their keys (mode 0600), in the state directory.
pub(crate) const DEVICES: &str = "devices.json";

/// The key is present only in the answer that enrolled this device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Device {
    pub(crate) name: String,
    pub(crate) added: Millis,
    pub(crate) key: Option<String>,
    /// Set when the device is another Razor server: this server calls it at `http://<address>`
    /// with the same key, and an address names it as `@<name>`.
    pub(crate) address: Option<String>,
    /// The phone's notification token; stored in `devices.json` and never listed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) push: Option<Push>,
    /// At most 8, a newer token for the same session replacing the older; stored as `push`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) follows: Vec<Follow>,
    /// What its notifications bring when it set that; stored as `push`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) notifications: Option<Notifications>,
}

/// What a device's notifications bring. `questions`: questions and stuck top-level sessions;
/// `agents`, the default: those, the agents' notes and deliveries, and a top-level turn's finish
/// only to the device that sent its input, while it has no stream open on the session; `all`:
/// every finish too; `off`: nothing. A muted session sends nothing, and a muted top-level
/// session's tree sends nothing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Notifications {
    pub(crate) level: NotificationLevel,
    /// At most 256.
    pub(crate) muted: Vec<SessionId>,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NotificationLevel {
    Questions,
    #[default]
    Agents,
    All,
    Off,
}

/// A notification token from Apple's push service, for the environment the app was signed for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Push {
    pub(crate) token: String,
    pub(crate) environment: Environment,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Environment {
    Development,
    Production,
}

/// A Live Activity following one top-level session, with that activity's own token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Follow {
    pub(crate) session: SessionId,
    pub(crate) push: Push,
}

/// What a followed session's Live Activity shows; the app's `ContentState` decodes exactly this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Following {
    pub(crate) state: State,
    /// Its open questions.
    pub(crate) asks: usize,
    /// Its latest turn ended in an error.
    pub(crate) failed: bool,
    /// At most 200 characters.
    pub(crate) headline: String,
    /// Its children that are not done: each one's state and open questions, at most 32.
    pub(crate) children: Vec<(State, usize)>,
}

/// This home as a phone knows it. `id` is made at the home's first start and never changes;
/// `label` is `server.label`, else `server.name`, else the machine's name, at most 64 characters.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Server {
    pub(crate) id: String,
    pub(crate) label: String,
    /// Enrollment name for addressing this server; empty for the connected home.
    pub(crate) name: String,
    /// Machine in plain words, independent of the display label.
    pub(crate) machine: String,
    pub(crate) realm: String,
    /// Last contact when unavailable; None while reachable.
    pub(crate) silent: Option<Millis>,
    /// Its `transcribe` is set, so it takes voice notes.
    pub(crate) transcribe: bool,
    /// It has a key file or a `live.url`, so it takes live calls; otherwise left out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) call: bool,
    /// `meetrec capabilities` listed `ingest` at its start, so it takes meetings recorded on the
    /// phone; otherwise left out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) meetings: bool,
    /// This home keeps the person's shared settings.
    pub(crate) shared: bool,
    /// It takes `ClientInput::video`.
    pub(crate) video: bool,
    pub(crate) load: Option<Load>,
    /// Locally stored saved-views version; absent until a paired server reports it.
    pub(crate) views_version: Option<u64>,
    /// Its residents by name; left out when it has none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) residents: Vec<ResidentDay>,
}

/// A resident's day as the agents list draws it, the wakes ahead and the runs behind; its home
/// session's status row carries its state and asks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ResidentDay {
    /// Its name, which is its home session's id on its server.
    pub(crate) session: SessionId,
    /// What comes due in the next 24 hours and when, soonest first; empty while stopped.
    pub(crate) upcoming: Vec<(String, Millis)>,
    /// Its runs opened in the last 24 hours, newest first: what ran, when it ended (else when it
    /// opened), and its state, as `GET /v1/residents` shows each.
    pub(crate) runs: Vec<(String, Millis, RunState)>,
}

/// Measured one-minute load per core, and available/total memory in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct Load {
    pub(crate) cpu: f64,
    pub(crate) free: u64,
    pub(crate) total: u64,
}

/// Dropping the sign-in stream cancels the sign-in.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Login {
    Show { text: String },
    Done,
    Failed { error: String },
}

/// One row replaces the previous row; absent facts are never invented as zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentView {
    pub(crate) session: SessionId,
    pub(crate) parent: Option<SessionId>,
    pub(crate) launched: Option<EntryId>,
    pub(crate) name: String,
    pub(crate) model: String,
    pub(crate) effort: crate::protocol::Effort,
    #[serde(default)]
    pub(crate) speed: crate::protocol::Speed,
    pub(crate) state: State,
    #[serde(default)]
    pub(crate) closing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) close_reason: Option<super::CloseReason>,
    pub(crate) headline: String,
    pub(crate) headline_at: Option<Millis>,
    pub(crate) last_activity: Millis,
    /// Last person input from any device.
    #[serde(default)]
    pub(crate) person_input_at: Option<Millis>,
    /// When the model last showed reply text in this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) spoke_at: Option<Millis>,
    /// Bounded first sentence of the latest reply, reconstructed from the record.
    #[serde(default)]
    pub(crate) last_reply: String,
    /// When any reader of the session's stream last opened or closed it, or reported `seen`,
    /// since this server started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) looked_at: Option<Millis>,
    /// The last entry a reader reported seeing, since this server started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) seen: Option<EntryId>,
    pub(crate) context: Option<u64>,
    pub(crate) window: Option<u64>,
    pub(crate) threshold: f64,
    pub(crate) branch: Option<String>,
    pub(crate) changed_files: Vec<PathBuf>,
    /// Unresolved tool calls, running now.
    pub(crate) calls: Vec<EntryId>,
    /// Tool calls recorded over the whole session.
    pub(crate) calls_made: u32,
    /// Calls whose result was an error.
    pub(crate) failed: u32,
    pub(crate) asks: Vec<Ask>,
    pub(crate) report: Option<Report>,
    /// The newest turn's cache reads over its request input, 0 to 1; absent until a turn
    /// reports usage.
    pub(crate) cache: Option<f64>,
    /// This conversation's account readings, at most 32 windows; never another session's.
    pub(crate) usage: Vec<UsageWindow>,
    /// How the parent settled this finished child; absent until it has.
    pub(crate) settled: Option<Decision>,
    /// The last name of the folder the session started in (a worktree child's: its origin
    /// checkout's); an address's project matches it on a session that is no child.
    pub(crate) project: String,
    /// The repository the session's folder belongs to (`children::repo`); absent outside git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) repo: Option<String>,
    /// When the session's context was last rebuilt, from its record; absent before the first.
    pub(crate) rebuilt: Option<Millis>,
    /// The latest turn end's error sentence; absent once a later turn ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    /// An interruption ended the latest turn (its sentence is `error`); false once any input
    /// is admitted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) interrupted: bool,
    /// The resident a thread or fresh run belongs to, from its `Start`; absent on any other
    /// session. The session stays a root, so its notes still reach the person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) resident: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) resident_project: Option<String>,
    /// Terminal-app streams attached to this session, sent as `terminal`: whether any is.
    #[serde(
        rename = "terminal",
        serialize_with = "any",
        deserialize_with = "count"
    )]
    pub(crate) terminals: u32,
}

fn any<S: serde::Serializer>(n: &u32, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_bool(*n > 0)
}

fn count<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    bool::deserialize(d).map(u32::from)
}

/// One named account window, measured on this conversation's sign-in, not token usage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct UsageWindow {
    pub(crate) name: String,
    /// Percent used, 0 to 100; absent if only a status was reported.
    pub(crate) utilization: Option<f64>,
    pub(crate) resets_at: Option<Millis>,
    pub(crate) rejected: bool,
    /// Why extra usage did not cover a rejected window, in plain words, when the provider said.
    pub(crate) reason: Option<String>,
}

/// A pack's product name and colours, passed through to clients; nothing in the core reads it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Brand {
    /// Wherever a client names the product.
    pub(crate) name: String,
    /// Theme colour names to `#rrggbb`. A client keeps
    /// its own colour for each name left out and ignores what it cannot use.
    pub(crate) colours: BTreeMap<String, String>,
}
impl Default for Brand {
    fn default() -> Self {
        Self {
            name: "Razor".into(),
            colours: BTreeMap::new(),
        }
    }
}

/// How a person's clients draw, from the config's `[ui]`; passed through like `Brand`, and
/// nothing in the core reads it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ui {
    /// Whether clients draw motion; false draws each moving thing in the settled form its
    /// times lead to.
    pub(crate) motion: bool,
    /// The block glyphs for pictures: octants, or half blocks for fonts without them.
    pub(crate) blocks: Blocks,
    /// The workshop's stage at the status pane's foot, or `off` to give its rows to the pane.
    pub(crate) stage: Stage,
    /// Terminal action keys; absent names inherit their shipped defaults.
    pub(crate) keys: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}
impl Default for Ui {
    fn default() -> Self {
        Self {
            motion: true,
            blocks: Blocks::Octant,
            stage: Stage::Workshop,
            keys: BTreeMap::new(),
        }
    }
}
/// How the phone app presents this home, from the config's `[phone]`; passed through like
/// `Brand`, and nothing in the core reads it. Every field left out is the app's own choice.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Phone {
    /// The resident the app opens into, showing only it and its threads; absent, the full app.
    home: Option<SessionId>,
    /// The detail levels offered, in order, in the app's words; empty offers them all.
    details: Vec<String>,
    /// The level a newly paired phone starts at.
    detail: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stage {
    #[default]
    Workshop,
    Off,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Blocks {
    #[default]
    Octant,
    Half,
}

/// Live lines are superseded by recorded entries or by the next snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Live {
    Attached {
        session: SessionId,
        last: EntryId,
        /// Withdrawable inputs at `last`.
        pending: Vec<EntryId>,
    },
    /// Replaces the withdrawable set after unread inputs return to the queue.
    Pending {
        inputs: Vec<EntryId>,
    },
    Delta {
        text: String,
    },
    /// Words of the session's `n`th thought since it loaded, `start` on its first; each
    /// `Thinking` entry supersedes the oldest thought no entry took and no `void` line voided.
    Thought {
        n: u64,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        text: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        start: bool,
        /// Every thought through `n` that no entry took is void; the line has no words.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        void: bool,
    },
    Taken {
        input: EntryId,
    },
    Tail {
        job: JobId,
        /// The call entry that started the job.
        call: EntryId,
        text: String,
        at: Millis,
    },
    Agents {
        agents: Vec<AgentView>,
        config: Option<String>,
        /// The last good config's; sent again whenever it changes.
        brand: Brand,
        /// As `brand`.
        ui: Box<Ui>,
        /// As `brand`.
        #[serde(default)]
        phone: Phone,
        /// The home this stream is from; the API fills it, the controller leaves it empty.
        #[serde(default)]
        server: Box<Server>,
        /// Connected home and its directly paired servers; peer records remain native.
        #[serde(default)]
        servers: Vec<Server>,
    },
    /// Presence heartbeat, emitted only when requested by a peer reader.
    Server(Box<Server>),
    /// Opt-in resolved views; no stored definitions travel on the stream.
    Views(views::ViewsStatus),
    Agent(Box<AgentView>),
    Delivery(Box<Delivery>),
    /// A sentence about a plugin no record entry carries: a refused input, a failed observer.
    Plugin {
        text: String,
    },
}

/// Selects a writable settings file; the deployment pack is never a destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum McpScope {
    Global,
    Project,
    Private,
}

/// File changes are immediate; importing only previews additions and unresolved credentials.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum McpOperation {
    List,
    Add {
        name: String,
        scope: McpScope,
        config: super::Mcp,
    },
    Remove {
        name: String,
        scope: McpScope,
    },
    Enable {
        name: String,
        scope: McpScope,
    },
    Disable {
        name: String,
        scope: McpScope,
    },
    Reconnect {
        name: String,
    },
    SignIn {
        name: String,
    },
    SignOut {
        name: String,
    },
    Import {
        source: PathBuf,
        scope: McpScope,
    },
}

/// Reports references, never credential bytes; connection state belongs to the session.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct McpRow {
    pub(crate) name: String,
    pub(crate) file: Option<PathBuf>,
    /// The writable file `file` is, which enable, disable and remove name; none for a pack's.
    pub(crate) scope: Option<McpScope>,
    pub(crate) config: super::Mcp,
    pub(crate) state: String,
    /// The session's tools from this server after its allow list: each one's own name and its
    /// description's first line, cut to 120 characters. Empty without a session or connection.
    pub(crate) tools: Vec<(String, String)>,
    pub(crate) signed_in: bool,
    pub(crate) error: Option<String>,
}

/// A search of the person's past conversations: `GET /v1/search`'s query, and `recall`'s input,
/// whose `query` is `q`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Search {
    #[serde(default, alias = "query")]
    pub(crate) q: String,
    pub(crate) limit: Option<usize>,
    /// Only this resident's home session and threads.
    pub(crate) resident: Option<SessionId>,
    /// Every matching entry of this one conversation, not each conversation's newest.
    pub(crate) session: Option<SessionId>,
    /// This server's conversations only, not its paired servers'.
    #[serde(default)]
    pub(crate) local: bool,
}

/// A conversation's newest entry holding every word searched for, or with `session`, each one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Hit {
    /// `<id>@<server>` on a paired server.
    pub(crate) session: SessionId,
    pub(crate) name: String,
    pub(crate) cwd: PathBuf,
    pub(crate) resident: Option<SessionId>,
    pub(crate) resident_project: Option<String>,
    pub(crate) at: Millis,
    pub(crate) entry: EntryId,
    /// `person`, `agent`, or the name of the agent or child it came from.
    pub(crate) from: String,
    /// At most 200 characters of the entry's text, from shortly before the first word found.
    pub(crate) snippet: String,
}

/// A search's hits, newest first, and whether a paired server did not answer in 2 seconds.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Found {
    pub(crate) hits: Vec<Hit>,
    pub(crate) partial: bool,
}

/// A folder in a resident's `projects` folder, by the folder's name, which never changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ResidentProject {
    pub(crate) id: String,
    /// The folder's `.razor/name`, else its id.
    pub(crate) name: String,
    /// Its newest chat's last activity, else the folder's last change.
    pub(crate) at: Millis,
    pub(crate) chats: usize,
}
