"""Razor's file actions: read pages by line; edit needs exactly one match and otherwise changes
nothing; write makes the missing parent folders. On the Claude route Read, Edit and Write are
Claude Code's own (docs/design/tools.md), so these actions serve Razor's own loop."""
import json

from driver import flow
from responses import PLAIN, call, done, response

notes = "".join(f"line-{n}\n" for n in range(1, 6))
twice = "same start\nsame end\n"
steps = [("read", {"path": "notes.txt", "limit": 2}),
         ("read", {"path": "notes.txt", "offset": 3, "limit": 2}),
         ("edit", {"path": "twice.txt", "old_string": "same", "new_string": "other"}),
         ("edit", {"path": "notes.txt", "old_string": "line-5", "new_string": "line-five"}),
         ("write", {"path": "made/deeper/new.txt", "content": "made\n"})]
items = [call(name, json.dumps(arguments), f"call_{n}") for n, (name, arguments) in enumerate(steps)]
with flow([response(items), done()], model="gpt-6.1-sol", config=PLAIN) as f:
    (f.cwd / "notes.txt").write_text(notes)
    (f.cwd / "twice.txt").write_text(twice)
    result = f.run("--print=json", "Page the notes, try both edits, write the new file.")
    assert result.returncode == 0, result.stderr
    answer = json.loads(result.stdout)
    assert answer["message"] == "flow done", answer
    entries = list(f.record(answer["session"]))
    outputs = [e for e in entries if e["kind"] == "result"]
    assert [e["call"] for e in outputs] == [i["call_id"] for i in items], outputs
    first, second, refused, edited, written = outputs
    assert all(e["outcome"] == "success" for e in (first, second, edited, written)), outputs
    page = json.loads(first["output"])
    assert page["lines"] == ["line-1", "line-2"] and page["next"] == 3, page
    page = json.loads(second["output"])
    assert page["first"] == 3 and page["lines"] == ["line-3", "line-4"] and page["next"] == 5, page
    assert refused["outcome"] == "error", refused
    assert (f.cwd / "twice.txt").read_text() == twice, "a refused edit changed the file"
    assert (f.cwd / "notes.txt").read_text() == notes.replace("line-5", "line-five")
    assert (f.cwd / "made/deeper/new.txt").read_text() == "made\n"
    assert json.loads(written["output"])["created"] is True, written
