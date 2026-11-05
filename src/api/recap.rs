//! A recap for the person (docs/design/api.md), never recorded or put in a model context.

use super::{App, Page, Refusal};
use crate::profiles::{self, Transport};
use crate::protocol::{Body, Entry, EntryId, SessionId};
use crate::record::Summary;
use axum::extract::{Json, Path, Query, State};
use axum::http::StatusCode;
use serde_json::{Value, json};
use std::time::Duration;

const PROMPT: &str = include_str!("recap.md");
/// The evidence's bytes, labels and all, about 8,000 tokens: the person's words, the headline and
/// open questions, then the agent's newest words and as many newest steps as fit (RULES 11).
const EVIDENCE: usize = 32 << 10;
/// The newest entries a recap reads, in one page that also stops at `PAGE_BYTES` (RULES 10): about
/// twice what 32 KiB of steps needs with the results, thinking and responses between them.
const SCAN: u64 = 4096;
/// Where newest words longer than the room lose their middle, keeping the conclusion at their end.
const CUT: &str = "\n[…middle cut…]\n";

/// Always displayable `text` on success, at most 400 characters, or a 503 sentence; the deadline
/// covers reading the evidence too. No model request when unchanged; concurrent recaps each get
/// their own model request.
pub(super) async fn recap(
    State(app): State<App>,
    Path(id): Path<SessionId>,
    Query(q): Query<Page>,
) -> super::Reply {
    let after = EntryId(q.after.unwrap_or(0));
    let summary = app.controller.summary(&id).await.map_err(|e| match e {
        crate::controller::Error::NoSession(_) => e.into(),
        _ => unavailable(),
    })?;
    if summary.last <= after {
        return Ok(Json(
            json!({ "text": "Nothing new since your last visit." }),
        ));
    }
    let asked = async {
        let evidence = evidence(&app, &summary, after).await?;
        let config = app.controller.config().await;
        // Trey, 2026-11-04: "Claude sessions should use haiku medium, gpt sessions terra low,
        // open source model sessions also haiku".
        match profiles::model(&summary.model).transport {
            Transport::Responses => app
                .responses
                .recap(&config.responses.base_url, &id, PROMPT, &evidence)
                .await
                .ok(),
            Transport::Claude | Transport::Chat(_) => {
                crate::claude::recap(&summary.cwd, PROMPT, &evidence)
                    .await
                    .ok()
            }
        }
    };
    let text = tokio::time::timeout(Duration::from_secs(20), asked)
        .await
        .ok()
        .flatten()
        .ok_or_else(unavailable)?;
    let words = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text: String = words
        .chars()
        .filter(|c| !c.is_control())
        .take(400)
        .collect();
    if text.is_empty() {
        return Err(unavailable());
    }
    Ok(Json(json!({ "text": text })))
}

/// At most `EVIDENCE` bytes unless the person's words, headline and questions fill it, from one
/// read of the newest `SCAN` entries after `after`: the agent's words, newest first, its newest
/// whole or else its start and end; the person's latest words, the headline and open questions;
/// then one line per call of its name, target and outcome, newest first, so the oldest drop
/// first. Each part reads oldest first; `None` if the record cannot be read.
async fn evidence(app: &App, s: &Summary, after: EntryId) -> Option<String> {
    let words = async {
        let Some(ask) = s.asks.last() else {
            return Some(String::new());
        };
        let line = app.controller.get(&s.session, *ask).await.ok()?;
        Some(match serde_json::from_str::<Entry>(&line).ok()?.body {
            Body::Input(input) => {
                format!("The person's latest words: {}\n", cut(&input.text, 2048))
            }
            _ => String::new(),
        })
    };
    // Queued together, so both are usually answered from one load of a cold session.
    let scan = (s.last.0 - after.0).min(SCAN) as usize;
    let (words, page) = tokio::join!(words, app.controller.page(&s.session, None, scan));
    let (mut known, mut page) = (words?, page.ok()?);
    // The page's words and its calls with their outcomes once recorded, oldest first: at most its
    // `SCAN` entries and `PAGE_BYTES` (RULES 11).
    let (mut said, mut steps) = (Vec::new(), Vec::new());
    while let Some(line) = page.next_line().await {
        match serde_json::from_str::<Entry>(&line.ok()?).ok()?.body {
            Body::Text { text } => said.push(text + "\n"),
            Body::Call { call, name, input } => steps.push((
                call,
                format!("- {name} {}", target(&input)),
                "Running".into(),
            )),
            Body::Result { call, outcome, .. } => {
                if let Some(step) = steps.iter_mut().rev().find(|s| s.0 == call) {
                    step.2 = format!("{outcome:?}");
                }
            }
            _ => {}
        }
    }
    known += &format!("Headline: {}\n", cut(&s.headline, 512));
    for ask in &s.questions {
        known += &format!("Open question: {}\n", cut(&ask.text, 512));
    }
    known += "Its steps, oldest first:\n";
    let head = "The agent's words:\n";
    let mut room = EVIDENCE.saturating_sub(head.len() + known.len());
    let newest = said.pop().map(|t| ends(t, room)).unwrap_or_default();
    room -= newest.len().min(room);
    // The newest lines that fit in the room left, oldest first.
    let mut fit = |lines: Vec<String>| {
        let fits = |l: String| {
            Some(l)
                .filter(|l| l.len() <= room)
                .inspect(|l| room -= l.len())
        };
        let kept: Vec<_> = lines.into_iter().rev().map_while(fits).collect();
        kept.into_iter().rev().collect::<String>()
    };
    let said = fit(said) + &newest;
    let steps = fit(steps
        .into_iter()
        .map(|s| format!("{}: {}\n", s.1, s.2))
        .collect());
    Some(format!("{head}{said}{known}{steps}"))
}

/// What a call acted on: its command, file, pattern, path, address or query, else its first text,
/// on one line of at most 120 bytes.
fn target(input: &Value) -> String {
    let named = ["command", "file_path", "pattern", "path", "url", "query"];
    let text = named.iter().find_map(|k| input[k].as_str());
    let text = text.or_else(|| input.as_object()?.values().find_map(Value::as_str));
    let words = text.unwrap_or_default().split_whitespace();
    cut(&words.collect::<Vec<_>>().join(" "), 120).to_owned()
}

fn unavailable() -> Refusal {
    Refusal(
        StatusCode::SERVICE_UNAVAILABLE,
        "No recap this time; the transcript has everything.".into(),
    )
}

/// `text` whole within `bytes`, else as much of its start as of its end around `CUT`.
fn ends(text: String, bytes: usize) -> String {
    if text.len() <= bytes {
        return text;
    }
    let half = bytes.saturating_sub(CUT.len()) / 2;
    let end = text.ceil_char_boundary(text.len() - half);
    format!("{}{CUT}{}", cut(&text, half), &text[end..])
}

/// The longest prefix within `bytes`, ending on a character boundary.
fn cut(text: &str, bytes: usize) -> &str {
    &text[..text.floor_char_boundary(bytes)]
}
