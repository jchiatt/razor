//! Jobs share one wait and result path; secret input reaches the shell only from a file.

use super::{Error, Tools, guard, io};
use crate::{
    protocol::{EntryId, JobId, SPILL},
    shell::{self, Command, Input, Stdin},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::{io::AsyncReadExt, time::Instant};

/// The most bytes a secret file may hold.
const SECRET: u64 = 64 << 10;

#[derive(Default, Deserialize)]
struct Wait {
    pattern: Option<String>,
    quiet_ms: Option<u64>,
    max_output_chars: Option<usize>,
}

#[derive(Deserialize)]
pub(super) struct Bash {
    command: String,
    cwd: Option<String>,
    wait_ms: Option<u64>,
    deadline_ms: Option<u64>,
    #[serde(default)]
    pty: bool,
    stdin: Option<String>,
    #[serde(default)]
    open: bool,
    #[serde(flatten)]
    wait: Wait,
}

#[derive(Deserialize)]
pub(super) struct Handle {
    job: u32,
    text: Option<String>,
    key: Option<String>,
    secret_file: Option<String>,
    #[serde(default)]
    end: bool,
    #[serde(default)]
    timeout_ms: u64,
    #[serde(flatten)]
    wait: Wait,
}

pub(super) async fn bash(tools: &Tools, at: EntryId, input: Bash) -> Result<Value, Error> {
    let clock = Instant::now();
    let cwd = input
        .cwd
        .as_deref()
        .map_or_else(|| tools.cwd.clone(), |p| tools.path(p));
    let delete = tools.state().config.delete_guard;
    let (runs_post, kills, bare) =
        guard::command(&input.command, &cwd, tools.since, delete, &tools.shell).await?;
    // Each stopped job's end arrives as its usual notice; the rest of the command then runs.
    for &job in &kills {
        tools.shell.kill(job).await?;
    }
    if !bare {
        return Err(Error::Refused("Razor stopped the jobs that `pkill` matched; nothing else in this command ran, so send the rest again without `pkill`.".into()));
    }
    let mut env = tools.env.clone();
    let session = env.get("RAZOR_SESSION").cloned().unwrap_or_default();
    if let Some(id) = tools
        .post
        .participant(&session, &tools.cwd, runs_post)
        .await
    {
        env.insert("POST_PARTICIPANT".into(), id);
    }
    let command = Command {
        text: match kills.is_empty() {
            true => input.command,
            false => format!("pkill() {{ :; }}; {}", input.command),
        },
        cwd,
        env,
        pty: input.pty,
        stdin: match (input.stdin, input.open) {
            (Some(text), _) => Stdin::Text(text),
            (None, true) => Stdin::Open,
            (None, false) => Stdin::Closed,
        },
        deadline: input.deadline_ms.map(Duration::from_millis),
    };
    let job = tools.shell.start(at, command).await?;
    watch(
        tools,
        job,
        input.wait_ms.unwrap_or(60_000),
        input.wait,
        clock,
    )
    .await
}

pub(super) async fn handle(tools: &Tools, name: &str, input: Handle) -> Result<Value, Error> {
    let clock = Instant::now();
    let job = JobId(input.job);
    if name == "kill" {
        tools.shell.kill(job).await?;
        let wait = Wait {
            max_output_chars: input.wait.max_output_chars,
            ..Wait::default()
        };
        return watch(tools, job, 0, wait, clock).await;
    }
    if name == "send" {
        let given = [
            input.text.is_some(),
            input.key.is_some(),
            input.secret_file.is_some(),
            input.end,
        ];
        if given.into_iter().filter(|given| *given).count() != 1 {
            return Err(Error::Refused(
                "Give exactly one of text, key, secret_file or end.".into(),
            ));
        }
        let typed = match (input.text, input.key, input.secret_file) {
            (Some(text), _, _) if text.is_empty() => None,
            (Some(text), _, _) => Some(Input::Text(text)),
            (_, Some(key), _) => Some(Input::Key(key)),
            (_, _, Some(file)) => Some(Input::Text(secret(tools, &file).await?)),
            _ => Some(Input::End),
        };
        if let Some(typed) = typed {
            tools.shell.send(job, typed).await?;
        }
    }
    watch(tools, job, input.timeout_ms, input.wait, clock).await
}

pub(super) async fn script(
    tools: &Tools,
    at: EntryId,
    command: String,
    input: Value,
) -> Result<Value, Error> {
    let clock = Instant::now();
    let job = tools
        .shell
        .start(
            at,
            Command {
                text: command,
                cwd: tools.cwd.clone(),
                env: tools.env.clone(),
                pty: false,
                stdin: Stdin::Text(input.to_string()),
                deadline: None,
            },
        )
        .await?;
    watch(tools, job, 60_000, Wait::default(), clock).await
}

async fn secret(tools: &Tools, file: &str) -> Result<String, Error> {
    let opened = tokio::fs::File::open(tools.path(file))
        .await
        .map_err(|e| io(e, file))?;
    let mut text = String::new();
    opened
        .take(SECRET + 1)
        .read_to_string(&mut text)
        .await
        .map_err(|e| io(e, file))?;
    if text.len() as u64 > SECRET {
        return Err(Error::Refused(format!(
            "{file} is over 64 KiB; a secret is shorter."
        )));
    }
    Ok(text)
}

async fn watch(
    tools: &Tools,
    job: JobId,
    timeout: u64,
    wait: Wait,
    clock: Instant,
) -> Result<Value, Error> {
    let mut output = tools
        .shell
        .wait(
            job,
            shell::Wait {
                timeout: Duration::from_millis(timeout),
                pattern: wait.pattern,
                quiet: wait.quiet_ms.map(Duration::from_millis),
            },
        )
        .await?;
    if let Some((at, character)) = output
        .text
        .char_indices()
        .rev()
        .nth(wait.max_output_chars.unwrap_or(SPILL).min(SPILL))
    {
        output.text.drain(..at + character.len_utf8());
        output
            .path
            .get_or_insert_with(|| tools.dir.join(format!("job-{}", job.0)));
    }
    Ok(
        json!({"job": job.0, "end": output.end, "output": output.text, "path": output.path,
        "cursor": output.screen.map(|screen| [screen.cursor.0, screen.cursor.1]),
        "wall_time_seconds": clock.elapsed().as_secs_f64()}),
    )
}
