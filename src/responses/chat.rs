//! A streamed chat completion to one completed response (docs/design/generic.md).

use super::{Error, Response, stream};
use crate::protocol::{FromBridge, Usage};
use serde_json::{Map, Value};
use std::collections::HashSet;

/// The fields whose deltas are appended; every other field's latest value stands.
const APPENDED: [&str; 4] = ["content", "reasoning", "reasoning_content", "arguments"];

/// The assistant message as the deltas assemble it, with the endpoint's own fields kept: a
/// `role` repeated in every chunk is set, not doubled, and a call delta continues the latest
/// call with its `index` unless it has no `index` or names another `id` (Gemini's compatible
/// stream sends whole calls with no `index`). Complete only at `[DONE]` after a `finish_reason`
/// of `stop` or `tool_calls`, with every call holding a unique id and a name; `length` and
/// `content_filter` are `Cut`. Text deltas are shown as they arrive, and so are reasoning
/// deltas, as one thought (`say` numbers it), each chunk's from the field its entry prefers.
pub(super) async fn read(
    response: reqwest::Response,
    say: &impl Fn(FromBridge),
) -> Result<Response, Error> {
    let (mut message, mut calls, mut finish, mut usage, mut thought) =
        (Map::new(), Vec::new(), None, Value::Null, false);
    stream::events(response, |data| {
        if data == b"[DONE]" {
            let calls = std::mem::take(&mut calls);
            return Some(complete(
                std::mem::take(&mut message),
                calls,
                finish.take(),
                &usage,
            ));
        }
        let Ok(mut chunk) = serde_json::from_slice::<Value>(data) else {
            return Some(Err(Error::Incomplete));
        };
        if chunk["error"].is_object() {
            return Some(Err(Error::Incomplete));
        }
        if chunk["usage"].is_object() {
            usage = chunk["usage"].take();
        }
        let choice = chunk.get_mut("choices").and_then(|c| c.get_mut(0));
        let choice = choice?;
        if let Some(reason) = choice["finish_reason"].as_str() {
            finish = Some(reason.to_owned());
        }
        let Value::Object(fields) = choice["delta"].take() else {
            return None;
        };
        let field = |key| fields.get(key).and_then(Value::as_str);
        let reasoning = field("reasoning_content").or_else(|| field("reasoning"));
        if let Some(text) = reasoning.filter(|t| !t.is_empty()) {
            let (text, start) = (text.into(), !std::mem::replace(&mut thought, true));
            say(FromBridge::Thought { n: 0, text, start });
        }
        for (key, value) in fields {
            match (key.as_str(), value) {
                ("tool_calls", Value::Array(deltas)) => {
                    deltas.into_iter().for_each(|d| call(&mut calls, d))
                }
                (key, value) => {
                    if let ("content", Some(text)) = (key, value.as_str()) {
                        say(FromBridge::Delta { text: text.into() });
                    }
                    merge(&mut message, key, value);
                }
            }
        }
        None
    })
    .await
}

/// Merges one call delta into the calls so far.
fn call(calls: &mut Vec<Value>, delta: Value) {
    let Value::Object(delta) = delta else {
        return;
    };
    let index = delta.get("index").filter(|i| !i.is_null()).cloned();
    let id = delta.get("id").and_then(Value::as_str).map(str::to_owned);
    let latest = index.and_then(|i| calls.iter_mut().rev().find(|c| c.get("index") == Some(&i)));
    match latest {
        Some(Value::Object(call))
            if id.is_none() || call.get("id").and_then(Value::as_str) == id.as_deref() =>
        {
            delta
                .into_iter()
                .for_each(|(key, value)| merge(call, &key, value));
        }
        _ => calls.push(Value::Object(delta)),
    }
}

fn merge(into: &mut Map<String, Value>, key: &str, value: Value) {
    match (into.get_mut(key), value) {
        (_, Value::Null) => {}
        (Some(Value::String(old)), Value::String(new)) if APPENDED.contains(&key) => {
            old.push_str(&new)
        }
        (Some(Value::Object(old)), Value::Object(new)) => {
            new.into_iter()
                .for_each(|(key, value)| merge(old, &key, value));
        }
        (_, value) => {
            into.insert(key.to_owned(), value);
        }
    }
}

fn complete(
    mut message: Map<String, Value>,
    mut calls: Vec<Value>,
    finish: Option<String>,
    usage: &Value,
) -> Result<Response, Error> {
    match finish.as_deref() {
        Some("stop" | "tool_calls") => {}
        Some(reason @ ("length" | "content_filter")) => return Err(Error::Cut(reason.into())),
        _ => return Err(Error::Incomplete),
    }
    let mut ids = HashSet::new();
    for call in &mut calls {
        if let Some(call) = call.as_object_mut() {
            call.remove("index");
        }
        let named = call["function"]["name"]
            .as_str()
            .is_some_and(|n| !n.is_empty());
        if !named
            || !call["id"]
                .as_str()
                .is_some_and(|id| ids.insert(id.to_owned()))
        {
            return Err(Error::Incomplete);
        }
    }
    if !calls.is_empty() {
        message.insert("tool_calls".into(), Value::Array(calls));
    }
    message.insert("role".into(), "assistant".into());
    let count = |at: &str| usage.pointer(at).and_then(Value::as_u64).unwrap_or(0);
    let (prompt, cache_read) = (
        count("/prompt_tokens"),
        count("/prompt_tokens_details/cached_tokens"),
    );
    // Output is all a request generated: a host may count reasoning outside completion_tokens.
    let output = (usage.get("total_tokens").and_then(Value::as_u64))
        .map_or(count("/completion_tokens"), |total| {
            total.saturating_sub(prompt)
        });
    let usage = Usage {
        input: prompt.saturating_sub(cache_read),
        output,
        cache_read,
        cache_write: 0,
        requests: 1,
        waited: 0,
    };
    Ok(Response {
        items: vec![Value::Object(message)],
        usage,
        end_turn: true,
        turn_state: None,
        windows: Vec::new(),
        tier: None,
    })
}
