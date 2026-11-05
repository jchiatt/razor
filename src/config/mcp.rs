//! Comment-preserving MCP file operations; importing previews and never copies credential bytes.

use super::{Error, Settings, pack};
use crate::protocol::{McpOperation, McpScope};
use serde_json::{Map, Value, json};
use std::{
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::Path,
    sync::{Mutex, PoisonError},
};
use toml_edit::{DocumentMut, Item, Table};

/// Serializes this server's writes to settings files.
static WRITES: Mutex<()> = Mutex::new(());

/// Writes one add, remove, enable or disable to the scope's file: the fresh bytes and the result
/// must both parse, and the file is replaced atomically only if no one changed it meanwhile.
pub(super) fn edit(settings: &Settings, operation: &McpOperation) -> Result<Value, Error> {
    use McpOperation::*;
    let (Add { name, scope, .. }
    | Remove { name, scope }
    | Enable { name, scope }
    | Disable { name, scope }) = operation
    else {
        return Err(Error::Invalid(
            "This MCP operation needs a session's connections.".into(),
        ));
    };
    if name.trim().is_empty() {
        return Err(Error::Invalid(
            "An MCP server needs a nonempty name.".into(),
        ));
    }
    let (path, project) = settings.destination(*scope)?;
    let _write = WRITES.lock().unwrap_or_else(PoisonError::into_inner);
    let creating = project && *scope == McpScope::Private && !path.try_exists().map_err(io)?;
    let before = pack::text(&path, true)?;
    pack::parse(&before, &path, project)?;
    let mut document: DocumentMut = before.parse().map_err(|error| {
        Error::Invalid(format!(
            "{} does not parse: {error}; repair it first.",
            path.display()
        ))
    })?;
    let servers = document
        .entry("mcp")
        .or_insert_with(|| Item::Table(Table::new()));
    let servers = servers
        .as_table_like_mut()
        .ok_or_else(|| Error::Invalid("mcp must be a table; repair the settings first.".into()))?;
    let enabled = matches!(operation, Enable { .. });
    match operation {
        Add { config, .. } => {
            let table = toml_edit::ser::to_document(config).map_err(|error| {
                Error::Invalid(format!("The MCP definition cannot be saved: {error}."))
            })?;
            servers.insert(name, Item::Table(table.into_table()));
        }
        Remove { .. } => drop(servers.remove(name)),
        _ => match servers.get_mut(name).and_then(Item::as_table_like_mut) {
            // Enabling removes a disable marker, restoring the inherited server.
            Some(server)
                if enabled
                    && server.len() == 1
                    && server.get("enabled").and_then(Item::as_bool) == Some(false) =>
            {
                drop(servers.remove(name))
            }
            Some(server) => drop(server.insert("enabled", toml_edit::value(enabled))),
            // A disable marker hides an inherited server this file does not define.
            None if !enabled => {
                let stub = Table::from_iter([("enabled", toml_edit::value(false))]);
                servers.insert(name, Item::Table(stub));
            }
            None => {}
        },
    }
    let after = document.to_string();
    if after.len() > 1 << 20 {
        return Err(Error::Invalid(
            "The changed settings would pass 1 MiB; shorten them first.".into(),
        ));
    }
    pack::parse(&after, &path, project)?;
    replace(&path, &before, &after)?;
    if creating {
        // A private file stays local when possible; a git exclusion never blocks saving it.
        let _ = exclude_private(&path);
    }
    Ok(json!({"file": path, "name": name, "written": true}))
}

/// Writes `after` to `path` at mode 0600 through a rename, only if the file still holds `before`
/// (a missing file holds nothing).
pub(super) fn replace(path: &Path, before: &str, after: &str) -> Result<(), Error> {
    let parent = path.parent().unwrap_or(Path::new("/"));
    let temp = parent.join(format!(".razor-{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let written = std::fs::create_dir_all(parent)
        .and_then(|()| options.open(&temp))
        .and_then(|mut file| {
            file.write_all(after.as_bytes())?;
            file.sync_all()
        });
    let replaced = written
        .map_err(io)
        .and_then(|()| match pack::text(path, true)? == before {
            true => std::fs::rename(&temp, path).map_err(io),
            false => Err(Error::Invalid(format!(
                "{} changed during the edit; retry.",
                path.display()
            ))),
        });
    if replaced.is_err() {
        // Only this operation's create_new file, never the destination.
        let _ = std::fs::remove_file(&temp);
    }
    replaced
}

/// Exclude a newly created private file at the main checkout, including linked worktrees.
fn exclude_private(path: &Path) -> Result<(), Error> {
    let Some(root) = path.ancestors().nth(2) else {
        return Ok(());
    };
    if !root.join(".git").is_dir() {
        return Ok(());
    }
    let exclude = root.join(".git/info/exclude");
    let before = pack::text(&exclude, true)?;
    let entry = ".razor/config.local.toml";
    if !before
        .lines()
        .any(|line| line.trim().trim_start_matches('/') == entry)
    {
        let newline = if before.is_empty() || before.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(exclude)
            .and_then(|mut file| file.write_all(format!("{newline}{entry}\n").as_bytes()))
            .map_err(io)?;
    }
    Ok(())
}

/// The additions an import would make, the existing entries they replace, and the fields left
/// for the person because they hold or need credentials. Nothing is written.
pub(crate) fn preview(settings: &Settings, source: &Path, scope: McpScope) -> Result<Value, Error> {
    let (destination, project) = settings.destination(scope)?;
    let source = std::path::absolute(source).map_err(io)?;
    let text = pack::text(&source, false)?;
    let unreadable =
        |error: String| Error::Invalid(format!("{} does not parse: {error}.", source.display()));
    let value: Value = match source.extension().is_some_and(|e| e == "toml") {
        true => {
            let table: toml::Table = text
                .parse()
                .map_err(|e: toml::de::Error| unreadable(e.message().into()))?;
            serde_json::to_value(table).map_err(|e| unreadable(e.to_string()))?
        }
        false => serde_json::from_str(&text).map_err(|e| unreadable(e.to_string()))?,
    };
    let servers = ["mcpServers", "mcp_servers", "mcp"]
        .iter()
        .find_map(|key| value[key].as_object());
    let mut servers = servers.cloned().unwrap_or_default();
    // Claude Code also keeps a project's entries in ~/.claude.json under its absolute path.
    if let Some(root) = settings.project.as_ref().and_then(|root| root.to_str())
        && let Some(project) = value["projects"][root]["mcpServers"].as_object()
    {
        servers.extend(project.clone());
    }
    if servers.is_empty() {
        let why = "No MCP servers were found; choose a Claude Code, Codex or OpenCode configuration file.";
        return Err(Error::Invalid(why.into()));
    }
    let (current, _) = pack::parse(&pack::text(&destination, true)?, &destination, project)?;
    let existing = current.get("mcp").and_then(toml::Value::as_table);
    let (mut operations, mut replacements, mut unresolved, mut errors) =
        (vec![], vec![], vec![], vec![]);
    for (name, raw) in servers {
        let (config, fields) = imported(&raw, source.parent().unwrap_or(Path::new("/")));
        unresolved.extend(
            fields
                .into_iter()
                .map(|field| json!({"name": name, "field": field})),
        );
        let Ok(config) = serde_json::from_value(config) else {
            let error = "This server has no supported stdio or HTTP connection.";
            errors.push(json!({"name": name, "error": error}));
            continue;
        };
        if existing.is_some_and(|mcp| mcp.contains_key(&name)) {
            replacements.push(name.clone());
        }
        operations.push(McpOperation::Add {
            name,
            scope,
            config,
        });
    }
    Ok(
        json!({"source": source, "destination": destination, "operations": operations,
        "replacements": replacements, "unresolved": unresolved, "errors": errors}),
    )
}

/// One Claude Code, Codex or OpenCode entry as Razor's definition, and the fields left out.
fn imported(raw: &Value, dir: &Path) -> (Value, Vec<String>) {
    let mut config = Map::new();
    let mut unresolved = Vec::new();
    config.insert(
        "enabled".into(),
        raw.get("enabled").cloned().unwrap_or(json!(true)),
    );
    // OpenCode's local command is an argv array; Claude Code and Codex give command and args.
    match &raw["command"] {
        Value::String(command) => {
            config.insert("command".into(), json!(command));
            config.insert("args".into(), raw.get("args").cloned().unwrap_or(json!([])));
        }
        Value::Array(argv) if !argv.is_empty() => {
            config.insert("command".into(), argv[0].clone());
            config.insert("args".into(), json!(argv[1..]));
        }
        _ => config.extend(raw.get("url").map(|url| ("url".into(), url.clone()))),
    }
    config.extend(
        raw["tool_timeout_sec"]
            .as_u64()
            .and_then(|seconds| seconds.checked_mul(1000))
            .map(|ms| ("timeout".into(), json!(ms))),
    );
    config.extend(
        ["search", "tools", "timeout"]
            .iter()
            .filter_map(|key| Some((key.to_string(), raw.get(*key)?.clone()))),
    );
    // A literal value may be a secret; only a reference to the same inherited variable needs
    // no choice, and no bytes are copied.
    let env = raw
        .get("env")
        .or_else(|| raw.get("environment"))
        .and_then(Value::as_object);
    for (name, value) in env.into_iter().flatten() {
        if value.as_str().and_then(env_name) != Some(name.as_str()) {
            unresolved.push(format!("env.{name}"));
        }
    }
    let mut headers = Map::new();
    for key in ["headers", "http_headers", "env_http_headers"] {
        for (name, value) in raw
            .get(key)
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let found = match key {
                "env_http_headers" => value.as_str().map(|env| json!({"env": env})),
                _ => reference(value, dir),
            };
            match found {
                Some(found) => drop(headers.insert(name.clone(), found)),
                None => unresolved.push(format!("headers.{name}")),
            }
        }
    }
    // A token variable is no whole Authorization value, and a reference adds no prefix.
    if raw.get("bearer_token_env_var").is_some() {
        unresolved.push("headers.Authorization".into());
    }
    if !headers.is_empty() {
        config.insert("headers".into(), Value::Object(headers));
    }
    if let Some(oauth) = raw.get("oauth").and_then(Value::as_object) {
        let mut safe = Map::new();
        for (from, to) in [
            ("issuer", "issuer"),
            ("client_id", "client_id"),
            ("clientId", "client_id"),
        ] {
            safe.extend(oauth.get(from).map(|value| (to.into(), value.clone())));
        }
        if let Some(secret) = oauth
            .get("client_secret")
            .or_else(|| oauth.get("clientSecret"))
        {
            match reference(secret, dir) {
                Some(secret) => drop(safe.insert("client_secret".into(), secret)),
                None => unresolved.push("oauth.client_secret".into()),
            }
        }
        config.insert("oauth".into(), Value::Object(safe));
    }
    (Value::Object(config), unresolved)
}

/// An env or file reference, or a `${NAME}` / `{env:NAME}` string naming a variable.
fn reference(value: &Value, dir: &Path) -> Option<Value> {
    if let Some(env) = value["env"].as_str() {
        return Some(json!({"env": env}));
    }
    if let Some(file) = value["file"].as_str() {
        return Some(json!({"file": dir.join(file)}));
    }
    value
        .as_str()
        .and_then(env_name)
        .map(|env| json!({"env": env}))
}

fn env_name(value: &str) -> Option<&str> {
    let name = value
        .strip_prefix("${")
        .or_else(|| value.strip_prefix("{env:"))?
        .strip_suffix('}')?;
    (!name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'))
        .then_some(name)
}

fn io(source: std::io::Error) -> Error {
    Error::Other {
        source,
        context: "writing MCP settings",
    }
}
