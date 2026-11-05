//! Children are ordinary sessions. Their lifecycle is facts in the parent's record, mail is
//! input to the recipient, and a tree shares one channel file (docs/design/children.md).

mod agent;
pub(crate) mod git;
mod message;
mod post;
mod remote;
mod search;
pub(crate) use remote::{
    child, child_row, continuation, deliver, descendants, records, recover, recover_loaded, root,
    update,
};
pub(crate) use search::search;
mod turn;
mod watch;
pub(crate) use message::{name, project, receive, repo, repo_at};
pub(crate) use post::Post;
pub(crate) use turn::Turn;

use crate::controller::{self, Controller};
use crate::protocol::{Body, EntryId, Input, Origin, ReportStatus, SessionId, Spoken, State};
use crate::record::{ChildSummary, Summary};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::{fmt, path::PathBuf, sync::OnceLock};
use tokio::sync::Mutex;

/// The most bytes in a first input, a continuation, a headline or an encoded channel post.
const TEXT: usize = 64 * 1024;

/// One controller's child, mail and channel actions. It keeps no session state: every fact
/// it acts on is read from the controller when it acts.
pub(crate) struct Children {
    home: PathBuf,
    /// Held from a start's name check to its record, so two parents cannot take one name.
    starts: Mutex<()>,
    /// Held from a watcher's name to its start, through its video's admission.
    watching: Mutex<()>,
    /// Post, when installed: each session's participant and watch.
    pub(crate) post: Post,
    /// The API server's voice note maker, set once as it starts serving.
    speak: OnceLock<Speak>,
}

/// Makes a root's note at its call's entry spoken, given its text and whether the person asked
/// for it: its voice and sound, or a refusal's sentence.
type Speak = Box<
    dyn Fn(SessionId, EntryId, String, bool) -> BoxFuture<'static, Result<Spoken, Error>>
        + Send
        + Sync,
>;

/// A sentence the model can act on, or a machine failure with what was being done.
#[derive(Debug)]
pub(crate) enum Error {
    Refused(String),
    Other {
        source: Box<dyn std::error::Error + Send + Sync>,
        context: &'static str,
    },
}

impl Children {
    /// Scans no records and opens no file; one serves the whole controller.
    pub(crate) fn open(home: PathBuf) -> Self {
        Self {
            post: Post::open(home.clone()),
            home,
            starts: Mutex::new(()),
            watching: Mutex::new(()),
            speak: OnceLock::new(),
        }
    }

    /// Makes the `message` action's voice notes through `speak`; a second call does nothing.
    pub(crate) fn speak(&self, speak: Speak) {
        let _ = self.speak.set(speak);
    }

    /// Runs `caller`'s recorded call `at`: `agent`, `message`, `recall` or `status`. Beside the output,
    /// a finished child's report waits for the caller's result, usage and close before admission.
    pub(crate) async fn run(
        &self,
        controller: &Controller,
        caller: &SessionId,
        at: EntryId,
        action: &str,
        input: Value,
    ) -> Result<(Value, Option<(SessionId, Input)>), Error> {
        let output = match action {
            "agent" => return self.agent(controller, caller, at, parse(input)?).await,
            "message" => self.message(controller, caller, at, parse(input)?).await?,
            "recall" => search::recall(controller, input).await?,
            // The controller sends children nothing but agent, message, recall and status.
            _ => status(self, controller, caller).await?,
        };
        Ok((output, None))
    }

    fn dir(&self, session: &SessionId) -> PathBuf {
        self.home.join("sessions").join(session.as_str())
    }
}

/// Every agent in the caller's tree but settled helpers; local working children have current
/// changed files.
async fn status(
    children: &Children,
    controller: &Controller,
    caller: &SessionId,
) -> Result<Value, Error> {
    let mut rows = controller.tree(caller).await?;
    for row in &mut rows {
        if row.state != State::Done
            && row.session.address().is_none()
            && let Some(files) = files(controller, &row.session).await?
        {
            row.changed_files = files;
        }
    }
    let rows = children.rows(controller, rows).await.into_iter();
    let agents: Vec<Value> = rows
        .map(|r| {
            json!({"session": r.session, "parent": r.parent, "name": r.name, "headline": r.headline,
                "state": r.state, "changed_files": r.changed_files, "branch": r.branch})
        })
        .collect();
    Ok(json!({ "agents": agents }))
}

/// Records and returns the files a child's branch changed since its base when git can list
/// them and they differ from the last list; otherwise the last list stands.
async fn files(c: &Controller, session: &SessionId) -> Result<Option<Vec<PathBuf>>, Error> {
    let files = changed(&c.summary(session).await?).await;
    if let Some(files) = files.clone() {
        c.append(session, Body::ChangedFiles { files }).await?;
    }
    Ok(files)
}

async fn changed(summary: &Summary) -> Option<Vec<PathBuf>> {
    let worktree = summary.child.as_ref()?.worktree.as_ref()?;
    let files = git::files(worktree, &branch(&summary.session)).await?;
    (files != summary.changed_files).then_some(files)
}

/// Prepares the same parent, bounded report and changed files for explicit and implicit finish,
/// without re-entering the session task that is preparing its own report.
pub(crate) async fn report(
    summary: &Summary,
    status: ReportStatus,
    text: String,
) -> Result<(SessionId, Input, Option<Vec<PathBuf>>), Error> {
    if text.len() > TEXT {
        return refuse("The report is over 64 KiB; shorten it.");
    }
    let Some(child) = &summary.child else {
        return refuse("Only a child finishes; answer the person in your reply.");
    };
    let no_changes = match &child.worktree {
        None => true,
        Some(w) => git::timed(&w.origin, 10, &["rev-parse", &branch(&summary.session)])
            .await
            .is_ok_and(|tip| tip == w.base),
    };
    let from = Origin::Report {
        child: summary.session.clone(),
        status,
    };
    Ok((
        child.parent.clone(),
        Input {
            child_task: summary.child_task,
            child_no_changes: no_changes,
            ..Input::new(from, text)
        },
        changed(summary).await,
    ))
}

/// Renames a top-level session, for the person's `PUT /v1/sessions/{id}/name` and the `agent`
/// tool's `name` action: one line of 1 to 64 characters. A child's name is its parent's to give.
pub(crate) async fn rename(
    controller: &Controller,
    session: &SessionId,
    name: &str,
) -> Result<(), Error> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 || name.contains(['\n', '\r']) {
        return refuse("A name is one line of 1 to 64 characters.");
    }
    if controller.summary(session).await?.child.is_some() {
        return refuse("A child's name is its parent's to give; only a top-level session renames.");
    }
    let name = name.into();
    controller.append(session, Body::Name { name }).await?;
    Ok(())
}

/// The caller's direct child, by name or session id.
fn find(parent: &Summary, name: &str) -> Result<ChildSummary, Error> {
    let named = |c: &&ChildSummary| c.child.name == name || c.session.as_str() == name;
    match parent.children.iter().find(named) {
        Some(child) => Ok(child.clone()),
        None => refuse(format!(
            "This session has no child `{name}`; status lists them."
        )),
    }
}

/// The sole branch name for a child's existing session, shared with its status row.
pub(crate) fn branch(session: &SessionId) -> String {
    format!("child/{session}")
}

fn refuse<T>(sentence: impl Into<String>) -> Result<T, Error> {
    Err(Error::Refused(sentence.into()))
}

fn parse<T: serde::de::DeserializeOwned>(input: Value) -> Result<T, Error> {
    serde_json::from_value(input).or_else(|e| refuse(format!("The input does not fit: {e}.")))
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

impl From<controller::Error> for Error {
    fn from(e: controller::Error) -> Self {
        match e {
            controller::Error::Invalid(sentence) => Error::Refused(sentence),
            e => other("asking the controller")(e),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Refused(sentence) => f.write_str(sentence),
            Error::Other { source, context } => write!(f, "Razor failed {context}: {source}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<crate::devices::Error> for Error {
    fn from(e: crate::devices::Error) -> Self {
        match e {
            crate::devices::Error::Refused(sentence) => Error::Refused(sentence),
            e => other("reaching a paired server")(e),
        }
    }
}
