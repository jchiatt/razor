//! One consume child per open meeting, drained apart from the record, and the completion watch.
use super::*;
use crate::protocol::{Input, Live, Origin, Speaker};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, BufReader},
    sync::mpsc,
    task::JoinSet,
    time::Instant,
};

/// Starts following an open meeting unless the server already is.
pub(super) async fn ensure(app: &App, id: &SessionId, meeting: &str, folder: &str) {
    let key = (id.clone(), meeting.to_owned());
    let mut readers = app.meetings.readers.lock().await;
    if readers.contains_key(&key) {
        return;
    }
    readers.insert(key.clone(), None);
    let (app, folder) = (app.clone(), folder.to_owned());
    tokio::spawn(async move {
        follow(&app, &key, &folder).await;
        app.meetings.readers.lock().await.remove(&key);
    });
}

/// From the server's start, follows the open meetings of every local session a status row names.
pub(in crate::api) async fn run(app: App) {
    let mut rows = app.controller.status();
    while let Some(row) = rows.recv().await {
        let ids = match row {
            Live::Agents { agents, .. } => agents.into_iter().map(|a| a.session).collect(),
            Live::Agent(a) => vec![a.session],
            _ => Vec::new(),
        };
        for id in ids.into_iter().filter(|id| id.address().is_none()) {
            let Ok(summary) = app.controller.summary(&id).await else {
                continue;
            };
            for m in summary
                .meetings
                .iter()
                .filter(|m| m.state != MeetingState::Closed)
            {
                ensure(&app, &id, &m.id, &m.folder).await;
            }
        }
    }
}

/// One row of `meetrec live --follow --json`; a row of another kind is skipped.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Row {
    Line {
        speaker: String,
        who: String,
        text: String,
        #[serde(default)]
        start: f64,
        #[serde(default)]
        end: f64,
    },
    Status {
        live: Worker,
    },
    End,
}

#[derive(Deserialize)]
struct Worker {
    #[serde(default)]
    alive: bool,
    lag_s: Option<f64>,
}

async fn follow(app: &App, key: &(SessionId, String), folder: &str) {
    let (id, meeting) = key;
    // At most 256 rows wait, each from a line of at most 64 KiB. A full queue drops the row and
    // never blocks meetrec's stdout; dropped lines stay in live.jsonl for `meetrec live`.
    let (send, mut rows) = mpsc::channel(256);
    // Dropping the set, at the loop's end or when this task is dropped, ends the child too.
    let mut reading = JoinSet::new();
    reading.spawn(consume(folder.to_owned(), format!("razor:{id}"), send));
    if let Err(e) = command(&["ingest", folder]).await {
        eprintln!("razor: meeting {meeting} on {id}: {e}");
    }
    let (mut ingested, mut heard) = (Instant::now(), Instant::now());
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    let mut following = true;
    loop {
        tokio::select! {
            row = rows.recv(), if following => match row {
                None | Some(Row::End) => {
                    following = false;
                    app.meetings.readers.lock().await.insert(key.clone(), None);
                }
                Some(Row::Status { live }) => {
                    heard = Instant::now();
                    let lag = live.lag_s.filter(|_| live.alive);
                    app.meetings.readers.lock().await.insert(key.clone(), lag);
                    if !live.alive && ingested.elapsed() >= Duration::from_secs(60) {
                        ingested = Instant::now();
                        if let Err(e) = command(&["ingest", folder]).await {
                            eprintln!("razor: meeting {meeting} on {id}: {e}");
                        }
                    }
                }
                Some(Row::Line { speaker, who, text, start, end }) => {
                    let ms = |s: f64| (s.max(0.0) * 1000.0) as u64;
                    let (who, name, label, start_ms, end_ms) = (Speaker::Room, Some(who), Some(speaker), ms(start), ms(end));
                    let line = CallEvent::Line { who, name, label, text, start_ms, end_ms };
                    if let Err(e) = fact(app, id, meeting, line).await {
                        eprintln!("razor: meeting {meeting} on {id}: {e}");
                    }
                }
            },
            _ = tick.tick() => {
                if heard.elapsed() > Duration::from_secs(15) {
                    app.meetings.readers.lock().await.insert(key.clone(), None);
                }
                match finish(app, id, meeting, folder).await {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(e) => eprintln!("razor: meeting {meeting} on {id}: {e}"),
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct Words {
    ready: String,
    failed: String,
}

static WORDS: std::sync::LazyLock<Words> = std::sync::LazyLock::new(|| {
    // Invariant: this compiled-in file parses in every build, as flow 71 shows.
    toml::from_str(include_str!("../meetings.toml")).expect("meetings.toml parses")
});

/// Advances the folder and sends the notes input once `final.json` is there; true once the
/// meeting is no longer open. An error leaves it open for the next tick.
async fn finish(app: &App, id: &SessionId, meeting: &str, folder: &str) -> Result<bool> {
    let _writing = app.meetings.write(id).await;
    let summary = app.controller.summary(id).await?;
    let open = summary
        .meetings
        .iter()
        .find(|m| m.id == meeting && m.state != MeetingState::Closed);
    let Some(m) = open else { return Ok(true) };
    let done = advance(folder).await?["final"].take();
    if done.is_null() {
        return Ok(false);
    }
    let text = if done["ok"] == true {
        let names: Vec<_> = (done["speakers"].as_array().into_iter().flatten())
            .filter_map(Value::as_str)
            .collect();
        let transcript = FilePath::new(folder).join("transcript.md");
        (WORDS
            .ready
            .replace("{transcript}", &transcript.to_string_lossy()))
        .replace("{duration}", &done["duration_s"].to_string())
        .replace("{names}", &names.join(", "))
    } else {
        (WORDS.failed.replace("{folder}", folder))
            .replace("{step}", done["step"].as_str().unwrap_or("transcription"))
            .replace(
                "{error}",
                done["error"].as_str().unwrap_or("the batch pass failed"),
            )
    };
    let (request, from, device) = (Some(meeting.into()), Origin::Razor, m.device.clone());
    let input = Input {
        text,
        from,
        request,
        meeting: Some(meeting.into()),
        device,
        ..Default::default()
    };
    app.controller.admit(id, input).await?;
    Ok(true)
}

/// Runs the consume child, restarting it no faster than every five seconds, until meetrec says
/// every line is delivered or nobody reads the rows.
async fn consume(folder: String, owner: String, rows: mpsc::Sender<Row>) {
    while !rows.is_closed() {
        let started = Instant::now();
        let child = tokio::process::Command::new("meetrec")
            .args([
                "live",
                &folder,
                "--subscriber",
                &owner,
                "--consume",
                "--follow",
                "--json",
            ])
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        // The child's stdin stays open while it runs: meetrec exits when it closes.
        if let Ok(mut child) = child
            && let Some(stdout) = child.stdout.take()
        {
            let mut stdout = BufReader::new(stdout);
            while let Some(line) = line(&mut stdout).await {
                match serde_json::from_slice(&line) {
                    Ok(Row::End) => return,
                    Ok(row) => drop(rows.try_send(row)),
                    Err(_) => {}
                }
            }
            let _ = child.wait().await;
        }
        tokio::time::sleep_until(started + Duration::from_secs(5)).await;
    }
}

/// The next line, keeping at most 64 KiB of it and draining the rest, so the line after starts
/// at its own beginning even when a broken child writes no newline. None at the end.
async fn line(reader: &mut (impl AsyncBufRead + Unpin)) -> Option<Vec<u8>> {
    let mut line = Vec::new();
    loop {
        let bytes = reader.fill_buf().await.ok()?;
        if bytes.is_empty() {
            return (!line.is_empty()).then_some(line);
        }
        let n = bytes
            .iter()
            .position(|b| *b == b'\n')
            .map_or(bytes.len(), |n| n + 1);
        // Invariant: `line` never exceeds 64 KiB, so the subtraction cannot underflow.
        line.extend_from_slice(&bytes[..n.min((64 << 10) - line.len())]);
        let done = bytes[n - 1] == b'\n';
        reader.consume(n);
        if done {
            return Some(line);
        }
    }
}
