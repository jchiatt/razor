//! The project's history, read from git on request and never stored.

use super::App;
use crate::protocol::SessionId;
use axum::extract::{Json, Path, State};
use serde_json::json;
use std::{path::Path as Folder, process::Stdio, time::Duration};
use tokio::process::Command;

/// The folder's counts, named branch and uncommitted files, each omitted when unknown;
/// detached heads have no branch, and the shared two-second deadline or a cancelled request
/// kills all git processes. Status takes no optional lock, so it never blocks the agents' git.
pub(super) async fn project(State(app): State<App>, Path(id): Path<SessionId>) -> super::Reply {
    let cwd = app.controller.summary(&id).await?.cwd;
    let (commits, merges, branch, status) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(
            git(&cwd, &["rev-list", "--count", "HEAD"]),
            git(
                &cwd,
                &["rev-list", "--count", "--first-parent", "--merges", "HEAD"]
            ),
            git(&cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"]),
            git(&cwd, &["--no-optional-locks", "status", "--porcelain"])
        )
    })
    .await
    .unwrap_or_default();
    let count = |n: String| n.parse::<u64>().ok();
    let (commits, merges) = commits.and_then(count).zip(merges.and_then(count)).unzip();
    let mut project = json!({"commits": commits, "merges": merges, "branch": branch,
                            "changed": status.map(|s| s.lines().count())});
    if let Some(fields) = project.as_object_mut() {
        fields.retain(|_, value| !value.is_null());
    }
    Ok(Json(project))
}

async fn git(cwd: &Folder, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    output.status.success().then_some(())?;
    Some(std::str::from_utf8(&output.stdout).ok()?.trim().to_owned())
}
