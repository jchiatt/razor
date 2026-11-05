//! Versioned filing and external editing of shared group definitions.
use super::super::{Error, error};
use super::{App, root};
use crate::protocol::{SessionId, views::SectionKind};
use reqwest::Method;
use tokio::io::AsyncReadExt;

/// Files the root in an existing saved group using the same versioned editor route.
pub(super) fn file(app: &mut App, path: &str) {
    let Some(id) = app.focus.clone().map(|id| root(app, id)) else {
        return app.say("Open an agent before filing it.");
    };
    if path.is_empty() {
        let mut path = Vec::new();
        for section in app
            .list
            .layout()
            .into_iter()
            .flat_map(|l| &l.sections)
            .filter(|s| s.kind == SectionKind::Group)
        {
            path.truncate(section.depth as usize);
            path.push(section.name.as_str());
            if section.sessions.iter().any(|r| r.session == id) {
                return app.say(&path.join(" / "));
            }
        }
        return app.say("This agent has no saved group.");
    }
    let path: Vec<_> = path.split('/').map(|p| p.trim().to_owned()).collect();
    let view = app.list.view.clone().unwrap_or_default();
    let id = id.address().map_or(id.clone(), |(native, _)| native);
    app.write(|api| async move {
        let saved = async {
            use crate::protocol::views::{Match, Views};
            let value = api.json(Method::GET, "v1/views", None).await?;
            let mut views: Views =
                serde_json::from_value(value).map_err(|e| error(e.to_string()))?;
            let view = views
                .views
                .iter_mut()
                .find(|v| v.name == view)
                .ok_or_else(|| error("That view changed; use /views."))?;
            let mut groups = &view.groups;
            let mut target = String::new();
            for part in path {
                let matching: Vec<_> = groups.iter().filter(|g| g.name == part).collect();
                if matching.len() != 1 {
                    return Err(error(
                        "That group path is unknown or ambiguous; use /views.",
                    ));
                }
                target.clone_from(&matching[0].id);
                groups = &matching[0].groups;
            }
            let mut pending: Vec<_> = view.groups.iter_mut().collect();
            while let Some(g) = pending.pop() {
                g.matches
                    .retain(|c| !matches!(c, Match::Session(s) if s.as_str().split('@').next() == Some(id.as_str())));
                if g.id == target {
                    g.matches.push(Match::Session(id.clone()));
                }
                pending.extend(g.groups.iter_mut());
            }
            api.json(Method::PUT, "v1/views", Some(serde_json::json!(views)))
                .await
        }
        .await;
        move |app: &mut App| match saved {
            Ok(_) => app.say("Agent filed."),
            Err(e) => app.say(&e.to_string()),
        }
    });
}
/// Leaves raw mode in the caller, edits a bounded temporary definition and saves its original version.
pub(in crate::terminal) async fn edit(app: &mut App) -> Result<(), Error> {
    use crate::protocol::views::Views;
    use tokio::io::AsyncWriteExt;
    let Some(api) = app.api.clone() else {
        let unavailable = "The API is unavailable; reconnect before editing views.";
        return Err(error(unavailable));
    };
    let value = api.json(Method::GET, "v1/views", None).await?;
    let path = std::env::temp_dir().join(format!("razor-views-{}.json", SessionId::new()));
    let edited = async {
        let before = serde_json::to_vec_pretty(&value)?;
        let mut options = tokio::fs::OpenOptions::new();
        let options = options.write(true).create_new(true).mode(0o600);
        let mut file = options.open(&path).await?;
        file.write_all(&before).await?;
        drop(file);
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
        let mut shell = tokio::process::Command::new("sh");
        let shell = shell.arg("-c").arg(format!(r#"{editor} "$1""#));
        if !shell
            .arg("razor-views")
            .arg(&path)
            .status()
            .await?
            .success()
        {
            let failed = "The editor did not exit successfully.";
            return Err(std::io::Error::other(failed));
        }
        let (mut bytes, limit) = (Vec::new(), crate::protocol::BODY_BYTES as u64 + 1);
        let file = tokio::fs::File::open(&path).await?;
        file.take(limit).read_to_end(&mut bytes).await?;
        if bytes == before {
            return Ok(None);
        }
        let mut edited: Views = serde_json::from_slice(&bytes)?;
        edited.version = serde_json::from_value::<Views>(value)?.version;
        Ok(Some(edited))
    };
    let failure = |e: String| {
        let path = path.display();
        let words = format!("Views were not saved; your edits remain at {path}: {e}");
        error(words)
    };
    if let Some(edited) = edited.await.map_err(|e| failure(e.to_string()))? {
        let saved = api.json(Method::PUT, "v1/views", Some(serde_json::json!(edited)));
        saved.await.map_err(|e| failure(e.to_string()))?;
    }
    let _ = tokio::fs::remove_file(path).await;
    Ok(())
}
