//! A native conversation's request state on Razor's own loop, Responses or chat, replayed from
//! native data only.

use super::{Error, Profile, Replay, Transport};
use crate::protocol::{Effort, Millis, SessionId, Speed};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Component, Path, PathBuf},
};
use tokio::io::AsyncReadExt;

/// The most bytes one image a request carries may hold.
pub(crate) const IMAGE: u64 = 10 << 20;
/// An upload with less than this left of Google's 48 hours is made again.
const HOUR: Millis = 3_600_000;

/// The wire input of one native conversation. `prefix` is the `razor_prefix` descriptor: the
/// opening input and top-level tools, recorded with the first response and never re-rendered.
/// Its size is checked when `body` encodes it, so it outgrows 64 MiB by at most one exchange
/// before the next request refuses and the conversation closes.
pub(crate) struct Conversation {
    prefix: Value,
    items: Vec<Value>,
    pending: Vec<Value>,
    lite: bool,
    /// On chat: how reasoning goes back, the model's top effort and the request's added fields.
    chat: Option<(Replay, Effort, Map<String, Value>)>,
}

/// A chat profile's settings for its conversation.
fn chat(profile: &Profile) -> Option<(Replay, Effort, Map<String, Value>)> {
    let Transport::Chat(row) = profile.transport else {
        return None;
    };
    let endpoint = &profile.endpoint;
    Some((endpoint.replay, row.top, endpoint.request.clone()))
}

/// A declaration's name in either wire's shape.
fn named(tool: &Value) -> Option<&str> {
    tool["name"]
        .as_str()
        .or_else(|| tool["function"]["name"].as_str())
}

impl Conversation {
    /// A fresh fixed prefix from `tools` and the profile's prompt; code mode puts declarations
    /// in an addition, plain mode at the top level. The first completed response records it.
    pub(crate) fn new(profile: &Profile, tools: Vec<Value>) -> Self {
        let (lite, chat) = (profile.code_mode, chat(profile));
        let mut input = Vec::new();
        if lite {
            input.push(addition(tools.clone(), true));
        }
        if !profile.prompt.is_empty() && chat.is_some() {
            input.push(json!({"role": "system", "content": profile.prompt}));
        } else if !profile.prompt.is_empty() {
            let text = json!([{"type": "input_text", "text": profile.prompt}]);
            input.push(json!({"type": "message", "role": "developer", "content": text}));
        }
        let tools = if lite { Vec::new() } else { tools };
        let prefix = json!({"type": "razor_prefix", "input": input, "tools": tools, "version": profile.version});
        let pending = vec![prefix.clone()];
        Self {
            prefix,
            items: Vec::new(),
            pending,
            lite,
            chat,
        }
    }

    /// Continues a recorded conversation exactly: its descriptor first, then its wire items. A
    /// changed profile version needs a rebuild instead.
    pub(crate) fn resume(profile: &Profile, items: Vec<Value>) -> Result<Self, Error> {
        let mut items = items.into_iter();
        let prefix = items
            .next()
            .filter(|p| p["type"] == "razor_prefix" && p["version"] == profile.version.as_str());
        let prefix = prefix.ok_or_else(|| {
            Error::Invalid(
                "The recorded native conversation does not match this profile; rebuild the context"
                    .into(),
            )
        })?;
        let (items, pending, lite) = (items.collect(), Vec::new(), profile.code_mode);
        let mut conversation = Self {
            prefix,
            items,
            pending,
            lite,
            chat: chat(profile),
        };
        // It resumes between turns, so every turn's videos have been seen.
        conversation.spend(profile);
        Ok(conversation)
    }

    /// The descriptor, then every wire item: what `Resume::Responses` carries.
    pub(crate) fn snapshot(&self) -> Vec<Value> {
        std::iter::once(&self.prefix)
            .chain(&self.items)
            .cloned()
            .collect()
    }

    /// Appends the declarations whose bytes differ from the latest ones this conversation sent.
    pub(crate) fn offer(&mut self, profile: &Profile) {
        let prefix = self.prefix["input"].as_array().into_iter().flatten();
        let added = (prefix.chain(&self.items)).filter(|item| {
            matches!(
                item["type"].as_str(),
                Some("additional_tools" | "razor_tools")
            )
        });
        let added = added.flat_map(|item| item["tools"].as_array().into_iter().flatten());
        let added = added.flat_map(|tool| match tool["type"] == "namespace" {
            true => tool["tools"].as_array().map_or(&[][..], Vec::as_slice),
            false => std::slice::from_ref(tool),
        });
        let top = self.prefix["tools"].as_array().into_iter().flatten();
        let latest: BTreeMap<_, _> = top
            .chain(added)
            .filter_map(|t| Some((named(t)?, t)))
            .collect();
        let changed: Vec<Value> = (profile.tools.iter())
            .filter(|t| named(t).is_some_and(|name| latest.get(name) != Some(t)))
            .cloned()
            .collect();
        if changed.is_empty() {
            return;
        }
        // Chat has no way to add a tool after the prefix: `body` folds these into `tools`.
        let item = match self.chat {
            Some(_) => json!({"type": "razor_tools", "tools": changed}),
            None => addition(changed, self.lite),
        };
        self.items.push(item.clone());
        self.pending.push(item);
    }

    /// Appends a request-side item the task builds in the Responses shape (an input or a call's
    /// output); on chat it is kept as chat sends it. The next response records it.
    pub(crate) fn send(&mut self, item: Value) {
        let item = match self.chat {
            Some(_) => self.chat_item(item),
            None => item,
        };
        self.items.push(item.clone());
        self.pending.push(item);
    }

    /// A user message of `text` and `image_url` parts, or a `tool` message naming its call's
    /// function, from the Responses-shaped item.
    fn chat_item(&self, item: Value) -> Value {
        if item["type"] == "function_call_output" {
            let id = &item["call_id"];
            let calls = self.items.iter().rev();
            let mut calls = calls.flat_map(|m| m["tool_calls"].as_array().into_iter().flatten());
            let call = calls.find(|c| c["id"] == *id);
            let name = call.map_or(Value::Null, |c| c["function"]["name"].clone());
            return json!({"role": "tool", "tool_call_id": id, "name": name, "content": item["output"]});
        }
        let parts = item["content"].as_array().into_iter().flatten();
        let parts = parts.map(|p| match p["type"].as_str() {
            Some("input_image") => {
                json!({"type": "image_url", "image_url": {"url": p["image_url"]}})
            }
            Some("razor_image" | "razor_video") => p.clone(),
            _ => json!({"type": "text", "text": p["text"]}),
        });
        json!({"role": item["role"], "content": parts.collect::<Vec<_>>()})
    }

    /// Appends a completed response's items and returns the additions it was the first to carry.
    /// On chat each message is first made what goes back: `content` is `""` when none streamed,
    /// arguments that are not a JSON object go back as `{}` (the call's recorded input keeps
    /// them), and reasoning moves as the profile's replay says.
    pub(crate) fn response(&mut self, items: &mut [Value]) -> Vec<Value> {
        if let Some((replay, ..)) = self.chat {
            items
                .iter_mut()
                .for_each(|message| outgoing(message, replay));
        }
        self.items.extend_from_slice(items);
        std::mem::take(&mut self.pending)
    }

    /// The latest upload of each video, from the `razor_upload` items `renew` keeps.
    fn uploads(&self) -> HashMap<&str, &Value> {
        let uploads = self.items.iter().filter(|i| i["type"] == "razor_upload");
        uploads
            .filter_map(|u| Some((u["path"].as_str()?, u)))
            .collect()
    }

    /// `shown`, the videos about to be sent, and those still in the requests, less every one
    /// whose latest upload has more than an hour of Google's 48 left.
    pub(crate) fn stale(&self, mut shown: Vec<PathBuf>, now: Millis) -> Vec<PathBuf> {
        let parts = self.items.iter();
        let parts = parts.flat_map(|i| i["content"].as_array().into_iter().flatten());
        let videos = parts.filter(|p| p["type"] == "razor_video");
        shown.extend(videos.filter_map(|p| p["path"].as_str().map(PathBuf::from)));
        let uploads = self.uploads();
        let fresh = |p: &PathBuf| {
            let upload = uploads.get(p.to_str().unwrap_or_default());
            upload.and_then(|u| u["expires"].as_u64()) > Some(now + HOUR)
        };
        shown.retain(|p| !fresh(p));
        shown.sort_unstable();
        shown.dedup();
        shown
    }

    /// Keeps `path`'s upload for this request and every later one: an item the next response
    /// records, so a reopened conversation has it too.
    pub(crate) fn renew(&mut self, path: &Path, uri: &str, expires: Millis) {
        self.upload(json!({"type": "razor_upload", "path": path, "uri": uri, "expires": expires}));
    }

    /// Keeps why `path` could not be uploaded, as `renew` keeps an upload; `stale` tries it again.
    pub(crate) fn unsent(&mut self, path: &Path, error: &str) {
        self.upload(json!({"type": "razor_upload", "path": path, "error": error}));
    }

    fn upload(&mut self, item: Value) {
        self.items.push(item.clone());
        self.pending.push(item);
    }

    /// Ends the videos a finished turn saw: each one not kept for the conversation's life
    /// becomes a line saying it is no longer shown.
    pub(crate) fn spend(&mut self, profile: &Profile) {
        let parts = self.items.iter_mut().filter_map(|i| i.get_mut("content"));
        let parts = parts.flat_map(|p| p.as_array_mut().into_iter().flatten());
        for part in parts.filter(|p| p["type"] == "razor_video" && p["keep"] != true) {
            let path = part["path"].as_str().unwrap_or_default();
            let text = profile.say("video_spent", &[("path", path)]);
            *part = json!({"type": "text", "text": text});
        }
    }

    /// The data URLs of the latest `count` images this conversation sent, oldest first: what
    /// `num_last_images_to_include` names. An image whose file under `dir` can no longer be read
    /// is left out, so fewer come back, as when fewer were sent.
    pub(crate) async fn images(&self, count: usize, dir: &Path) -> Vec<String> {
        let parts = (self.items.iter().rev())
            .flat_map(|item| item["content"].as_array().into_iter().flatten().rev());
        let parts =
            parts.filter(|p| matches!(p["type"].as_str(), Some("razor_image" | "input_image")));
        let mut urls = Vec::new();
        for part in parts.take(count) {
            urls.extend(match part["image_url"].as_str() {
                Some(url) => Some(url.to_owned()),
                None => data_url(part, dir).await,
            });
        }
        urls.reverse();
        urls
    }

    /// What a `web.run` call sends as its recent input, as Codex does: the last two user
    /// messages and the assistant text between them, text only, each at most its last 4,000
    /// characters.
    pub(crate) fn recent(&self) -> Vec<Value> {
        let text = |item: &Value| {
            let user = match (item["type"].as_str(), item["role"].as_str()) {
                (Some("message"), Some(role @ ("user" | "assistant"))) => role == "user",
                _ => return None,
            };
            let parts = item["content"]
                .as_array()?
                .iter()
                .filter_map(|c| c["text"].as_str());
            let text = parts.collect::<Vec<_>>().join("\n");
            (!text.is_empty()).then_some((user, text))
        };
        let (mut recent, mut users) = (Vec::new(), 0);
        for (user, text) in self.items.iter().rev().filter_map(text) {
            if !user && users == 0 {
                continue;
            }
            users += usize::from(user);
            let text: String = text
                .chars()
                .skip(text.chars().count().saturating_sub(4000))
                .collect();
            let (role, kind) = if user {
                ("user", "input_text")
            } else {
                ("assistant", "output_text")
            };
            recent.push(
                json!({"type": "message", "role": role, "content": [{"type": kind, "text": text}]}),
            );
            if users == 2 {
                break;
            }
        }
        recent.reverse();
        recent
    }

    /// The whole request: stateless (`store: false`), streamed, with encrypted reasoning kept. On
    /// chat: the system message, then every message as kept, each assistant message whole, and
    /// the declarations with later ones replacing same-named earlier ones in place. Each image
    /// is read from its file under `dir` here, one message's at a time, and never kept; each
    /// video is its latest upload's `uri`.
    pub(crate) async fn body(
        &self,
        profile: &Profile,
        choice: crate::protocol::ModelSettings,
        session: &SessionId,
        dir: &Path,
    ) -> Vec<u8> {
        let crate::protocol::ModelSettings { effort, speed } = choice;
        let model = profile
            .endpoint
            .speed_ids
            .get(&speed)
            .filter(|_| speed != Speed::Standard)
            .unwrap_or(&profile.endpoint.id)
            .as_str();
        let prefix = self.prefix["input"].as_array().into_iter().flatten();
        let (mut body, key, items) = match &self.chat {
            Some((_, top, request)) => {
                let (body, messages) = self.chat_body(model, effort.min(*top), request);
                (body, "messages", messages)
            }
            None => (
                self.envelope(model, effort, session),
                "input",
                prefix.chain(&self.items).collect(),
            ),
        };
        let uploads = self.uploads();
        // The envelope is a nonempty object; its items are written after its last field.
        body.pop();
        let tier = match (speed, &self.chat) {
            (Speed::Fast, None) => Some(if profile.endpoint.public() {
                "fast"
            } else {
                "priority"
            }),
            (Speed::Ultrafast, None) => Some("ultrafast"),
            (Speed::Fast, Some((_, _, r)))
                if profile.endpoint.id.starts_with("gemini-")
                    && !r.contains_key("service_tier") =>
            {
                Some("priority")
            }
            _ => None,
        };
        if let Some(tier) = tier {
            body.extend_from_slice(format!(r#","service_tier":"{tier}""#).as_bytes());
        }
        body.extend_from_slice(format!(r#","{key}":["#).as_bytes());
        for (n, item) in items.into_iter().enumerate() {
            if n > 0 {
                body.push(b',');
            }
            let shown = self.shown(item, profile, dir, &uploads).await;
            // Plain JSON values with string keys always serialize.
            let _ = serde_json::to_writer(&mut body, shown.as_ref().unwrap_or(item));
        }
        body.extend_from_slice(b"]}");
        body
    }

    /// The item with each `razor_image` part as the image its file holds now, or a line saying it
    /// can no longer be read, and each `razor_video` part as its upload, or a line saying why it
    /// could not be sent; `None` when it has no such part.
    async fn shown(
        &self,
        item: &Value,
        profile: &Profile,
        dir: &Path,
        uploads: &HashMap<&str, &Value>,
    ) -> Option<Value> {
        let media = |p: &Value| matches!(p["type"].as_str(), Some("razor_image" | "razor_video"));
        item["content"].as_array()?.iter().find(|p| media(p))?;
        let (mut item, chat) = (item.clone(), self.chat.is_some());
        for part in item["content"].as_array_mut().into_iter().flatten() {
            if part["type"] == "razor_video" {
                let path = part["path"].as_str().unwrap_or_default();
                let upload = uploads.get(path);
                *part = match upload.and_then(|u| u["uri"].as_str()) {
                    Some(uri) => json!({"type": "image_url", "image_url": {"url": uri}}),
                    None => {
                        let error = upload.and_then(|u| u["error"].as_str()).unwrap_or_default();
                        let text = profile.say("video_unsent", &[("path", path), ("error", error)]);
                        json!({"type": "text", "text": text})
                    }
                };
                continue;
            }
            if part["type"] != "razor_image" {
                continue;
            }
            *part = match data_url(part, dir).await {
                Some(url) if chat => json!({"type": "image_url", "image_url": {"url": url}}),
                Some(url) => json!({"type": "input_image", "image_url": url}),
                None => {
                    let path = dir.join(part["path"].as_str().unwrap_or_default());
                    let text = profile.say("image_gone", &[("path", &path.to_string_lossy())]);
                    json!({"type": if chat { "text" } else { "input_text" }, "text": text})
                }
            };
        }
        Some(item)
    }

    /// The Responses request without its input.
    fn envelope(&self, model: &str, effort: Effort, session: &SessionId) -> Vec<u8> {
        #[derive(Serialize)]
        struct Body<'a> {
            model: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            tools: Option<&'a Value>,
            tool_choice: &'a str,
            parallel_tool_calls: bool,
            reasoning: Value,
            store: bool,
            stream: bool,
            include: [&'a str; 1],
            prompt_cache_key: &'a str,
        }
        let mut reasoning = json!({"effort": effort, "summary": "auto"});
        if self.lite {
            reasoning["context"] = json!("all_turns");
        }
        let body = Body {
            model,
            tools: (!self.lite).then_some(&self.prefix["tools"]),
            tool_choice: "auto",
            parallel_tool_calls: !self.lite,
            reasoning,
            store: false,
            stream: true,
            include: ["reasoning.encrypted_content"],
            prompt_cache_key: session.as_str(),
        };
        // Plain JSON values with string keys always serialize.
        serde_json::to_vec(&body).unwrap_or_default()
    }

    /// The chat request without its messages, and the messages.
    fn chat_body(
        &self,
        model: &str,
        effort: Effort,
        request: &Map<String, Value>,
    ) -> (Vec<u8>, Vec<&Value>) {
        #[derive(Serialize)]
        struct Body<'a> {
            model: &'a str,
            tools: Vec<&'a Value>,
            reasoning_effort: Effort,
            stream: bool,
            stream_options: Value,
            #[serde(flatten)]
            request: &'a Map<String, Value>,
        }
        let mut tools: Vec<&Value> = self.prefix["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .collect();
        let mut messages: Vec<&Value> = self.prefix["input"]
            .as_array()
            .into_iter()
            .flatten()
            .collect();
        for item in self.items.iter().filter(|i| i["type"] != "razor_upload") {
            if item["type"] != "razor_tools" {
                messages.push(item);
                continue;
            }
            for tool in item["tools"].as_array().into_iter().flatten() {
                match tools.iter_mut().find(|t| named(t) == named(tool)) {
                    Some(slot) => *slot = tool,
                    None => tools.push(tool),
                }
            }
        }
        let body = Body {
            model,
            tools,
            reasoning_effort: effort,
            stream: true,
            stream_options: json!({"include_usage": true}),
            request,
        };
        // Plain JSON values with string keys always serialize.
        (serde_json::to_vec(&body).unwrap_or_default(), messages)
    }
}

/// A `razor_image` part's file as a data URL: only a relative path of plain names that resolves,
/// symlinks followed, inside the session directory `dir`, and at most `IMAGE` bytes.
async fn data_url(part: &Value, dir: &Path) -> Option<String> {
    let path = Path::new(part["path"].as_str()?);
    if !path.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let file = tokio::fs::canonicalize(dir.join(path)).await.ok()?;
    if !file.starts_with(tokio::fs::canonicalize(dir).await.ok()?) {
        return None;
    }
    let mut bytes = Vec::new();
    let file = tokio::fs::File::open(file).await.ok()?;
    let read = file.take(IMAGE + 1).read_to_end(&mut bytes).await.ok()?;
    let media = part["media_type"].as_str().unwrap_or_default();
    (read as u64 <= IMAGE).then(|| format!("data:{media};base64,{}", STANDARD.encode(&bytes)))
}

/// Makes a streamed chat message what goes back (`Conversation::response`).
fn outgoing(message: &mut Value, replay: Replay) {
    if message["content"].is_null() {
        message["content"] = json!("");
    }
    let calls = message.get_mut("tool_calls").and_then(Value::as_array_mut);
    for call in calls.into_iter().flatten() {
        let arguments = &mut call["function"]["arguments"];
        let object = arguments
            .as_str()
            .and_then(|a| serde_json::from_str::<Value>(a).ok());
        if !object.is_some_and(|o| o.is_object()) {
            *arguments = json!("{}");
        }
    }
    let Some(fields) = message.as_object_mut() else {
        return;
    };
    let to = match replay {
        Replay::Streamed => return,
        Replay::Reasoning => Some("reasoning"),
        Replay::ReasoningContent => Some("reasoning_content"),
        Replay::None => None,
    };
    let text = fields
        .remove("reasoning_content")
        .or_else(|| fields.remove("reasoning"));
    fields.remove("reasoning");
    if let (Some(to), Some(text)) = (to, text) {
        fields.insert(to.into(), text);
    }
}

fn addition(tools: Vec<Value>, lite: bool) -> Value {
    let tools = match lite {
        true => {
            json!([{"type": "namespace", "name": "functions", "description": "Tool functions.", "tools": tools}])
        }
        false => json!(tools),
    };
    json!({"type": "additional_tools", "role": "developer", "tools": tools})
}
