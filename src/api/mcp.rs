//! `razor mcp`: the shell's words as the same MCP operations every client sends.

use super::client::send;
use super::{Error, LOCAL, other};
use crate::protocol::{McpRow, McpScope, SessionId};
use serde_json::{Value, json};
use std::path::Path;

/// Validate CLI words before connecting. File paths are absolute against the shell's directory.
pub fn parse(home: &Path, words: &[String]) -> Result<(&'static str, Value), Error> {
    let usage = || Error::Refused("Use razor mcp --help for commands and options.".into());
    let (mut args, mut scope, mut config) = (Vec::new(), "private", None);
    let mut words = words.iter().map(String::as_str);
    while let Some(word) = words.next() {
        match word {
            "--scope" => {
                let named = words
                    .next()
                    .filter(|s| ["global", "project", "private"].contains(s));
                scope = named.ok_or_else(usage)?;
            }
            "--config" => {
                let given = serde_json::from_str::<Value>(words.next().ok_or_else(usage)?);
                let why = |e| Error::Refused(format!("The MCP definition is invalid JSON: {e}."));
                config = Some(given.map_err(why)?);
            }
            word => args.push(word),
        }
    }
    if let ["complete", redirect] = args[..] {
        return Ok(("/v1/mcp/callback", json!({"redirect": redirect})));
    }
    let (op, name) = match args[..] {
        [] | ["list"] => ("list", ""),
        ["add", _] if config.is_none() => return Err(usage()),
        [
            op @ ("add" | "remove" | "enable" | "disable" | "reconnect" | "sign-in" | "sign-out"
            | "import"),
            name,
        ] => (op, name),
        _ => return Err(usage()),
    };
    let cwd = std::env::current_dir().map_err(other("Reading the current directory"))?;
    let session = std::env::var("RAZOR_SESSION").ok().map(SessionId::try_from);
    let session = session.transpose().map_err(|_| {
        Error::Refused("RAZOR_SESSION names no valid session; unset it outside an agent.".into())
    })?;
    let session = session.filter(|id| {
        home.join("sessions")
            .join(id.to_string())
            .join("record.jsonl")
            .is_file()
    });
    Ok((
        "/v1/mcp",
        json!({"op": op.replace('-', "_"), "name": name, "source": cwd.join(name), "scope": scope,
        "config": config, "cwd": cwd, "session": session}),
    ))
}

/// Send a validated management request and render the answer for the person.
pub async fn run(
    client: &reqwest::Client,
    key: &str,
    (route, body): (&str, Value),
) -> Result<String, Error> {
    let request = if route == "/v1/mcp/callback" {
        client.get(format!("{LOCAL}{route}")).query(&body)
    } else {
        client.post(format!("{LOCAL}{route}")).json(&body)
    };
    let value = answer(request, key).await.map_err(|e| {
        if body["op"] == "sign_in" {
            Error::Refused(format!(
                "{e} Run razor mcp sign-in {}.",
                body["name"].as_str().unwrap_or_default()
            ))
        } else {
            e
        }
    })?;
    match body["op"].as_str() {
        Some("list") => {
            let rows: Vec<McpRow> =
                serde_json::from_value(value).map_err(other("Reading the MCP list"))?;
            if rows.is_empty() {
                return Ok("No MCP servers configured. Run razor mcp add --help.".into());
            }
            Ok(rows
                .into_iter()
                .map(|row| {
                    let scope = match row.scope {
                        Some(McpScope::Global) => "global",
                        Some(McpScope::Project) => "project",
                        Some(McpScope::Private) => "private",
                        None => "pack",
                    };
                    format!(
                        "{}: {} ({scope}){}",
                        row.name,
                        row.state,
                        row.error.map(|e| format!("; {e}")).unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        Some("import") => Ok(preview(&value)),
        _ => serde_json::to_string_pretty(&value).map_err(other("Rendering MCP management")),
    }
}

/// The import's additions, replacements, missing values and errors, without configuration bytes.
fn preview(value: &Value) -> String {
    let mut lines = Vec::new();
    for (key, label) in [
        ("operations", "Add"),
        ("replacements", "Replace"),
        ("unresolved", "Needs a value"),
        ("errors", "Error"),
    ] {
        for row in value[key].as_array().into_iter().flatten() {
            let name = row
                .as_str()
                .or_else(|| row["name"].as_str())
                .unwrap_or_default();
            let detail = row["field"]
                .as_str()
                .or_else(|| row["error"].as_str())
                .map(|s| format!(": {s}"))
                .unwrap_or_default();
            lines.push(format!("{label}: {name}{detail}"));
        }
    }
    lines.push("Preview only; nothing was saved. To review and apply, open razor and run /mcp import <file>.".into());
    lines.join("\n")
}

/// The shared HTTP path knows neither CLI operations nor their presentation.
async fn answer(request: reqwest::RequestBuilder, key: &str) -> Result<Value, Error> {
    send(request.bearer_auth(key))
        .await?
        .json()
        .await
        .map_err(other("Reading MCP management"))
}
