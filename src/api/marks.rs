//! The session map's marks, read as one fixed snapshot and never stored.

use super::{App, Error, other};
use crate::protocol::{Body, End, Entry, EntryId, Outcome, SessionId};
use axum::extract::{Json, Path, State};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read};

/// The newest 4,096 marks in entry order; the scan and JSON decoding run off the request loop.
pub(super) async fn marks(State(app): State<App>, Path(id): Path<SessionId>) -> super::Reply {
    let file = app.controller.file(&id, "record.jsonl").await?;
    let bytes = file
        .metadata()
        .await
        .map_err(other("Reading the record's size"))?
        .len();
    let file = file.into_std().await;
    let marks = tokio::task::spawn_blocking(move || read(file, bytes))
        .await
        .map_err(other("Reading the session map"))??;
    Ok(Json(json!(marks)))
}

fn read(file: std::fs::File, bytes: u64) -> Result<Vec<(EntryId, &'static str)>, Error> {
    let mut reader = BufReader::with_capacity(64 << 10, file.take(bytes));
    let mut line = String::new(); // One complete entry, as in the record's own reader.
    let mut marks = BTreeMap::new(); // At most 4,096 answer marks.
    let mut child = false;
    loop {
        line.clear();
        if reader
            .read_line(&mut line)
            .map_err(other("Reading the session map"))?
            == 0
            || !line.ends_with('\n')
        {
            break;
        }
        let entry: Entry =
            serde_json::from_str(&line).map_err(other("Decoding the session map"))?;
        let mark = match entry.body {
            Body::Start { child: started, .. } => {
                child = started.is_some();
                None
            }
            Body::Input(input) if input.from.is_person() => Some((entry.id, "you")),
            Body::Withdrawn { inputs } => {
                for input in inputs {
                    marks.remove(&input);
                }
                None
            }
            Body::Question { .. } => Some((entry.id, "question")),
            Body::Rebuild { .. } => Some((entry.id, "rebuild")),
            Body::TurnEnd {
                end: End::Error(_), ..
            } => Some((entry.id, "error")),
            Body::Call { name, input, .. } => (!child
                && crate::record::ask(&name, &input).is_some())
            .then_some((entry.id, "question")),
            Body::Result {
                outcome: Outcome::Error,
                ..
            } => Some((entry.id, "failed")),
            _ => None,
        };
        if let Some((id, kind)) = mark {
            marks.insert(id, kind);
            if marks.len() > 4096 {
                marks.pop_first();
            }
        }
    }
    Ok(marks.into_iter().collect())
}
