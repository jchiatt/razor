// The `razor` global and its dispatch function: no argument ends registration and lists it;
// JSON text dispatches an event or tool call (docs/design/extensions.md).
(() => {
  const before = { prompt: [], input: [], call: [], rebuild: [] };
  const after = {}, tools = {}, commands = [];
  let prompts = 0, loading = true;
  const check = (handler) => {
    if (!loading) throw new Error('A plugin registers only while it loads.');
    if (typeof handler !== 'function') throw new TypeError('A handler must be a function.');
    return handler;
  };
  const razor = globalThis.razor = Object.freeze({
    before(event, handler) {
      if (!Object.hasOwn(before, event)) throw new TypeError(`There is no before-event "${event}".`);
      before[event].push(check(handler));
    },
    after(kind, handler) { (after[kind] ??= []).push(check(handler)); },
    tool({ name, description = '', input }, handler) {
      if (typeof name !== 'string' || input?.type !== 'object') {
        throw new TypeError('A tool needs a name and an input schema whose root is an object.');
      }
      tools[name] = { description, input, handler: check(handler) };
    },
    command(name, description, handler) {
      check(handler);
      commands.push({ name, description });
      razor.before('input', async (p) => {
        const [, word, rest = ''] = ((p.from === 'person' || p.from?.person) && /^\/(\S+)(?:\s+([\s\S]*))?$/.exec(p.text)) || [];
        return word === name ? { ...p, text: String(await handler(rest)) } : p;
      });
    },
    prompt(text) {
      razor.before('prompt', (p) => ({ ...p, prompt: `${p.prompt}\n\n${text}` }));
      prompts += 1;
    },
  });
  return async (text) => {
    if (text === undefined) {
      loading = false;
      const listed = Object.entries(tools).map(([name, t]) => ({ name, description: t.description, input: t.input }));
      const handled = Object.keys(before).filter((event) => before[event].length);
      return JSON.stringify({ before: handled, after: Object.keys(after), tools: listed, commands, prompts });
    }
    let p = JSON.parse(text);
    if (p.tool !== undefined) return JSON.stringify((await tools[p.tool].handler(p.input)) ?? null);
    if (p.before === undefined) {
      for (const handler of after[p.entry.kind] ?? []) await handler(p);
      return '';
    }
    for (const handler of before[p.before]) p = (await handler(p)) ?? p;
    return JSON.stringify(p);
  };
})()
