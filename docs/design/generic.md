# The generic path: Gemini, DeepSeek and GLM

## Per-model limits and overflow recovery

Trey's ruling, “okay, i like that, lets do it”, replaces `[chat."NAME"]` with
`[models."NAME"]`. All models accept a positive `window` in tokens and a finite
`threshold` greater than zero and at most one; connection fields (`base_url`,
`key`, `id`, `request`, `variant`, `replay`) remain exclusive to chat routes,
including the shipped chat rows. A Claude or GPT row with any connection field
refuses with a sentence naming the row and the fields to remove. No migration:
no live config had a chat table yet. The table remains a machine
setting, with the existing whole-row replacement between pack and person.

```toml
[models.local]
base_url = "http://127.0.0.1:1234/v1"
id = "my-loaded-model"
window = 32768
threshold = 0.75

[models.opus]
threshold = 0.8
```

```rust
// config::Chat becomes config::Model; Config::chat becomes Config::models.
pub struct Model { /* existing connection fields */
    pub window: Option<u64>,
    pub threshold: Option<f64>,
}
// profiles/models.rs, re-exported by profiles:
/// Configured window wins over the catalog, then Claude's positive SDK report;
/// configured threshold wins over rebuild.threshold. Reads the live config snapshot;
/// unknown chat windows stay unknown, and Claude's SDK settings are never changed.
pub fn limits(name: &str, config: &Config, reported: Option<u64>) -> (Option<u64>, f64);
impl profiles::Model {
    /// Refuses invalid limits, native connection overrides and unusable chat endpoints.
    pub fn validate(&self, name: &str, configured: &config::Model) -> Result<(), profiles::Error>;
}
// protocol::Trigger, one additional variant:
/// The host rejected the request because it exceeded the model's context window.
Overflow,
```

1. Config parsing resolves GPT aliases in model-table keys once, as it resolves the selected model, and refuses two rows naming the same resolved model.
2. `Model::validate` shares chat endpoint validation and replaces the native-row ban; the shipped-window refusal is deleted, with window validation shared across routes.
3. `limits` alone resolves effective values from the live config snapshot, re-read at the next admitted turn, with no separate limits cache. Only Claude reports a window, so removing an added model's window cannot leave a stale configured value behind.
4. `Endpoint::window` is deleted. Responses and chat report no window in Context and TurnEnd; their catalog lookup is deleted too. `profiles::limits` alone resolves limits for the controller and API.
5. The controller retains the raw route report and resolves limits at turn start, context readings, turn end and status publication. A Claude window override changes only Razor's threshold and fit checks (and their displayed limit); it never changes SDK options, requests or reports. Unloaded rows and the API model list use the same resolver without an SDK report.
6. Responses and chat preserve `Error::ContextOverflow` as `TurnEnd.reason = "prompt_too_long"`; the private `Task::end` takes an optional reason, with existing callers leaving it absent. No displayed error text is matched.
7. Claude already forwards `SDKResultMessage.terminal_reason`, whose pinned SDK type includes `prompt_too_long` (`sdk.d.ts`, `TerminalReason`, SDK 0.3.312); it enters the same controller branch without bridge changes.
8. A first overflow reaches the controller as a failed native TurnEnd (`success = false`, `reason = "prompt_too_long"`). The record keeps its usage and host sentence as `End::Error`, with no inputs answered, then calls `rebuild(Trigger::Overflow, None)`. The ordinary rebuild carries the latest note, asks, settled calls and live work; it neither asks for a fresh note nor replays completed tools. Held inputs remain held.
9. `Conversation::rebuilt` is the loop guard: if the rebuilt request also overflows, the session pauses with the existing error naming its byte size, without a second rebuild or another request. Existing successful context readings and turn-end rules govern when that guard clears; no retry counter or alternate rebuild path is added.
10. The recovery keeps the session id and drops the rejected native conversation. Its rebuilding entry explains the overflow, and later inputs continue on the fresh history. The provider error sentence says Razor rebuilds and retries once; a failed rebuilt request reports its byte size.

The hard decision is to reuse the existing rebuild and its size guard rather
than request a continuation note or resend the rejected history. A note request
cannot fit that history either.
One extra trigger names the actual cause; `Restart` would falsely tell the model
Razor restarted. Proposed glossary line: **overflow**: a host rejection because
a request exceeds the model's context window; Razor rebuilds once from the
record, and pauses if the rebuilt request is also rejected.

Implementation updates only the authorized Models and Continuity sentences
in SPEC, plus all writers and guidance for the renamed table in the eval runner,
flows and this page.

Checks: extend flow 17 so a per-model threshold fires below the global default,
and a shipped model's configured window governs both readings and handover.
An edit to the model's threshold/window takes effect at the next admitted turn;
on Claude the override wins over a subsequent SDK report without changing what
the SDK sends. Extend flow 46's overflow case to retry in the same session,
retain its note and unanswered ask, omit an old large response, and answer the
next input. A separate case rejects both the original and rebuilt requests:
assert exactly one rebuild, exactly two rejected requests, and a paused session
with an error naming the rebuilt byte size. Cover Responses and Claude with
the same local-host rejection in the existing continuity flows. Red-proof each new behavior against
main, with the same scripted responses and dummy keys; no real model calls.
Run flows 17, 18, 19, 25, 28, 30, 34 and 46 in their own systemd units, fmt,
strict all-target Clippy, size and bridge typecheck.

## Configured providers and code-mode defaults (2026-10-31)

Any name outside the native Claude and GPT catalog uses the generic chat transport;
it needs a row in the person's `~/.razor/config.toml` (or pack). For example, a
local server exposing OpenAI-compatible chat accepts this configuration:

```toml
model = "local"

[models.local]
base_url = "http://127.0.0.1:1234/v1"
id = "my-loaded-model"
window = 32768 # Optional tokens; hand over at this model's threshold or rebuild.threshold.
# key = "LOCAL_MODEL_API_KEY" # Optional environment-variable name for an authenticated host.

[models.local.request]
max_completion_tokens = 4096 # Optional, using the host's own request field.
```

The row's name is Razor's model name; `id` defaults to that name, `key` defaults
to no authentication, replay to `streamed`, and effort is passed unchanged.
There is no assumed context window for an unknown model. Every model may set a
positive `window` and a `threshold` fraction in its `[models."<model>"]` row;
configured limits beat the catalog or SDK report, and `rebuild.threshold` is the
default. Claude's override governs Razor's checks only; the SDK is unchanged.
An overflow rebuilds once from the record using the latest continuation note.
If that request also overflows, the session pauses with its rebuilt byte size.
The four plain file and shell tools (`read`, `write`, `edit`, `bash`) accompany Razor's coordination
tools. A configured variant replaces that default as it does for shipped rows;
shipped Gemini, DeepSeek and GLM keep their evaluated variants. Configured names
also appear in the API's model list. No new price keys: Razor currently records
token usage, and has no price reader; request limits already belong in `request`.

Public surface changes are confined to the existing types: `profiles::model`
returns a shared plain `Chat` row for an unknown name, `Config::profile` requires
its configured address and resolves its fields, and `Profile::code_mode` in
configuration becomes `Option<bool>`. `Endpoint::public() -> bool` guarantees
that the parsed URL's host is `api.openai.com`, shared by profile defaults,
profile refusal and request authentication. `Chat::endpoint(name, config)` shares
host validation between config parsing and profile construction. Model names and
this resolution live in `profiles/models.rs`, keeping the rendering file below
800 physical lines. Other signatures stay unchanged.

An unknown name without an address refuses: “NAME is neither a model Razor ships
nor a configured chat model; check the name (opus, sonnet, haiku, fable, sol, terra,
luna, astra, or a full id from the model list), or add [models."NAME"] with its base_url.” A native Claude or GPT row accepts limits and refuses connection fields, asking for their removal. Invalid URLs keep the
existing HTTP-or-HTTPS sentence. A public API-key GPT route defaults to plain
tools, the signed-in GPT route to code mode, and Claude to plain tools; explicit
settings win. Chat always uses plain tools. Explicit code mode on the public
API route keeps “API-key and chat routes support plain tools only.”

The hard choice is resolving unknown names as chat before config supplies their
address. This preserves the single model resolver used by session start,
rebuild, recap and request completion; profile construction then refuses an
unconfigured route before any model request. Threading config through every
resolver would widen the controller and consume its exhausted line budget.

Checks extend flow 46 with an unknown row's tool round trip and an unauthenticated local request,
and flow 28 with a public API-key turn and explicit refusal, red-proofed against
main. Public-API traffic must stay on a local stand-in with a dummy key.
Run those, 39, and existing config/API flows 25 and 30, plus fmt, clippy and size.

The rulings: Gemini 3.8 Flash at high, DeepSeek V4.1 Flash 1017 at max, GLM 5.3 Flash at high (about 20:10: "so gemini deepseek glm"). DeepSeek and GLM run on RunInfra with Fireworks as the fallback, his host ruling of 2026-10-27 after a benchmark of the hosts that day. Every agent-facing choice gets hill-climbed per model once the path works (about 20:00: "we ship the optimal, most token efficient, most performant and high capability version"). The evaluation starts cheap: "we can get directional evidence high level cheaply, then better evidence w/ more data for the hard calls".

In one sentence: the generic path is the Responses loop's task over a second wire, OpenAI-compatible chat, which sends every assistant message back as it streamed in (with one setting for which field the reasoning goes back under). Everything the model sees is data that an evaluation arm can swap without code.

## The models and their hosts

| Razor's name | Host (default; the shipped row) | Id on the wire | Key | Window | Effort sent |
| --- | --- | --- | --- | ---: | --- |
| `gemini-3.8-flash` | Google, `https://generativelanguage.googleapis.com/v1beta/openai` | `gemini-3.8-flash` | `GEMINI_API_KEY` | 1,048,576 | low, medium, high; Razor's xhigh and max go as high |
| `deepseek-v4.1-flash-1017` | RunInfra, `https://api.runinfra.ai/v1` | `deepseek-v4-1-flash-1017` | `RUNINFRA_API_KEY` | 1,048,576 | as Razor names it; max is sent explicitly |
| `glm-5.3-flash` | RunInfra, `https://api.runinfra.ai/v1` | `glm-5-3-flash` | `RUNINFRA_API_KEY` | 1,048,576 | as Razor names it; RunInfra honours low and high and runs any other value at the model's maximum |

The fallback is Fireworks, `https://api.fireworks.ai/inference/v1` with `FIREWORKS_API_KEY`, ids `accounts/fireworks/models/deepseek-v4p1-flash-1017` and `accounts/fireworks/models/glm-5p3-flash`, usable only after its own probe passes (Hosts, below). The 2026-10-27 benchmark ran both there, and every response's `model` matched the id; Fireworks' own page lists the GLM model at $0.15, $0.03 and $0.50 per million (input, cached, output), and the benchmark's price table had the DeepSeek build at $0.30, $0.006 and $1.20.

What each host's documentation says, read today:

- **Google** (model page, thinking guide, thought-signature page with its OpenAI-compatibility section): 1,048,576 input and 65,536 output tokens; thinking "low, medium, high. Note: `minimal` is not supported and returns an error"; on the compatible endpoint the signature rides on a call as `extra_content.google.thought_signature`, and the first call of each step in the current turn must carry its own, or the request fails with 400. Chat completions is the only OpenAI-shaped route Google documents.
- **RunInfra** (`runinfra.ai/docs`: chat completions, tool calling, streaming, session affinity, prompt caching; the model library): `reasoning_effort` is "the one honored spelling"; `thinking`, `enable_thinking` and `chat_template_kwargs` have no effect or are dropped. For DeepSeek V4 Flash it documents `high` and `xhigh` running as `max`, and no effort as `medium` on a request with tools; it publishes nothing for the 1017 build, and any rewrite it applies comes back in `X-RunInfra-Reasoning-Effort-Applied`. GLM 5.3 Flash honours `low` and `high`; every other value runs at its maximum. "Extra message fields pass through." Each `tool_calls[].function.arguments` "must hold a JSON object", and an assistant message "needs `content` or a non-empty `tool_calls`". `x-session-id` keeps a session's requests near its warm cache. Usage, with the dollar cost in `usage.cost`, comes with `[DONE]` when `stream_options.include_usage` is set. Prices: DeepSeek $0.15, $0.01, $0.60; GLM $0.11, $0.03, $0.45 ($0.05, $0.01, $0.22 until 2026-11-04 07:59 UTC).
- **What RunInfra streams**, which its documentation does not say: OMP's transcripts from the 2026-10-27 benchmark record where each reasoning delta was read (OMP checks `reasoning_content`, then `reasoning`). RunInfra's DeepSeek 1017 and GLM 5.3 Flash streams carried it as `reasoning`; Fireworks' as `reasoning_content`. Whether RunInfra's serving stack puts an echoed `reasoning` back into the model's prompt is documented nowhere. The probe measures it.
- **Z.ai** documents for GLM 5.3 Flash that thinking cannot be turned off, that interleaved thinking between tool calls is on by default and "thinking blocks should be explicitly preserved and returned together with the tool results", and that its standard API drops earlier turns' reasoning unless the request sets `thinking.clear_thinking: false` and the complete, unmodified `reasoning_content` comes back. Its default effort is max; Trey's high is sent explicitly. RunInfra drops `thinking`, so on RunInfra the model's serving default decides.
- **Fireworks** maps DeepSeek V4.1's max to its own 100 and documents a request field, `reasoning_history: "preserved"`, without which its serving template can strip earlier reasoning; it documents that field's default for neither model, nor GLM's effort mapping.

So sending reasoning back is necessary but not sufficient: a host can need a request-side field to keep what it is sent. That field is per-endpoint data (`request`, below), and the probe proves retention by an answer only kept reasoning can give, never by a 200 alone.

## Public surface

```rust
// ---- src/profiles/mod.rs -------------------------------------------------------------------

/// The wire a model is reached over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Claude,
    Responses,
    /// OpenAI-compatible `POST {url}/chat/completions`, streamed.
    Chat(&'static Chat),
}

/// A chat model's shipped row. `[models."<model>"]` in config replaces any of its host fields.
#[derive(Debug, PartialEq, Eq)]
pub struct Chat {
    pub base_url: &'static str,
    /// The name of the environment variable holding the host's key. The value is read for
    /// each request, sent only as the bearer, and never written to the record, a log or an
    /// error: `post` takes it out of any host text that echoes it.
    pub key: &'static str,
    /// The host's id for the model.
    pub id: &'static str,
    /// The highest effort the model takes; a higher one is sent as this.
    pub top: Effort,
    /// Which field previous reasoning goes back under (below); the probe sets each row's.
    pub replay: Replay,
    /// The variant the model gets when config names none: its leader in the evaluation, as
    /// JSON compiled into the binary (Each model's shipped default, below).
    pub variant: &'static str,
}

/// How a chat message's reasoning goes back: as it streamed, moved to `reasoning` or to
/// `reasoning_content`, or not at all (config: `"streamed"`, `"reasoning"`,
/// `"reasoning_content"`, `"none"`).
pub enum Replay { Streamed, Reasoning, ReasoningContent, None }

// A row's `replay` is what the probe proved on that host; a host's history switch (Fireworks'
// `{"reasoning_history": "preserved"}`, Z.ai's `{"thinking": {"type": "enabled",
// "clear_thinking": false}}`) is config's `request` table, which no shipped row needs. A row
// joins only after its probe passes.

/// Where one profile's requests go, as config resolved it at this rendering.
pub struct Endpoint {
    /// The Responses base the server fixed at startup, or the chat host's address.
    pub url: String,
    /// The chat host's key variable; `None` on Responses, which uses Razor's own sign-in or
    /// key file.
    pub key: Option<String>,
    /// The model id the request names: the host's id, or Razor's name on Responses.
    pub id: String,
    pub replay: Replay,
    /// The row's `request` fields, with config's replacing them by name.
    pub request: serde_json::Map<String, Value>,
}

// A variant is plain JSON (Decision 4), read from the file `[models."<model>"] variant` names
// when its config file is read, and again when either file's stat changes; a malformed one
// refuses that config file with one sentence. A configured variant replaces the row's whole;
// the empty variant, `{}`, is the plain profile.

/// Unchanged signature; three rows added, as in the table above.
pub fn model(name: &str) -> Result<Model, Error>;

pub struct Profile {
    // ...the existing public fields...
    pub endpoint: Endpoint,
}

/// On `Chat`: the plain declarations GPT's plain mode already ships (`read`, `write`, `edit`,
/// `bash`, then Razor's own) in chat's `{"type": "function", "function": {…}}` shape, with
/// the variant's entries set over them field by field (config's variant, else the row's;
/// docs/design/profiles.md); the web verbs in
/// wording that names no native web tool; no hosted tools; `razor`'s route part is the
/// variant's `route`, or `tools-generic.md`, then `record.md`. The version names the variant, the host's address and id, and the replay
/// setting, so a score certifies them and changing any of them rebuilds instead of resuming.
/// Code mode on `Chat` is refused.
pub fn load(endpoint: Endpoint, model: &str, transport: Transport, code_mode: bool,
    variant: Option<&Value>, actions: Arc<[tools::Action]>) -> Result<Profile, Error>;

impl Profile {
    /// Unchanged signature. A call to a variant's declaration returns its action's name, with
    /// the arguments the variant's `keys` name renamed to the action's; others pass unchanged.
    pub fn input(&self, name: &str, input: Value) -> Result<(String, Value), Error>;
}

impl Conversation {
    // Signatures unchanged except `response`. On a chat profile:
    // `send` takes the Responses-shaped item the task builds (a user message of `input_text`
    //   and `input_image` parts, or a `function_call_output`) and keeps it as chat sends it: a
    //   user message of `text` and `image_url` parts, or a `tool` message with the call's id
    //   and its function's name.
    // `offer` keeps changed declarations as one `razor_tools` item, record metadata like the
    //   prefix descriptor and never a message; `body` folds it into `tools`, a later
    //   declaration replacing a same-named earlier one in place.
    // `body` is `{model, messages, tools, reasoning_effort, stream, stream_options}`: the
    //   system message, then every message in order, each assistant message as kept.
    /// Keeps a completed response's items and returns the additions it was the first to carry.
    /// On chat it first makes each message what goes back, in place, so the record holds what
    /// is sent: `content` is `""` when the stream sent none; a call whose `arguments` are not a
    /// JSON object goes back as `{}` (its Call entry, made before this, keeps the raw text, and
    /// its result is the error); reasoning goes back as the profile's `Replay` says.
    pub fn response(&mut self, items: &mut [Value]) -> Vec<Value>;
}

// ---- src/responses/ ------------------------------------------------------------------------

impl Responses {
    /// `base_url` is gone: the task reads `profile.endpoint` for each request, so a changed
    /// `[models."<model>"]` reaches the next request, and the Responses base stays what the
    /// server fixed at startup.
    pub fn start(self: &Arc<Self>, dir: PathBuf, profile: watch::Receiver<Arc<Profile>>,
        incoming: mpsc::Receiver<ToBridge>, outgoing: mpsc::Sender<Line>);
}

pub enum Error {
    // ...the existing variants...
    /// The chat host's key variable is unset, or the host answered 401 or 403. Says to set
    /// that variable in the server's environment; never retried.
    Key(String),
    /// The reply stopped at `length` or `content_filter`. Nothing of it is recorded and it is
    /// not retried: the same request would stop the same way.
    Cut(String),
}

// ---- src/responses/chat.rs (new, private to the module) -------------------------------------

/// One streamed chat completion as a `Response` whose one item is the assistant message.
/// Deltas merge into it: `content`, `reasoning`, `reasoning_content` and each call's
/// `function.arguments` are appended; every other non-null field (ids, names,
/// `extra_content`) is set, so a `role` repeated in every chunk is not doubled. A call delta
/// continues the latest call with its `index` unless it has no `index` or names another `id`
/// (Gemini's compatible stream sends whole calls with no `index`); `index` is dropped from the
/// message. Complete only at `[DONE]` after a `finish_reason` of `stop` or `tool_calls`
/// (a stream that closes without `[DONE]` is incomplete, as RunInfra documents), with every call holding a unique id and a name; `length` and
/// `content_filter` are `Cut`; anything else is `Incomplete`, which `post` retries. Usage:
/// cache read is `prompt_tokens_details.cached_tokens`, input is `prompt_tokens` less that,
/// output is `total_tokens` less `prompt_tokens`, so reasoning a host counts outside
/// `completion_tokens` is still counted.
pub(super) async fn read(response: reqwest::Response, delta: &impl Fn(String))
    -> Result<Response, Error>;

// ---- src/config/mod.rs ---------------------------------------------------------------------

/// `[models."<model>"]`, refused in a project's file like `responses.base_url`: `base_url`, `key`
/// (a variable's name, never a value) and `id` replace the shipped row's host; `replay`
/// (`"streamed"`, `"reasoning"`, `"reasoning_content"` or `"none"`) and `request` (a table)
/// replace the row's; `variant` names a variant file. Native Claude and GPT rows accept only
/// `window` and `threshold`; `base_url` must be HTTP or HTTPS. Moving DeepSeek to Fireworks is
/// this one table, once Fireworks' probe has passed with the `request` it proved.
pub models: BTreeMap<String, Model>,
```

Every chat request carries `x-session-id` with the session's id; it is RunInfra's cache-placement header, and the other hosts ignore it.

The evaluation runner (not included in this repository), `<evaluation>/run.py`:

```toml
# <evaluation>/arms.toml: a chat arm names its host, key, prices (dollars per million: input, cache
# read, output) and the model's output ceiling; the runner points the attempt's
# [models."<model>"] base_url at its spend proxy, which forwards to the host, and copies no
# sign-in.
[deepseek]
program = "razor"
model = "deepseek-v4.1-flash-1017"
effort = "max"
base_url = "https://api.runinfra.ai/v1"
id = "deepseek-v4-1-flash-1017"
keys = ["RUNINFRA_API_KEY"]
prices = [0.15, 0.01, 0.60]
max_output = 384000

[deepseek-maker]                 # one experiment: the same arm with a variant
program = "razor"
model = "deepseek-v4.1-flash-1017"
effort = "max"
base_url = "https://api.runinfra.ai/v1"
id = "deepseek-v4-1-flash-1017"
keys = ["RUNINFRA_API_KEY"]
prices = [0.15, 0.01, 0.60]
max_output = 384000
variant = "<evaluation>/variants/deepseek-maker.json"
```

```
python3 <evaluation>/run.py --arm deepseek --set probe      # the probe's five steps; files nothing
python3 <evaluation>/run.py --arm deepseek --spend D …       # no request may take the spend log past D dollars
```

**The spend cap is a hard bound, checked before every request.** A priced arm's requests go through the runner's spend proxy (a loopback HTTP server in `<evaluation>/`, forwarding each request and its streamed reply unchanged). It listens on one fixed port, 27463, so an arm's address, and with it the profile version its score certifies, is the same in every run; a run holds the spend log locked for its whole life, so a second priced run refuses to start and one cap bounds everything the log counts. Before forwarding a request it computes that request's worst case: its body's bytes as input tokens (a token is at least one byte) at the input price, plus the arm's `max_output` at the output price. When the spend log's total, plus the worst cases of requests in flight, plus this one would pass `--spend D`, it answers 402 with one sentence instead, and the attempt ends there; the runner then stops with that sentence. Every request is charged, retries, cut and incomplete replies included: at the host's own `usage.cost` where the reply gives one (RunInfra's), else at the arm's prices times the reply's usage, else at the worst case when no usage came back. Each request appends one line to the runner's spend log, `spend.jsonl` (time, arm, task, attempt, HTTP status, input, cache read and output tokens, dollars, and whether the dollars are the worst case). The bound is the cap; the log's total is the spend. A chat arm's score names its transport `chat`, and its certification holds the host and id through the profile's version.

## Data flow

1. `profiles::model` returns `Chat(&row)` for the three names; the controller treats `Chat` as it treats `Responses`: resume from the record's native entries, or start the same task.
2. `config.profile` applies `[models."<model>"]` to the row and `load` renders the profile: its `Endpoint`, its declarations (the variant's where it gives them) and its route text.
3. The task is the Responses task, unchanged in its turn logic: queued inputs, one request at a time, each call recorded, then admitted, then run by the controller, results answered in call order, one result for every call, stop and settle.
4. A request: `Conversation::body`, then `POST {url}/chat/completions` with the key from the server's environment as the bearer and `x-session-id`. `post`'s retry budget is the Responses one: four for a failed send or a 5xx, five for an incomplete stream, none for 429.
5. `chat::read` assembles the message through the byte framer the Responses stream uses, which moves out of `stream::read` into one function both readers call. Nothing runs before the message completes.
6. The task makes the entries from the message as it streamed: Thinking from `reasoning` or `reasoning_content`, Text from `content`, one Call per tool call with its arguments decoded from JSON text; malformed arguments stay verbatim and become that call's own error result.
7. Then `Conversation::response` makes the message what goes back, and the task records `Body::Response { usage }` with `Native::Responses { additions, items: [message] }`.
8. Each result: the task builds the `function_call_output` it builds today, and `send` keeps it as a `tool` message. Images a tool result names go with the next request, each after its path, ahead of inputs admitted mid-turn, in one user message after all of the batch's tool messages (tools.md, Seeing an image file).
9. A restart resumes from the descriptor, additions and messages in the record; a model switch or a changed profile version (variant, host, replay) rebuilds.
10. A rebuild opens a fresh conversation. No reasoning or signature crosses it, and none is ever made up.

## Decision 1, the hard one: one transport, not a native Gemini adapter beside it

| Option | Added lines | What it keeps and loses |
| --- | ---: | --- |
| **One chat wire for all three** (chosen) | about 330 | Keeps Gemini's validated rule (the call signatures) and the three hosts' reasoning text by one rule: send each assistant message back whole. Loses, on Gemini only: cache reads and thought tokens may not be reported (forum reports say the compatible usage holds prompt, completion and total only; the probe settles it); thought summaries are not requested; a signature on a text-only reply, which Google recommends returning but does not validate, is kept only if the compatible stream carries one; images in tool results travel in a following user message. |
| A native Gemini adapter beside chat | about 530 | Keeps cached and thought token counts, text-part signatures, summaries, images inside function responses. Costs a second request shape (`contents`, `parts`, `systemInstruction`, `functionDeclarations`, `thinkingConfig`), a second stream reader (`candidates`, `usageMetadata`), function responses with id and name, and a third shape in `Conversation`. And Google's 3.8 Flash guide now leads with the Interactions API, which keeps conversation state on Google's side by default, while its `generateContent` pages sit apart as the older API (one is titled "Generate Content API (Legacy)"). Native today means building on the older API or the new one. |
| DeepSeek and GLM over the Responses loop (RunInfra also serves `/v1/responses`) | about 40 more | Gemini still needs chat, so this adds a route instead of removing one, and RunInfra documents its reasoning control for chat; running two hosts' three models over chat is what proves chat for Kimi, Qwen and the rest. |

The design leaned to one loop unless the compatible endpoint loses something the native one keeps. It does lose measurement on Gemini (possibly cache reads) and a signature Google recommends but does not require; neither is a correctness rule. The probe records what Gemini's usage reports, and the evaluation shows whether Gemini does worse than its peers in ways a transcript traces to continuation. A native adapter waits for one of those two.

**What each route changes about reasoning carry-over.** Only Google checks the echo; the two hosts accept whatever they are sent. So "send it back as it came" is enough on Google, and on RunInfra and Fireworks the question is whether what is sent back reaches the model, which the probe's retention steps answer:

| Model on host | Streams reasoning as | Checks the echo | Request switch to keep history | Sent back under | Settled by |
| --- | --- | --- | --- | --- | --- |
| Gemini on Google | no text; a signature on the first call of each response | yes: 400 without it | none | the message as streamed | the probe's step 2 |
| DeepSeek or GLM on RunInfra | `reasoning` (the 2026-10-27 transcripts) | no ("extra message fields pass through") | none documented; `thinking` is dropped | as streamed, unless only `reasoning_content` passes the retention steps | steps 4 and 5 |
| DeepSeek or GLM on Fireworks | `reasoning_content` | not documented | `reasoning_history: "preserved"`, its default undocumented for both | as streamed | its own probe, with that field |
| GLM on Z.ai's API (a climb arm, below) | `reasoning_content` | no | `thinking.clear_thinking: false` | as streamed | its own probe |

DeepSeek's own API, which validates `reasoning_content` with a 400, is not used: the shipped row reaches DeepSeek through RunInfra.

**The one switch likely to be needed: schema keywords.** Client libraries report Gemini refusing `additionalProperties`, `$schema`, `patternProperties`, `const` and `exclusiveMaximum` in a tool schema and strip them before sending (a Laravel AI fix merged in June 2026 among them); Razor's declarations use `additionalProperties: false`. If the probe's first Gemini request is refused for a keyword, Gemini's row gets a schema-dialect switch: declarations keep only the keywords Gemini's schema accepts, at every depth (about 15 lines). It is not built unless the probe shows the refusal.

## Decision 2: where it lives

Inside the Responses loop's module and the profiles, because the turn is the same thing on both wires.

- **Shared, unchanged:** the whole task (inputs, request, response, admission, answers in call order, results, images, stop, settle, rebuild), `post`'s retry policy, the error type, the 64 MiB caps and the idle timeout, the byte framer, `Conversation`'s prefix descriptor, additions, resume and snapshot, `Profile::result` and `notice`, the four plain declarations GPT's plain mode already ships, and the controller's handling of `Native::Responses` and `Resume::Responses`. In the glossary a response is "one model reply on Razor's own loop", so `Native::Responses` reads as a response's native data on either wire; renaming it would break every stored record.
- **Not shared:** the ChatGPT sign-in, its refresh, the account and turn-state headers, Lite and the plan-usage windows, against one bearer key; the stream reader; code-mode cells and `web.run` (chat refuses code mode); the hosted `web_search`.
- **No trait.** The wires differ at four points: the URL and headers in `post`, the reader, the arm that maps a completed message to entries, and `Conversation`'s chat shapes. Four matches are smaller than a trait and a task generic over it.

**The session controller.** Its two exhaustive matches on `Transport` (`resume` and `open` in `controller/session/conversation.rs`) gain `| Transport::Chat(_)`, and `open` stops passing the URL to `start`. `api/recap.rs` gets the same pattern: a generic session's recap is made by `gpt-6-luna` on the ChatGPT sign-in, as a GPT session's is. `Effort` gains `PartialOrd, Ord` on its derive line, so `min(effort, top)` is the clamp.

## Decision 3: the conformance probe and what it decides

`<evaluation>/run.py --set probe` runs, for each named arm, five steps against the real host as task folders under `<evaluation>/probe/`, one attempt each, under the spend proxy:

1. **One call round trip.** "Read code.txt and reply with the code it holds." Passes with one `read`, two requests and the code in the reply.
2. **A second step with reasoning replay.** One session, two inputs: "Reply with the word ready." (a reply with no call), then "code.txt names a second file; read code.txt, then that file, and reply with its code." That second turn is two sequential tool steps; Google validates every step's signature in the current turn. Passes when every request was accepted and the reply holds the second code.
3. **A multiple-call round trip.** "Read x.txt and y.txt with two calls in one response, then reply with both codes." Passes with both codes in two requests, so both calls came in one response.
4. **Retention within a turn.** "Choose a random six-digit number in your private reasoning; never write it in a message, a tool argument or a command. Then run `echo ok` with bash, and after its result reply with only that number." Passes when the reply's number is one the turn's recorded reasoning chose before the call, and appears in no message or call before the reply: only reasoning sent back with the call can carry it past the tool result.
5. **Retention across turns.** Two inputs: the same choice, then "Reply with only the word ready"; then "Reply with only the number you chose." Passes the same way, across a user message.

Steps 4 and 5 are inconclusive, never passed, when the recorded reasoning chose no number, or the number leaked into a message or call. Gemini's reasoning is not visible, so on Google they are inconclusive by construction; its continuation is the signature, which step 2 proves. The probe also records what the host reports per request: usage (cache reads, reasoning tokens), `X-RunInfra-Reasoning-Effort-Applied`, and whether the stream ended with `[DONE]`.

What it decides:

- One gate: a model's evaluation starts only after steps 1, 2 and 3 pass, the two-call step included. A refusal in any of them is a transport defect, fixed before any further spend.
- Each row's `replay` and `request`: the setting under which steps 4 and 5 pass. Streamed is tried first; if it fails, `reasoning_content` (or `reasoning`) is tried as a second arm. When no setting passes or the result is inconclusive, the row keeps streamed replay and the report says what failed.
- If step 5 fails for GLM on RunInfra, the climb gets one arm on Z.ai's own API with `thinking.clear_thinking: false` (`ZAI_API_KEY`; Trey, about 20:40), probed the same way first.
- Whether `max` reaches DeepSeek 1017 as `max` on RunInfra, from the effort header.
- A host that never sends `[DONE]` fails step 1 as incomplete; the probe's report says so, and that row gains a setting before it joins.
- The usage each host reports (cache reads, reasoning tokens), which decides whether that model's spend-log figure is exact or an upper bound; the first request's `prompt_tokens` is the generic prefix in that model's tokenizer, which must be under SPEC section 5's 12,000; whether Gemini needs the schema switch.

During the build, one scratch request to Google with the signature removed checks that Google still refuses it, so step 2's pass means the echo was tested; that scratch is not committed.

## Decision 4: the hill-climb, expressed as data

Trey wants each model's agent-facing choices climbed once the path works. A variant is data that an evaluation arm names, never code, and each knob below is either a variant field or a `[models."<model>"]` setting:

| What varies | Where it is set | Built here |
| --- | --- | --- |
| Which tools are declared | variant `tools`: an entry with `"omit": true` takes that action's declaration out | yes |
| Names, descriptions, schemas | variant `tools`: `{"action", …}` and only the fields that change, each a path such as `"schema/required"` set over `data/tools.json` (docs/design/profiles.md, 2026-10-30) | yes |
| Argument names | variant `tools[].keys`: `{"file_path": "path"}` renames before the action sees them | yes |
| Edit format | variant `tools`: a declaration bound to the `edit` action or to `apply_patch` | yes |
| System prompt | variant `route` replaces the route part; the person's `startup` list already replaces the rest | yes |
| Reasoning carry-over | `[models."<model>"] replay`: `"streamed"`, `"reasoning"`, `"reasoning_content"`, `"none"`; `request` for the host's history switch | yes (the rows need it) |
| Effort | the arm's `effort` | already |
| Parallel calls | `[models."<model>"] parallel = false` sends `parallel_tool_calls: false` | at the climb, about 3 lines |
| Result shape | variant `results = "text"` uses the Claude-trained text rendering that exists for shell results | at the climb, about 5 lines |

A variant file:

```json
{"version": "dsh-f3803c96",
 "route": "…the maker's tool guidance, or none…",
 "tools": [
   {"action": "read", "name": "read", "description": "Read a UTF-8 text file and return line-numbered content.",
    "schema": {"type": "object", "properties": {"file_path": {"type": "string"}, "offset": {"type": "number"},
               "limit": {"type": "number"}}, "required": ["file_path"]},
    "keys": {"file_path": "path"}},
   {"action": "write", "omit": true}]}
```

One experiment is one arm in `<evaluation>/arms.toml` (the base arm plus a `variant` or a setting) and one run of the fifteen tasks. The variant's `version` enters the profile's version, so its score certifies exactly what ran and drift compares like for like. A choice outside these knobs (a new action such as a line-hash edit, a new result renderer, a native Gemini wire) is code once, and data after. Shipping the optimum is making the winning variant a model's shipped default: its data moves into `src/profiles/data/variants/`, and its row names it (Each model's shipped default, below).

The two variants the evaluation below uses are open item 1's other two profiles, built as data here:

- **The nearest maker-shaped alias.** DeepSeek's is DeepSeek Harness (`deepseek-ai/deepseek-harness`, its generated `the tool catalog` read at f3803c96): `read`, `write`, `edit`, `bash` keyed `file_path`, with `workdir` and `timeoutMs`, descriptions verbatim less the sentences about its sandbox and `$DSH_*` variables. GLM's is Z.ai's ZCode (`zai-org/ZCode`): `Read`, `Write`, `Edit`, `Bash` keyed `file_path`. Gemini's is Gemini CLI's Gemini 3 set: `read_file`, `write_file`, `replace`, `run_shell_command`. Each is pinned at a commit read during the build. A maker argument Razor's action has no equivalent for (`run_in_background`, `end_line`, Gemini's `instruction`) is left out of the alias's schema; the actions ignore an argument they do not read.
- **One bare `bash`:** the plain profile with `read`, `write` and `edit` omitted.

## The evaluation, in two stages

**Basis.** GPT's plain-tools arm (`gpt-plain`, 2026-10-29, the same fifteen tasks with the same tools): 45 attempts, 415 requests, 7.8M input tokens of which 88% were cache reads. Calibration from the 2026-10-27 benchmark at `high`: DeepSeek 1017 on RunInfra averaged 584 output tokens a request with 93% of input read from cache, GLM 5.3 Flash on RunInfra 591 with 86%. Assumed for the low figure: 1.5 times GPT's requests, DeepSeek at max writing 1,200 output tokens a request, GLM 600, Gemini 1,500, and those cache shares (88% for Gemini). The high figure doubles GPT's requests and the output, and halves the cache share (Gemini's to none, since its cache reads may not be reported, and then the log charges every input token in full). The spend log prices GLM at RunInfra's list rate, so it reads high until the discount ends on 2026-11-04. Gemini's estimate was several times the other two models'.

**Stage 1, directional, under a spend cap**, the cheaper models first:

1. Probes, all three models.
2. Pilot: task 1, one attempt, plain profile, each model. The estimates are re-made from it before step 3.
3. The plain profile, three attempts, each model (each route's first-candidate evaluation, SPEC section 7).
4. Open item 1's other two profiles (maker alias, bare `bash`), one attempt, each model, on six file-work tasks (1 bug fix, 2 multi-file feature, 3 failing build, 4 rename, 9 large output, 13 review). This experiment tests the tool set first: three profiles, the same actions; plain's three attempts on those six tasks are the comparison.

If Gemini's pilot lands at the high estimate, its two step 4 arms wait for stage 2 and are not dropped (Trey, about 20:40).

**Stage 2, confirmation, the reserve:** what stage 1 leaves of the whole cap, through the same proxy. The hard calls get three attempts: any arm within one task of plain on its model on the six, and Gemini's two arms if they waited. What remains funds the climb's first directional experiments at one attempt each. The thirty-task set waits for these hard calls and needs its fifteen new tasks written first. Anything that would pass the cap comes back for a ruling first.


## Hosts: the fallback is a config edit

Moving DeepSeek or GLM to Fireworks is one `[models."<model>"]` table: `base_url`, `key`, `id`, and `request = {reasoning_history = "preserved"}`. It is usable only after Fireworks' own probe passes with that table, since its serving template can strip reasoning and its history table lists neither model; until then the fallback is documented, not shipped. Automatic failover is ruled out in this version. Automatic failover would cost: classifying which failures move a session (a 503 after `post`'s retries, a 429, an outage) without moving on a model's own errors; a second key and id per model; a rebuild at every switch, since the profile version names the host and the two hosts stream reasoning under different fields; a cold cache on the new host, so the first request pays full price for the whole context; and recording which host served each response so a score stays honest. About 50 lines and a flow, plus a mid-session change of host the person did not choose.

## The flow

`flows/46-generic.py`, against the stand-in, which replays chat streams as it replays the others. The driver also writes each chat model's `[models."<model>"] base_url` as the stand-in, and the flow sets each key to a dummy.

- **DeepSeek** (effort max), reasoning streamed as `reasoning` the way RunInfra sends it: `write`, then `edit` and `bash` in one response, then a text reply; a second input, then `read`; one call with malformed arguments. Asserts the file and each result; that request 2 sends back reply 1's message as assembled (reasoning joined across a chunk cut inside a multibyte character, `role` once, no `index`) with one `tool` message; that request 4 carries the text-only reply's reasoning; the malformed call's error result and `{}` in what goes back; tool messages in call order; `reasoning_effort` max; `x-session-id`; the key's value in no file under the flow's home, also after a reply and a 400 error that echo it, whose text the person still sees with the key removed.
- **GLM** (effort high), reasoning streamed as `reasoning_content`, with `replay = "reasoning"` and the bare-`bash` variant: the request's declarations hold no `read`, `write` or `edit`, the variant's version is in the record's profile version, and the reasoning goes back under `reasoning` only.
- **Gemini** (effort max): two whole `read` calls with no `index` in one chunk, the first with a signature; a text reply with a message-level `extra_content`; a second input. Asserts the signature on the first call only, the text reply's `extra_content` in the next request, both `tool` messages in call order with their names, and `reasoning_effort` high.
- **Early ends:** a stream with a complete call and no `finish_reason`, and one with `finish_reason` but no `[DONE]`, each followed by a complete reply: neither first call runs.

## Smaller decisions

- **A `tool` message names its function.** Google's compatible examples carry `name`, and 3.8 Flash requires it on native function responses; the probe's first step shows whether each host accepts it.
- **No `max_tokens`, `tool_choice`, `parallel_tool_calls` or `temperature`.** Each host's default is what Razor wants; a row's `request` adds only its history switch. A reply cut at `length` ends the turn rather than being sent again.
- **What the person sees.** Reasoning text becomes Thinking entries; Gemini's thought summaries are not requested (Google's compatible endpoint takes `include_thoughts` in its own `extra_body` thinking config, which this route does not send).
- **Web on the generic route.** The three verbs, when `[web]` names a provider with a key, worded without "use your native web tool first", since this route has none; with no `[web]` provider, no web.
- **Code mode** stays refused on chat. The cells are wire-agnostic, so adding it later is the `exec` declaration in function shape and a route text.
- **In the build, as agreed:** `docs/flows-and-tasks.md` names flow 46 for the generic probe on the stand-in; the web verbs get their generic wording.

## Rulings recorded

- If the probe shows RunInfra drops GLM's earlier reasoning, the climb gets one arm on Z.ai's own API with `thinking.clear_thinking: false` (`ZAI_API_KEY`). If Gemini's pilot lands at the high figure, its two extra profiles wait for stage 2 and are not dropped. (Trey, about 20:40.)
- The host is RunInfra, not DeepInfra (settling a review question).

## Probe results (2026-10-29, an earlier release build)

Run as `<evaluation>/run.py --set probe` under the spend proxy. Every request in every run was answered 200 and every stream ended with `[DONE]`; none was retried.

| Model on host | Steps 1, 2, 3 | Step 4, within a turn | Step 5, across turns | Effort |
| --- | --- | --- | --- | --- |
| DeepSeek 1017 on RunInfra | pass, pass, pass | pass, with reasoning sent back as streamed (`reasoning`) | pass (one more attempt inconclusive: the reasoning wrote no number) | `max` forwarded unchanged: no `X-RunInfra-Reasoning-Effort-Applied` header came back |
| GLM 5.3 Flash on RunInfra | pass, pass, pass | pass, as streamed (`reasoning`) | inconclusive four times: the model wrote the number into a message, or refused to give it | `high` forwarded unchanged |
| Gemini 3.8 Flash on Google | pass, pass, pass | inconclusive by construction: no reasoning text is returned | inconclusive by construction | `high` |

- **Rows unchanged.** Every row keeps `replay = streamed` and no `request`: streamed replay passed wherever the result was conclusive, and is kept where it was not.
- **Retention proved for GLM and DeepSeek on RunInfra** (stage 2, `<evaluation>/carry.py`). Step 5 cannot judge a model that will not keep a number out of its messages, so the carry check replays a fixed history whose only copy of a code word is an earlier assistant message's reasoning, and asks for the word: within a turn (after a tool result) and across turns (after a user message), with the reasoning under `reasoning`, under `reasoning_content`, and under neither as the control. Three runs on both models and a fourth on GLM: within a turn DeepSeek returned the word under both fields every time (3 of 3 each) and GLM 4 of 4 under `reasoning` and 3 of 4 under `reasoning_content`; across turns GLM 3 of 4 and 2 of 4, DeepSeek 2 of 3 and 3 of 3; the control never returned it. Each field reaches the model on RunInfra's templates for both models; every miss was the model choosing a new word or, in GLM's fourth run across turns, declining to say it, not the host dropping the field (the same field worked in the other runs). The Z.ai arm (Rulings) is not triggered.
- **Google's validator, checked.** A scratch request sending a call back without its `extra_content.google.thought_signature` got HTTP 400 ("Function call is missing a thought_signature in functionCall parts"); the same history with the signature got 200. Step 2's pass therefore means the signatures went back.
- **Google reports cache reads** on the compatible endpoint (`prompt_tokens_details.cached_tokens`, over 300,000 tokens in one probe), so Gemini's spend-log figures are exact, not upper bounds. It reports no reasoning tokens apart from `total_tokens`.
- **No schema switch needed.** Google accepted the declarations, `additionalProperties: false` included.

## Stage 1 results (2026-10-29)

An earlier release build, every request through the spend proxy. The plain profile ran the fifteen tasks three times per model; each was filed as a score. Two runners ran at once, on different hosts, with caps chosen so their joint bound stayed within the stage's cap; wall times are therefore slightly noisier than one-at-a-time runs.

| Plain profile, 15 tasks × 3 | Attempts finished | Tasks passing at least once | Blocked (first-candidate rule) | Mean tokens an attempt (cache read) | Requests | Seconds |
| --- | ---: | ---: | --- | ---: | ---: | ---: |
| DeepSeek 1017, max | 43 of 45 | 15 of 15 | none | 110,678 (94,370) | 9.2 | 35 |
| GLM 5.3 Flash, high | 35 of 45 | 13 of 15 | 10 (three wrong tables after the rebuild) and 15 (the child never posted before editing) | 93,684 (77,739) | 8.1 | 19 |
| Gemini 3.8 Flash, high | 45 of 45 | 15 of 15 | none | 497,261 (420,362) | 19.8 | 116 |

The tool-set experiment, on the six file-work tasks, one attempt per profile; plain is its three attempts on the same six, and every mean counts failed attempts too:

| Six tasks (1, 2, 3, 4, 9, 13) | Plain | Maker-shaped alias | Bare `bash` |
| --- | --- | --- | --- |
| DeepSeek | 18 of 18; 34,276 tokens, 6.3 requests | 6 of 6; 37,653, 6.5 | 6 of 6; 30,786, 5.8 |
| GLM | 17 of 18; 23,595, 5.1 | 6 of 6; 21,618, 4.8 | 5 of 6 (task 2); 14,888, 4.0 |
| Gemini | 18 of 18; 181,325, 13.8 | 6 of 6; 238,300, 14.5 | 6 of 6; 293,735, 15.2 |

Five requests to RunInfra got no reply at all (the proxy's 502: four GLM, one DeepSeek, all in tasks 10 and 15, where children make requests at once; each was retried), and the log charges them at their worst case, so its total is an upper bound.

What stage 2 should test, cheapest and largest first:

1. **Bare `bash` on GLM and DeepSeek** (GLM: 37% fewer tokens, one miss; DeepSeek: 10% fewer, none): three attempts on the six, then on held-out tasks. Pennies per run.
2. **Gemini's request volume.** Gemini makes twice DeepSeek's requests and four and a half times its tokens, writing its own test scripts; on Gemini the plain profile beat both alternatives on tokens. The next axis there is the prompt and results: a short instruction against unrequested checks, or a smaller read window, three attempts on the six.
3. **GLM's misses on tasks 6, 10 and 15.** Delegation and the channel (Razor's `agent` and `message` declarations) and the rebuild.
4. **GLM's retention across turns** stayed inconclusive; a probe whose answer needs earlier reasoning the model will not write out is still to design.
5. **The thirty-task set** for the confirmation stage: fifteen new tasks, written before any held-out confirmation.

## Stage 2 results (2026-10-30)

An earlier release build, every request through the spend proxy. From stage 2 on, every RunInfra arm, plain included, sends `max_tokens = 32768` so the proxy's worst case for a request is bounded; the largest reply in 4,738 RunInfra requests was 10,827 tokens, so the field changed no reply. Means count failed attempts. An attempt the host refused (HTTP 429 ending a turn) is not counted and was rerun.

**Screens, three attempts a task.**

| Six file-work tasks (1, 2, 3, 4, 9, 13) | Plain | Candidate |
| --- | --- | --- |
| DeepSeek, bare `bash` | 18 of 18; 32,938 tokens, 5.8 requests, 12 s | 18 of 18; 26,306 (−20%), 5.4, 9 s |
| GLM, bare `bash` | 17 of 18; 21,561, 4.9, 6 s | 17 of 18; 15,089 (−30%), 3.9, 6 s |
| Gemini, a short instruction against unrequested work (`gemini-lean`) | 18 of 18; 215,069, 15.2, 77 s | 18 of 18; 166,275 (−23%), 11.7 (−23%), 70 s |
| Gemini, a 200-line read window (`gemini-window`) | as above | 18 of 18; 231,130 (+7%), 16.8, 102 s |

GLM's one miss on the six is task 2 under both profiles. `gemini-lean` keeps every declaration and adds two lines to the route: read only what the task needs and batch independent reads; make the change, run one check that proves it, and stop, without exploring git history or the environment or writing test scripts nobody asked for.

| Coordination tasks, finished | 6 delegate and merge | 10 cross the threshold | 15 through the channel |
| --- | --- | --- | --- |
| DeepSeek plain | 2 of 3 | 2 of 3 | 1 of 3 |
| DeepSeek, bare `bash` | 1 of 3 | 3 of 3 | 1 of 2 |
| DeepSeek, coordination wording | 3 of 3 | 3 of 3 | 2 of 3 |
| GLM plain | 0 of 3 | 1 of 3 | 1 of 3 |
| GLM, bare `bash` | 0 of 3 | 0 of 3 | 0 of 3 |
| GLM, coordination wording | 3 of 3 | 0 of 3 | 2 of 3 |
| Both models, `bash` with the wording | 3 of 3 each | see below | 3 of 3 each |

The coordination wording (`<evaluation>/variants/coord.json`) changes two declarations, `agent` and `message`, and no others. `agent`'s description gains how to settle a child that reports: `merge` merges its branch, `merged` records a merge made with git, `discard` keeps the branch unmerged. `message`'s description says a note goes to the tree's channel with `to: channel`; its `to` argument becomes required, and `to`'s own description is rewritten to lead with `channel` (the tree's shared channel, where an @name in the text wakes that agent), then `parent`, a child, or an agent outside the tree. Task 6's misses were children never settled and task 15's were notes never posted to the channel, which is what the two sentences say how to do. `<evaluation>/variants/bash-coord.json` is bare `bash` with that wording and a route naming only the tools it declares; the bare `bash` variant left the route describing `read`, `edit` and `write`, which it omits. On task 10 the `bash-coord` profile finished 3 of 3 for GLM in the confirmation run below, where every other GLM profile finished at most 1 of 3; the task asks for each file to be read with the file-reading tool, and `bash-coord` declares none, so the model reads with the shell. Why that fixes the table is not established.

**Confirmation on the thirty tasks.** The fifteen held-out tasks (`<evaluation>/held/`, 16 to 30) were written, and each check proved red on its repository and green after a reference fix, before any held-out run. Two were corrected after their first runs and rerun for every arm: 18 required deleting `settings.ini`, which Razor's delete guard refuses, and 23's docstring did not say that spaces at either end were allowed or that fractions were refused, both of which its check required. Each leader ran the original fifteen and the held-out fifteen three times; plain ran the held-out fifteen three times, and plain's original-fifteen figures are stage 1's filed scores. The first three attempts each arm got on a task count. The ratio column is the geometric mean over tasks of leader over plain, so one long task does not dominate.

| | Plain: finished; tokens, requests, seconds | Leader | Leader / plain: tokens, requests, seconds |
| --- | --- | --- | --- |
| DeepSeek, original fifteen | 43 of 45; 110,678, 9.2, 35 | `bash-coord`: 44 of 45; 95,881, 8.3, 30 | 0.75, 0.88, 0.91 |
| DeepSeek, held-out fifteen | 44 of 45; 85,064, 6.8, 30 | 45 of 45; 55,774, 6.4, 32 | 0.86, 0.93, 1.01 |
| DeepSeek, thirty | 87 of 90; 97,871, 8.0, 33 | 89 of 90; 75,827, 7.4, 31 | 0.80, 0.91, 0.96 |
| GLM, original fifteen | 35 of 45; 93,684, 8.1, 19 | `bash-coord`: 44 of 45; 86,461, 7.4, 20 | 0.75, 0.87, 0.69 |
| GLM, held-out fifteen | 43 of 45; 24,712, 5.1, 7 | 43 of 45; 17,330, 4.2, 7 | 0.77, 0.84, 0.72 |
| GLM, thirty | 78 of 90; 59,198, 6.6, 13 | 87 of 90; 51,895, 5.8, 13 | 0.76, 0.86, 0.70 |
| Gemini, original fifteen | 45 of 45; 497,261, 19.8, 116 | `gemini-lean`: 45 of 45; 219,335, 14.4, 87 | 0.60, 0.77, 0.81 |
| Gemini, held-out fifteen | 44 of 45; 405,030, 20.9, 114 | 44 of 45; 315,954, 17.1, 109 | 0.76, 0.84, 0.88 |
| Gemini, thirty | 89 of 90; 451,145, 20.4, 115 | 89 of 90; 267,644, 15.8, 98 | 0.68, 0.80, 0.84 |

Misses. DeepSeek plain: 10, 15 and 22 (the limit passed while it searched the machine for the task's tests); its leader: 6. GLM plain: 2, 6 twice, 10 three times, 12, 15 three times, 18 and 27; its leader: 2, 24 and 29 (the reply did not name the injection, after two requests). Gemini plain: 23 (the limit passed after it searched `/tmp` and `/home` for the function's name); `gemini-lean`: 22 (the limit passed).

**What each model's default should be, as data.** DeepSeek and GLM: `<evaluation>/variants/bash-coord.json` as the model's variant, and their `[models."<model>"]` rows unchanged (`replay = streamed`, no `request`). Gemini: `<evaluation>/variants/gemini-lean.json`, row unchanged. Shipping is moving each file into `src/profiles/data/` and naming it in the model's row (Decision 4); this stage did not do that. The coordination wording is not model-specific; whether it belongs in the shared declarations for every model needs the Claude and GPT arms measured with it.

**Spend.** Stage 2 stayed under its cap; requests that got no usable reply are charged at their worst case, so the log is an upper bound by design.

**What stays open.**

- RunInfra's shared DeepSeek refused 39 of its 2,140 stage 2 requests with HTTP 429 ("reached its limit of 60 concurrent requests"), in bursts; GLM none of 1,612. Razor does not retry a 429, so a refused request ends the turn; were refusals independent at that rate, a 25-request task would fail about one time in three. The runner reran the refused attempts.
- Attempts are not isolated from the evaluation's own files. In the failed attempts whose records were kept, DeepSeek plain searched `/tmp`, `/opt`, `/srv` and `/home` for the task's tests, Gemini plain ran `grep -rn parse_duration /tmp /home` (whose output held task 23's check and its reference fix), and Gemini listed sibling attempts' folders. Passing attempts' records are deleted, so how often a pass looked is unknown; both arms of each comparison had the same access.
- One DeepSeek `bash-coord` attempt's whole folder vanished mid-attempt during task 23's first rerun, removed by a process outside the evaluation; not a Razor fault.

## Each model's shipped default (2026-10-30)

Trey's "we ship the optimal" makes each model's stage 2 leader what it gets with no config.

**What ships.** `<evaluation>/variants/bash-coord.json` and `<evaluation>/variants/gemini-lean.json` move, unchanged, to `src/profiles/data/variants/`, and each `Chat` row names its leader's file through `include_str!` in a new field, `variant`: DeepSeek's and GLM's rows `bash-coord.json`, Gemini's `gemini-lean.json`. `load` renders a chat profile from config's variant when there is one, and otherwise from the row's, parsed only then. Nothing else in the render changes, so a session with no config on DeepSeek or GLM declares `bash`, Razor's own tools with the coordination wording on `agent` and `message`, and the route naming only those; Gemini keeps every declaration and gets the two lines against unrequested work. The profile version names the variant as before, so these sessions' versions say `bash-coord-1` or `lean-1`, exactly what the stage 2 leader arms certified; a session recorded under the old default's `plain` rebuilds once instead of resuming, as any changed profile does.

**How a config overrides it.** As today: `[models."<model>"] variant = "<file>"` names a variant, and it replaces the row's whole, never field by field. A merge would leave no way back to plain and would make a variant's meaning depend on which model's row it lands on.

**How plain stays reachable.** The empty variant: a file holding `{}` declares nothing in place of the shipped declarations and leaves the route to `tools-generic.md`, and its missing `version` reads as `plain`. That is the plain profile exactly, under the same profile version stage 1 and stage 2 filed for it, so the next climb's comparison against plain still certifies like for like. The options:

| Plain reached by | Code | What it costs |
| --- | ---: | --- |
| **An empty variant file** (chosen): `<evaluation>/variants/plain.json` | none | One data file; a person wanting plain writes `{}` to a file and names it. |
| A config word, `variant = "plain"` or `variant = false` | about 6 lines in `config/pack.rs` and the `Chat` type | A key that is sometimes a path and sometimes a word, or a second type for it, to save writing two characters to a file. |
| A shipped plain variant named by a word | a lookup from word to compiled file | A name table for one entry; the same thing as the empty file with a mechanism around it. |

**The evaluation's arms keep their meaning.** Every arm that ran plain names `<evaluation>/variants/plain.json`: `deepseek`, `glm`, `gemini` (the probe's arms too) and the two second-replay arms, `deepseek-rc` and `glm-rc`. The leader arms, `deepseek-bashcoord`, `glm-bashcoord` and `gemini-lean`, name the moved files under `src/profiles/data/variants/`, so one copy of each leader exists and an arm still means its leader after a later climb changes a model's default. The other arms already name their own variants.

**The flow.** Flow 46's DeepSeek runs name the empty variant, proving plain is reachable (the four plain tools are declared and the version says `plain`); its GLM run names none and must declare `bash` with no `read`, `write` or `edit`, carry the leader's route and coordination wording, and record `bash-coord-1`; its Gemini runs name none and must carry `gemini-lean`'s route and record `lean-1`. DeepSeek's empty variant winning over a shipped leader is the proof that a configured variant still replaces the row's whole.

## The chat route retries HTTP 429 (30 October)

Trey, 2026-10-30, to "Retry when an open-model host says 'too many requests,' waiting as long as the host asks, with a cap ... for those hosts only": "8. yes." RunInfra's DeepSeek refused 39 of 2,140 stage 2 requests with 429 ("reached its limit of 60 concurrent requests"), in bursts, and one refusal ends the turn (What stays open, above).

RunInfra's rate-limit page: "Every `429` carries `Retry-After`", "the RFC 9110 header in whole seconds, always at least `1`", and at capacity "wait for `Retry-After` and retry unchanged; the same request succeeds once a slot frees." Codex at the pin does the same for a rate limit (`core/src/responses_retry.rs`, `protocol/src/error.rs`): it waits until the server's `Retry-After`, or else its own backoff from 200 ms doubling, at most `request_max_retries` (4) times, and never retries a quota or usage-limit refusal. ChatGPT's 429 on the Responses route is a plan limit, so it keeps failing at once.

### Surface

```rust
// src/responses/mod.rs
/// The longest one request waits in all on a chat host's 429s, its `Retry-After`s included.
const BUSY: Duration = Duration::from_secs(60);

impl Responses {
    /// Signature unchanged. Its doc's retry sentence becomes: four times for a failed send, a
    /// server error or, on chat, a 429; five times for a stream that did not complete. A chat
    /// 429 waits the host's `Retry-After` when it gives whole seconds, and the backoff
    /// otherwise; a wait that would take the request past `BUSY` in all is not taken, and the
    /// refusal ends the turn naming what was waited. The response's usage says what was waited.
    async fn post(/* unchanged */) -> Result<Response, Error>;
}

// src/protocol/mod.rs, a field of Usage:
/// Milliseconds the response's request waited on a chat host's 429s before it was answered,
/// summed over the turn's requests like the counts.
pub waited: u64,
```

### Data flow

1. `post` sends the chat request. A 429 from a chat host (`endpoint.key` set) takes the retry arm that a 5xx takes, sharing its count of four.
2. The wait is `Retry-After`, as delay seconds or an HTTP date; with no header it is the backoff: 200, 400, 800, 1,600 ms, Codex's curve without its jitter.
3. If the waits so far plus this one would pass `BUSY`, `post` stops and returns the host's rejection; otherwise it sleeps, adds the wait to the request's total and sends the same body again.
4. On success the total goes into the response's `usage.waited`; the task already sums each field of usage into the turn's, so `Body::Response` and `TurnEnd` both say what was waited.
5. On giving up, `Error::Rejected` keeps the host's sentence and adds what was waited ("…; Razor retried for 31 s"), and the turn ends with it, as any rejection does.
6. The Responses route (`key` unset) answers a 429 at once, as now.

### Decisions

- **One count with the 5xx retries.** Four is Codex's default and the count Razor already gives a 5xx; RunInfra's example sets OpenAI's client to five. A second counter would be a second policy to tune. A request that met a 5xx and then 429s is bounded by the same four.
- **Both forms of `Retry-After`** (a review finding; RFC 9110, 10.2.3): delay seconds, or an HTTP date, read with `jiff`'s RFC 2822 parser (`jiff` is already a dependency), and a date already past means at once. Both count against the same 60 seconds. RunInfra's extra `Retry-After-Ms` is finer by under a second and is not read.
- **No jitter.** RunInfra suggests it for many workers on one key so that they do not return at once; Razor's children are a few among a pool shared with other tenants, and the shipped curve is Codex's without its 10% jitter. If refusals cluster after retries, jitter is one line.
- **A quota 429 costs a little time.** A host that sends 429 for an exhausted balance gets its four tries before the turn ends with the host's own sentence: at most `BUSY`, and seconds with no `Retry-After`. Telling it apart would mean reading every host's error codes.
- **The person sees the wait only in the record.** Nothing streams during it; the session stays working. Codex shows "Reconnecting... n/max" in its UI; Razor's terminal has no line for it, and none is added here.

**As built.** `post` gains one arm before its match and the giving-up sentence; `backoff` now returns its delay; `retry_after` reads both forms; the task sums `waited` like the counts. The evaluation's spend proxy forwards `Retry-After` on an error reply (one line in `<evaluation>/run.py`).

**Checked in a scratch run** against the debug build and the stand-in, outside the repository. A turn met two 429s, one with `Retry-After: 1` and one with an HTTP date ten seconds ahead, then got its answer. The response's and the turn's `usage.waited` held about 10,000 ms. The next turn met `Retry-After: 1` and then `Retry-After: 120`. It did not take the second wait and ended "The model endpoint returned HTTP 429: reached its limit of 60 concurrent requests; Razor retried for 1 s."

**Flows.** 46 (the chat route on the stand-in) and 31 (the shared retry loop on Responses) run. No flow answers 429, so the retry is exercised in a scratch run against a local stand-in that refuses twice with `Retry-After`, outside the repository. The evaluation's spend proxy forwards no `Retry-After` on an error reply (`<evaluation>/run.py`, the `HTTPError` branch), so under the evaluation Razor falls back to the backoff; forwarding that one header is a line in the runner.

## Glossary line proposed

- **variant**: a file of declaration data (tool declarations, argument names, route text) that replaces a generic profile's shipped data for one model or one evaluation arm.
