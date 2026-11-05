use super::{Children, Error, TEXT, branch, files, find, git, other, refuse, remote};
use crate::config;
use crate::controller::Controller;
use crate::protocol::Speed;
use crate::protocol::{
    Body, Child, ChildEvent, ChildRequest, ChildUpdate, Decision, Effort, EntryId, Input, MailKind,
    Origin, ReportStatus, SessionId, Video,
};
use crate::record::Summary;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{fs, io::AsyncReadExt};

/// A child's first words, before its shared brief and task: who it is and how its tree works.
const CONTRACT: &str = include_str!("text/child.md");
/// Razor's role texts, each in place of `{role}` unless `[children]` names a file for it.
const REVIEWER: &str = include_str!("text/reviewer.md");
const RESEARCHER: &str = include_str!("text/researcher.md");

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(super) enum Agent {
    Start(Start),
    Continue {
        child: String,
        brief: String,
        model: Option<String>,
        effort: Option<Effort>,
        speed: Option<Speed>,
    },
    Finish {
        status: ReportStatus,
        report: String,
    },
    Settle {
        child: String,
        decision: Decision,
    },
    Stop {
        child: String,
    },
    Wait {
        timeout_ms: Option<u64>,
    },
    #[serde(alias = "progress")]
    Headline {
        text: String,
    },
    Name {
        name: String,
    },
}

#[derive(Deserialize)]
pub(super) struct Start {
    pub(super) name: String,
    pub(super) server: Option<String>,
    pub(super) brief: String,
    pub(super) shared_brief: Option<PathBuf>,
    /// A task of the parent's project to claim for the child.
    pub(super) task: Option<u32>,
    #[serde(default)]
    pub(super) kind: Kind,
    pub(super) cwd: Option<PathBuf>,
    pub(super) model: Option<String>,
    pub(super) effort: Option<Effort>,
    pub(super) speed: Option<Speed>,
    #[serde(default)]
    pub(super) placement: Placement,
    /// A watcher's video, set by Razor and never by a model: its brief is its whole first input.
    #[serde(skip)]
    pub(super) video: Option<Box<Video>>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Placement {
    #[default]
    Worktree,
    InPlace,
}

/// What a child is told after its contract; nothing else about a child depends on it.
#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    #[default]
    Worker,
    Reviewer,
    Researcher,
}

impl Start {
    /// The child's first input: its contract, its role's text (the file `[children]` names, else
    /// Razor's; a worker has none), the shared brief, then its own brief; at most 64 KiB.
    async fn text(&self, parent: &Summary, roles: &config::Children) -> Result<String, Error> {
        let (file, role) = match self.kind {
            Kind::Worker => (None, ""),
            Kind::Reviewer => (roles.reviewer.as_deref(), REVIEWER),
            Kind::Researcher => (roles.researcher.as_deref(), RESEARCHER),
        };
        let role = match file {
            Some(file) => read(file).await?,
            None => role.into(),
        };
        let shared = match &self.shared_brief {
            Some(path) => read(&parent.cwd.join(path)).await?,
            None => String::new(),
        };
        let role = match role.trim_end() {
            "" => String::new(),
            role => format!("{role}\n\n"),
        };
        let text = CONTRACT
            .replace("{name}", &self.name)
            .replace("{parent}", parent.session.as_str())
            .replace("{role}", &role)
            + &shared
            + "\n"
            + &self.brief;
        if text.len() > TEXT {
            return refuse("The role text and briefs are over 64 KiB together; shorten them.");
        }
        Ok(text)
    }
}

/// A file's text, read no further than one byte past what a first input holds.
async fn read(path: &Path) -> Result<String, Error> {
    let mut text = String::new();
    let file = fs::File::open(path).await;
    match async { file?.take(TEXT as u64 + 1).read_to_string(&mut text).await }.await {
        Ok(_) => Ok(text),
        Err(e) => refuse(format!("{} cannot be read: {e}.", path.display())),
    }
}

impl Children {
    pub(super) async fn agent(
        &self,
        controller: &Controller,
        caller: &SessionId,
        at: EntryId,
        action: Agent,
    ) -> Result<(Value, Option<(SessionId, Input)>), Error> {
        let output = match action {
            Agent::Start(start) => {
                let (task, name) = (start.task, start.name.clone());
                let mut started = self.start(controller, caller, at, start).await?;
                let child = started["session"].as_str().unwrap_or_default().to_owned();
                if let (Some(id), Ok(child)) = (task, SessionId::try_from(child)) {
                    let parent = controller.summary(caller).await?;
                    let memory = controller.memory();
                    if let Err(e) = memory.start(controller, &parent, (&child, &name), id).await {
                        started["task"] = json!(e.to_string());
                    }
                }
                Ok(started)
            }
            Agent::Continue {
                child,
                brief,
                model,
                effort,
                speed,
            } => {
                let parent = controller.summary(caller).await?;
                let child = find(&parent, &child)?;
                // A child the parent counts as working adds none to the cap.
                if !child.working() {
                    cap(&controller.settings(caller).await?.config.children, &parent)?;
                }
                if brief.len() > TEXT {
                    return refuse("The brief is over 64 KiB; shorten it.");
                }
                let (id, kind, from) = ((caller.clone(), at), MailKind::Note, String::new());
                let next = Input {
                    model: model.map(crate::profiles::id),
                    effort,
                    speed,
                    ..Input::new(Origin::Mail { id, kind, from }, brief)
                };
                if let Some((session, server)) = child.session.address() {
                    // The owner decides from the child's own state; a child this fact counts as
                    // working keeps its task, even one a failed continue never sent it.
                    let task = match child.working() {
                        true => child.task,
                        false => super::parse(
                            controller
                                .update_child(caller, child.session.clone(), ChildUpdate::Continued)
                                .await?["entry"]
                                .clone(),
                        )?,
                    };
                    let input = Input {
                        child_task: Some(task),
                        ..next
                    };
                    controller
                        .devices()
                        .child(server, &ChildRequest::Continue { session, input })
                        .await?;
                } else {
                    controller.continue_child(&child.session, next).await?;
                }
                Ok(json!({}))
            }
            Agent::Finish { status, report } => {
                let summary = controller.summary(caller).await?;
                let (parent, report, files) = super::report(&summary, status, report).await?;
                if let Some(files) = files {
                    controller
                        .append(caller, Body::ChangedFiles { files })
                        .await?;
                }
                return Ok((json!({}), Some((parent, report))));
            }
            Agent::Settle { child, decision } => {
                self.settle(controller, caller, &child, decision).await
            }
            Agent::Stop { child } => {
                let child = find(&controller.summary(caller).await?, &child)?;
                controller.close(&child.session).await?;
                if child.session.address().is_none() {
                    files(controller, &child.session).await?;
                }
                let (session, event) = (child.session, ChildEvent::Stopped);
                controller
                    .append(caller, Body::Child { session, event })
                    .await?;
                Ok(json!({}))
            }
            Agent::Wait { timeout_ms } => {
                let timeout = Duration::from_millis(timeout_ms.unwrap_or(600_000));
                let Some(entry) = controller.wait_input(caller, at, timeout).await? else {
                    return Ok((Value::Null, None));
                };
                let entry = controller.get(caller, entry).await?;
                serde_json::from_str(&entry).map_err(other("reading the input it waited for"))
            }
            Agent::Headline { text } => {
                if text.len() > TEXT || text.contains(['\n', '\r']) {
                    return refuse("A headline is one line under 64 KiB.");
                }
                controller.append(caller, Body::Headline { text }).await?;
                Ok(json!({}))
            }
            Agent::Name { name } => super::rename(controller, caller, &name)
                .await
                .map(|()| json!({})),
        }?;
        Ok((output, None))
    }

    pub(super) async fn start(
        &self,
        controller: &Controller,
        caller: &SessionId,
        at: EntryId,
        start: Start,
    ) -> Result<Value, Error> {
        let name = start.name.as_str();
        let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
        if name.is_empty() || name.len() > 64 || !name.bytes().all(word) {
            return refuse("A child's name is 1 to 64 letters, digits, underscores or hyphens.");
        }
        let parent = controller.summary(caller).await?;
        let children = controller.settings(caller).await?.config.children;
        cap(&children, &parent)?;
        let text = match start.video {
            Some(_) => start.brief.clone(),
            None => start.text(&parent, &children).await?,
        };
        let cwd = parent.cwd.join(start.cwd.clone().unwrap_or_default());
        if let Some(server) = &start.server {
            let root = controller
                .summary(&super::remote::root(controller, caller).await?)
                .await?;
            if root
                .child
                .as_ref()
                .is_some_and(|c| c.parent.address().is_some())
            {
                return refuse(
                    "A remote child's subtree starts children on its own server; omit server.",
                );
            }
            // Validate enrollment before recording an outstanding task.
            if controller.devices().peer(server).await.is_none() {
                return refuse(format!(
                    "{server} is not paired; pair it before starting a child."
                ));
            }
        }
        let cwd = if start.server.is_some() {
            cwd
        } else {
            fs::canonicalize(cwd)
                .await
                .map_err(other("opening the child's folder"))?
        };
        let found = match (&start.server, &start.placement) {
            (None, Placement::Worktree) => git::plan(&cwd).await?,
            _ => None,
        };
        let session = SessionId::new();
        let dir = self.dir(&session);
        let (worktree, cwd) = match found {
            Some((worktree, inside)) => (Some(worktree), dir.join("worktree").join(inside)),
            None => (None, cwd),
        };
        let held = self.starts.lock().await;
        let taken = remote::names(controller, caller).await?;
        if taken.iter().any(|n| n == name) {
            return refuse(format!(
                "`{name}` is taken in this tree; choose another name."
            ));
        }
        let child = Child {
            parent: caller.clone(),
            name: start.name,
            worktree: worktree.clone(),
            launched: at,
        };
        // Unnamed, they are the parent's own, never the config's defaults.
        let model = start.model.map_or(parent.model, crate::profiles::id);
        let effort = start.effort.unwrap_or(parent.settings.effort);
        let speed = start.speed.unwrap_or_default();
        let (id, kind, from) = ((caller.clone(), at), MailKind::Note, String::new());
        let mut input = Input::new(Origin::Mail { id, kind, from }, text);
        input.video = start.video;
        // A local child's record comes first; the parent's `Started` is the task its reports name.
        let remote = start.server.as_ref().map(|server| session.on(server));
        if remote.is_none() {
            let (id, choice) = (Some(session.clone()), (Some(effort), Some(speed)));
            let child = Some(child.clone());
            let model = Some(model.clone());
            controller
                .create(cwd.clone(), model, choice, id, child, None)
                .await?;
        }
        let started = remote.clone().unwrap_or_else(|| session.clone());
        let event = ChildEvent::Started(child.clone());
        let started = Body::Child {
            session: started,
            event,
        };
        input.child_task = Some(controller.append(caller, started).await?);
        drop(held);
        if let (Some(server), Some(remote)) = (&start.server, remote) {
            let request = ChildRequest::Start {
                session,
                child,
                cwd,
                model,
                effort,
                speed,
                in_place: matches!(start.placement, Placement::InPlace),
                input,
            };
            let result = controller.devices().child(server, &request).await;
            return match result {
                Ok(mut value) => {
                    value["session"] = json!(remote);
                    Ok(value)
                }
                Err(e) => {
                    controller
                        .append(
                            caller,
                            Body::Child {
                                session: remote.clone(),
                                event: ChildEvent::Stopped,
                            },
                        )
                        .await?;
                    refuse(format!(
                        "Starting child {remote} failed; inspect its session and use agent stop before retrying: {e}"
                    ))
                }
            };
        }
        if let Some(worktree) = &worktree
            && let Err(e) = git::add(worktree, &dir.join("worktree"), &branch(&session)).await
        {
            let (session, event) = (session, ChildEvent::Stopped);
            controller
                .append(caller, Body::Child { session, event })
                .await?;
            return Err(e);
        }
        controller.admit(&session, input).await?;
        let branch = worktree.map(|_| branch(&session));
        Ok(json!({ "session": session, "cwd": cwd, "branch": branch }))
    }

    /// Settles report-only helpers after closeout releases the parent session's task.
    pub(crate) fn closed(&self, c: &Controller, parent: &Summary) {
        let (c, parent) = (c.clone(), parent.clone());
        tokio::spawn(async move {
            for child in parent.children {
                let finished = child
                    .report
                    .is_some_and(|r| r.status == ReportStatus::Finished && r.no_changes);
                if child.settled.is_none() && !child.stopped && finished {
                    let result = c.children().settle(
                        &c,
                        &parent.session,
                        &child.child.name,
                        Decision::Discard,
                    );
                    if let Err(e) = result.await {
                        eprintln!("razor: settling helper {}: {e}", child.session);
                    }
                }
            }
        });
    }

    async fn settle(
        &self,
        controller: &Controller,
        caller: &SessionId,
        name: &str,
        decision: Decision,
    ) -> Result<Value, Error> {
        let parent = controller.summary(caller).await?;
        let child = find(&parent, name)?;
        if child.settled.is_some() || (child.report.is_none() && !child.stopped) {
            return refuse("Settle a finished or stopped child once; stop a working child first.");
        }
        let text = if let Some((session, server)) = child.session.address() {
            controller
                .devices()
                .child(server, &ChildRequest::Settle { session, decision })
                .await?["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        } else {
            self.settle_git(controller, &child.session, &child.child, decision)
                .await?
        };
        let memory = controller.memory();
        memory.settled(controller, &parent, &child, decision).await;
        let (session, event) = (child.session, ChildEvent::Settled(decision));
        controller
            .append(caller, Body::Child { session, event })
            .await?;
        Ok(json!({ "text": text }))
    }
}

/// Refuses one more working child once `children.max` of the parent's are at work.
fn cap(children: &config::Children, parent: &Summary) -> Result<(), Error> {
    let max = children.max;
    let working = parent.children.iter().filter(|c| c.working()).count();
    if working >= max {
        return refuse(format!(
            "This session has {working} working children and children.max is {max}; raise it live or wait for one to finish."
        ));
    }
    Ok(())
}
