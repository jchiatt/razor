"""A reply and a durable turn, through the real Claude SDK and --print; what the startup list
and the session's facts give the model, and the startup.md that shows it."""
import json
import subprocess
from driver import BINARY, REPLIES, Reply, flow
from terminal import Terminal

prompt = "Hello"
with flow([Reply((REPLIES / "hello.sse").read_bytes())]) as f:
    # The default list: Razor's text, then the project's AGENTS.md (an include in place) and CLAUDE.md.
    (f.cwd / "AGENTS.md").write_text("flow agents rules\n@shared.md\n@absent.md\n")
    (f.cwd / "shared.md").write_text("flow shared include\n")
    (f.cwd / "CLAUDE.md").write_text("flow claude rules\n")
    result = f.run(prompt, "--print=json", "--speed", "fast")
    assert result.returncode == 0, result.stderr
    answer = json.loads(result.stdout)
    assert answer["message"] == "flow reply", answer
    session = answer["session"]
    entries = list(f.record(session))
    assert entries[0]["cwd"] == str(f.cwd), entries[0]
    inputs = [e for e in entries if e["kind"] == "input"]
    assert len(inputs) == 1 and inputs[0]["text"] == prompt, inputs
    texts = [e["text"] for e in entries if e["kind"] == "text"]
    assert texts == ["flow reply"], texts
    assert entries[-1]["inputs"] == [inputs[0]["id"]], entries[-1]
    assert entries[-1]["end"] == "done", entries[-1]
    request = json.loads((f.requests / "001.body").read_bytes())
    assert request.get("speed") == "fast", request
    assert prompt in json.dumps(request["messages"]), request
    system = request["system"][-1]["text"]
    marks = ["You are working in Razor", "# The record and your context", "flow agents rules",
             "flow shared include\n@absent.md", "flow claude rules"]
    assert all(m in system for m in marks), system
    assert [system.index(m) for m in marks] == sorted(system.index(m) for m in marks), system
    first = request["messages"][0]["content"][0]["text"]
    shown = (f.home / ".razor/sessions" / session / "startup.md").read_text()
    assert system in shown and first.removesuffix(prompt).strip() in shown, shown
    assert '"builtin": "Bash"' in shown and "## The tools (" in shown, shown

# A list without `razor`: a folder's files by name, `~/` the home folder, the route part last, and
# a missing entry named in the facts, never fatal.
with flow([Reply((REPLIES / "hello.sse").read_bytes())],
          config='startup = ["~/notes", "~/missing.md", "project"]') as f:
    (f.home / "notes").mkdir()
    (f.home / "notes/b.md").write_text("flow note b")
    (f.home / "notes/a.md").write_text("flow note a")
    (f.cwd / "AGENTS.md").write_text("flow agents rules")
    result = f.run("--print=json", input=prompt)
    assert result.returncode == 0, result.stderr
    request = json.loads((f.requests / "001.body").read_bytes())
    assert request.get("speed") in (None, "standard"), request
    system = request["system"][-1]["text"]
    assert system.startswith(f"flow note a\n\nflow note b\n\nContents of {f.cwd.resolve()}/AGENTS.md:\n\nflow agents rules\n\n# Your tools"), system
    assert "You are working in Razor" not in system, system
    first = request["messages"][0]["content"][0]["text"]
    assert f"Left out of your startup list: {f.home}/missing.md (" in first, first

with flow([Reply((REPLIES / "hello.sse").read_bytes())]) as f:
    with Terminal(f, "Reply to this first sentence.") as t:
        t.shown("flow reply")

# Six clients start on a stopped home at once: the servers that lose the lock exit, and the winner
# answers every call.
with flow([Reply((REPLIES / "hello.sse").read_bytes())] * 6) as f:
    clients = [subprocess.Popen([BINARY, "--print=json", prompt], env=f.env, cwd=f.cwd, text=True,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE) for _ in range(6)]
    results = [(client.communicate(timeout=90), client.returncode) for client in clients]
    assert [code for _, code in results] == [0] * 6, results
