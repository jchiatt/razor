"""Three reads in one model message run at once; two edits to one file apply in call order.

Each read is of a named pipe that the flow writes only after every pipe has a reader, last pipe
first, so reads run one at a time deadlock and never return. Claude's SDK returns results as
they finish (docs/design/claude.md); Razor's own loop returns them in call order (SPEC 3)."""
import json
import os
from pathlib import Path
import tempfile
import threading

from claude import reply
from driver import REPLIES, Reply, flow
from responses import PLAIN, call, done, response

PIPES = "abc"


def serve_pipes(folder):
    served = []

    def write():
        for name in reversed(PIPES):
            with open(folder / f"{name}.pipe", "w") as pipe:
                pipe.write(f"pipe-{name}\n")
            served.append(name)
    for name in PIPES:
        os.mkfifo(folder / f"{name}.pipe")
    threading.Thread(target=write, daemon=True).start()
    return served


def check(f, folder, served, result, ids, final):
    assert result.returncode == 0, result.stderr
    answer = json.loads(result.stdout)
    assert answer["message"] == "flow done", answer
    entries = list(f.record(answer["session"]))
    calls = [e for e in entries if e["kind"] == "call"]
    outputs = [e for e in entries if e["kind"] == "result"]
    assert [e["call"] for e in calls] == ids, calls
    assert sorted(e["call"] for e in outputs) == sorted(ids), outputs
    assert served == list(reversed(PIPES)), served
    for name, id in zip(PIPES, ids):
        output = next(e for e in outputs if e["call"] == id)
        assert output["outcome"] == "success" and f"pipe-{name}" in output["output"], output
    edits = [e for e in outputs if e["call"] in ids[3:]]
    assert [e["call"] for e in edits] == ids[3:], edits
    assert all(e["outcome"] == "success" for e in edits), edits
    assert (folder / "edits.txt").read_text() == final
    return [e["call"] for e in outputs]


# Claude: Claude Code's own Read and Edit; the first edit makes the file the second changes.
with tempfile.TemporaryDirectory() as scratch:
    folder = Path(scratch)
    ids = [f"toolu_read_{name}" for name in PIPES] + ["toolu_edit_1", "toolu_edit_2"]
    edits = [{"file_path": str(folder / "edits.txt"), "old_string": "", "new_string": "alpha beta\n"},
             {"file_path": str(folder / "edits.txt"), "old_string": "beta", "new_string": "gamma"}]
    batch = [(id, "Read", {"file_path": str(folder / f"{name}.pipe")}) for id, name in zip(ids, PIPES)]
    batch += [(id, "Edit", edit) for id, edit in zip(ids[3:], edits)]
    with flow([reply(*batch), Reply((REPLIES / "done.sse").read_bytes())]) as f:
        served = serve_pipes(folder)
        result = f.run("--print=json", "Read the three pipes, then make both edits.", timeout=30)
        order = check(f, folder, served, result, ids, "alpha gamma\n")
        request = json.loads((f.requests / "002.body").read_bytes())
        seen = [block["tool_use_id"] for block in request["messages"][-1]["content"]
                if block["type"] == "tool_result"]
        assert order == seen, (order, seen)

# Razor's own loop: its read and edit actions, and every result in call order.
with tempfile.TemporaryDirectory() as scratch:
    folder = Path(scratch)
    (folder / "edits.txt").write_text("alpha beta\n")
    ids = [f"call_read_{name}" for name in PIPES] + ["call_edit_1", "call_edit_2"]
    edits = [{"path": str(folder / "edits.txt"), "old_string": "beta", "new_string": "gamma"},
             {"path": str(folder / "edits.txt"), "old_string": "gamma", "new_string": "delta"}]
    items = [call("read", json.dumps({"path": str(folder / f"{name}.pipe")}), id)
             for id, name in zip(ids, PIPES)]
    items += [call("edit", json.dumps(edit), id) for id, edit in zip(ids[3:], edits)]
    with flow([response(items), done()], model="gpt-6.1-sol", config=PLAIN) as f:
        served = serve_pipes(folder)
        result = f.run("--print=json", "Read the three pipes, then make both edits.", timeout=30)
        order = check(f, folder, served, result, ids, "alpha delta\n")
        assert order == ids, order
        second = json.loads((f.requests / "002.body").read_bytes())
        seen = [i["call_id"] for i in second["input"] if i["type"] == "function_call_output"]
        assert seen == ids, seen
