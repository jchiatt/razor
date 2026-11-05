//! Voice-note settings and the actor families shared by clients and the server.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::LazyLock};

/// Actor families in catalogue order; adding a family changes selection once for every client.
pub(crate) static FAMILIES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    include_str!("../terminal/data/stage.txt")
        .lines()
        .filter_map(|s| {
            s.split_once(' ')?
                .0
                .strip_prefix("%actor/")?
                .strip_suffix("/0")
        })
        .collect()
});
/// Native root id selects one family everywhere; resident threads pass their resident's id.
pub(crate) fn family(native: &str) -> usize {
    native.bytes().fold(2166136261u32, |h, c| {
        (h ^ u32::from(c)).wrapping_mul(16777619)
    }) as usize
        % FAMILIES.len()
}
/// The person's permission to make voice notes; away permits an explicitly requested one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VoiceWhen {
    #[default]
    Always,
    Away,
    Never,
}
/// A delivery's automatic playback destination, chosen once before speech is made.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Sound {
    Terminal,
    Phone,
    Silent,
}
/// A synthesized note and its automatic playback destination, selected once by the owner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Spoken {
    pub(crate) voice: super::Voice,
    pub(crate) sound: Sound,
    pub(crate) terminal: Option<String>,
}
/// Owner-local quiet hours, inclusive from and exclusive to, in HH:MM, wrapping midnight.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Quiet {
    pub(crate) from: String,
    pub(crate) to: String,
}
/// One person's settings, owned by one home; usage is read-only to clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct VoiceSettings {
    pub(crate) when: VoiceWhen,
    pub(crate) voice: String,
    pub(crate) families: BTreeMap<String, String>,
    pub(crate) gain_db: f64,
    pub(crate) expressiveness: f64,
    pub(crate) memos: bool,
    pub(crate) quiet: Option<Quiet>,
    pub(crate) daily_characters: u32,
    pub(crate) used_today: u32,
    /// GPT-Live's voice for calls; empty is Marin.
    pub(crate) call_voice: String,
    pub(crate) call_daily_dollars: f64,
    pub(crate) call_used_today: f64,
}
impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            when: VoiceWhen::Always,
            voice: String::new(),
            families: BTreeMap::new(),
            gain_db: 9.5,
            expressiveness: 0.5,
            memos: false,
            quiet: None,
            daily_characters: 3000,
            used_today: 0,
            call_voice: String::new(),
            call_daily_dollars: 10.0,
            call_used_today: 0.0,
        }
    }
}
/// What a call fact is about: a live call, or a meeting recorded on the phone, whose `call` is
/// the phone's meeting id.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    #[default]
    Call,
    Meeting,
}
impl Source {
    pub(crate) fn is_call(&self) -> bool {
        *self == Source::Call
    }
}
/// One fact of a live call or a recorded meeting, in its order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum CallEvent {
    /// A meeting's start: the folder meetrec made and the phone recording it.
    Started {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        folder: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device: Option<String>,
    },
    /// One speaker's words until the other spoke, a hand-off or a pause; times are on the
    /// call's own timeline. A meeting's line is `Room`, with meetrec's speaker label and name.
    Line {
        who: Speaker,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        text: String,
        start_ms: u64,
        end_ms: u64,
    },
    /// The person named a meeting's live label; later lines carry the name.
    Named {
        label: String,
        name: String,
    },
    Ended {
        reason: CallReason,
    },
}
/// Who said a call's line; the voice's words are never the person's. `Room` is anyone in a
/// meeting, the person included, since which label is theirs is not known until named.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Speaker {
    Person,
    Voice,
    Room,
}
/// Why a call or meeting ended, as the record and the phone both name it; a meeting ends
/// `Ended` or `Canceled`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallReason {
    Ended,
    Idle,
    Cap,
    Key,
    Lost,
    Error,
    Canceled,
}
/// A hand-off's call, by the provider's session id, and the person's lines its words are.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct VoiceCallInput {
    pub(crate) id: String,
    pub(crate) lines: Vec<super::EntryId>,
}
