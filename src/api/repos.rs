//! Past session folders and shallow checkout discovery, with no retained discovery cache.

use super::{App, Refusal, Reply, bad, other};
use axum::extract::{Json, Query, State};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// At most this many entries are examined during discovery.
const EXAMINED: usize = 4_096;

#[derive(Deserialize)]
pub(super) struct Under {
    under: Option<PathBuf>,
}

/// Past folders, newest first, then checkouts with no `at` in `under` (by default the folder most
/// past folders sit in), its children and grandchildren. A timed-out blocking thread stays blocked
/// until macOS answers its filesystem call.
pub(super) async fn repos(State(app): State<App>, Query(q): Query<Under>) -> Reply {
    if q.under.as_ref().is_some_and(|p| !p.is_absolute()) {
        return Err(bad(
            "Name an absolute folder to look in, such as /Users/you/Code.",
        ));
    }
    let records = crate::record::list(&app.home, None, None, false, usize::MAX)
        .await
        .map_err(crate::controller::Error::from)?;
    let mut known = BTreeMap::<_, SystemTime>::new();
    for (_, path, .., at) in records {
        known
            .entry(path)
            .and_modify(|last| *last = (*last).max(at))
            .or_insert(at);
    }
    let scan = tokio::task::spawn_blocking(move || scan(known, q.under));
    tokio::time::timeout(Duration::from_secs(2), scan)
        .await
        .map_err(|_| bad("Looking for repositories timed out; choose a smaller readable folder."))?
        .map_err(other("Looking for repositories"))?
        .map(Json)
}

fn scan(known: BTreeMap<PathBuf, SystemTime>, under: Option<PathBuf>) -> Result<Value, Refusal> {
    let row = |path: &PathBuf, at: Option<SystemTime>| {
        let at = at
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|t| t.as_millis() as u64);
        let repo = crate::children::repo_at(path);
        json!({"path": path, "repo": repo, "at": at})
    };
    let mut repos: BTreeMap<_, _> = known
        .into_iter()
        .filter(|(path, _)| path.is_dir())
        .map(|(path, at)| {
            let item = row(&path, Some(at));
            (path, item)
        })
        .collect();
    let mut parents = BTreeMap::<_, usize>::new();
    for path in repos.keys().filter_map(|p| p.parent()) {
        *parents.entry(path.to_owned()).or_default() += 1;
    }
    let most = parents.into_iter().max_by_key(|(_, n)| *n).map(|(p, _)| p);
    // Only a named folder that cannot be read refuses; an unreadable inferred one is skipped.
    let named = under.is_some();
    let mut queue: Vec<_> = under.or(most).map(|under| (under, 0)).into_iter().collect();
    let (mut examined, mut truncated) = (0, false);
    'folders: while let Some((path, depth)) = queue.pop() {
        let entries = match std::fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(e) if depth == 0 && named => {
                return Err(bad(format!(
                    "Razor cannot read {}: {e}; name a folder it can.",
                    path.display()
                )));
            }
            Err(_) => continue,
        };
        if std::fs::symlink_metadata(path.join(".git")).is_ok() {
            repos
                .entry(path.clone())
                .or_insert_with(|| row(&path, None));
            continue;
        }
        if depth >= 2 {
            continue;
        }
        for entry in entries.flatten() {
            if examined == EXAMINED {
                truncated = true;
                break 'folders;
            }
            examined += 1;
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                queue.push((entry.path(), depth + 1));
            }
        }
    }
    let mut repos: Vec<_> = repos.into_values().collect();
    repos.sort_by_key(|r| std::cmp::Reverse(r["at"].as_u64()));
    Ok(json!({"repos": repos, "truncated": truncated}))
}
