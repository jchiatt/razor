"""Plugins (docs/design/extensions.md): a JavaScript plugin's slash command is expanded at admission
and recorded; a hook refuses Claude Code's Bash call with its sentence; the plugin's tool counts
the results it observed, exactly one; `razor plugins` lists both."""
import json
import claude
from driver import flow

HOOK = """#!/bin/sh
case "$(cat)" in
*'"name":"Bash"'*) echo 'No shell in this flow.' >&2; exit 2 ;;
esac
"""
PLUGIN = """let results = 0;
razor.after('result', () => { results += 1; });
const input = { type: 'object', properties: { a: { type: 'number' }, b: { type: 'number' } } };
razor.tool({ name: 'add', description: 'Adds two numbers.', input }, ({ a, b }) => ({ sum: a + b, results }));
razor.command('flow', 'Runs this flow.', (rest) => `Run the shell, then add ${rest}.`);
"""
CONFIG = """[[plugins]]
file = "refuse.sh"
before = ["call"]

[[plugins]]
file = "tools.js"
"""
replies = [claude.reply((None, "Bash", {"command": "echo shell-ran"})),
           claude.reply((None, "mcp__razor__add", {"a": 2, "b": 3})), claude.reply(text="flow done")]
with flow(replies, config=CONFIG) as f:
    hook = f.home / ".razor/refuse.sh"
    hook.write_text(HOOK)
    hook.chmod(0o755)
    (f.home / ".razor/tools.js").write_text(PLUGIN)
    result = f.run("--print=json", "/flow 2 and 3")
    assert result.returncode == 0, result.stderr
    answer = json.loads(result.stdout)
    assert answer["message"] == "flow done", answer
    entries = list(f.record(answer["session"]))
    asked = "Run the shell, then add 2 and 3."
    assert [e["text"] for e in entries if e["kind"] == "input"] == [asked], entries
    assert asked in (f.requests / "001.body").read_text()
    bash, add = (e for e in entries if e["kind"] == "call")
    results = {e["call"]: e for e in entries if e["kind"] == "result"}
    refused = results[bash["call"]]
    assert bash["name"] == "Bash" and refused["outcome"] == "error", refused
    assert "No shell in this flow." in refused["output"] and "shell-ran" not in refused["output"]
    assert json.loads(results[add["call"]]["output"]) == {"sum": 5, "results": 1}, results
    listed = f.run("plugins")
    assert listed.returncode == 0, listed.stderr
    hook, tools = map(json.loads, listed.stdout.splitlines())
    assert hook["file"].endswith("refuse.sh") and hook["hook"] and hook["before"] == ["call"], hook
    assert not tools["hook"] and tools["before"] == ["input"] and tools["after"] == ["result"], tools
    assert [t["name"] for t in tools["tools"]] == ["add"], tools
    assert [c["name"] for c in tools["commands"]] == ["flow"], tools
