//! The messages both model paths exchange with the session controller.

use super::{Body, CallId, EntryId, Image, Native, SessionId, Usage, UsageWindow};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ToBridge {
    /// Replaces the native conversation; a resume keeps history through its named message.
    Open {
        session: SessionId,
        open: u64,
        cwd: PathBuf,
        model: String,
        effort: crate::protocol::Effort,
        speed: crate::protocol::Speed,
        resume: Option<Resume>,
        /// The system prompt, read from the startup list.
        prompt: String,
        tools: Vec<serde_json::Value>,
        /// The live declarations, offered after the opening's fixed prefix.
        additions: Vec<serde_json::Value>,
    },
    /// Appends changed declarations without rewriting the native conversation's prefix.
    Add {
        session: SessionId,
        tools: Vec<serde_json::Value>,
    },
    /// Applies model, effort and speed before the next request, without a rebuild.
    Effort {
        session: SessionId,
        model: String,
        effort: crate::protocol::Effort,
        speed: crate::protocol::Speed,
        /// A changed effort's fact: a busy Claude turn yields with one notice at its boundary.
        notice: Option<EntryId>,
    },
    /// One admitted batch; each input's image/text blocks keep their order in one user message.
    Input {
        session: SessionId,
        inputs: Vec<BridgeInput>,
    },
    /// The Call is durable; its action may run, on `input` when a plugin changed it. A call a
    /// plugin refused gets its `Result` instead.
    Admitted {
        session: SessionId,
        call: CallId,
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<serde_json::Value>,
    },
    /// The model path mirrors this answer into exactly one Result Entry.
    Result {
        session: SessionId,
        call: CallId,
        output: String,
        error: bool,
        /// Settle this batch before any further request (handover or finish).
        settle: bool,
        /// On the Claude path, images the cell's nested results named, shown after its output.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<super::Image>,
    },
    /// Escape: the turn stops and the path ends it stopped itself; the conversation goes on.
    Interrupt { session: SessionId },
    /// Ends the conversation; the controller drops its subsequent lines.
    Close { session: SessionId },
}

/// The model's rendered input, already durable; image paths are absolute here, not in the record.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct BridgeInput {
    pub(crate) entry: EntryId,
    pub(crate) text: String,
    pub(crate) images: Vec<Image>,
    /// The input's video, absolute, for a model path that takes video; the others ignore it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) video: Option<PathBuf>,
    /// The video stays in every later request of this native conversation (a watcher's own);
    /// otherwise it leaves the requests when its turn ends.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) keep: bool,
}

/// The native history a clean reopening continues.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Resume {
    Claude {
        conversation: String,
        at: String,
    },
    /// The razor_prefix descriptor first, then the exact wire conversation.
    Responses {
        items: Vec<serde_json::Value>,
    },
}

/// The open number lets the controller discard lines from conversations it already closed.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Line {
    pub(crate) session: SessionId,
    pub(crate) open: u64,
    #[serde(flatten)]
    pub(crate) says: FromBridge,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // Entry is the common message; a box would allocate each.
pub(crate) enum FromBridge {
    /// A native message mirrored to the canonical record.
    Entry { body: Body, native: Option<Native> },
    /// Display-only text, never recorded.
    Delta { text: String },
    /// Display-only words of the conversation's `n`th thought, never recorded; `start` begins it.
    /// The path numbers a thought before its lines can be dropped, so a later line still says
    /// which thought it belongs to.
    Thought { n: u64, text: String, start: bool },
    /// The conversation's thoughts through the `n`th that no entry took are void: their response
    /// was cut, and the path is trying it again.
    Void { n: u64 },
    /// Run only after the corresponding Call Entry was admitted.
    Run {
        call: CallId,
        tool: String,
        input: serde_json::Value,
    },
    /// One per model message: its request's input plus cache reads and writes.
    Context {
        tokens: u64,
        /// The window, where the route can say before the turn ends (Claude's SDK can).
        window: Option<u64>,
        /// The latest input that request carried: the model has read it and every input the
        /// path received before it.
        read: Option<EntryId>,
    },
    /// The turn's result, usage and model window, and the latest input it took, which the path
    /// will not hand the model again; a path takes inputs in the order it received them.
    TurnEnd {
        success: bool,
        reason: Option<String>,
        message: String,
        usage: Usage,
        window: u64,
        took: Option<EntryId>,
    },
    /// This conversation's latest account readings, at most 32 named windows.
    Usage { windows: Vec<UsageWindow> },
    /// The marked batch settled and the native conversation closed, retaining its turn's usage.
    Settled { usage: Usage },
    /// A conversation ended by itself, with a reason on failure.
    Closed { error: Option<String> },
}
