# Code mode

`src/code_mode/` shares the 6,000-line tool-service row with the rest of the tool service; target 900 lines. One fresh JavaScript runtime per `exec`, retained only while its cell is live. An exec cell and a shell job have separate lifetimes and identifiers.

## Public surface

```rust
/// Cells end with their turn; no record, actions or session execution lock.
pub struct CodeMode { /* turn-scoped cells, session directory, controller sender */ }
/// A nested intent in invocation order; the controller records it before acting and replies once.
pub struct Run {
    pub call: CallId, pub name: String, pub input: serde_json::Value,
    pub reply: oneshot::Sender<Result<serde_json::Value, Error>>,
}
/// Cell failures are returned to the model; Other preserves its source and context sentence.
pub enum Error {
    Refused(String),
    Other { source: Box<dyn std::error::Error + Send + Sync>, context: &'static str },
}
impl CodeMode {
    /// The sender reaches the same controller run path used by direct calls, with capacity 64.
    pub fn new(dir: PathBuf, runs: mpsc::Sender<Run>) -> Self;
    /// Runs raw JavaScript with the pinned pragma; returns completion or a resumable cell id.
    pub async fn exec(&self, call: CallId, source: String, nested: Arc<[profiles::Declaration]>,
        stop: watch::Receiver<bool>) -> Result<String, Error>;
    /// Implements Codex's cell_id/yield_time_ms/max_tokens/terminate schema; returns new output only.
    pub async fn wait(&self, input: serde_json::Value) -> Result<String, Error>;
}
```

## Data flow

1. The controller admits the complete model response and appends the outer Call before starting a cell; raw source remains recallable and native input stays unchanged.
2. A dedicated thread owns a plain QuickJS Runtime and async module; synchronous host functions send calls then return Ctx::promise(). The thread drives execute_pending_job itself; no rquickjs futures feature or JS work on the session loop.
3. Install `tools` from Profile.nested, every Razor tool `handover` included, plus `text`, `exit`, `setTimeout`, `clearTimeout`, `ALL_TOOLS` and `yield_control`; no Node, console, filesystem, network or import loader.
4. A cell is named by its exec's call id, which `wait` takes as `cell_id`; exec refuses an id that is not 1 to 128 ASCII letters, digits, `-` or `_` (every id on the code-mode routes is), and a second cell of the same name in a session fails to create its spool. A nested invocation assigns `<exec-call-id>:<counter>` and queues its Body::Call synchronously before returning a promise. The session channel alone is capped (64); blocking_send waits on the cell thread when full, never refuses a nested call for capacity.
5. The controller records intent, normalizes through the same profile, and polls Tools::run futures in that order. The controller's FIFO read/write lock serializes shell/change actions; read-only actions may overlap. Session-bound actions stay in the controller. No lock is held by exec/wait.
6. Record each nested result once, then resolve/reject its JS promise with the structured value, or with the text itself when a result is not JSON (`handover`'s). A command that ran (an object `end`, exit or signal) resolves even when its recorded outcome is an error; refusals, launch failures and unknown outcomes reject. `text` supplies outer output; uncaught JS error text reaches its Error Result verbatim, enabling omitted-helper failure counts without stubs. Nested calls are not wire calls.
7. Exec yields after its pragma's `yield_time_ms` (30,000 ms default), or `yield_control`; wait defaults to 10,000 ms and retains the same cell. Both keep Codex's 10,000-token default output budget.
8. Output is spooled; max_output_tokens × 4 characters, capped at SPILL (30,000), bounds a preview keeping half from the start and half from the end, with a line counting the omitted bytes and the full-output path. Only two bounded spool ranges are read. Wait consumes only the new range; no output log remains in memory. Exec/wait return Codex's "Script running with cell ID …" / "Script completed" status lines verbatim.
9. Cap four running cells per session, 64 MiB engine heap per cell and 64 timers per cell. Interrupt checks observe stop/terminate even during synchronous JS; completing a module drops unawaited promises/timers, never undoes an already-started action.
10. A cell ends with its turn; its shell jobs keep running. Only running cells count toward the cap; a finished cell is its spool file. Escape stops the turn's cells at once, before the model path ends the turn, and their foreground jobs, not detached jobs or children. Rebuild waits for the active batch, never yielded cells. Restart resumes no JS state and reruns nothing; unfinished intents become Unknown. News a nested call takes is answered by the next Result that reads the cell, its exec's or a `wait`'s (controller.md).

## Decision: QuickJS instead of Codex's V8

Choose **rquickjs 0.14.0 / QuickJS-NG 0.16.2**, replacing a hand-built evaluator/event bridge; Codex uses the direct **v8 152.2.0** binding, not deno_core (`code-mode-runtime/Cargo.toml:15–27`, `runtime/mod.rs:178–289`). A JIT buys little for orchestration that spends its time awaiting tools.

Scratch measurement, Linux x86-64, Rust 1.98.1 release/stripped, five runs of 1,000 fresh runtime/context + async/await/`Promise.allSettled` + disposal cells:

| Engine | Median cell, ms | Process-cold median/p95, ms (20 starts) | RSS baseline/one/20 held, KiB | Probe binary, bytes |
| --- | ---: | ---: | ---: | ---: |
| QuickJS | 0.092 | 0.956 / 1.533 | 2,148 / 3,268 / 6,540 | 1,662,376 |
| V8 | 1.001 | 4.423 / 5.234 | 4,216 / 15,484 / 38,592 | 48,196,696 |

The empty Rust binary was 346,144 bytes. These measure engine plumbing, not Codex's host or Razor; cold means a new process with warm filesystem pages. Idle sessions retain no engine. Replacing `allSettled` with `all` made both probes fail. The first measurement failed because the V8 probe dropped retained isolates in creation order; after fixing it to reverse order, all runs passed. A separate QuickJS probe passed top-level module await, out-of-order promise resolution, missing host globals, heap limit and interruption; removing the interrupt made its infinite loop exceed the external deadline. Scratch sources/results remain outside the repository.

Exec/wait names, grammar, pragmas, defaults, retained helper lines and the batching instruction remain verbatim at Codex **aa854dabf54e1dd6c2bda80653944b4f2c0672d7** (`code-mode-protocol/src/description.rs`; `core/src/tools/code_mode/{execute_spec,wait_spec}.rs`). Profile `razor-4` uses `Declaration::sample()` for both exec's tool sections and `ALL_TOOLS`; `Declaration::section()` adds Codex's heading for live additions. Shared MCP Types appear even without MCP tools, as Codex's code-mode-only builder requires. Remaining departures:

- The engine line names a fresh QuickJS runtime instead of a fresh V8 isolate.
- `store` is omitted: no flow; a count over 898 earlier exec cells found six calls.
- `load` is omitted: no flow; the same count found three calls.
- `notify` is omitted: it needs extra outputs for one call; the same count found none.
- `image` is a no-op: Razor shows every image a nested result names after the cell's output, so a Codex script passing `view_image`'s result to it runs unchanged.
- `audio` is omitted: it needs structured audio output; the same count found none.
- `generatedImage` writes its argument's `output_hint` as text and shows nothing itself, for the same reason.
- Deferred-tool catalog and `tool_search` guidance are omitted: Razor does not expose Codex's deferred catalog or BM25 search.
- MCP return signatures remain `Promise<unknown>`: `tools::Schema` supplies no output schema to the profile, so Codex's output-schema/`CallToolResult<T>` rendering has no input.

These helper omissions are deliberate, not claims of non-use. The proof or GPT evaluation reaching for an omitted helper reopens that omission; an observed script feature QuickJS cannot execute is the signal to switch back to V8. The GPT evaluations and the code-mode comparison decide, not this microbenchmark.

The real GPT proof ran actual async cells, shell actions and durable results; batched shell reads executed serially. Scratch checks also exercised yield/wait, 80 queued actions, timer/exit and synchronous-loop interruption. Installed-controller ordering, another session's responsiveness and terminal Escape remain unproved. `secret_file` is the only secret form: send reads its bounded bytes and records only the path. Source is never rewritten after append; nothing scans for secrets.

## Second pass (2026-10-28)

A running cell shares only `terminate`, `exited`, `done` and one `Notify` with its thread; the first version's closed flag (a copy of the turn's stop, which both tasks send when they end), its written-bytes counter and its self-removal went; the running map drops finished cells when the next `exec` takes the lock. A result reads the cell's status before its length: once the status is final, so is the length. The first version read the length first, so a cell that finished between the two reads reported `Script completed` without its last output. With a 500 ms pause between the reads, this version returned `Script running` with the cell's last line and then `Script completed`; the same build with the reads swapped returned `Script completed` and lost the line.

## `handover` inside a cell (5 November)

On 4 November a GPT cell in a resident's thread called `tools.handover({note})` for the person's `/handover`. `handover` was then direct-only, beside exec and wait, so the cell failed with "Error: not a function"; the model went on without a note, the rebuild at the turn's end had none, and the rebuilt conversation answered the person's last ask a second time. The same thread's idle handover two hours later failed the same way before the model called the direct tool. Codex shows every tool inside exec, and so does SPEC section 5 ("code mode renders it as a JavaScript object inside one `exec` tool"), so `handover` is now one of exec's tools like the rest, with only exec and wait direct. A direct call still runs, since a call's name is checked against the nested tools too. A cell's handover settles its batch as a cell's `finish` already did. Its result is a sentence, not JSON, which a cell rejected ("expected value at line 1 column 1"); a nested result that is not JSON now resolves as its text, as exec's description promises ("Nested tools return either an object or a string"). The profile version stays `razor-7`: live conversations take the changed declarations in place, as with the helper task declarations of 4 November.

The other two fixes considered: a cell's `tools.handover` refusing with a sentence naming the direct tool costs a round trip and still depends on the model trying again, and declaring `handover` both ways shows the model one tool twice. A rebuild with no fresh note stays: SPEC section 3 has one reuse the latest note, and across both homes' records (to 5 November) the model called `handover` in 219 of the 220 threshold, person and idle handovers; this incident was the other one.
