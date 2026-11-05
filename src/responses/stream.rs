//! Server-sent events to one completed response.

use super::{CAP, Error, IDLE, Response};
use crate::protocol::{FromBridge, Usage};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Frames bytes before decoding them, so a split character or event changes nothing: each
/// event's data goes to `event` until it returns a result. A stream that ends first, goes
/// silent past `IDLE` or passes `CAP` is incomplete.
pub(super) async fn events<T>(
    mut stream: reqwest::Response,
    mut event: impl FnMut(&[u8]) -> Option<Result<T, Error>>,
) -> Result<T, Error> {
    let (mut line, mut data, mut received) = (Vec::new(), Vec::new(), 0);
    loop {
        let chunk = tokio::time::timeout(IDLE, stream.chunk()).await;
        let chunk = chunk
            .ok()
            .and_then(Result::ok)
            .flatten()
            .ok_or(Error::Incomplete)?;
        received += chunk.len();
        if received > CAP {
            return Err(Error::Size);
        }
        let mut rest = &chunk[..];
        while let Some(at) = memchr::memchr(b'\n', rest) {
            line.extend_from_slice(&rest[..at]);
            rest = &rest[at + 1..];
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if let Some(field) = line.strip_prefix(b"data:") {
                if !data.is_empty() {
                    data.push(b'\n');
                }
                data.extend_from_slice(field.strip_prefix(b" ").unwrap_or(field));
            } else if line.is_empty() && !data.is_empty() {
                if let Some(done) = event(&data) {
                    return done;
                }
                data.clear();
            }
            line.clear();
        }
        line.extend_from_slice(rest);
    }
}

/// The response is its `output_item.done` items in index order; it completes only at
/// `response.completed` with every added index done and every call id unique. Text and
/// reasoning summary deltas are shown as they arrive, a thought per reasoning item (`say`
/// numbers it) whose later summary parts each begin with the line break that joins them in its
/// entry; nothing else before completion is acted on.
pub(super) async fn read(
    stream: reqwest::Response,
    say: &impl Fn(FromBridge),
) -> Result<Response, Error> {
    let (mut added, mut done, mut part) = (BTreeSet::new(), BTreeMap::new(), None);
    events(stream, |data| {
        let Ok(mut event) = serde_json::from_slice::<Value>(data) else {
            return Some(Err(Error::Incomplete));
        };
        let index = event["output_index"].as_u64();
        match event["type"].as_str().unwrap_or_default() {
            "response.output_item.added" => match index {
                Some(index) => {
                    added.insert(index);
                }
                None => return Some(Err(Error::Incomplete)),
            },
            "response.output_item.done" => {
                let item = event
                    .get_mut("item")
                    .filter(|item| item.is_object())
                    .map(Value::take);
                match (index, item) {
                    (Some(index), Some(item)) => {
                        done.insert(index, item);
                    }
                    _ => return Some(Err(Error::Incomplete)),
                }
            }
            "response.output_text.delta" => {
                if let Some(text) = event["delta"].as_str() {
                    say(FromBridge::Delta { text: text.into() });
                }
            }
            "response.reasoning_summary_text.delta" => {
                if let Some(text) = event["delta"].as_str().filter(|t| !t.is_empty()) {
                    let last = part.replace((index, event["summary_index"].as_u64()));
                    let start = last.map(|(item, _)| item) != Some(index);
                    let gap = if start || last == part { "" } else { "\n" };
                    let text = format!("{gap}{text}");
                    say(FromBridge::Thought { n: 0, text, start });
                }
            }
            "response.completed" => {
                return Some(complete(
                    &event["response"],
                    &added,
                    std::mem::take(&mut done),
                ));
            }
            "response.incomplete" => {
                let reason = event.pointer("/response/incomplete_details/reason");
                return Some(Err(Error::Cut(message(
                    reason.and_then(Value::as_str),
                    "unknown reason",
                ))));
            }
            "response.failed" | "error" => {
                let error = event
                    .pointer("/response/error")
                    .or_else(|| event.get("error"))
                    .unwrap_or(&event);
                return Some(Err(match error["code"] == "context_length_exceeded" {
                    true => Error::ContextOverflow(message(
                        error["message"].as_str(),
                        "reduce the context",
                    )),
                    false => Error::Failed(message(
                        error["message"].as_str().or_else(|| error["code"].as_str()),
                        "check the endpoint and model configuration",
                    )),
                }));
            }
            _ => {}
        }
        None
    })
    .await
}

fn message(text: Option<&str>, fallback: &str) -> String {
    text.unwrap_or(fallback).chars().take(30_000).collect()
}

fn complete(
    completed: &Value,
    added: &BTreeSet<u64>,
    done: BTreeMap<u64, Value>,
) -> Result<Response, Error> {
    let mut ids = HashSet::new();
    let mut calls = done.values().filter(|i| {
        matches!(
            i["type"].as_str(),
            Some("function_call" | "custom_tool_call")
        )
    });
    let unique = calls.all(|call| call["call_id"].as_str().is_some_and(|id| ids.insert(id)));
    if completed["status"] != "completed" || !unique || added.iter().any(|i| !done.contains_key(i))
    {
        return Err(Error::Incomplete);
    }
    let usage = &completed["usage"];
    let count = |at: &str| usage.pointer(at).and_then(Value::as_u64).unwrap_or(0);
    let (cache_read, cache_write) = (
        count("/input_tokens_details/cached_tokens"),
        count("/input_tokens_details/cache_write_tokens"),
    );
    let input = count("/input_tokens")
        .saturating_sub(cache_read)
        .saturating_sub(cache_write);
    let usage = Usage {
        input,
        output: count("/output_tokens"),
        cache_read,
        cache_write,
        requests: 1,
        waited: 0,
    };
    let end_turn = completed["end_turn"] != false;
    Ok(Response {
        items: done.into_values().collect(),
        usage,
        end_turn,
        turn_state: None,
        windows: Vec::new(),
        tier: completed["service_tier"].as_str().map(str::to_owned),
    })
}
