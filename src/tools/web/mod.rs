//! The web verbs (docs/design/web.md): the model names a verb, never a provider. Razor tries
//! the verb's providers in order and moves on only when one reports a failure in its own
//! fields or extracts no text.

use super::{Action, Error, Kind, Tools};
use crate::protocol::EntryId;
use reqwest::{Client, redirect::Policy};
use serde::{Deserialize, de::Error as _};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::LazyLock, time::Duration};

/// A provider, named only in configuration.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Provider {
    Exa,
    Firecrawl,
    Parallel,
}

/// `[web]`: each verb's providers in the order they are tried, empty by default. Config loading
/// refuses a provider named twice in one list, and Firecrawl in `answer`.
#[derive(Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub(crate) struct Config {
    pub(crate) search: Vec<Provider>,
    pub(crate) fetch: Vec<Provider>,
    pub(crate) answer: Vec<Provider>,
    /// `[web.<provider>]`; a provider without one is reached at its public endpoint.
    #[serde(flatten)]
    connections: BTreeMap<Provider, Connection>,
}

/// Where a provider is reached. Its key is never configuration: it is read, at each use, from
/// the provider's own environment variable (`EXA_API_KEY`, `FIRECRAWL_API_KEY`,
/// `PARALLEL_API_KEY`).
#[derive(Clone, PartialEq, Deserialize)]
pub(super) struct Connection {
    /// An HTTP or HTTPS URL; config loading refuses anything else.
    #[serde(deserialize_with = "endpoint")]
    base_url: String,
}

fn endpoint<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let url = String::deserialize(d)?;
    match reqwest::Url::parse(&url) {
        Ok(u) if matches!(u.scheme(), "http" | "https") => Ok(url),
        _ => Err(D::Error::custom("a [web] base_url must be HTTP or HTTPS")),
    }
}

impl Provider {
    /// Its one-word name, its key's environment variable and its public endpoint.
    fn facts(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Exa => ("exa", "EXA_API_KEY", "https://api.exa.ai"),
            Self::Firecrawl => (
                "firecrawl",
                "FIRECRAWL_API_KEY",
                "https://api.firecrawl.dev",
            ),
            Self::Parallel => ("parallel", "PARALLEL_API_KEY", "https://api.parallel.ai"),
        }
    }

    fn key(self) -> Option<String> {
        std::env::var(self.facts().1)
            .ok()
            .filter(|key| !key.trim().is_empty())
    }
}

/// Each provider's key variable and whether it is set, for `razor doctor`; never a value.
pub fn keys() -> impl Iterator<Item = (&'static str, bool)> {
    let providers = [Provider::Exa, Provider::Firecrawl, Provider::Parallel];
    providers
        .map(|p| (p.facts().1, p.key().is_some()))
        .into_iter()
}

impl Config {
    /// The verbs with a listed provider whose key is set, in the order search, fetch, answer.
    pub(crate) fn actions(&self) -> impl Iterator<Item = Action> + '_ {
        ["search", "fetch", "answer"]
            .into_iter()
            .filter(|verb| self.providers(verb).iter().any(|p| p.key().is_some()))
            .map(|verb| Action {
                name: verb.into(),
                read_only: true,
                schema: None,
                kind: Kind::Web,
            })
    }

    fn providers(&self, verb: &str) -> &[Provider] {
        match verb {
            "search" => &self.search,
            "fetch" => &self.fetch,
            _ => &self.answer,
        }
    }
}

/// Runs `search`, `fetch` or `answer`. The first provider to succeed gives `{output, path}`,
/// its name the first line of both the output and any spill file; when all fail, the refusal
/// names the last one first, then each one's failure. No result or refusal holds a key.
pub(super) async fn run(
    tools: &Tools,
    at: EntryId,
    verb: &str,
    input: Value,
) -> Result<Value, Error> {
    let field = if verb == "fetch" { "url" } else { "query" };
    let Some(value) = input[field].as_str().filter(|v| !v.trim().is_empty()) else {
        return Err(Error::Refused(format!("{verb} needs a `{field}`.")));
    };
    let config = tools.state().config.web.clone();
    let mut failures = Vec::new(); // At most three: a list names each provider once.
    for &provider in config.providers(verb) {
        let (name, _, public) = provider.facts();
        let base = config
            .connections
            .get(&provider)
            .map_or(public, |c| &c.base_url);
        match attempt(provider, verb, base, value).await {
            Ok(text) => {
                let text = match input["find"].as_str() {
                    Some(find) if verb == "fetch" => matching(&text, find),
                    _ => text,
                };
                let (mut output, path) = tools.spill(at, redact(format!("{name}\n{text}"))).await?;
                if path.is_some() {
                    // A spill keeps the text's end; the provider still comes first.
                    output = format!("{name}\n{output}");
                }
                return Ok(json!({"output": output, "path": path}));
            }
            Err(reason) => failures.push(format!("{name}: {reason}")),
        }
    }
    let last = config.providers(verb).last().map_or("web", |p| p.facts().0);
    Err(Error::Refused(format!(
        "{last}\nWeb failed ({}); try another URL or change [web].",
        failures.join(", ")
    )))
}

/// One provider's call: its text, or its failure as a fixed phrase, never provider text.
async fn attempt(
    provider: Provider,
    verb: &str,
    base: &str,
    value: &str,
) -> Result<String, &'static str> {
    // One client, so connections are reused; no redirects, so a key never follows one.
    static CLIENT: LazyLock<Option<Client>> = LazyLock::new(|| {
        let client = Client::builder().redirect(Policy::none());
        client.timeout(Duration::from_secs(90)).build().ok()
    });
    let client = CLIENT.as_ref().ok_or("no HTTP client")?;
    let key = provider.key().ok_or("no key")?;
    let (path, body) = request(provider, verb, value);
    let request = client
        .post(format!("{}{path}", base.trim_end_matches('/')))
        .json(&body);
    let request = match provider {
        Provider::Firecrawl => request.bearer_auth(key),
        _ => request.header("x-api-key", key),
    };
    let mut response = request
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|_| "no successful HTTP response")?;
    let mut bytes = Vec::new(); // At most 4 MiB.
    while let Some(chunk) = response.chunk().await.map_err(|_| "response cut short")? {
        if bytes.len() + chunk.len() > 4 << 20 {
            return Err("response over 4 MiB");
        }
        bytes.extend_from_slice(&chunk);
    }
    let data: Value = serde_json::from_slice(&bytes).map_err(|_| "response not JSON")?;
    if failed(&data) {
        return Err("provider reported failure");
    }
    let text = text(provider, verb, &data, value);
    if text.trim().is_empty() {
        return Err("empty text");
    }
    Ok(text)
}

fn request(provider: Provider, verb: &str, value: &str) -> (&'static str, Value) {
    match (provider, verb) {
        (Provider::Exa, "search") => (
            "/search",
            json!({"query": value, "numResults": 10, "contents": {"text": {"maxCharacters": 1200}}}),
        ),
        (Provider::Exa, "fetch") => ("/contents", json!({"urls": [value], "text": true})),
        (Provider::Exa, _) => ("/answer", json!({"query": value, "text": false})),
        (Provider::Firecrawl, "search") => (
            "/v2/search",
            json!({"query": value, "limit": 10, "sources": ["web"]}),
        ),
        // Config loading keeps Firecrawl out of `answer`, so this is always `fetch`.
        (Provider::Firecrawl, _) => (
            "/v2/scrape",
            json!({"url": value, "formats": ["markdown"], "parsers": ["pdf"]}),
        ),
        (Provider::Parallel, "search") => (
            "/v1/search",
            json!({"search_queries": [value], "max_chars_total": 12000}),
        ),
        (Provider::Parallel, "fetch") => (
            "/v1/extract",
            json!({"urls": [value], "advanced_settings": {"full_content": {"max_chars_per_result": 1_000_000}}}),
        ),
        (Provider::Parallel, _) => (
            "/v1/responses",
            json!({"model": "parallel", "input": value, "stream": false}),
        ),
    }
}

/// A failure the provider reports in its own documented fields (docs/design/web.md,
/// "Decision"). No provider uses another's field for anything else, so one test serves all.
fn failed(data: &Value) -> bool {
    !data["error"].is_null()
        || data["errors"].as_array().is_some_and(|e| !e.is_empty())
        || data["success"] == false
        || matches!(data["status"].as_str(), Some("failed" | "incomplete"))
        || list(&data["statuses"]).any(|s| s["status"] != "success")
        || data["data"]["metadata"]["statusCode"]
            .as_u64()
            .is_some_and(|code| code >= 400)
}

/// A success's text from the fields its provider documents; empty when it extracted none.
/// Search excerpts are bounded by the request, so ten results stay under the spill.
fn text(provider: Provider, verb: &str, data: &Value, url: &str) -> String {
    let (items, field): (Vec<&Value>, _) = match (provider, verb) {
        (Provider::Exa, "answer") => (vec![data], "answer"),
        (Provider::Exa, _) => (list(&data["results"]).collect(), "text"),
        (Provider::Firecrawl, "search") => (list(&data["data"]["web"]).collect(), "description"),
        (Provider::Firecrawl, _) => (vec![&data["data"]], "markdown"),
        (Provider::Parallel, "search") => (list(&data["results"]).collect(), "excerpts"),
        (Provider::Parallel, "fetch") => (list(&data["results"]).collect(), "full_content"),
        (Provider::Parallel, _) => {
            let parts = list(&data["output"]).flat_map(|item| list(&item["content"]));
            (parts.collect(), "text")
        }
    };
    let source = |item: &Value| format!("{}\n{}", string(&item["title"]), string(&item["url"]));
    let blocks: Vec<String> = items
        .into_iter()
        .take(10)
        .map(|item| (item, string(&item[field])))
        .filter(|(_, text)| !text.trim().is_empty())
        .map(|(item, text)| {
            let shown = match verb {
                "search" => source(item) + "\n" + &text,
                _ => text,
            };
            // Only an answer has citations: Exa's `citations`, Parallel's `annotations`.
            let cited = list(&item["citations"]).chain(list(&item["annotations"]));
            cited.fold(shown, |shown, citation| shown + "\n" + &source(citation))
        })
        .collect();
    match blocks.join("\n\n") {
        text if verb == "fetch" && !text.is_empty() => format!("{url}\n{text}"),
        text => text,
    }
}

fn list(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

/// A string, or an array's strings one per line; anything else is empty.
fn string(value: &Value) -> String {
    let parts = value.as_str().into_iter();
    let parts = parts.chain(list(value).filter_map(Value::as_str));
    parts.collect::<Vec<_>>().join("\n")
}

/// The lines holding `find` (a literal) with one line either side, or a sentence saying none.
fn matching(text: &str, find: &str) -> String {
    let lines: Vec<&str> = text.lines().collect(); // Bounded by the 4-MiB response.
    let near = |i: usize| {
        lines[i.saturating_sub(1)..lines.len().min(i + 2)]
            .iter()
            .any(|l| l.contains(find))
    };
    let kept: Vec<&str> = (0..lines.len())
        .filter(|&i| near(i))
        .map(|i| lines[i])
        .collect();
    if kept.is_empty() {
        "No lines match find.".into()
    } else {
        kept.join("\n")
    }
}

/// Every provider's key replaced, wherever a provider echoed one back.
fn redact(mut text: String) -> String {
    let providers = [Provider::Exa, Provider::Firecrawl, Provider::Parallel];
    for key in providers.into_iter().filter_map(Provider::key) {
        text = text.replace(&key, "[redacted]");
    }
    text
}
