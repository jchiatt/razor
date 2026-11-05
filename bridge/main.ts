// One process serves every Claude session; one SDK query owns each native conversation.
// Open and Add supply every declaration and prompt the bridge sends to the SDK.
import { once } from 'node:events';
import { createReadStream } from 'node:fs';
import { createInterface } from 'node:readline';
import { buffer, json } from 'node:stream/consumers';
import { createSdkMcpServer, query } from '@anthropic-ai/claude-agent-sdk';
import type { EffortLevel, HookCallback, Query, SDKMessage, SDKRateLimitInfo, SDKUserMessage } from '@anthropic-ai/claude-agent-sdk';
import { CallToolRequestSchema, ListToolsRequestSchema } from '@modelcontextprotocol/sdk/types.js';
import type { CallToolRequest, Tool } from '@modelcontextprotocol/sdk/types.js';

const app = `razor/${process.env.RAZOR_VERSION ?? 'dev'}`;
const effortNotice = { type: 'text' as const, text: 'The person changed your effort.' };
const refusals = {
  unrecorded: 'Razor could not record this call, so it did not run.',
  unknownCall: 'Razor could not tell which call this is, so nothing ran.',
  closed: 'The conversation closed before this call had its result.',
  stopped: 'The person stopped this call before it had its result.',
  unreadable: (path: string, e: unknown) => `Razor could not read the image ${path} (${e instanceof Error ? e.message : e}).`,
  tooLarge: (path: string) => `${path} is an image over 7.5 MB; make a smaller copy with a command and read that.`,
};
// An image's base64, holding at most one byte past the shared cap (protocol::IMAGE_BYTES), since a
// file Razor checked can grow before the bridge reads it; a refusal sentence when it cannot be sent.
const IMAGE_BYTES = 7_500_000;
const base64 = (path: string) => buffer(createReadStream(path, { end: IMAGE_BYTES })).then(
  (bytes) => bytes.length > IMAGE_BYTES ? Promise.reject(refusals.tooLarge(path)) : bytes.toString('base64'),
  (e) => Promise.reject(refusals.unreadable(path, e)));
const PREFIX = '00000000-0000-4000-8000-';
const uuidOf = (entry: number) => (PREFIX + entry.toString(16).padStart(12, '0')) as Uuid;
const entryOf = (uuid: string) => (uuid.startsWith(PREFIX) ? parseInt(uuid.slice(PREFIX.length), 16) : null);
type Builtin = { builtin: string; to?: string };
type Declaration = Builtin | Tool;
const isBuiltin = (d: Declaration): d is Builtin => 'builtin' in d;
type Image = { media_type: 'image/jpeg' | 'image/png' | 'image/webp'; path: string };
type Input = { entry: number; text: string; images: Image[] };
type Content = Exclude<SDKUserMessage['message']['content'], string>;
type Uuid = NonNullable<SDKUserMessage['uuid']>;
type Open = {
  session: string; open: number; cwd: string; model: string; effort: EffortLevel; speed: string;
  resume: { claude: { conversation: string; at: string } } | null; prompt: string; tools: Declaration[];
  additions: Declaration[];
  env: Record<string, string> | null; // the conversation's own sign-in and settings, over the server's
  account: string | null; usage_file: string | null; // the sign-in's account; its usage's file
};
type Answer = { output: string; error: boolean; images?: Image[] };
type Admission = { input?: Record<string, unknown>; refused?: string };
type UsageWindow = { name: string; utilization: number | null; resets_at: number | null; rejected: boolean; reason?: string };
const limitNames: Record<string, string> = { five_hour: '5-hour', seven_day: '7-day', seven_day_opus: 'Opus', seven_day_sonnet: 'Sonnet', seven_day_oauth_apps: 'OAuth apps', seven_day_overage_included: 'Included models', overage: 'Extra usage' };
const conversations = new Map<string, Conversation>();
// Why extra usage did not cover a rejected window, when the CLI says.
const blocked = (r: SDKRateLimitInfo) => r.limitScope === 'group_pool' ? "the team's shared budget is used up"
  : r.overageDisabledReason === 'out_of_credits' || r.errorCode === 'credits_required' ? 'out of usage credits'
  : r.overageEnabled === false || r.overageDisabledReason && !['fetch_error', 'unknown'].includes(r.overageDisabledReason) ? 'extra usage is off' : undefined;
const text = (content: unknown) =>
  typeof content === 'string' ? content : Array.isArray(content) ? content.map((b) => (b.type === 'text' ? b.text : `[${b.type}]`)).join('\n') : '';

// The controller sends at most 64 inputs the model has not read.
class Inbox implements AsyncIterable<SDKUserMessage> {
  private waiting: SDKUserMessage[] = [];
  private wake?: () => void;
  private done = false;
  push(message: SDKUserMessage) { this.waiting.push(message); this.wake?.(); }
  close() { this.done = true; this.wake?.(); }
  async *[Symbol.asyncIterator]() {
    for (;;) {
      while (this.waiting.length) yield this.waiting.shift()!;
      if (this.done) return;
      await new Promise<void>((resolve) => (this.wake = resolve));
    }
  }
}

class Conversation {
  session: string;
  open: number;
  query: Query;
  inbox = new Inbox();
  razor: ReturnType<typeof createSdkMcpServer>['instance']['server'];
  served = new Map<string, Tool>();
  additions: Tool[] | null; // Held until the first request has fixed its declarations.
  queue: Promise<unknown> = Promise.resolve(); // Add, Effort and Input keep arrival order.
  gates = new Map<string, ReturnType<typeof Promise.withResolvers<Admission>>>();
  runs = new Map<string, (answer: Answer) => void>();
  stopped = new Set<string>(); // Calls Escape stopped before their answer left, until the CLI's result.
  // A result with images: the SDK records its own copies' paths, the record keeps Razor's.
  imaged = new Map<string, string>();
  relist?: ReturnType<typeof Promise.withResolvers<void>>;
  sized = '';
  thought = false; // A thinking block began and has sent no words yet: its first words start a thought.
  thoughts = 0; // Thoughts begun in this conversation; each one's lines carry its number.
  landed = 0; // The newest thought whose entry was sent; one after it at a message_start was cut.
  announced = false;
  window = 0; // The SDK's raw context window, asked at init so the first turn has it, and at a switch.
  // The latest input the CLI took in, a candidate until the reply to its request begins or its
  // turn ends, Escape's included. A batch's uuid is its last entry.
  started: number | null = null;
  took: number | null = null; // The turn's latest input whose request's reply began.
  settle = false;
  settling = false;
  // Inputs that arrive while the CLI runs a turn wait here, merged, for its batch's end: the CLI
  // would put them inside the next tool result, where a model rightly distrusts orders. The
  // batch then ends the CLI's turn (`yielded`) and they go as its next prompt, a user message of
  // their own, in the same Razor turn, whose tokens `carried` sums.
  busy = false;
  held: { content: Content; uuid: Uuid } | null = null;
  yielded = false;
  escaped = false;
  carried = { input: 0, output: 0, cache_read: 0, cache_write: 0 };
  requests = 0; // The turn's model requests whose reply began, over every CLI turn it spans.
  closed = false;
  fast = false;
  model: string; // The model the CLI runs now, a kept switch's after it.
  fastState?: string;
  windows = new Map<string, UsageWindow>(); // At most 32 named windows, not a reading history.
  usageAt = 0;
  usagePending = false;
  usageFile: string | null;
  account: string | null;

  constructor(options: Open) {
    const { session, open, cwd, model, effort, resume, prompt, tools } = options;
    [this.fast, this.model] = [options.speed === 'fast', model];
    this.session = session;
    this.open = open;
    this.usageFile = options.usage_file;
    this.account = options.account;
    this.additions = options.additions.filter((t): t is Tool => !isBuiltin(t));
    const builtins = tools.filter(isBuiltin);
    const server = createSdkMcpServer({ name: 'razor' });
    this.razor = server.instance.server;
    this.serve(tools.filter((t): t is Tool => !isBuiltin(t)));
    this.razor.registerCapabilities({ tools: { listChanged: true } });
    this.razor.setRequestHandler(ListToolsRequestSchema, () => {
      this.relist?.resolve(); this.relist = undefined;
      return { tools: [...this.served.values()] };
    });
    this.razor.setRequestHandler(CallToolRequestSchema, (request) => this.run(request.params));
    // Every call, including Claude Code's own, waits for its recorded intent, then runs as a
    // plugin changed it, or is refused with the sentence the controller answered it with.
    const admit: HookCallback = async (input) => {
      if (input.hook_event_name !== 'PreToolUse') return {};
      const { input: changed, refused } = await this.gate(input.tool_use_id).promise;
      this.gates.delete(input.tool_use_id);
      const decision = refused !== undefined ? { permissionDecision: 'deny' as const, permissionDecisionReason: refused }
        : changed && { updatedInput: changed };
      return decision ? { hookSpecificOutput: { hookEventName: 'PreToolUse' as const, ...decision } } : {};
    };
    // Registering PostToolBatch also prevents calls stepping aside for a message. A settled batch
    // has already ended the CLI's turn by its result (`run`) unless that result was an error.
    const settle: HookCallback = async () => {
      if (this.settle) {
        this.settling = true;
        return { continue: false, stopReason: 'Razor settled the batch.' };
      }
      this.yielded = this.held !== null;
      // The CLI shows the model this reason after the message, as 'PostToolBatch hook stopped
      // continuation: <reason>'.
      return this.yielded ? { continue: false, stopReason: 'Razor delivered a new message with these tool results.' } : {};
    };
    this.query = query({ prompt: this.inbox, options: {
      cwd, model, effort, systemPrompt: prompt,
      // The API omits thinking text unless asked: from Opus 4.7 its default display is omitted.
      thinking: { type: 'adaptive', display: 'summarized' },
      tools: builtins.map((b) => b.builtin),
      toolAliases: Object.fromEntries(builtins.flatMap((b) => (b.to ? [[b.builtin, `mcp__razor__${b.to}`]] : []))),
      mcpServers: { razor: server },
      hooks: { PreToolUse: [{ hooks: [admit], onFailure: 'block' }], PostToolBatch: [{ hooks: [settle], onFailure: 'block' }] },
      settingSources: [],
      settings: { fastMode: options.speed === 'fast', autoCompactEnabled: false, precomputeCompactionEnabled: false, idleCompaction: false, attribution: false, disableClaudeAiConnectors: true },
      permissionMode: 'bypassPermissions', allowDangerouslySkipPermissions: true,
      includePartialMessages: true, verbatimPrompts: true, title: `razor ${session}`,
      ...(resume ? { resume: resume.claude.conversation, resumeSessionAt: resume.claude.at } : {}),
      env: {
        ...process.env, ...options.env,
        CLAUDE_CODE_PROMPT_CACHE_TTL: '1h', CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL: '1h',
        ENABLE_TOOL_SEARCH: 'true', CLAUDE_CODE_INLINE_TOOLS: '1',
        CLAUDE_BASH_MAINTAIN_PROJECT_WORKING_DIR: '1', CLAUDE_AGENT_SDK_CLIENT_APP: app,
        // The CLI cuts a tool description at 4,096 characters; code mode's exec holds every tool.
        CLAUDE_CODE_MAX_MCP_DESCRIPTION_LENGTH: '1000000',
      },
    } });
    void this.pump();
  }

  // Pipe backpressure slows the SDK rather than buffering its stream without bound.
  async send(line: Record<string, unknown>, final = false) {
    if (this.closed && !final) return;
    const { session, open } = this;
    if (!process.stdout.write(JSON.stringify({ session, open, ...line }) + '\n')) await once(process.stdout, 'drain');
  }
  notice(text: string) { return this.send({ type: 'entry', body: { kind: 'notice', text }, native: null }); }
  gate(call: string) {
    let gate = this.gates.get(call);
    if (!gate) this.gates.set(call, gate = Promise.withResolvers<Admission>());
    return gate;
  }
  close() {
    this.closed = true;
    if (conversations.get(this.session) === this) conversations.delete(this.session);
    for (const gate of this.gates.values()) gate.resolve({ refused: refusals.unrecorded });
    for (const answer of this.runs.values()) answer({ output: refusals.closed, error: true });
    this.relist?.resolve();
    this.inbox.close();
    // Closing alone gives a CLI whose tool was cut off time to start another request.
    this.query.interrupt().catch(() => {});
    this.query.close();
  }
  // Escape: the SDK ends the turn aborted. A call still in `runs`, whose answer has not gone to
  // the SDK, gets the CLI's own result, which carries no answer, so it is not mirrored and the
  // turn's end records it unknown. Runs and admissions are answered here, after the interrupt
  // so no further request starts.
  async interrupt() {
    this.escaped = this.busy;
    this.stopped = new Set(this.runs.keys());
    await this.query.interrupt().catch(() => {});
    for (const answer of this.runs.values()) answer({ output: refusals.stopped, error: true });
    for (const gate of this.gates.values()) gate.resolve({ refused: refusals.stopped });
    this.runs.clear();
  }
  serve(tools: Tool[]) {
    for (const tool of tools) this.served.set(tool.name, { ...tool, _meta: { ...tool._meta, 'anthropic/alwaysLoad': true } });
  }
  enqueue(step: () => Promise<unknown>) {
    this.queue = this.queue.then(() => this.closed ? undefined : step())
      .catch((e) => console.error(`razor bridge: session ${this.session}: ${e instanceof Error ? e.message : e}`));
  }
  async add(tools: Tool[]) {
    if (tools.every((tool) => JSON.stringify(this.served.get(tool.name)) === JSON.stringify({ ...tool, _meta: { ...tool._meta, 'anthropic/alwaysLoad': true } }))) return;
    this.serve(tools);
    const relist = this.relist ??= Promise.withResolvers<void>();
    await this.razor.sendToolListChanged();
    await relist.promise;
  }
  async run({ name, arguments: input, _meta }: CallToolRequest['params']) {
    const call = _meta?.['claudecode/toolUseId'];
    if (typeof call !== 'string') return { content: [{ type: 'text' as const, text: refusals.unknownCall }], isError: true };
    const answered = new Promise<Answer>((resolve) => this.runs.set(call, resolve));
    await this.send({ type: 'run', call, tool: name, input: input ?? {} });
    const answer = await answered;
    let value: { images?: Image[]; structured?: unknown; is_error?: boolean } = {};
    try { value = JSON.parse(answer.output) ?? {}; } catch { /* Most native results are text. */ }
    // A result's own images, then those its cell's nested calls named, each after its path.
    const images = (await Promise.all([...(value.images ?? []), ...(answer.images ?? [])].map(({ path, media_type }) => base64(path).then(
      (data) => [{ type: 'text' as const, text: path }, { type: 'image' as const, mimeType: media_type, data }],
      (refused: string) => [{ type: 'text' as const, text: refused }])))).flat();
    if (images.length) this.imaged.set(call, answer.output);
    this.runs.delete(call); // The answer reaches the SDK only now: until here Escape stops the call.
    // A settling result ends the CLI's turn itself: the hook's stop would leave a note after it,
    // which the resume just after cuts and every later one puts back, rewriting the cache.
    return { content: [{ type: 'text' as const, text: answer.output }, ...images],
      ...(this.settle ? { _meta: { 'claude/endTurn': true } } : {}),
      ...(value.structured == null ? {} : { structuredContent: value.structured }), isError: answer.error || value.is_error === true };
  }
  async input(inputs: Input[]) {
    const last = inputs.at(-1);
    if (!last) return;
    // One message per batch; each input's images precede its own text, not earlier text.
    const content = (await Promise.all(inputs.map(async ({ text, images }) => {
      const blocks = await Promise.all(images.map(({ media_type, path }) => base64(path).then(
        (data) => ({ type: 'image' as const, source: { type: 'base64' as const, media_type, data } }),
        (refused: string) => ({ type: 'text' as const, text: refused }))));
      return [...blocks, { type: 'text' as const, text }];
    }))).flat();
    const uuid = uuidOf(last.entry);
    if (!this.busy) return this.prompt(content, uuid);
    this.held = { content: [...this.held?.content ?? [], ...content], uuid };
  }
  prompt(content: Content, uuid: Uuid) {
    this.busy = true;
    this.inbox.push({ type: 'user', message: { role: 'user', content }, parent_tool_use_id: null, uuid });
  }
  async pump() {
    let error: string | undefined;
    try {
      for await (const m of this.query) {
        if (this.closed) break;
        await this.mirror(m);
      }
    } catch (e) { error = e instanceof Error ? e.message : String(e); }
    if (!this.closed) {
      this.close();
      await this.send({ type: 'closed', error: error ?? null }, true);
    }
  }
  measure(name: string, utilization: unknown, reset: unknown, status?: string, reason?: string) {
    if (!name || name.length > 64 || !this.windows.has(name) && this.windows.size >= 32) return;
    const old = this.windows.get(name);
    const measured = typeof utilization === 'number' && Number.isFinite(utilization) && utilization >= 0 && utilization <= 100;
    if (!measured && status === undefined) return;
    this.windows.set(name, { name, utilization: measured ? utilization : old?.utilization ?? null,
      resets_at: measured ? typeof reset === 'number' && Number.isSafeInteger(reset) && reset > 0 ? reset : null : old?.resets_at ?? null,
      rejected: status === 'rejected', reason: status === 'rejected' ? reason : undefined });
  }
  async usage() {
    if (this.closed || this.usagePending || Date.now() - this.usageAt < 60000) return;
    this.usageAt = Date.now(); this.usagePending = true;
    let measured = false;
    // Another program's file of accounts.<realm>.<account>.limits (percent 0-100, epoch seconds)
    // gives model rows only, as rate-limit events give fresher 5-hour and 7-day ones; it goes
    // first so the CLI's own answer replaces them. Past 1 MiB it is cut.
    const { usageFile: file, account } = this;
    if (file && account) try {
      const doc: any = await json(createReadStream(file, { end: 1 << 20 }));
      const row: any = Math.abs(Date.now() / 1000 - doc?.observed_at) < 1200 && Object.values(doc.accounts ?? {}).map((realm: any) => realm?.[account]).find(Boolean);
      for (const limit of row?.status === 'ok' ? row.limits : []) {
        if (limit?.kind === 'weekly_scoped' && typeof limit.model === 'string') { this.measure(limit.model, limit.percent, limit.resets_at * 1000); measured = true; }
      }
      if (measured) await this.send({ type: 'usage', windows: [...this.windows.values()] });
    } catch { /* A missing, unreadable or changed file measures nothing. */ }
    try {
      const u = await this.query.usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET({ skipBehaviors: true });
      const limits = u?.rate_limits_available && u.rate_limits;
      if (!limits || typeof limits !== 'object') return;
      for (const key of ['five_hour', 'seven_day', 'seven_day_oauth_apps', 'seven_day_opus', 'seven_day_sonnet'] as const) {
        const value = limits[key];
        if (value) this.measure(limitNames[key]!, value.utilization, typeof value.resets_at === 'string' ? Date.parse(value.resets_at) : null);
      }
      // Scoped rows are additive, filtered by the SDK's allowlist; they cannot erase other models.
      // The same model name replaces its legacy reading; equal numbers alone do not prove an alias.
      if (Array.isArray(limits.model_scoped)) {
        for (const value of limits.model_scoped.slice(0, 30)) {
          if (typeof value?.display_name === 'string') this.measure(value.display_name, value.utilization, typeof value.resets_at === 'string' ? Date.parse(value.resets_at) : null);
        }
      }
      await this.send({ type: 'usage', windows: [...this.windows.values()] });
    } catch { /* Experimental: a missing or changed answer leaves rate-limit events in charge. */ }
    finally { this.usagePending = false; }
  }
  async mirror(m: SDKMessage) {
    const { session } = this;
    // The pinned SDK passes the CLI's command_lifecycle frames through without typing them.
    const lifecycle = m as unknown as { type: string; state?: string; command_uuid?: string };
    if (lifecycle.type === 'command_lifecycle' && lifecycle.state === 'started') this.started = entryOf(lifecycle.command_uuid ?? '');
    if (this.fast && 'fast_mode_state' in m && m.fast_mode_state && m.fast_mode_state !== this.fastState) {
      this.fastState = m.fast_mode_state;
      if (m.fast_mode_state !== 'on') await this.notice(`Fast unavailable; provider used Standard (${m.fast_mode_disabled_reason ?? m.fast_mode_state}).`);
    }
    switch (m.type) {
      case 'assistant': {
        if (m.parent_tool_use_id === null) this.took = this.started; // An API error's own frame too.
        if (m.parent_tool_use_id !== null || m.error) return;
        const native = { claude: { conversation: m.session_id, uuid: m.uuid } };
        if (this.sized !== m.message.id) {
          this.sized = m.message.id;
          const u = m.message.usage;
          await this.send({ type: 'context', tokens: u.input_tokens + (u.cache_read_input_tokens ?? 0) + (u.cache_creation_input_tokens ?? 0), window: this.window || null, read: this.took });
        }
        for (const block of m.message.content) {
          const body = block.type === 'text' ? { kind: 'text', text: block.text }
            : block.type === 'thinking' && block.thinking ? { kind: 'thinking', text: block.thinking }
            : block.type === 'tool_use' ? { kind: 'call', call: block.id, name: block.name, input: block.input }
            : undefined;
          if (body) await this.send({ type: 'entry', body, native });
          if (body?.kind === 'thinking') this.landed = this.thoughts;
        }
        return;
      }
      case 'user': {
        if (m.parent_tool_use_id !== null || !Array.isArray(m.message.content)) return;
        const native = { claude: { conversation: m.session_id, uuid: m.uuid } };
        for (const block of m.message.content) {
          if (block.type === 'tool_result' && !this.stopped.delete(block.tool_use_id)) {
            const output = this.imaged.get(block.tool_use_id) ?? text(block.content);
            this.imaged.delete(block.tool_use_id);
            await this.send({ type: 'entry', body: { kind: 'result', call: block.tool_use_id, outcome: block.is_error ? 'error' : 'success', output }, native });
          }
        }
        return;
      }
      case 'stream_event':
        if (m.parent_tool_use_id !== null) return;
        if (m.event.type === 'message_start') {
          const tools = this.additions;
          this.additions = null;
          if (tools) this.enqueue(() => this.add(tools));
          [this.took, this.requests] = [this.started, this.requests + 1];
          // The CLI tries a cut message again as a new one: the cut one's thoughts have no entries.
          if (this.landed < this.thoughts) await this.send({ type: 'void', n: this.thoughts });
          this.landed = this.thoughts;
        }
        // A redacted or omitted thought has no words, so it sends nothing.
        if (m.event.type === 'content_block_start') this.thought = m.event.content_block.type === 'thinking';
        if (m.event.type !== 'content_block_delta') return;
        if (m.event.delta.type === 'text_delta') await this.send({ type: 'delta', text: m.event.delta.text });
        if (m.event.delta.type === 'thinking_delta' && m.event.delta.thinking) {
          this.thoughts += Number(this.thought);
          await this.send({ type: 'thought', n: this.thoughts, text: m.event.delta.thinking, start: this.thought });
          this.thought = false;
        }
        return;
      case 'result': {
        // A result before init is the CLI refusing to start (a resume it cannot find), not a
        // turn: the conversation closed, which the controller rebuilds when it was resumed.
        if (!this.announced) {
          this.close();
          return this.send({ type: 'closed', error: m.subtype === 'success' ? null : m.errors.join(' ') }, true);
        }
        void this.usage();
        const u = m.usage, c = this.carried;
        const tokens = { input: c.input + u.input_tokens, output: c.output + u.output_tokens, cache_read: c.cache_read + (u.cache_read_input_tokens ?? 0),
          cache_write: c.cache_write + (u.cache_creation_input_tokens ?? 0) };
        const usage = { ...tokens, requests: this.requests, waited: 0 };
        if (this.settling) {
          this.close();
          await this.send({ type: 'settled', usage }, true);
          return;
        }
        const { held, yielded, escaped } = this;
        [this.held, this.yielded, this.escaped, this.busy] = [null, false, false, false];
        if (held && yielded && !escaped) {
          this.carried = tokens;
          return this.prompt(held.content, held.uuid);
        }
        [this.carried, this.requests] = [{ input: 0, output: 0, cache_read: 0, cache_write: 0 }, 0];
        // An input whose request Escape aborted is in the conversation, and the next request reads it.
        // Escape after the batch yielded finds the CLI's turn ended cleanly: it is still stopped.
        const sent = this.send({ type: 'turn_end', success: m.subtype === 'success' && !m.is_error && !yielded, reason: yielded ? 'stopped' : m.terminal_reason ?? null,
          // The CLI's advice for a missing sign-in names its own /login, which Razor does not have.
          message: (m.subtype === 'success' ? m.result : m.errors.join(' ')).replace(/Not logged in[^\n]*\/login/, 'Claude is not signed in; run razor setup'), usage,
          window: Math.max(0, ...Object.values(m.modelUsage).map((usage) => usage.contextWindow)), took: this.started });
        this.took = null;
        // What arrived too late for a batch opens the CLI's next turn, as the controller expects,
        // before the line's drain: an input arriving during it must queue behind this one.
        if (held) held.content = held.content.filter((b) => b !== effortNotice);
        if (held?.content.length) this.prompt(held.content, held.uuid);
        return sent;
      }
      case 'rate_limit_event': {
        const r = m.rate_limit_info;
        // Current CLIs also send these windows, although this pinned SDK omits them from its type.
        const unified = (r as typeof r & { unifiedWindows?: Record<string, { utilization?: number; resetsAt?: number; status?: string }> }).unifiedWindows;
        for (const [key, value] of Object.entries(unified ?? {}).slice(0, 32)) {
          const name = limitNames[key];
          if (name && value && typeof value === 'object') this.measure(name, typeof value.utilization === 'number' ? value.utilization * 100 : null, typeof value.resetsAt === 'number' ? value.resetsAt * 1000 : null, value.status, blocked(r));
        }
        const name = r.rateLimitType && limitNames[r.rateLimitType];
        if (name) this.measure(name, typeof r.utilization === 'number' ? r.utilization * 100 : null, r.resetsAt === undefined ? null : r.resetsAt * 1000, r.status, blocked(r));
        await this.send({ type: 'usage', windows: [...this.windows.values()] });
        return;
      }
      case 'system':
        if (m.subtype === 'notification' && this.fast && m.key.startsWith('fast-mode')) await this.notice(m.text);
        // Warnings and stops only: info, notice and suggestion lines are the CLI's own status and tips.
        if (m.subtype === 'informational' && (m.level === 'warning' || m.prevent_continuation)) await this.notice(m.content);
        if (m.subtype === 'init' && !this.announced) {
          this.announced = true;
          void this.usage();
          this.query.getContextUsage({ detail: 'summary' }).then((u) => (this.window = u.rawMaxTokens), () => {});
          console.error(`razor bridge: session ${session}: Claude Code ${m.claude_code_version}, model ${m.model}, API key source ${m.apiKeySource}.`);
        }
        if (m.subtype === 'compact_boundary') console.error(`razor bridge: session ${session} was compacted by the SDK, though compaction is off; this is a defect.`);
    }
  }
}

for await (const line of createInterface({ input: process.stdin, crlfDelay: Infinity })) {
  const m = JSON.parse(line);
  const c = conversations.get(m.session);
  switch (m.type) {
    case 'open': c?.close(); conversations.set(m.session, new Conversation(m)); break;
    case 'add':
      if (c?.additions) c.additions = m.tools;
      else c?.enqueue(() => c.add(m.tools));
      break;
    case 'effort': c?.enqueue(async () => {
      c.fast = m.speed === 'fast';
      await c.query.applyFlagSettings({ effortLevel: m.effort, fastMode: c.fast, model: m.model });
      if (m.model !== c.model) [c.model, c.window] = [m.model, (await c.query.getContextUsage({ detail: 'summary' })).rawMaxTokens];
      // Claude's per-turn effort changes only on a user message beyond tool results; yield one held notice.
      if (m.notice !== null && c.busy) {
        const held = c.held ??= { content: [], uuid: uuidOf(m.notice) };
        if (!held.content.includes(effortNotice)) held.content.push(effortNotice);
      }
    }); break;
    case 'input': c?.enqueue(() => c.input(m.inputs)); break;
    case 'admitted': c?.gate(m.call).resolve({ input: m.input }); break;
    case 'result': {
      if (m.settle && c) c.settle = true;
      const run = c?.runs.get(m.call);
      // A result for a call that has not run is a plugin's refusal: its gate denies it.
      if (run && c) c.enqueue(async () => { run({ output: m.output, error: m.error, images: m.images }); });
      else c?.gate(m.call).resolve({ refused: m.output });
      break;
    }
    case 'interrupt': void c?.interrupt(); break;
    case 'close': c?.close(); break;
  }
}
for (const conversation of [...conversations.values()]) conversation.close();
setTimeout(() => process.exit(0), 5000).unref();
