# Web

`src/tools/web/mod.rs`: 259 lines (tool service: 6,000), plus 51 seam lines in `src/tools/mod.rs`, `src/profiles/`, `src/config/mod.rs` and Start's version in `src/controller/surface.rs`. Uses existing `reqwest`.

## Surface

```rust
/// A provider, named only in configuration.
pub enum Provider { Exa, Firecrawl, Parallel }
/// [web]: each verb's providers in the order they are tried, empty by default.
#[derive(Clone, Default, PartialEq, Deserialize)]
pub struct Config {
    pub search: Vec<Provider>, pub fetch: Vec<Provider>, pub answer: Vec<Provider>,
    /// [web.<provider>]; a provider without one is reached at its public endpoint.
    #[serde(flatten)] pub connections: BTreeMap<Provider, Connection>,
}
/// The key is never config: it is the provider's own EXA_API_KEY, FIRECRAWL_API_KEY or
/// PARALLEL_API_KEY, read at each use.
pub struct Connection { pub base_url: String }
impl Config {
    /// The verbs with a listed provider whose key is set, in the order search, fetch, answer.
    pub fn actions(&self) -> impl Iterator<Item = tools::Action> + '_;
}
```

Config loading refuses a provider named twice in one list, Firecrawl in `answer`, an unknown provider or `[web]` key, and a `base_url` that is not HTTP or HTTPS. The verbs are plain read-only actions named `search`, `fetch` and `answer`; `Tools::run(at, name, input)` stays the execution surface.

| Read-only action | Input | Guarantee |
| --- | --- | --- |
| `search` | `{query}` | Returns up to ten titles, URLs and excerpts. |
| `fetch` | `{url, find?}` | Returns the URL and extracted text; literal `find` selects matching lines and neighbours; no match returns that fact, without fallback. |
| `answer` | `{query}` | Returns the provider's answer and citations, never a Razor synthesis. |

Output: `{output, path}`, provider name first, kept first after a spill. Exhaustion: `tools::Error::Refused`, the last provider's name, then one sentence listing each provider's failure and suggesting another URL or configuration. No result or refusal holds a key, an authentication header or provider error text.

## Data flow

1. `src/config/mod.rs` reads `[web]` into `Config::tools()` at admission.
2. `Config::actions()` feeds `Tools::configure`, which puts the verbs after the other built-ins.
3. The profiles declare these verbs beside the vendor's own web tools, which are always there (below, 2026-10-29): Claude Code's `WebSearch` and `WebFetch`, plain GPT's hosted `web_search`, GPT code mode's `web__run`.
4. A verb configured or dropped mid-session joins or leaves through the addition path, as an MCP tool does: it is declared at the next turn without a rebuild, and a dropped one is refused by name until the next rebuild takes it off the list. Before 5 November `profiles::load` put `-web-` and one digit per verb in `Profile.version`, so a toggle rebuilt every open conversation and could ask the model for a note saying it was switching models; a profile version now names only the shipped data, the route and the code-mode choice. Claude code mode is the exception: Claude Code keeps the request's tool list it opened with and replaces a definition in place only for a tool it first announced inline, so a changed `exec` never reaches the model there (a verb turned on was callable in a cell but never declared). On that route the version still names the verbs, `-web-` and one digit each, and a toggle rebuilds.
5. One HTTP client per process: no redirects (an `x-api-key` would follow one), 90 seconds per call.
6. Each attempt holds one response, at most 4 MiB.
7. Exa: `/search`, `/contents`, `/answer`; Firecrawl: `/v2/search`, `/v2/scrape`; Parallel: `/v1/search`, `/v1/extract`, `/v1/responses`.
8. Any attempt error (no key, no 2xx, a cut or oversized body, not JSON), a reported failure or empty text advances.
9. Text comes from the one field each provider documents: Exa `text`, `answer` and `citations`; Firecrawl `description`, `markdown`; Parallel `excerpts`, `full_content`, and the output message's `text` and `annotations`. Search excerpts are bounded in the request, so ten stay under SPILL.
10. After a success, `find` applies, every key is redacted, then the spill.

## Decision: use failure fields, not guesses

[Exa](https://docs.exa.ai/reference/get-contents): errors and `statuses[].status`; [Firecrawl](https://docs.firecrawl.dev/api-reference/endpoint/scrape): `success: false` or `data.metadata.statusCode >= 400`; [Parallel](https://docs.parallel.ai/api-reference/extract): `errors[]`, responses `status: failed|incomplete`. These and empty text trigger fallback. No provider uses another's field, so one test of all of them serves all three. Nonempty error pages and JavaScript shells pass through: no keywords, length or filename guesses.

All twelve probes succeeded (normal page, FlightAware, JavaScript app, PDF per provider). Exa returned 383 app characters versus Firecrawl's 12,827 without signalling failure; its correct PDF extraction was 16 characters. Length guesses discard valid text; semantic judging adds model cost and uncertainty. Flow 29 labels synthetic failures with their source URL.

## The vendors' own web tools, always (built 2026-10-29)

Trey, 2026-10-29: "[...] From now on, my vision here is we let the subscriptions pay for web search. [...] Claude models automatically get the, whatever the default Claude Code, like web search, web fetch tools are. And GPT models automatically get whatever the default GPT web search tools are. [...] And then for any other model family, it can just keep working the way it does right now."

- **Claude**, both modes: `WebSearch` and `WebFetch` are always declared (today only when `[web]` names no `search` or `fetch`). They run inside Claude Code, on the subscription.
- **Plain GPT**: the hosted `{"type": "web_search"}` always (today only without `search`).
- **GPT code mode**: `web__run`, Codex's client-run search on the ChatGPT sign-in (docs/design/responses.md).
- **This module is unchanged.** Its verbs keep their readers and their flows; a profile declares them when `[web]` names a provider whose key is set, as today.

### Decision: the `[web]` verbs stay beside the vendor's tools when configured

| Option | What the model sees on Claude and GPT | Cost |
| --- | --- | --- |
| **Configured verbs stay** (chosen) | The vendor's web tools, plus `search`, `fetch` and `answer` only where `[web]` names a provider with a key | Two deletions in `src/profiles/` and nothing here; flow 29 keeps its route. The spend stops only when the person's config stops naming providers: A pack that lists Exa first for `search` keeps the spend, so such a pack's `[web]` table has to go in the same landing. |
| Never on Claude or GPT | Only the vendor's web tools | Matches the family split in Trey's words with no config edit. But no shipped family would offer the verbs until the generic family is chosen, so this module's 265 counted lines and flow 29's provider half would have no route: unproven code that RULES.md 19 counts against, or a deletion to restore later. |

With the verbs configured, a model sees two ways to search. Trey ruled on 2026-10-29 which comes first: "basically first pass and routine stuff always native tools", and Exa or Firecrawl only when those do not give the model what it needs or it needs a second, verifying search. Each verb's own description (`src/profiles/data/tools.json`) says so: search and fetch with the native web tool first and for routine work, and use the verb when that did not find what is needed, a page refused it, or a claim needs a second, independent check. The descriptions are declared exactly when the verbs are, so the rule never names a tool the model lacks. Flow 29 checks the wording on Claude.
