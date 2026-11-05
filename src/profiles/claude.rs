//! Claude's own register: its plain-mode declarations and Claude Code's shell names.

use super::Error;
use crate::tools::Action;
use serde_json::{Value, json};

/// Claude Code's built-ins, its web tools included, and the shell's aliases, then every other
/// action as an MCP declaration: Razor's own tools with the declaration GPT's two modes share
/// (data/tools.json), scripted and MCP tools with their own.
pub(super) fn tools(actions: &[Action], builtins: &[Value]) -> Result<Vec<Value>, Error> {
    let mut tools = super::shipped(include_str!("data/claude.json"))?
        .as_array()
        .cloned()
        .unwrap_or_default();
    for action in actions {
        let shared = |b: &&Value| b["action"] == action.name.as_str() && b["mode"] == "both";
        let (description, schema) = match (&action.schema, builtins.iter().find(shared)) {
            // An alias above stands for the action of its name.
            _ if tools.iter().any(|t| t["name"] == action.name.as_str()) => continue,
            (Some(own), _) => (json!(own.description), &own.input),
            (None, Some(b)) => (b["description"].clone(), &b["schema"]),
            (None, None) => continue,
        };
        tools.push(
            json!({"name": action.name, "description": description, "inputSchema": schema,
            "annotations": {"readOnlyHint": action.read_only}}),
        );
    }
    Ok(tools)
}

/// Maps Claude Code's shell names onto the shell's actions; `None` for every other tool.
pub(super) fn input(name: &str, value: &Value) -> Option<Result<(String, Value), Error>> {
    Some(match name {
        "Bash" | "bash" => {
            let timeout = value["timeout"].as_u64();
            let (wait, deadline) = match value["run_in_background"] == true {
                true => (0, timeout.unwrap_or(1_800_000).min(7_200_000)),
                false => {
                    let wait = timeout.unwrap_or(120_000).min(600_000);
                    (wait, wait + 1_800_000)
                }
            };
            let mapped = json!({"command": value["command"], "wait_ms": wait, "pty": false, "deadline_ms": deadline});
            Ok(("bash".into(), mapped))
        }
        "monitor" => {
            if !value["command"].is_string() {
                return Some(Err(Error::Invalid("monitor needs a shell command.".into())));
            }
            let deadline = value["timeout_ms"]
                .as_u64()
                .unwrap_or(300_000)
                .clamp(1, 1_800_000);
            Ok((
                "bash".into(),
                json!({"command": value["command"], "wait_ms": 0, "deadline_ms": deadline}),
            ))
        }
        "TaskStop" | "kill" => {
            let id = value
                .get("task_id")
                .or_else(|| value.get("shell_id"))
                .or_else(|| value.get("job"));
            job(id).map(|job| ("kill".into(), json!({"job": job})))
        }
        "terminal" => terminal(value),
        _ => return None,
    })
}

fn job(value: Option<&Value>) -> Result<u32, Error> {
    let text = |v: &Value| {
        v.as_str()
            .and_then(|s| s.strip_prefix("job-").unwrap_or(s).parse().ok())
    };
    let number = value.and_then(|v| v.as_u64().or_else(|| text(v)));
    number.and_then(|n| u32::try_from(n).ok()).ok_or_else(|| {
        Error::Invalid("The tool needs a job id returned by Bash or terminal".into())
    })
}

/// Starts a program under a terminal, or types into, waits on or reads a job's screen.
fn terminal(value: &Value) -> Result<(String, Value), Error> {
    let (pattern, quiet) = (value.get("pattern"), value.get("quiet_ms"));
    if let Some(command) = value.get("command") {
        if value.get("job").is_some() {
            return Err(Error::Invalid(
                "terminal needs command or job, not both".into(),
            ));
        }
        let wait = value["timeout_ms"].as_u64().unwrap_or(10_000);
        let start = json!({"command": command, "pty": true, "open": true, "wait_ms": wait, "pattern": pattern, "quiet_ms": quiet});
        return Ok(("bash".into(), start));
    }
    let job = job(value.get("job"))?;
    let typed: Vec<_> = ["text", "key", "secret_file"]
        .into_iter()
        .filter(|k| value.get(*k).is_some())
        .collect();
    if typed.len() > 1 {
        return Err(Error::Invalid(
            "terminal accepts only one of text, key or secret_file".into(),
        ));
    }
    let idle = typed.is_empty() && pattern.is_none() && quiet.is_none();
    let timeout = value["timeout_ms"]
        .as_u64()
        .unwrap_or(if idle { 0 } else { 10_000 });
    let mut mapped =
        json!({"job": job, "pattern": pattern, "quiet_ms": quiet, "timeout_ms": timeout});
    match typed.first() {
        Some(key) => {
            mapped[*key] = value[*key].clone();
            Ok(("send".into(), mapped))
        }
        None => Ok(("wait".into(), mapped)),
    }
}
