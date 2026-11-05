//! Display facts decoded once when a tool result arrives.
use super::*;
use crate::protocol::Image;

/// A result's display copy, decoded once on arrival and never during a paint: Razor's read and
/// shell results, or Claude Code's numbered Read text. Evicted with its result; skill metadata
/// has explicit field caps.
#[derive(Default)]
pub(in crate::terminal) struct Decoded {
    pub(in crate::terminal) skill: Option<skill::Info>,
    pub(super) text: String,
    pub(super) first: Option<u64>,
    pub(super) count: usize,
    pub(super) complete: bool,
    pub(super) job: Option<JobId>,
    pub(super) end: Option<JobEnd>,
    // The pictures the result names, each drawn as a line under its call.
    pub(super) images: Vec<Image>,
    // Where a shell or MCP result's whole output was saved when it ran past what it held.
    pub(super) path: Option<String>,
}
impl Decoded {
    pub(in crate::terminal) fn read(raw: &str) -> Option<Self> {
        let Ok(mut value) = serde_json::from_str::<Value>(raw) else {
            let (mut first, mut lines) = (None, Vec::new());
            for line in raw.lines() {
                let (number, text) = line.split_once('\t')?;
                let number: u64 = number.trim().parse().ok()?;
                if number != *first.get_or_insert(number) + lines.len() as u64 {
                    return None;
                }
                lines.push(text);
            }
            let (text, count) = (lines.join("\n"), lines.len());
            return Some(Self {
                text,
                first,
                count,
                ..Self::default()
            });
        };
        // Any tool may print JSON; only an object is one of the shapes above, and indexing
        // anything else by key would panic.
        if !value.is_object() {
            return None;
        }
        let skill = skill::Info::read(&value);
        let first = value["first"].as_u64();
        let shell = value.get("output").is_some();
        let path = (value["path"].as_str().filter(|_| shell)).map(String::from);
        let complete = if shell {
            value["path"].is_null()
        } else {
            value["next"].is_null()
        };
        let job = value["job"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(JobId);
        let end = serde_json::from_value(value["end"].take()).ok();
        let images: Vec<Image> = serde_json::from_value(value["images"].take()).unwrap_or_default();
        let (text, count) = match value["output"].take() {
            Value::String(text) => {
                let count = text.lines().count();
                (text, count)
            }
            _ if !images.is_empty() => Default::default(),
            _ => {
                let lines = value["lines"].as_array()?;
                let text: Vec<_> = lines.iter().filter_map(Value::as_str).collect();
                (text.join("\n"), lines.len())
            }
        };
        Some(Self {
            skill,
            text,
            first,
            count,
            complete,
            job,
            end,
            images,
            path,
        })
    }
}
