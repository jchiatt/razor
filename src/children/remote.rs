//! Direct-child operations between paired homes; each owner alone opens its records and git.
use super::{Children, Error, TEXT, branch, git, other, parse, refuse};
use crate::controller::Controller;
use crate::protocol::{
    AgentView, Body, Child, ChildRequest, ChildUpdate, Decision, EntryId, Input, Origin, SessionId,
    State,
};
use crate::record::{ChildSummary, Summary};
use serde_json::{Value, json};
use std::path::PathBuf;

use tokio::fs;

impl Children {
    /// Runs only operations owned here; a received operation is never forwarded to another owner.
    pub(crate) async fn receive_child(
        &self,
        controller: &Controller,
        server: &str,
        request: ChildRequest,
    ) -> Result<Value, Error> {
        match request {
            ChildRequest::Update {
                parent,
                child,
                update,
            } => {
                native(&parent)?;
                native(&child)?;
                Ok(controller
                    .update_child(&parent, child.on(server), update)
                    .await?)
            }
            ChildRequest::Start {
                session,
                mut child,
                cwd,
                model,
                effort,
                speed,
                in_place,
                input,
            } => {
                native(&child.parent)?;
                child.parent = child.parent.on(server);
                let input = Input {
                    model: Some(model),
                    effort: Some(effort),
                    speed: Some(speed),
                    ..input
                };
                self.start_peer(controller, session, child, cwd, in_place, input)
                    .await
            }
            ChildRequest::Continue { session, mut input } => {
                let (_, child) = owned(controller, &session, server).await?;
                task_input(&mut input, &child.parent)?;
                let entry = controller.continue_child(&session, input).await?;
                Ok(json!({"entry":entry}))
            }
            ChildRequest::Close { session, retire } => {
                owned(controller, &session, server).await?;
                if retire {
                    self.retire(controller, &session).await?;
                } else {
                    controller.close(&session).await?;
                }
                Ok(json!({}))
            }
            ChildRequest::Settle { session, decision } => {
                let (_, child) = owned(controller, &session, server).await?;
                let text = self
                    .settle_git(controller, &session, &child, decision)
                    .await?;
                Ok(json!({"text":text}))
            }
            ChildRequest::Report { session } => {
                owned(controller, &session, server).await?;
                Ok(json!(outgoing(controller, &session).await?))
            }
        }
    }

    async fn start_peer(
        &self,
        controller: &Controller,
        session: SessionId,
        mut child: Child,
        cwd: PathBuf,
        in_place: bool,
        mut input: Input,
    ) -> Result<Value, Error> {
        native(&session)?;
        if child.worktree.is_some() {
            return refuse("A peer start has no worktree; the target selects its checkout.");
        }
        task_input(&mut input, &child.parent)?;
        let cwd = fs::canonicalize(cwd)
            .await
            .map_err(other("opening the remote child's folder"))?;
        let found = if in_place {
            None
        } else {
            git::plan(&cwd).await?
        };
        let (worktree, cwd) = match found {
            Some((worktree, inside)) => (
                Some(worktree),
                self.dir(&session).join("worktree").join(inside),
            ),
            None => (None, cwd),
        };
        child.worktree = worktree.clone();
        controller
            .create(
                cwd.clone(),
                input.model.clone(),
                (input.effort, input.speed),
                Some(session.clone()),
                Some(child),
                None,
            )
            .await?;
        if let Some(worktree) = &worktree {
            git::add(
                worktree,
                &self.dir(&session).join("worktree"),
                &branch(&session),
            )
            .await?;
        }
        controller.admit(&session, input).await?;
        Ok(json!({"session":session,"cwd":cwd,"branch":worktree.map(|_|branch(&session))}))
    }

    /// Retires every descendant using the same owner-limited walk as close and tree.
    pub(super) async fn retire(
        &self,
        controller: &Controller,
        session: &SessionId,
    ) -> Result<(), Error> {
        let (local, mut failure) = descendants(controller, session, true).await;
        for id in local {
            if let Err(e) = controller.retire(&id).await {
                failure = Some(e.into());
            }
        }
        failure.map_or(Ok(()), Err)
    }

    pub(super) async fn settle_git(
        &self,
        controller: &Controller,
        session: &SessionId,
        child: &Child,
        decision: Decision,
    ) -> Result<String, Error> {
        self.retire(controller, session).await?;
        let mut text = String::new();
        if let Some(worktree) = &child.worktree {
            let branch = branch(session);
            if decision == Decision::Merge {
                text = git::merge(&worktree.origin, &branch).await?;
            }
            let dir = self.dir(session);
            let delete = (decision != Decision::Discard).then_some(branch.as_str());
            let removed = git::remove(
                &worktree.origin,
                &dir.join("worktree"),
                &dir.join("uncommitted"),
                delete,
            )
            .await;
            text += &removed.unwrap_or_else(|e| e.to_string());
        }
        Ok(text)
    }

    /// Refreshes live fields for the status tool alone, without requesting peer git work.
    pub(super) async fn rows(
        &self,
        controller: &Controller,
        mut rows: Vec<AgentView>,
    ) -> Vec<AgentView> {
        for (device, _) in controller.devices().paired().await {
            let server = &device.name;
            if !rows
                .iter()
                .any(|r| r.session.address().is_some_and(|(_, name)| name == server))
            {
                continue;
            }
            for mut remote in controller
                .devices()
                .snapshot(&device)
                .await
                .unwrap_or_default()
            {
                remote.session = remote.session.on(server);
                if let Some(row) = rows.iter_mut().find(|r| r.session == remote.session) {
                    *row = remote;
                }
            }
        }
        controller.child_rows(&mut rows);
        rows
    }
}

/// Refuses operations unless this owner recorded the authenticated peer as the child's parent.
async fn owned(
    controller: &Controller,
    session: &SessionId,
    server: &str,
) -> Result<(Summary, Child), Error> {
    native(session)?;
    let summary = controller.summary(session).await?;
    let Some(child) = summary.child.clone() else {
        return refuse("This session is not a child.");
    };
    if child
        .parent
        .address()
        .is_none_or(|(_, name)| name != server)
    {
        return refuse("This peer is not the child's recorded parent.");
    }
    Ok((summary, child))
}

/// A received id must belong to this owner, without a forwarding suffix.
fn native(session: &SessionId) -> Result<(), Error> {
    if session.address().is_some() {
        return refuse("A child operation takes native ids and cannot be relayed.");
    }
    Ok(())
}
fn task_input(input: &mut Input, parent: &SessionId) -> Result<(), Error> {
    if input.child_task.is_none() || !input.images.is_empty() {
        return refuse("A remote task needs its parent's entry and no transferred images.");
    }
    if let Origin::Mail { id, .. } = &mut input.from {
        id.0 = parent.clone();
    } else {
        return refuse("A child task comes from its parent.");
    }
    Ok(())
}

/// Overlays the source parent's facts; all live readings still belong to the peer.
pub(crate) fn child_row(parent: &SessionId, fact: &ChildSummary, row: &mut AgentView) {
    row.changed_files.clear();
    row.parent = Some(parent.clone());
    row.name = fact.child.name.clone();
    row.launched = Some(fact.child.launched);
    row.report = fact.report.clone().filter(|_| !fact.stopped);
    row.settled = fact.settled;
    if !fact.working() {
        row.state = State::Done;
    }
}

/// Sends a child's recorded report to its parent once it and its descendants have closed; a
/// report a new task replaced is not sent, and the parent drops one it already has.
pub(crate) async fn deliver(
    controller: &Controller,
    session: &SessionId,
    parent: SessionId,
) -> Result<(), Error> {
    let Some(report) = outgoing(controller, session).await? else {
        return Ok(());
    };
    let (child, update) = (session.clone(), ChildUpdate::Report(Box::new(report)));
    match parent.address() {
        Some((parent, server)) => {
            let update = ChildRequest::Update {
                parent,
                child,
                update,
            };
            controller
                .devices()
                .child(server, &update)
                .await
                .map(drop)
                .map_err(Into::into)
        }
        None => Ok(controller
            .update_child(&parent, child, update)
            .await
            .map(drop)?),
    }
}

/// The report a child recorded for its current task, read once its descendants and then the
/// child itself have closed, with a descendant that would not close named in it; none while
/// it has not reported, and then nothing is closed.
async fn outgoing(controller: &Controller, session: &SessionId) -> Result<Option<Input>, Error> {
    let summary = controller.summary(session).await?;
    if summary.reported.is_none() {
        return Ok(None);
    }
    let mut failure = None;
    for fact in &summary.children {
        if let Err(e) = controller.close(&fact.session).await {
            failure = Some(e);
        }
    }
    // The owner's queue checks again before closing it: a new task forbids both.
    let mut report = controller.reported(session).await?;
    if let (Some(report), Some(e)) = (&mut report, failure) {
        let text = format!("{e}\n{}", report.text);
        report.text = text[..text.floor_char_boundary(TEXT)].to_owned();
    }
    Ok(report)
}

/// Pulls only outstanding reports; triggered by reload, reconnect or evidence a child has ended.
pub(crate) async fn recover(
    controller: &Controller,
    server: &str,
    only: Option<&[SessionId]>,
) -> Result<(), Error> {
    for (parent, fact) in controller.remote_children() {
        let Some((session, owner)) = fact.session.address() else {
            continue;
        };
        if owner != server
            || !fact.working()
            || only.is_some_and(|ids| !ids.contains(&fact.session))
        {
            continue;
        }
        let operation = ChildRequest::Report {
            session: session.clone(),
        };
        let value = controller.devices().child(server, &operation).await?;
        if !value.is_null() {
            let report: Input = parse(value)?;
            controller
                .update_child(
                    &parent,
                    fact.session.clone(),
                    ChildUpdate::Report(Box::new(report)),
                )
                .await
                .map_err(other("recovering the remote report"))?;
        }
    }
    Ok(())
}

/// A reloaded parent recovers its outstanding peer reports without waiting for peer polling,
/// and the reports its local children recorded that it never admitted, as after a restart.
pub(crate) fn recover_loaded(controller: Controller, summary: &Summary) {
    let parent = summary.session.clone();
    let children = summary.children.iter().filter(|c| c.working());
    let working: Vec<_> = children.map(|c| c.session.clone()).collect();
    if working.is_empty() {
        return;
    }
    tokio::spawn(async move {
        let servers = working
            .iter()
            .filter_map(|c| c.address().map(|(_, server)| server));
        for server in servers.collect::<std::collections::BTreeSet<_>>() {
            if let Err(e) = recover(&controller, server, None).await {
                eprintln!("razor: recovering children on {server}: {e}");
            }
        }
        for child in working.iter().filter(|c| c.address().is_none()) {
            if let Err(e) = deliver(&controller, child, parent.clone()).await {
                eprintln!("razor: recovering child {child}'s report: {e}");
            }
        }
    });
}

/// Validates a parent update against the fold while its queue is held; stale reports vanish.
pub(crate) fn update(
    summary: &Summary,
    child: &SessionId,
    update: ChildUpdate,
) -> Result<Option<Body>, Error> {
    let fact = summary
        .children
        .iter()
        .find(|c| c.session == *child)
        .ok_or_else(|| Error::Refused("This peer has no such direct child.".into()))?;
    Ok(Some(match update {
        // The child's owner found it closed, so it takes a new task whatever this fact says.
        ChildUpdate::Continued => {
            if fact.settled.is_some() {
                return refuse("A settled child cannot be continued.");
            }
            Body::Child {
                session: child.clone(),
                event: crate::protocol::ChildEvent::Continued,
            }
        }
        ChildUpdate::Report(input) => {
            let native = child
                .address()
                .map_or_else(|| child.clone(), |(native, _)| native);
            let status = match input.from {
                Origin::Report { child: id, status } if id == native => status,
                _ => return refuse("A child update must contain its own report."),
            };
            // A report of another task is stale; a local one recorded before tasks were tagged
            // carries none.
            if !fact.working() || input.child_task.is_some_and(|task| task != fact.task) {
                return Ok(None);
            }
            Body::Input(Input {
                child_no_changes: input.child_no_changes,
                ..Input::new(
                    Origin::Report {
                        child: child.clone(),
                        status,
                    },
                    input.text,
                )
            })
        }
    }))
}

/// The entry of the `Continued` a closed child's parent records for its new task, on the
/// parent's own server, refused for a settled child.
pub(crate) async fn continuation(
    controller: &Controller,
    summary: &Summary,
) -> Result<EntryId, Error> {
    let Some(child) = &summary.child else {
        return refuse("Only a child can be continued.");
    };
    let (session, update) = (summary.session.clone(), ChildUpdate::Continued);
    let entry = match child.parent.address() {
        Some((parent, server)) => {
            let update = ChildRequest::Update {
                parent,
                child: session,
                update,
            };
            controller.devices().child(server, &update).await?
        }
        None => {
            controller
                .update_child(&child.parent, session, update)
                .await?
        }
    };
    parse(entry["entry"].clone())
}

/// The owner-local folds and foreign child facts reached by one walk; errors retain siblings.
pub(crate) struct Records {
    pub(crate) local: Vec<Summary>,
    pub(crate) remote: Vec<(SessionId, ChildSummary)>,
    pub(crate) failure: Option<Error>,
}

/// Reads the fold of `session` and of each local descendant not settled, once each; a settled
/// child's own subtree was retired at settle and is never opened. Foreign children remain
/// leaves with their local parent.
pub(crate) async fn records(controller: &Controller, session: &SessionId) -> Records {
    let mut queue = vec![session.clone()];
    let mut records = Records {
        local: Vec::new(),
        remote: Vec::new(),
        failure: None,
    };
    while let Some(id) = queue.pop() {
        match controller.summary(&id).await {
            Ok(summary) => {
                for child in summary.children.iter().filter(|c| c.settled.is_none()) {
                    if child.session.address().is_some() {
                        records.remote.push((id.clone(), child.clone()));
                    } else {
                        queue.push(child.session.clone());
                    }
                }
                records.local.push(summary);
            }
            Err(e) => records.failure = Some(e.into()),
        }
    }
    records
}

/// Every name and session id taken in the tree holding `session`: those of the root, each helper
/// not settled and every child row they recorded, settled ones included. Names used only under a
/// settled helper are free again, since nothing can address them.
pub(super) async fn names(c: &Controller, session: &SessionId) -> Result<Vec<String>, Error> {
    let records = records(c, &root(c, session).await?).await;
    records.failure.map_or(Ok(()), Err)?;
    let rows = records.local.iter().flat_map(|s| &s.children);
    let rows = rows.map(|c| (&c.child.name, &c.session));
    let all = records
        .local
        .iter()
        .map(|s| (&s.name, &s.session))
        .chain(rows);
    Ok(all
        .flat_map(|(name, id)| [name.clone(), id.to_string()])
        .collect())
}

/// Stops or retires every foreign leaf; errors never skip local sessions or sibling leaves.
pub(crate) async fn descendants(
    controller: &Controller,
    session: &SessionId,
    retire: bool,
) -> (Vec<SessionId>, Option<Error>) {
    // A direct stop addresses the foreign child itself, without opening any local record.
    if let Some((native, server)) = session.address() {
        let failure = controller
            .devices()
            .child(
                server,
                &ChildRequest::Close {
                    session: native,
                    retire,
                },
            )
            .await
            .err()
            .map(Into::into);
        return (Vec::new(), failure);
    }
    let records = records(controller, session).await;
    let mut failure = records.failure;
    for (parent, fact) in records.remote {
        if !retire && !fact.working() {
            continue;
        }
        let Some((session, server)) = fact.session.address() else {
            continue;
        };
        let result = async {
            controller
                .devices()
                .child(server, &ChildRequest::Close { session, retire })
                .await?;
            if fact.working() {
                controller
                    .append(
                        &parent,
                        Body::Child {
                            session: fact.session.clone(),
                            event: crate::protocol::ChildEvent::Stopped,
                        },
                    )
                    .await?;
            }
            Ok::<(), Error>(())
        }
        .await;
        if let Err(e) = result {
            failure = Some(e);
        }
    }
    (
        records.local.into_iter().map(|s| s.session).collect(),
        failure,
    )
}

/// A foreign leaf contains only its parent's recorded child facts, with no invented live fields.
pub(crate) fn child(parent: &SessionId, fact: &ChildSummary) -> AgentView {
    let mut row = AgentView {
        session: fact.session.clone(),
        parent: None,
        launched: None,
        name: String::new(),
        model: String::new(),
        effort: crate::protocol::Effort::Medium,
        speed: crate::protocol::Speed::Standard,
        state: State::NeedsYou,
        closing: false,
        close_reason: None,
        headline: String::new(),
        last_reply: String::new(),
        headline_at: None,
        person_input_at: None,
        spoke_at: None,
        looked_at: None,
        seen: None,
        last_activity: 0,
        context: None,
        window: None,
        threshold: 0.0,
        branch: None,
        changed_files: Vec::new(),
        calls: Vec::new(),
        calls_made: 0,
        failed: 0,
        asks: Vec::new(),
        report: None,
        cache: None,
        usage: Vec::new(),
        settled: None,
        project: String::new(),
        repo: None,
        rebuilt: None,
        error: None,
        interrupted: false,
        resident: None,
        resident_project: None,
        terminals: 0,
    };
    child_row(parent, fact, &mut row);
    row
}

/// The root on this owner, ending before a foreign parent.
pub(crate) async fn root(controller: &Controller, session: &SessionId) -> Result<SessionId, Error> {
    let mut root = session.clone();
    while let Some(child) = controller.summary(&root).await?.child {
        if child.parent.address().is_some() {
            break;
        }
        root = child.parent;
    }
    Ok(root)
}
