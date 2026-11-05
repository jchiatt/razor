# Responses

`src/responses/` shares the model-path row with Claude. One session task owns requests, retries, stream completion and the tool loop; the controller owns the record and actions. No action starts before its completed response and Call are durable.

## Public surface

```rust
/// Shared HTTP client and sign-in mutex; credentials are read for each request.
pub struct Responses { /* client, home, auth */ }
/// Why a request or sign-in failed, as one sentence; nothing outside this module retries.
pub enum Error {
    Auth, ApiKey, Stopped, Incomplete, ContextOverflow(String), Size,
    Rejected { status: u16, message: String },
    Other { source: Box<dyn std::error::Error + Send + Sync>, context: &'static str },
}
impl Responses {
    /// Does not read credentials until login or a request.
    pub fn new(home: PathBuf) -> Result<Self, Error>;
    /// Runs in the server, streaming the device URL/code to `razor login`'s API client.
    pub async fn login(&self, instructions: mpsc::Sender<String>, stop: watch::Receiver<bool>)
        -> Result<(), Error>;
    /// One task per native conversation, opened by its first message; it ends with Closed.
    /// The controller updates the profile before Add and drains outgoing lines.
    pub fn start(self: &Arc<Self>, base_url: String, dir: PathBuf,
        profile: watch::Receiver<Arc<profiles::Profile>>, incoming: mpsc::Receiver<ToBridge>,
        outgoing: mpsc::Sender<Line>);
    // post(base, body, session, lite, turn_state, delta) is the task's one HTTP operation.
}
```

## Data flow

1. The server freezes `responses.base_url` at startup; changing it requires a restart. The controller loads model, effort and profile, starts this task with shared `ToBridge`/`FromBridge` messages, updates the profile watch before Add, and acknowledges each durable Call with Admitted.
2. The opening prompt and declarations are saved in the first Response's native additions, as a request-prefix descriptor; it is record metadata, never a wire item. Clean restart restores it rather than re-rendering changed data. Other additions are actual input items.
3. POST full input to `{base_url}/responses`: `store:false`, `stream:true`, `include:["reasoning.encrypted_content"]`, `reasoning.summary:"auto"` (Codex's default, subscription and API-key routes), session cache key. Send `session_id` and resend `x-codex-turn-state` within the turn; no server continuation or compaction. Read `x-codex-{primary,secondary}-{used-percent,window-minutes,reset-at}` from a successful response and emit `Usage { windows }` for that conversation when it completes. Invalid percentages and zero-length windows are absent, never measured zeros; names derive from the duration (300 minutes is 5-hour, 10,080 is 7-day). No headers means no reading, not a polling request.
4. GPT code mode uses Codex Lite: `additional_tools` prefix, `functions` namespace, Lite header, `reasoning.context:"all_turns"`, no top-level tools, `parallel_tool_calls:false`. Plain GPT uses standard Responses functions and parallel calls. API-key routes are plain only.
5. Parse complete SSE events as bytes before UTF-8/JSON, with a 64 MiB response cap and 300-second idle timeout. A response is its `output_item.done` items in index order; completion requires completed status, every added index's done, and unique call ids. Ignore other events. Never compare against completion's empty `output`, inspect item status, or execute argument deltas. The common Call entry decodes function arguments from JSON text; the native continuation keeps the provider's original shape. Custom tool source and malformed arguments remain verbatim.
6. `post` owns backoff, with budgets for the whole request: four HTTP retries for a failed send or a 5xx, five stream retries for an idle or incomplete stream, none for 429. On 401 reload credentials, refresh and retry once. Context overflow is the backend's `context_length_exceeded` error and pauses the turn; no reasoning is dropped.
7. The task maps the done items itself, without validating call arguments. Append the one new kind `Body::Response { usage }` with `Native::Responses { additions, items }`, then common Text, nonempty Thinking and Call entries. `Profile::input` errors become that call's Error Result; other calls run.
8. Calls pass through the controller: append intent, run once, append one result. Only model calls receive wire outputs, matched by call id and function/custom kind; nested ids are `<exec-call-id>:<counter>`. A crash marks missing results Unknown and rebuilds, never reruns them.
9. input is input_tokens minus cached_tokens minus cache_write_tokens; context accounting is therefore shared with Claude. `end_turn` is false only for explicit `end_turn:false`. Deltas use `try_send`; full display channels drop deltas, never block completion.
10. Rebuild clears request state and renders the full current prefix. `Summary::native` stays Claude-only, avoiding response-array clones. Stop discards the unadmitted response.

## Decision: Razor owns a separate sign-in

`$RAZOR_HOME/auth.json` holds only `tokens:{id_token,access_token,refresh_token,account_id}`. No Codex binary, file lock, last-refresh timestamp or second writer. The server's one async mutex covers login and refresh. Request `/api/accounts/deviceauth/usercode`, show `https://auth.openai.com/codex/device` and its code, poll `/api/accounts/deviceauth/token` at its interval for at most fifteen minutes, then exchange the code/verifier at `/oauth/token` with `/deviceauth/callback` as redirect. Failure names ChatGPT Security Settings' **Enable device code authorization for Codex** setting.

The default base is `https://chatgpt.com/backend-api/codex`. Read credentials with a 1 MiB cap. Send bearer and `chatgpt-account-id`; never log tokens or auth bodies. Refresh within five minutes of JWT expiry, otherwise on 401. Refresh endpoint `https://auth.openai.com/oauth/token` and client id `app_EMoamEEZ73f0CkXaXp7hrann` are pinned; expiry comes from `exp`, account id from the id token's `chatgpt_account_id`. Preserve omitted token fields on refresh, accept rotated refresh tokens, save a unique 0600 sibling, sync and rename. An explicit public API route reads `$RAZOR_HOME/key` only, never an environment variable.

Source: Codex **aa854dabf54e1dd6c2bda80653944b4f2c0672d7**, `core/src/client.rs:1011–1184`, `codex-api/src/sse/responses.rs:345–519`, `protocol/src/models.rs:1065–1219`, `login/src/auth/manager.rs:1657–1807`, `login/src/server.rs:840–896`, `login/src/device_code_auth.rs:62–227`. Dependencies: reqwest TLS/stream and futures-util replace HTTP/framing plumbing; base64 replaces JWT decoding; serde/Tokio are shared. Cancellation uses Tokio watch, adding no crate.

**Proof:** real shell exactly once/durability, encrypted reasoning replay, independent reads, cache reads with sticky headers, crash Unknown, and code-mode FIFO/yield/stop. Rebuild/recall is proved once memory recall is wired. Probe hosted `web_search` on plain functions only: Lite excludes hosted tools by source (`spec_plan.rs:696`); whether to offer it is a product ruling. Flows 28, 31–33 exercise the loop once wired.

## Second pass (2026-10-28)

The public surface the controller and `main.rs` call is unchanged: `Responses::{new, login, start}` and `Error`. `Request` was public but used only here; it is gone, with the body encoded by `profiles::Conversation::body`.

The task keeps what lives only while a turn runs in one `Turn` value; dropping it drops the request in flight and the cells' futures and stops the cells, so a stop, an error and a settle share one path. Deltas go straight to the outgoing channel with `try_send`. Changed behaviour, each checked against the controller:

- Retry budgets are per request: four HTTP retries and five stream retries in all. The first version allowed four HTTP retries inside each of six stream attempts. The backoff steps, 200 ms doubling, are the same.
- A repeated `output_item.added` or `output_item.done` for one index is no longer refused as incomplete; a later done replaces the earlier one. A missing done, a duplicate call id and a status other than completed still are.
- A second `Open` to a running task is ignored. The first version re-opened in place, but the controller starts a new task with every `Open` (`controller/session/conversation.rs`, `open`).
- Queued pictures are refused as soon as their encodings pass 64 MiB, before the request is built. Without the check, both versions refused such a turn too, but only after building the whole request. Four inputs queued behind a slow first request, the last three with three 7.5 MB pictures each, raised the server's peak resident memory to 361 MB on the first version, 390 MB on this one without the check and 154 MB with it. The turn's error is now Size's sentence, "The model request or response exceeds 64 MiB; rebuild with a smaller context.", not "Failed rendering model input: The request context exceeds 64 MiB..".
- Unanswered calls are no longer folded into the native conversation as Unknown outputs when a turn is cancelled. The controller closes the conversation after every Stop and the next turn rebuilds; a request starts only when no call is pending, so an errored turn has none.

## Web search on the sign-in (built 2026-10-29)

Trey, 2026-10-29: "we let the subscriptions pay for web search", and GPT models "automatically get whatever the default GPT web search tools are", with code mode kept as GPT's default. Read at Codex main **f32b6cfd2bda4fc1c16c6d93dcc38f854828b8be**, cloned and not run: `core/src/tools/spec_plan.rs` (`hosted_model_tool_specs` 693-719, `standalone_web_search_enabled` 1209-1217, `append_extension_tool_executors` 1655-1690, `is_hidden_by_code_mode_only` 855-866), `ext/web-search/` (`src/tool.rs`, `src/history.rs`, `src/extension.rs`, `web_run_description.md`), `codex-api/src/search.rs` and `endpoint/search.rs`, `core/src/config/mod.rs` (`resolve_web_search_mode_for_turn` 3206-3246), and the schema snapshot `core/tests/suite/snapshots/all__suite__scenarios__web_search__literature_search_web_run_schema.snap`.

**What Codex does.** Lite requests carry no hosted tools, so for every Lite model (all of Razor's GPT models) Codex registers a client-run tool, `web.run`. In a code-mode-only session it is not a direct tool: it is a nested binding, `tools.web__run`, under the heading `## web` / `Tools in the web namespace.` in `exec`'s description. A call POSTs `{base}/alpha/search` (base `https://chatgpt.com/backend-api/codex`) under the ChatGPT sign-in with `{id, model, input, commands, settings, max_output_tokens}`, and the model reads the response's `output` text; `encrypted_output` and `results` are not sent back.

### Surface

```rust
// ---- src/responses/search.rs (new, private to the module) ---------------------------------
impl Responses {
    /// One `web.run` call: POSTs Codex's search request to `{base}/alpha/search` with the
    /// sign-in's bearer and account id, refreshing the sign-in once on 401, and returns the
    /// response's `output`. `commands` must be an object and is sent as the model wrote it.
    /// The body read is at most 4 MiB. Refused on the public API route, which has no code mode.
    pub(super) async fn search(&self, base: &str, session: &SessionId, model: &str,
        input: Vec<serde_json::Value>, commands: serde_json::Value) -> Result<String, Error>;
}
```

The request: `id` the session id, `model` the session's model, `input` `profiles::Conversation::recent` (Codex keeps the previous user text, up to 1,000 tokens of assistant text after it, and the current user text; Razor's cap of 4,000 characters per message also bounds a pasted file or a rebuilt context, which are user messages here), `commands` the call's input, `settings` `{"allowed_callers": ["direct"], "external_web_access": false}`, `max_output_tokens` 10,000 (every listed model's truncation policy at the pin). The headers are `post`'s, less the stream and Lite ones.

### Data flow

1. `profiles::load` nests `web__run` in GPT code mode only, its description and input type from the shipped data; its call reaches the task as a cell's `Run`, like any nested call.
2. The task records the call (`Entry` of `Call`) and waits for `Admitted`, so a plugin can change or refuse it.
3. Admitted, a nested `web__run` runs in the task beside `exec` and `wait`, not in the controller: its future joins the turn's `cells`, so Escape, an error or a settle drops it with the turn.
4. When it ends, the task records the `Result` (the output text, or the failure's sentence as an error; a refused search gives its HTTP status in fixed wording, never the endpoint's message, which may echo the request's bearer) and answers the cell with the text as a string, as Codex's code mode gives a text output to the script. The text is not spilled: the model reads it only through the cell, whose output `exec`'s own `max_output_tokens` bounds, and the request already bounds it to 10,000 tokens.
5. Plain GPT's hosted `web_search` runs on OpenAI's side; its `web_search_call` items are kept in the native items and replayed, as today.

### Decision: run `web.run` in the task

It needs the sign-in, the session's model and the conversation's tail. The task holds all three; the tool service holds none, and getting them there would go through the controller, which has one line left in its row. Cost: the task gains a third kind of call it runs itself (about 15 lines in `task.rs`, about 65 in `search.rs`), and `web__run` is the one nested call the controller never runs.

### Decision: Codex's default mode, cached

`external_web_access: false` for `web.run`: Codex's `web_search` setting defaults to `cached` (`core/src/config/mod.rs` 3996-3997), and its `web.run` reads the session's setting (`ext/web-search/src/extension.rs` 45-98, built from the session's config at thread start and at a config change, never from the per-turn one), so an unsandboxed Codex session sends `false` too. Plain GPT's hosted tool stays `{"type": "web_search"}` as today, with no field Razor would have to choose. Against: Codex's hosted tool resolves `cached` to `live` for a session with no sandbox (`resolve_web_search_mode_for_turn` 3206-3246), every Razor session runs without one, and live search reaches pages newer than OpenAI's index. Choosing live would be a second reading of "Codex's default", or a config key; the ruling asked for the default.

**Risk.** The search address is undocumented, as is the whole sign-in path (SPEC section 3, Models). Codex also sends its `originator` header; Razor's requests do not, and the route did not need it: on 2026-10-29 a live GPT code-mode turn (gpt-6.1-sol, a scratch server with every `[web]` key unset) called `tools.web__run` from a cell, the search request succeeded without the header, and the model answered from its output.

## Image generation on the sign-in (30 October)

Trey, 2026-10-30: "I *must* have the native GPT image gen tool for GPT model family agents ... it's the best image model on earth ..." Read at the Codex pin **aa854dabf54e1dd6c2bda80653944b4f2c0672d7**, cloned and not run: `ext/image-generation/` (`src/tool.rs`, `src/extension.rs`, `src/artifact.rs`, `imagegen_description.md`), `codex-api/src/images.rs` and `src/endpoint/images.rs`, `core/src/tools/spec_plan.rs` (`image_generation_available` 787-822, `append_extension_tool_executors` 1640-1666), `features/src/lib.rs` 1863-1866, and the request and output shapes asserted in `app-server/tests/suite/v2/imagegen_extension.rs`.

**What Codex does at the pin.** It does not use the Responses API's hosted `image_generation` tool. It declares a function of its own, `imagegen` in the namespace `image_gen`, and runs each call itself:

- **Request.** `POST {base}/images/generations` (base `https://chatgpt.com/backend-api/codex`) under the ChatGPT sign-in, with `{"prompt", "background": "opaque" | "transparent", "model": "gpt-image-2", "quality": "auto", "size": "auto"}`. An edit goes to `{base}/images/edits` with the same fields plus `"images": [{"image_url": "data:…"}]`, at most five. Codex also sends `originator`, `x-codex-image-turn-id` and `x-codex-turn-metadata`.
- **Response.** JSON `{"created", "data": [{"b64_json", "generation_id"?}], "background", "quality", "size", "output_format", "usage"}`. The image is the first `b64_json`, a PNG.
- **Where the file goes.** `<codex home>/generated_images/<thread id>/<call id>.png`.
- **What the model is told.** The call's output is the image (`input_image`, a PNG data URL) followed by a hint: "Generated images are saved to {dir} as {path} by default. If you need to use a generated image at another path, copy it and leave the original in place unless the user explicitly asks you to delete it. The generated image is already displayed to the user. There is no need to render it in the final response as a Markdown image or file link."
- **Code mode.** The binding is `tools.image_gen__imagegen`, and the script passes the result (`{image_url, output_hint}`) to the `generatedImage()` helper to show it.
- **Who gets it.** The feature is on by default (`image_generation`, stable). The plan must not be Free, the model must take images, and the session must be on the ChatGPT sign-in, so an API key never gets it. In Razor it also waits for `[profiles.codex] image_gen = true` (2026-11-02, under Trey's approval of the startup-text changes): two calls in a week across 498 GPT sessions did not pay for its 2,489 bytes in every conversation. The key is live, so a session that turns it on gets the tool at its next turn through the addition path.
- **Its arguments.** `prompt`, `transparent_background` (default false), `referenced_image_paths` (absolute paths, at most five) or `num_last_images_to_include` (1 to 5 of the conversation's latest images), never both. A plan limit is a 429 whose body says `usage_limit_reached` ("image limit reached").

### The hard decision: Codex's own tool, run by Razor, not the hosted tool

| | The hosted `{"type": "image_generation"}` | Codex's `image_gen.imagegen`, run in the task (chosen) |
| --- | --- | --- |
| Code mode, the GPT default | unavailable: code-mode requests carry no hosted tools (`spec_plan.rs`) | a nested binding, like `web__run` |
| Plain GPT | an unknown on the sign-in: Codex at the pin declares no hosted image tool anywhere | one declaration |
| Where the image lands | in the response's output items, as base64, which the record keeps whole; Razor would then copy it out | in a file Razor names, before anything is recorded |
| Code | a new output item to save and render | about 80 lines beside `search.rs`, on the same sign-in plumbing |

It runs in the Responses task, for `web.run`'s reasons: it needs the sign-in, and an edit of "the last images" needs the conversation, and the task holds both while the tool service holds neither.

### Surface

```rust
// ---- src/responses/image.rs (new) -----------------------------------------------------------
impl Responses {
    /// One `imagegen` call: an edit of the files `referenced_image_paths` names, or of the
    /// conversation's latest images (`images`, data URLs the Responses task put in for
    /// `num_last_images_to_include`), else a generation, POSTed to `{base}/images/edits` or
    /// `/images/generations` with the sign-in's bearer and account id, refreshing the sign-in
    /// once on 401. Reads at most 48 MiB, writes the first image's PNG bytes to `saved` and
    /// returns their length. Arguments Codex refuses, a field of the wrong type among them, are
    /// `Refused` before any request; a refusal from the backend is `Rejected` with its message,
    /// the bearer taken out. `turn` is Codex's image turn id.
    pub async fn image(&self, base: &str, session: &SessionId, turn: &str, input: Value,
        saved: &Path) -> Result<u64, Error>;
}
pub enum Error { /* ... */ Refused(String) }   // a call's arguments the route refuses

// ---- src/profiles/ --------------------------------------------------------------------------
impl Conversation {
    /// The data URLs of the latest `count` images this conversation sent, oldest first: what
    /// `num_last_images_to_include` names. Fewer when fewer were sent.
    pub fn images(&self, count: usize) -> Vec<String>;
}
impl Profile {
    /// An image generation's result: the PNG's path, Codex's hint naming it, which code mode's
    /// `generatedImage()` writes, and the image to show the model when a request can hold it.
    pub fn generated(&self, path: &Path, size: u64) -> Value;
}
```

- **Declarations** (data, not counted: `data/image-gen.json` for the heading and the schema, `data/imagegen.md` for the description). Only on the Responses route with the sign-in, in plain and code mode; never on the public API route, Claude or chat. Plain GPT gets a top-level function `imagegen`. That is Codex's function without its namespace, because Razor's plain mode declares no namespaces (its MCP tools are plain functions too), and the call's name is `imagegen` either way. Code mode gets the nested `image_gen__imagegen` under the heading `## image_gen` / `Tools in the image_gen namespace.`, as `web__run` is shown. The schema is Codex's four fields. The description is Codex's at the pin, with its tool's name and its `view_image` filled in per mode: `imagegen` and `read` in plain mode, `image_gen.imagegen` and `view_image` in code mode.
- **The hint** is Codex's, in the result's `output_hint` (`results.json`: `generated`, and `generated_large` for an image that is not shown). Its last sentence, "The generated image is already displayed to the user…", becomes "The person sees the image only when you deliver it", since nothing displays it here. `generatedImage(result)` writes `result.output_hint` as text, as Codex's helper appends the hint, so the saved path reaches the model on the usual `generatedImage(await tools.image_gen__imagegen(…))` path, and when the image is too large to attach.
- **The call order.** `imagegen` is an action the controller runs, `Run::act`'s `imagegen` arm, so it waits for its place in the session's first-in-first-out order and holds it alone, like any action that is not read-only. An edit cannot read a file that an earlier `bash` is still writing. The sign-in and the HTTP request stay in the Responses module (`Responses::image`). The conversation's images stay with the Responses task: on `Admitted`, for `num_last_images_to_include`, it puts the latest images' data URLs into the input it sends with `Run`. That input is never recorded, so the Call keeps the model's own arguments. The file is `dir/images/gen-<entry>.png`, named after the call's entry like `read`'s copies and MCP images. The Result is `{path, images, output_hint}`, with `images` empty past `IMAGE_BYTES`.
- **Headers.** The bearer and `chatgpt-account-id`, plus `session_id`, `originator: codex_cli_rs` (the default Codex sends on every request, `login/src/auth/default_client.rs`) and `x-codex-image-turn-id`, here `<session>:<entry>`. Razor has no turn id of Codex's kind. `x-codex-turn-metadata` is optional in Codex and is not sent.

### Data flow

1. The model calls `imagegen` (plain) or `tools.image_gen__imagegen` (code mode). The task records the Call and waits for Admitted, so a plugin can change or refuse it, then sends `Run`; the controller runs the `imagegen` action in its place in the call order.
2. For `num_last_images_to_include` the task puts the conversation's latest images into the input. The action reads `referenced_image_paths`, at most five, each an absolute path to a PNG, JPEG or WebP file of at most `IMAGE_BYTES`, read with that bound. Giving both, or asking for more images than were sent, is refused, as Codex refuses it.
3. `Responses::image` posts the generation or the edit and saves the PNG under the session's `images/`; `Profile::generated` makes the Result.
4. The controller answers the call like any other; the model path records the Result (the path, the hint, and the image to show) and answers the model, or the cell, which gets the JSON.
5. At the next request the image goes to the model at the head of the user message, as `read`'s does. The native conversation data keeps its path, as for every image (see **Images in native data** below). The Call and Result entries hold only the prompt and the path.
6. The person sees the call, its prompt and the path in the transcript. Any client can fetch the file through the API's file route (`/v1/sessions/{id}/files/images/gen-….png`). The model delivers it when the person should have it.

**Edge cases handled.**

- Generation takes minutes ("imagegen needs a few minutes to finish", Codex's own description). In code mode the cell yields and the model waits for it, as Codex's description directs. In plain mode the call holds its batch. The HTTP client's 300-second idle timeout bounds a silent wait.
- A plan limit or a refused prompt is the call's error, in the host's words.
- An image over 7.5 MB is saved and not shown.
- A model call and a nested call take one path: the controller's action.

**What it costs and what it leaves.**

- **No Free-plan check.** Codex checks the plan before declaring the tool; Razor would have to read the plan from the sign-in's token. On a Free plan the backend refuses the call and the model reads why.
- **Headers** are Codex's, as above; the live check shows whether the route accepts Razor sending them.
- **`razor-7`** covers this and `view_image` in one profile change. Flow 28's code-mode prefix changes with the new section and the helper line.
- **Live proof.** One generation and one edit on the sign-in count against the person's plan and cost no API money. They need a Razor home that is signed in. No flow answers `/images/generations`.

## Images in native data (30 October)

Trey, 2026-10-30: "agreed re: pointer to image file". Each image a request shows the model (the person's, a `read` or `view_image` copy, an MCP result's, a generation) is kept in the conversation and in the Response entry's additions as `{"type": "razor_image", "path", "media_type"}`, the path relative to the session directory, and `Conversation::body` reads the file while it writes the request, one message's images at a time, so neither the record nor memory keeps the bytes. Before, each image put its base64 in the record: a session's Response entries of 1.85 and 2.16 MB; in a scratch run of a person image and a `read` image of 1.4 MB each, the two Response entries fell from 1,896,760 and 1,867,539 bytes to 30,041 (the prefix's declarations) and 810, and the resumed request was the same 3,764,158 bytes. The early checks stay: an image over 10 MiB, or queued images whose encodings pass 64 MiB, refuse the turn before the request is built, now from the file's size.

- **A file gone or over 10 MiB** when a request is built becomes the text "The image {path} can no longer be read, so it is not shown." (`results.json`, `image_gone`), and the turn goes on. So does a path that is absolute, holds `..`, or resolves through a symlink outside the session directory: a record cannot send another file to the model. An edit of the latest images leaves such an image out.

**Decision: read the file when the body is written, not when the history is loaded.** Loading would read each file once per resume, but the conversation would then hold every image's base64 for its lifetime, and each new image would be read once to send and kept in memory as well. Writing reads every image file again for every request of the conversation, which the file cache absorbs and the base64 encoding repeats, about what serializing the kept base64 cost before; in exchange memory holds paths only, and the request's peak is its own bytes plus one message's images, not twice its images.
