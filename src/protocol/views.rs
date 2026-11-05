//! The person's saved views of the agents list, kept by one server and shared by every client
//! (docs/design/agents-view.md). Session ids here are native, never `@name`.

use super::SessionId;
use serde::{Deserialize, Serialize};

/// The saved views as the server that keeps them stored them. `version` counts the accepted
/// writes; a write made from another version is refused, so no device overwrites another's
/// change unseen. Clients open to the first view and switch on their own.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Views {
    pub(crate) version: u64,
    #[serde(default)]
    pub(crate) views: Vec<View>,
    /// Sessions shown above every group in every view, in this order.
    #[serde(default)]
    pub(crate) pinned: Vec<SessionId>,
}

/// A named arrangement: its groups, then everything no group matches. No groups: a flat view.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct View {
    /// 1 to 64 characters, unique among the views.
    pub(crate) name: String,
    /// Opt-in resident for new unmatched roots in Auto; absent leaves automatic sorting off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) wake: Option<SessionId>,
    #[serde(default)]
    pub(crate) groups: Vec<Group>,
    #[serde(default)]
    pub(crate) everything_else: Arrangement,
    /// `Server.id`s and state words (`needs_you`, `working`, `done`, `stuck`) left out.
    #[serde(default)]
    pub(crate) hidden: Vec<String>,
}

/// How the sessions no group matches are grouped and ordered.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Arrangement {
    #[serde(default)]
    pub(crate) by: By,
    #[serde(default)]
    pub(crate) sort: Sort,
    /// The person's order when `sort` is `mine`.
    #[serde(default)]
    pub(crate) order: Vec<SessionId>,
}

/// A named, coloured section of a view, holding the top-level sessions its matches place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Group {
    /// Assigned on save when omitted; stable across renames.
    #[serde(default)]
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) colour: Colour,
    /// The phone's symbol name; the terminal draws none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    symbol: Option<String>,
    /// One sentence explaining this group, at most 200 bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) why: Option<String>,
    /// `claims` in views saved before the rename to `matches`.
    #[serde(default, alias = "claims")]
    pub(crate) matches: Vec<Match>,
    #[serde(default)]
    pub(crate) sort: Sort,
    #[serde(default)]
    pub(crate) order: Vec<SessionId>,
    /// At most three levels total, including this group's ancestors.
    #[serde(default)]
    pub(crate) groups: Vec<Group>,
}

/// What places a top-level session in a group; the most specific match wins: a session's,
/// then its folder's, then its repository's, then the first in the set, depth first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Match {
    /// The row's `repo`.
    Repo(String),
    /// The row's `project`: its start folder's last name (a worktree child's origin checkout's).
    Folder(String),
    /// One session.
    Session(SessionId),
}

/// attention: asking or interrupted, stuck, working, idle, done, then by creation; recent: last
/// activity first; created: oldest first; name: A to Z ignoring case; mine: `order`, then the
/// rest by creation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Sort {
    #[default]
    Attention,
    Recent,
    Created,
    Name,
    Mine,
}

/// What `everything_else` is grouped by; machine and realm are the row's server's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum By {
    #[default]
    None,
    Repository,
    Project,
    Machine,
    Realm,
    State,
}

/// A group's colour by name; each client draws its own muted value, apart from the state
/// colours and the agent's violet.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Colour {
    Slate,
    Steel,
    Indigo,
    Plum,
    Rose,
    Clay,
    Sand,
    Moss,
}

/// Complete view-name order; layouts replace only the named views. An error clears all.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewsStatus {
    pub(crate) version: u64,
    pub(crate) names: Vec<String>,
    pub(crate) layouts: Vec<Layout>,
    pub(crate) error: Option<String>,
}
/// One resolved view in display order, independent of when agent rows arrive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Layout {
    pub(crate) name: String,
    pub(crate) sections: Vec<Section>,
}
/// A stable heading and the roots and descendants underneath it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Section {
    pub(crate) id: String,
    pub(crate) kind: SectionKind,
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) why: Option<String>,
    pub(crate) colour: Option<Colour>,
    pub(crate) depth: u16,
    pub(crate) sessions: Vec<ViewRow>,
}
/// Identifies built-ins even when a person's group has the same name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SectionKind {
    Residents,
    NeedsYou,
    Pinned,
    #[default]
    Group,
    EverythingElse,
    Earlier,
}
/// Stream-qualified session id and depth along its parent links. Settled children are absent;
/// `folded` marks a finished child past its parent's newest three, listed after its siblings,
/// which clients draw as one line that opens them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ViewRow {
    pub(crate) session: SessionId,
    pub(crate) depth: u16,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) folded: bool,
}
