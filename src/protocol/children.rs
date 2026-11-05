//! The child facts shared by records and the children service.

use super::{EntryId, Input, Millis, SessionId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A child's independent checkout and the commit it was cut from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Worktree {
    pub(crate) origin: PathBuf,
    pub(crate) base: String,
}

/// Stored on Start for a child; model, effort and cwd stay on Start itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Child {
    pub(crate) parent: SessionId,
    pub(crate) name: String,
    pub(crate) worktree: Option<Worktree>,
    pub(crate) launched: EntryId,
}

/// Sender session and recorded call; mentions preserve the original post's id.
pub(crate) type MailId = (SessionId, EntryId);

/// Reports all finish the child, without inventing additional agent states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReportStatus {
    Finished,
    Partial,
    Blocked,
}

/// The report's entry is in its parent's record; the API shows the text's first line as a tip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Report {
    pub(crate) status: ReportStatus,
    /// True without a branch or when its tip matched the base while preparing this report.
    #[serde(default)]
    pub(crate) no_changes: bool,
    pub(crate) text: String,
    pub(crate) entry: EntryId,
}

/// Only the parent settles; `Merged` trusts its word and `Discard` keeps the branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Merge,
    Discard,
    Merged,
}

/// Lifecycle facts in the parent's record; the report itself is an ordinary input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildEvent {
    Started(Child),
    Continued,
    Stopped,
    Settled(Decision),
}

/// An answer references its original question, even across sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailKind {
    Note,
    Question,
    Answer {
        question: MailId,
    },
    /// A channel post that names its reader; the channel file holds the post.
    Mention,
}

/// Mail to an agent outside the sender's tree: what `message` hands `receive`, and the body of
/// `POST /v1/mail` when the agent is on a paired server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Mail {
    /// The recipient as the sender gave it, without `@server`: a session id, a name, a project.
    pub(crate) to: String,
    /// The sender's session and `message` call; an answer names it as its question.
    pub(crate) id: MailId,
    pub(crate) kind: MailKind,
    /// The sender's name and project; `receive` makes the origin's `from` of them.
    pub(crate) from: String,
    pub(crate) project: String,
    pub(crate) text: String,
}

/// One recorded channel line; readers also return an ending byte cursor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ChannelEntry {
    pub(crate) id: MailId,
    pub(crate) at: Millis,
    pub(crate) text: String,
}

/// A person-facing question, identified by the asking session's recorded Call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Ask {
    pub(crate) id: (SessionId, EntryId),
    pub(crate) text: String,
    pub(crate) options: Vec<String>,
    /// When it was asked; 0 from a server before it said.
    #[serde(default)]
    pub(crate) at: Millis,
}

/// One peer operation; ids are native and the authenticated enrollment supplies the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum ChildRequest {
    Start {
        session: SessionId,
        child: Child,
        cwd: PathBuf,
        model: String,
        effort: crate::protocol::Effort,
        #[serde(default)]
        speed: crate::protocol::Speed,
        in_place: bool,
        input: Input,
    },
    Continue {
        session: SessionId,
        input: Input,
    },
    Close {
        session: SessionId,
        retire: bool,
    },
    Settle {
        session: SessionId,
        decision: Decision,
    },
    Update {
        parent: SessionId,
        child: SessionId,
        update: ChildUpdate,
    },
    Report {
        session: SessionId,
    },
}

/// The only updates a remote child may make to its parent's facts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChildUpdate {
    Continued,
    Report(Box<Input>),
}
